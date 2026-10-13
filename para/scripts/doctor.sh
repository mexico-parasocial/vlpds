#!/usr/bin/env bash
# Diagnóstico de la plataforma PARA.
#
#   scripts/doctor.sh prod   configuración de producción de esta máquina
#
# El entorno local (make dev, con vlpds) se revisa con make dev-smoke.
#
# Cada revisión imprime ✅ / ⚠️ / ❌ y, cuando algo falla, el comando que lo
# arregla. Sale con código 1 si hubo algún ❌. Nunca imprime secretos: de los
# .env solo lee llaves con URLs, IPs o DIDs.
#
# Compatible con el bash 3.2 de macOS (sin arrays asociativos).

set -uo pipefail

MODE="${1:-prod}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$HERE" || exit 2

# Rutas de los repos y funciones comunes (env_value, repo_dir, lan_ip).
# shellcheck source=scripts/lib.sh
. "$HERE/scripts/lib.sh"

OKS=0; WARNS=0; FAILS=0
ok()   { OKS=$((OKS + 1));   printf '  ✅ %s\n' "$1"; }
warn() { WARNS=$((WARNS + 1)); printf '  ⚠️  %s\n' "$1"; [ -n "${2:-}" ] && printf '      → %s\n' "$2"; return 0; }
fail() { FAILS=$((FAILS + 1)); printf '  ❌ %s\n' "$1"; [ -n "${2:-}" ] && printf '      → %s\n' "$2"; return 0; }
section() { printf '\n\033[1m%s\033[0m\n' "$1"; }

http_code() { curl -s -m 4 -o /dev/null -w '%{http_code}' "$1" 2>/dev/null || true; }

# ---------------------------------------------------------------------------
section_repos() {
  section "Repositorios"
  local name dir branch dirty
  for pair in "WatZappa:$WATZAPPA_DIR" "mubEZ:$MUBEZ_DIR" "PARA:$PARA_DIR" "iM8:$IM8_DIR"; do
    name="${pair%%:*}"; dir="${pair#*:}"
    if [ ! -d "$dir" ]; then
      fail "$name no encontrado en $dir" "Define ${name}_DIR en .env (mayúsculas: WATZAPPA_DIR, MUBEZ_DIR, PARA_DIR, IM8_DIR)"
      continue
    fi
    if ! branch="$(git -C "$dir" branch --show-current 2>/dev/null)"; then
      warn "$name: git no responde" "Si dice 'Xcode license', ejecuta: sudo xcodebuild -license"
      continue
    fi
    dirty="$(git -C "$dir" status --porcelain 2>/dev/null | wc -l | tr -d ' ')"
    if [ "$dirty" = "0" ]; then ok "$name en rama $branch"
    else warn "$name en rama $branch con $dirty archivos sin commit"; fi
  done
}

# ---------------------------------------------------------------------------
check_service() { # nombre url esperado(200|any) arreglo
  local code; code="$(http_code "$2")"
  if [ "$3" = "any" ] && [ "$code" != "000" ] && [ -n "$code" ]; then ok "$1 responde ($2)"
  elif [ "$code" = "200" ]; then ok "$1 responde ($2)"
  else fail "$1 no responde en $2 (HTTP $code)" "$4"; fi
}

section_prod() {
  section "Producción (configuración de esta máquina)"
  local domain; domain="$(env_value .env PARA_DOMAIN)"
  if [ -z "$domain" ] || [ "$domain" = "example.org" ]; then
    fail "PARA_DOMAIN no está definido en $HERE/.env" "cp .env.example .env y define PARA_DOMAIN"
  else ok "PARA_DOMAIN=$domain"; fi

  local v tag
  for v in WATZAPPA_VERSION MUBEZ_VERSION; do
    tag="$(env_value .env "$v")"
    case "$tag" in
      ''|local|main|latest) fail "$v=${tag:-local}: no es una imagen fijada" "Usa sha-<commit> o X.Y.Z; make upgrade $v=..." ;;
      *) ok "$v=$tag" ;;
    esac
  done

  if "$HERE/scripts/secrets.sh" check >/dev/null 2>&1; then ok "Secretos en orden (secrets/)"
  else fail "Hay secretos que arreglar" "scripts/secrets.sh check"; fi

  if grep -q 'NODE_ENV: production' "$HERE/overrides/identity.yaml" 2>/dev/null &&
     grep -q 'dev_token_bootstrap":false' "$HERE/overrides/identity.yaml" 2>/dev/null; then
    ok "mubEZ se fuerza a producción sin dev-token bootstrap"
  else
    fail "overrides/identity.yaml no fuerza NODE_ENV=production y dev_token_bootstrap=false" "Cualquiera podría pedir tokens M8 para cualquier DID"
  fi
  local cors; cors="$(env_value .env M8_CORS_ORIGIN)"
  if [ "$cors" = "*" ]; then fail "M8_CORS_ORIGIN está abierto (*)" "Lista solo tus dominios, p. ej. https://$domain"
  else ok "CORS de mubEZ restringido (${cors:-https://$domain})"; fi

  [ -z "$domain" ] || [ "$domain" = "example.org" ] && return 0
  section "Endpoints públicos de $domain"
  check_service "PDS"        "https://pds.$domain/xrpc/_health"     200 "make logs S=pds"
  check_service "AppView"    "https://appview.$domain/xrpc/_health" 200 "make logs S=bsky"
  check_service "Ozone"      "https://ozone.$domain/xrpc/_health"   200 "make logs S=ozone"
  check_service "Bridge"     "https://bridge.$domain/healthz"       200 "make logs S=matrix-bridge"
  check_service "Broker M8"  "https://m8.$domain/v1/health"         200 "make logs S=mubez"
  printf '\n  Revisión completa del despliegue: make verify\n'
}

# ---------------------------------------------------------------------------
case "$MODE" in
  prod)
    printf 'Plataforma PARA — diagnóstico de producción\n'
    section_repos
    section_prod
    ;;
  *) echo "Uso: $0 [prod]"; exit 2 ;;
esac

printf '\nResumen: %s ✅  %s ⚠️  %s ❌\n' "$OKS" "$WARNS" "$FAILS"
[ "$FAILS" -eq 0 ]
