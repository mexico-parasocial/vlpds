#!/usr/bin/env bash
# Secretos de la plataforma PARA: un archivo por secreto en secrets/.
#
#   scripts/secrets.sh generate   crea los que faltan (nunca toca los que existen)
#   scripts/secrets.sh import     los trae de los .env de antes (WatZappa/.env,
#                                 env/identity.env, .env); no pisa nada
#   scripts/secrets.sh check      valida que estén todos y sean seguros
#   scripts/secrets.sh list       qué es cada uno y cuáles respaldar offline
#   scripts/secrets.sh perms      ajusta dueño y permisos (lo hace generate)
#   scripts/secrets.sh derive     solo calcula postgres_url* (lo usa Ansible:
#                                 el servidor nunca inventa un secreto)
#
# Los servicios los reciben como NAME_FILE=/run/secrets/<archivo>. Nunca
# imprime un secreto. Compatible con el bash 3.2 de macOS.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$HERE"
DIR="${PARA_SECRETS_DIR:-$HERE/secrets}"
GID="${PARA_SECRETS_GID:-10001}"

env_value() { # env_value ARCHIVO LLAVE
  [ -f "$1" ] || return 0
  # Una llave ausente no es un error (con pipefail, el grep vacío lo sería).
  { grep -E "^$2=" "$1" 2>/dev/null || true; } | tail -1 | cut -d= -f2- | sed -e 's/^["'\'']//' -e 's/["'\'']$//'
}
platform_env() { local v="${!1:-}"; [ -z "$v" ] && v="$(env_value .env "$1")"; printf '%s' "$v"; }
abs_dir() { (cd "$HERE" && cd "$1" 2>/dev/null && pwd) || echo "$1"; }

# nombre|tipo|offline|descripción|perfil que lo usa
#   tipo: hex (openssl rand -hex 32), derived (se calcula), jwk (par Ed25519),
#         provided (lo da un tercero: el proveedor del bucket, Synapse)
#   offline: yes = no vive en el bucket y perderlo no tiene arreglo; respáldalo
#            fuera de la máquina (docs/RESPALDOS.md)
#   perfil:  check solo exige los secretos de los perfiles de PROFILES
SPEC='
postgres_password|hex|no|Contraseña de Postgres (usuario POSTGRES_USER)|core
postgres_url|derived|no|URL de Postgres para AppView, dataplane, bsync y Ozone|core
postgres_url_bridge|derived|no|URL de Postgres (base bridge) para el matrix-bridge|matrix
s3_access_key_id|provided|no|Llave del bucket (PDS, mubEZ/Litestream, respaldos)|core
s3_secret_access_key|provided|no|Secreto del bucket|core
pds_jwt_secret|hex|no|PDS: firma de sesiones|core
pds_dpop_secret|hex|no|PDS: nonces DPoP de OAuth|core
pds_admin_password|hex|no|PDS: contraseña de administrador|core
pds_plc_rotation_key|hex|yes|PDS: llave de rotación PLC (k256). Controla los DIDs de todas las cuentas|core
pds_rate_limit_bypass_key|hex|no|PDS: llave para saltar rate limits (servicios de confianza)|core
bsky_service_signing_key|hex|yes|AppView: llave de firma del servicio|core
bsky_admin_passwords|hex|no|AppView: contraseña de administrador|core
ozone_signing_key|hex|yes|Ozone: llave de firma de etiquetas de moderación|core
ozone_admin_password|hex|no|Ozone: contraseña de administrador|core
authority_service_token|hex|no|Credencial compartida AppView ↔ bridge (autoridad de comunidades)|core
civic_delegation_resolver_secret|hex|no|Secreto compartido AppView ↔ mubEZ (delegaciones cívicas)|core
m8_jwt_secret|hex|no|mubEZ: firma de tokens M8|identity
m8_app_key|hex|no|mubEZ: APP_KEY de AdonisJS|identity
m8_cookie_secret|hex|no|mubEZ: firma de cookies|identity
m8_curp_pepper|hex|yes|mubEZ: pepper de los hashes de CURP. Perderlo rompe la deduplicación|identity
m8_civic_vote_proof_secret|hex|no|mubEZ: autorizaciones de voto público|identity
m8_issuer_private_jwk|jwk|yes|mubEZ: llave privada del emisor de credenciales (Ed25519)|identity
m8_issuer_public_jwk|jwk|no|mubEZ: llave pública del emisor|identity
matrix_admin_token|provided|no|Token de administrador de Synapse para el bridge|matrix
matrix_appservice_token|provided|no|as_token del appservice (registro del bridge)|matrix
matrix_hs_token|provided|no|hs_token del appservice (registro del bridge)|matrix
'

names() { printf '%s\n' "$SPEC" | sed '/^$/d' | cut -d'|' -f1; }
profiles() { local p; p="$(platform_env PROFILES)"; printf ',%s,' "${p:-core,matrix,identity,edge}"; }
in_use() { case "$(profiles)" in *",$(field "$1" 5),"*) return 0 ;; esac; return 1; }
field() { printf '%s\n' "$SPEC" | grep "^$1|" | cut -d'|' -f"$2"; }

write_secret() { # write_secret NOMBRE VALOR
  umask 077
  printf '%s' "$2" > "$DIR/$1.tmp"
  mv "$DIR/$1.tmp" "$DIR/$1"
}

fix_perms() {
  chmod 0750 "$DIR"
  if [ "$(id -u)" = 0 ]; then
    # El servidor: root:GID con 0440. Los contenedores leen gracias a
    # group_add: [GID]; nadie más en la máquina puede.
    chown -R "root:$GID" "$DIR"
    find "$DIR" -type f ! -name README.md -exec chmod 0440 {} +
  else
    # Una máquina de desarrollo: Docker Desktop monta los archivos como
    # root:root, así que el contenedor solo los lee si son legibles por todos.
    # El directorio (0750) sigue cerrando el paso a otros usuarios.
    find "$DIR" -type f ! -name README.md -exec chmod 0444 {} +
  fi
}

derive_urls() {
  [ -s "$DIR/postgres_password" ] || return 0
  local user db pw
  user="$(platform_env POSTGRES_USER)"; user="${user:-pg}"
  db="$(platform_env POSTGRES_DB)"; db="${db:-para}"
  pw="$(cat "$DIR/postgres_password")"
  chmod u+w "$DIR"/postgres_url* 2>/dev/null || true
  write_secret postgres_url "postgres://$user:$pw@postgres:5432/$db"
  write_secret postgres_url_bridge "postgres://$user:$pw@postgres:5432/bridge"
}

gen_jwk() {
  command -v node >/dev/null || { echo "  ⚠️  sin node: genera la llave del emisor con 'pnpm keys:generate' en mubEZ y ponla en secrets/m8_issuer_{private,public}_jwk" >&2; return 0; }
  node -e '
    const { generateKeyPairSync } = require("node:crypto")
    const fs = require("node:fs")
    const { privateKey, publicKey } = generateKeyPairSync("ed25519")
    fs.writeFileSync(process.argv[1], JSON.stringify(privateKey.export({ format: "jwk" })), { mode: 0o600 })
    fs.writeFileSync(process.argv[2], JSON.stringify(publicKey.export({ format: "jwk" })), { mode: 0o600 })
  ' "$DIR/m8_issuer_private_jwk" "$DIR/m8_issuer_public_jwk"
  echo "  + m8_issuer_private_jwk, m8_issuer_public_jwk"
}

cmd_generate() {
  mkdir -p "$DIR"
  chmod u+w "$DIR"
  local n kind created=0 pending=""
  for n in $(names); do
    [ -s "$DIR/$n" ] && continue
    kind="$(field "$n" 2)"
    case "$kind" in
      hex) write_secret "$n" "$(openssl rand -hex 32)"; echo "  + $n"; created=$((created + 1)) ;;
      jwk) [ "$n" = m8_issuer_private_jwk ] && gen_jwk ;;
      provided) pending="$pending $n" ;;
    esac
  done
  derive_urls
  fix_perms
  echo "Secretos en $DIR ($created nuevos)."
  if [ -n "$pending" ]; then
    echo "Faltan los que da un tercero; créalos tú (un valor por archivo, sin salto de línea):"
    for n in $pending; do echo "  secrets/$n — $(field "$n" 4)"; done
  fi
  echo "Respáldalos offline: $(for n in $(names); do [ "$(field "$n" 3)" = yes ] && printf '%s ' "$n"; done)"
}

cmd_import() {
  local wz id
  wz="$(abs_dir "$(platform_env WATZAPPA_DIR)")"; wz="${wz:-$HERE/../WatZappa}"
  id="$HERE/env/identity.env"
  mkdir -p "$DIR"; chmod u+w "$DIR"
  # secreto|archivo de origen|llave
  local map="
postgres_password|$wz/.env|POSTGRES_PASSWORD
s3_access_key_id|$wz/.env|PDS_BLOBSTORE_S3_ACCESS_KEY_ID
s3_secret_access_key|$wz/.env|PDS_BLOBSTORE_S3_SECRET_ACCESS_KEY
pds_jwt_secret|$wz/.env|PDS_JWT_SECRET
pds_dpop_secret|$wz/.env|PDS_DPOP_SECRET
pds_admin_password|$wz/.env|PDS_ADMIN_PASSWORD
pds_plc_rotation_key|$wz/.env|PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX
pds_rate_limit_bypass_key|$wz/.env|PDS_RATE_LIMIT_BYPASS_KEY
bsky_service_signing_key|$wz/.env|BSKY_SERVICE_SIGNING_KEY
bsky_admin_passwords|$wz/.env|ADMIN_PASSWORDS
ozone_signing_key|$wz/.env|OZONE_SIGNING_KEY_HEX
ozone_admin_password|$wz/.env|OZONE_ADMIN_PASSWORD
matrix_admin_token|$wz/.env|MATRIX_ADMIN_TOKEN
matrix_appservice_token|$wz/.env|MATRIX_APPSERVICE_TOKEN
matrix_hs_token|$wz/.env|MATRIX_HS_TOKEN
civic_delegation_resolver_secret|$wz/.env|PARA_CIVIC_DELEGATION_RESOLVER_SECRET
authority_service_token|$HERE/.env|PARA_AUTHORITY_SERVICE_TOKEN
m8_jwt_secret|$id|JWT_SECRET
m8_app_key|$id|APP_KEY
m8_cookie_secret|$id|COOKIE_SECRET
m8_curp_pepper|$id|CURP_PEPPER
m8_civic_vote_proof_secret|$id|CIVIC_VOTE_PROOF_SECRET
civic_delegation_resolver_secret|$id|CIVIC_DELEGATION_RESOLVER_SECRET
m8_issuer_private_jwk|$id|IDENTITY_ISSUER_PRIVATE_JWK
m8_issuer_public_jwk|$id|IDENTITY_ISSUER_PUBLIC_JWK
"
  local n file key v imported=0 sources=""
  while IFS='|' read -r n file key; do
    [ -z "$n" ] && continue
    [ -s "$DIR/$n" ] && continue
    v="$(env_value "$file" "$key")"
    case "$v" in ''|'<'*) continue ;; esac
    write_secret "$n" "$v"
    echo "  + $n  (de $key en ${file#$HERE/})"
    imported=$((imported + 1))
    case "$sources" in *"$file"*) ;; *) sources="$sources $file" ;; esac
  done <<EOF
$map
EOF
  # Los tokens del appservice viven en su registro de Synapse (YAML).
  local reg="$wz/deploy/matrix/synapse/para-bridge-registration.yaml" yk
  if [ -f "$reg" ]; then
    for pair in matrix_appservice_token:as_token matrix_hs_token:hs_token; do
      n="${pair%%:*}"; yk="${pair#*:}"
      [ -s "$DIR/$n" ] && continue
      v="$(sed -n "s/^$yk:[[:space:]]*[\"']\{0,1\}\([^\"']*\)[\"']\{0,1\}[[:space:]]*$/\1/p" "$reg" | head -1)"
      [ -n "$v" ] || continue
      write_secret "$n" "$v"
      echo "  + $n  (de $yk en el registro del appservice)"
      imported=$((imported + 1))
    done
  fi
  derive_urls
  fix_perms
  echo "$imported secretos importados."
  if [ -n "$sources" ]; then
    echo "Ahora quita esas llaves de:$sources"
    echo "(si un secreto queda en env/identity.env y también como archivo, mubEZ no arranca)."
  fi
  echo "Los que falten: make secrets"
}

OKS=0; FAILS=0
ok() { OKS=$((OKS + 1)); printf '  ✅ %s\n' "$1"; }
bad() { FAILS=$((FAILS + 1)); printf '  ❌ %s\n' "$1"; [ -n "${2:-}" ] && printf '      → %s\n' "$2"; return 0; }

cmd_check() {
  echo "Secretos ($DIR)"
  [ -d "$DIR" ] || { bad "No existe $DIR" "make secrets"; exit 1; }
  local n kind v seen="" dup
  for n in $(names); do
    kind="$(field "$n" 2)"
    in_use "$n" || continue
    if [ ! -s "$DIR/$n" ]; then
      case "$kind" in
        provided) bad "Falta $n — $(field "$n" 4)" "Crea secrets/$n con el valor (un archivo, sin salto de línea)" ;;
        *) bad "Falta $n" "make secrets" ;;
      esac
      continue
    fi
    v="$(cat "$DIR/$n")"
    if printf '%s' "$v" | grep -qiE 'change-?me|dev-secret|example|^password$|^secret$|^test'; then
      bad "$n parece un valor de desarrollo" "Bórralo y corre make secrets"; continue
    fi
    if [ "$kind" = hex ] && [ "${#v}" -lt 32 ]; then
      bad "$n es demasiado corto (${#v} caracteres, mínimo 32)" "Bórralo y corre make secrets"; continue
    fi
    if [ "$kind" = hex ]; then
      dup="$(printf '%s' "$v" | openssl dgst -sha256 | awk '{print $NF}')"
      case "$seen" in
        *"$dup"*) bad "$n repite el valor de otro secreto" "Cada secreto debe ser distinto: bórralo y corre make secrets"; continue ;;
      esac
      seen="$seen $dup"
    fi
    ok "$n"
  done
  local mode
  mode="$(stat -f '%Lp' "$DIR" 2>/dev/null || stat -c '%a' "$DIR")"
  case "$mode" in
    7[0-5]0) ok "secrets/ no es legible por otros ($mode)" ;;
    *) bad "secrets/ tiene modo $mode" "scripts/secrets.sh perms" ;;
  esac
  for f in "$HERE/env/identity.env" "$HERE/env/core.env" "$HERE/env/matrix.env"; do
    [ -f "$f" ] || continue
    for key in JWT_SECRET APP_KEY COOKIE_SECRET CURP_PEPPER CIVIC_VOTE_PROOF_SECRET IDENTITY_ISSUER_PRIVATE_JWK \
               PDS_JWT_SECRET PDS_DPOP_SECRET PDS_ADMIN_PASSWORD PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX \
               BSKY_SERVICE_SIGNING_KEY ADMIN_PASSWORDS OZONE_SIGNING_KEY_HEX MATRIX_ADMIN_TOKEN; do
      if [ -n "$(env_value "$f" "$key")" ]; then
        bad "${f#$HERE/} todavía define $key" "Quítalo: ese secreto ya es un archivo y el servicio no arrancaría con los dos"
      fi
    done
  done
  printf 'Resumen: %s ✅  %s ❌\n' "$OKS" "$FAILS"
  [ "$FAILS" -eq 0 ]
}

cmd_list() {
  printf '%-34s %-9s %-9s %-8s %s\n' SECRETO TIPO PERFIL OFFLINE DESCRIPCIÓN
  local n
  for n in $(names); do
    printf '%-34s %-9s %-9s %-8s %s\n' "$n" "$(field "$n" 2)" "$(field "$n" 5)" "$(field "$n" 3)" "$(field "$n" 4)"
  done
}

case "${1:-}" in
  generate) cmd_generate ;;
  import) cmd_import ;;
  check) cmd_check ;;
  list) cmd_list ;;
  perms) fix_perms; echo "Permisos ajustados en $DIR" ;;
  derive) chmod u+w "$DIR"; derive_urls; fix_perms ;;
  *) sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'; exit 64 ;;
esac
