#!/usr/bin/env bash
# make preflight — revisa una máquina de producción ANTES de levantar nada.
#
#   1. Configuración: dominio, imágenes fijadas, bucket, perfiles.
#   2. Secretos: todos presentes, largos, distintos, sin valores de desarrollo.
#   3. Bucket: escribe, lee, lista y borra un objeto de prueba bajo S3_PREFIX,
#      y comprueba que ese objeto NO se puede leer sin credenciales.
#   4. DNS: cada nombre público resuelve.
#   5. Compose: la configuración final es válida.
#
# Sale con 1 si algo falla. `make up` en producción debería ir siempre
# después de esto (el rol de Ansible lo hace con para_preflight: true).

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$HERE" || exit 2
# shellcheck source=scripts/lib.sh
. "$HERE/scripts/lib.sh"

OKS=0; WARNS=0; FAILS=0
ok()   { OKS=$((OKS + 1));   printf '  ✅ %s\n' "$1"; }
warn() { WARNS=$((WARNS + 1)); printf '  ⚠️  %s\n' "$1"; [ -n "${2:-}" ] && printf '      → %s\n' "$2"; return 0; }
fail() { FAILS=$((FAILS + 1)); printf '  ❌ %s\n' "$1"; [ -n "${2:-}" ] && printf '      → %s\n' "$2"; return 0; }
section() { printf '\n\033[1m%s\033[0m\n' "$1"; }
cfg() { local v="${!1:-}"; [ -z "$v" ] && v="$(env_value "$HERE/.env" "$1")"; printf '%s' "$v"; }

DOMAIN="$(cfg PARA_DOMAIN)"
PROFILES="$(cfg PROFILES)"; PROFILES="${PROFILES:-core,matrix,identity,edge}"
has_profile() { case ",$PROFILES," in *",$1,"*) return 0 ;; esac; return 1; }

# --- 1 -----------------------------------------------------------------------------
section "Configuración (.env)"
[ -f "$HERE/.env" ] || fail "No hay .env" "cp .env.example .env"
case "$DOMAIN" in
  ''|example.org) fail "PARA_DOMAIN no está definido" "Ponlo en .env (para siempre: está en cada DID)" ;;
  *) ok "PARA_DOMAIN=$DOMAIN" ;;
esac
profile="$(cfg PARA_PROFILE)"; profile="${profile:-tiny}"
if [ -f "$HERE/profiles/$profile.env" ]; then ok "Perfil $profile"; else fail "No existe profiles/$profile.env" "PARA_PROFILE=tiny o standard"; fi
for p in $(echo "$PROFILES" | tr ',' ' '); do
  case "$p" in core|matrix|identity|edge|monitoring) ;; local-s3) warn "PROFILES incluye local-s3: es solo para pruebas" ;; *) fail "Perfil de Compose desconocido: $p" ;; esac
done
versions="WATZAPPA_VERSION MUBEZ_VERSION"
has_profile web && versions="$versions PARA_WEB_VERSION"
for v in $versions; do
  tag="$(cfg "$v")"
  case "$tag" in
    ''|local) if [ "${BUILD:-0}" = 1 ]; then warn "$v=${tag:-local}: se construirá en esta máquina (BUILD=1)"
              else fail "$v=${tag:-local}: en producción se descarga una imagen fijada" "Pon sha-<commit> o X.Y.Z (los publica CI), o usa BUILD=1"; fi ;;
    main|latest) fail "$v=$tag no es inmutable: dos despliegues podrían correr código distinto" "Usa sha-<commit> o X.Y.Z" ;;
    *) ok "$v=$tag" ;;
  esac
done
for v in S3_BUCKET S3_PREFIX; do
  if [ -n "$(cfg "$v")" ]; then ok "$v=$(cfg "$v")"; else fail "$v vacío" "El bucket es donde vive el estado; defínelo en .env"; fi
done
case "$(cfg S3_ENDPOINT)" in
  *ACCOUNT_ID*) fail "S3_ENDPOINT todavía tiene el ejemplo" "Pon el endpoint real de tu bucket" ;;
  http://*) has_profile local-s3 && ok "S3_ENDPOINT local ($(cfg S3_ENDPOINT))" || warn "S3_ENDPOINT usa http sin TLS" ;;
  '') ok "S3_ENDPOINT vacío (AWS S3)" ;;
  *) ok "S3_ENDPOINT=$(cfg S3_ENDPOINT)" ;;
esac
if has_profile monitoring; then
  for v in PARA_METRICS_URL PARA_LOGS_URL; do
    [ -n "$(cfg "$v")" ] && ok "$v definido" || fail "$v vacío y el perfil monitoring está activo"
  done
fi

# vlpds envuelve con la KEK las llaves de firma que guarda en el bucket; una
# KEK distinta de la que se usó al crear el prefijo no abre esos repos.
if has_profile core && [ -s "${PARA_SECRETS_DIR:-$HERE/secrets}/vlpds_kek" ] && [ -s "${PARA_SECRETS_DIR:-$HERE/secrets}/vlpds_plc_rotation_key" ]; then
  ok "vlpds tiene KEK y llave PLC envuelta (respáldalas offline: docs/RESPALDOS.md)"
fi

if has_profile matrix; then
  missing=""
  for f in deploy/matrix/synapse deploy/matrix/mas/config.yaml deploy/matrix/mas/para-idp.env; do
    [ -e "$WATZAPPA_DIR/$f" ] || missing="$missing $f"
  done
  if [ -z "$missing" ]; then ok "Configuración de Synapse y MAS generada"
  else fail "Falta la configuración de Matrix:$missing" "make matrix-init  (una vez)"; fi
  if [ -f "$WATZAPPA_DIR/deploy/matrix/mas/config.yaml" ] &&
     grep -q 'public_base: http://localhost' "$WATZAPPA_DIR/deploy/matrix/mas/config.yaml"; then
    fail "MAS está configurado para localhost: nadie fuera de la máquina podrá iniciar sesión en el chat" \
      "docs/MATRIX.md, \"MAS generado para localhost\""
  fi
  [ -f "$WATZAPPA_DIR/deploy/matrix/synapse/element-config.json" ] ||
    warn "Element usará la config versionada (matrix.para.social)" "make matrix-init la genera para $DOMAIN"
  [ -f "$WATZAPPA_DIR/deploy/matrix/synapse/para-bridge-registration.yaml" ] ||
    warn "No hay registro del appservice del bridge: Synapse no le empujará eventos" "docs/MATRIX.md, \"Registro del appservice\""
fi

# --- 2 -----------------------------------------------------------------------------
section "Secretos"
sec_out="$(PROFILES="$PROFILES" "$HERE/scripts/secrets.sh" check 2>&1)"; sec_rc=$?
printf '%s\n' "$sec_out" | grep -E '❌|→' || true
if [ "$sec_rc" -eq 0 ]; then ok "Todos los secretos de $PROFILES en orden"
else fail "Hay secretos que arreglar (arriba)" "scripts/secrets.sh check"; fi

# --- 3 -----------------------------------------------------------------------------
section "Bucket"
if [ -s secrets/s3_access_key_id ] && [ -s secrets/s3_secret_access_key ] && [ -n "$(cfg S3_BUCKET)" ]; then
  key="$(cfg S3_PREFIX)/_preflight/$(hostname -s 2>/dev/null || echo host)-$(date +%s)"
  net_args=""
  has_profile local-s3 && net_args="--network para_default"
  # shellcheck disable=SC2086
  out="$(docker run --rm $net_args -v "$HERE/secrets:/s:ro" \
    -e E="$(cfg S3_ENDPOINT)" -e R="$(cfg S3_REGION)" -e B="$(cfg S3_BUCKET)" -e K="$key" \
    -e P="$(cfg S3_FORCE_PATH_STYLE)" --entrypoint sh rclone/rclone:1.68.2 -c '
      export RCLONE_CONFIG_X_TYPE=s3 RCLONE_CONFIG_X_PROVIDER=Other RCLONE_CONFIG_X_ENDPOINT="$E"
      export RCLONE_CONFIG_X_REGION="${R:-auto}" RCLONE_CONFIG_X_FORCE_PATH_STYLE="${P:-true}" RCLONE_CONFIG_X_NO_CHECK_BUCKET=true
      export RCLONE_CONFIG_X_ACCESS_KEY_ID="$(cat /s/s3_access_key_id)" RCLONE_CONFIG_X_SECRET_ACCESS_KEY="$(cat /s/s3_secret_access_key)"
      echo "para-preflight $K" | rclone rcat "X:$B/$K" || { echo WRITE_FAIL; exit 1; }
      [ "$(rclone cat "X:$B/$K")" = "para-preflight $K" ] || { echo READ_FAIL; exit 1; }
      rclone lsf "X:$B/${K%/*}/" | grep -q "${K##*/}" || { echo LIST_FAIL; exit 1; }
      echo PUT_GET_LIST_OK
      code=$(wget -S -O /dev/null "$E/$B/$K" 2>&1 | sed -n "s/^ *HTTP\/[0-9.]* \([0-9]*\).*/\1/p" | tail -1)
      echo "ANON_${code:-none}"
      rclone deletefile "X:$B/$K" && echo DELETE_OK
    ' 2>&1)"
  case "$out" in *PUT_GET_LIST_OK*) ok "Escribe, lee y lista en s3://$(cfg S3_BUCKET)/$(cfg S3_PREFIX)/" ;;
    *) fail "El bucket no acepta las operaciones básicas: $(echo "$out" | tail -2 | tr '\n' ' ')" "Revisa endpoint, región, nombre y llaves (con permiso de lectura/escritura/borrado)" ;; esac
  case "$out" in *DELETE_OK*) ok "Borra (la retención de respaldos lo necesita)" ;; *PUT_GET_LIST_OK*) fail "No pudo borrar el objeto de prueba" "La llave necesita permiso de borrado" ;; esac
  case "$out" in
    *ANON_200*) fail "El objeto se pudo leer SIN credenciales: el bucket es público" "Quita el acceso público: ahí viven respaldos y la base de mubEZ" ;;
    *ANON_*) ok "Sin credenciales no se lee (privado)" ;;
  esac
else
  fail "Faltan las llaves o el nombre del bucket; no lo pude probar"
fi

# --- 4 -----------------------------------------------------------------------------
if has_profile edge && [ -n "$DOMAIN" ] && [ "$DOMAIN" != example.org ]; then
  section "DNS"
  hosts="pds appview ozone bridge"
  has_profile identity && hosts="$hosts m8"
  has_profile matrix && hosts="$hosts matrix auth idp chat"
  for h in "$DOMAIN" $(for s in $hosts; do echo "$s.$DOMAIN"; done); do
    ipaddr="$(python3 -c 'import socket,sys
try: print(socket.gethostbyname(sys.argv[1]))
except Exception: pass' "$h")"
    if [ -n "$ipaddr" ]; then ok "$h → $ipaddr"; else fail "$h no resuelve" "Crea el registro A hacia esta máquina; Caddy lo necesita para el certificado"; fi
  done
  handle_domain="$(cfg PARA_HANDLE_DOMAIN)"; handle_domain="${handle_domain:-.$DOMAIN}"
  probe="handle-check-$(date +%s)$handle_domain"
  if python3 -c 'import socket,sys; socket.gethostbyname(sys.argv[1])' "$probe" 2>/dev/null; then
    ok "Comodín *$handle_domain resuelve (handles de usuario)"
  else
    fail "*$handle_domain no resuelve: los handles de usuario no tendrán HTTPS" "Crea un registro A comodín *$handle_domain hacia esta máquina"
  fi
fi

# --- 5 -----------------------------------------------------------------------------
section "Compose"
if make -s -C "$HERE" config >/dev/null 2>&1; then ok "La configuración final es válida (make config)"
else fail "docker compose config falla" "make config  para ver el error"; fi

printf '\nResumen: %s ✅  %s ⚠️  %s ❌\n' "$OKS" "$WARNS" "$FAILS"
[ "$FAILS" -eq 0 ]
