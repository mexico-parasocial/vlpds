#!/usr/bin/env bash
# make verify — comprueba un despliegue DESPUÉS de `make up`.
#
#   1. Contenedores: todos los de PROFILES corriendo y, si tienen healthcheck,
#      sanos.
#   2. Endpoints públicos (si el perfil edge está activo): salud de cada
#      servicio a través de Caddy y con HTTPS válido.
#   3. Estado durable: mubEZ replica al bucket, los respaldos corren.
#
# Son las mismas comprobaciones que el rol de Ansible imprime al final, y las
# que hay que repetir a mano tras cualquier cambio. Sale con 1 si algo falla.

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
compose() { make -s -C "$HERE" compose ARGS="$*"; }

section "Contenedores"
states="$(compose ps --all --format '{{.Service}}|{{.State}}|{{.Health}}|{{.ExitCode}}' 2>/dev/null)"
if [ -z "$states" ]; then
  fail "No hay contenedores del proyecto para" "make up"
else
  while IFS='|' read -r svc state health code; do
    [ -z "$svc" ] && continue
    case "$svc" in synapse-init) [ "$state" = exited ] && [ "$code" = 0 ] && { ok "$svc terminó bien"; continue; } ;; esac
    if [ "$state" != running ]; then fail "$svc: $state (código $code)" "make logs S=$svc"
    elif [ "$health" = unhealthy ]; then fail "$svc: corre pero su healthcheck falla" "make logs S=$svc"
    elif [ "$health" = starting ]; then warn "$svc: todavía arrancando" "Repite make verify en un minuto"
    else ok "$svc${health:+ ($health)}"; fi
  done <<EOF
$states
EOF
fi

if has_profile edge && [ -n "$DOMAIN" ] && [ "$DOMAIN" != example.org ]; then
  section "Endpoints públicos de $DOMAIN"
  check() { # nombre url
    local code
    code="$(curl -s -m 8 -o /dev/null -w '%{http_code}' "$2" 2>/dev/null || true)"
    if [ "$code" = 200 ]; then ok "$1 ($2)"
    elif [ "$code" = 000 ]; then fail "$1: sin respuesta o TLS inválido en $2" "make logs S=caddy  (¿DNS? ¿puertos 80/443?)"
    else fail "$1: HTTP $code en $2" "make logs S=caddy"; fi
  }
  check "PDS" "https://pds.$DOMAIN/xrpc/_health"
  check "PDS describeServer" "https://pds.$DOMAIN/xrpc/com.atproto.server.describeServer"
  check "AppView" "https://appview.$DOMAIN/xrpc/_health"
  check "DID del AppView" "https://appview.$DOMAIN/.well-known/did.json"
  check "Ozone" "https://ozone.$DOMAIN/xrpc/_health"
  has_profile matrix && check "Bridge" "https://bridge.$DOMAIN/healthz"
  if has_profile matrix; then
    matrix_host="$(cfg MATRIX_SERVER_NAME)"; matrix_host="${matrix_host:-matrix.$DOMAIN}"
    check "Matrix: descubrimiento" "https://$matrix_host/.well-known/matrix/client"
    check "Matrix /login (vía MAS)" "https://$matrix_host/_matrix/client/v3/login"
    check "MAS (emisor OIDC)" "https://auth.$DOMAIN/.well-known/openid-configuration"
    check "para-idp" "https://idp.$DOMAIN/healthz"
    check "Element" "https://chat.$DOMAIN/"
  fi
  if has_profile web; then
    check "App web" "https://$DOMAIN/"
    check "App web /matrix-auth" "https://$DOMAIN/matrix-auth"
  fi
  has_profile identity && check "Broker M8" "https://m8.$DOMAIN/v1/health"
  code="$(curl -s -m 8 -o /dev/null -w '%{http_code}' "https://pds.$DOMAIN/xrpc/com.atproto.admin.getAccountInfos" 2>/dev/null || true)"
  case "$code" in
    404) ok "Las rutas de operador del PDS no se ven desde fuera (404)" ;;
    *) warn "com.atproto.admin.* respondió $code desde esta máquina" "Normal si sales a internet por una IP privada; desde fuera debe ser 404" ;;
  esac
fi

if has_profile core; then
  section "Handles de usuario"
  code="$(compose exec -T pds wget -qO- "http://127.0.0.1:2583/tls-check?domain=pds.$DOMAIN" 2>/dev/null || true)"
  if [ "$code" = OK ]; then ok "El PDS responde /tls-check (Caddy emite certificados de handles bajo demanda)"
  else fail "El PDS no responde /tls-check: los handles de usuario no tendrán certificado" "¿La imagen del PDS incluye /tls-check? WATZAPPA_VERSION reciente"; fi
fi

section "Estado durable"
if has_profile identity; then
  if compose logs --no-log-prefix mubez 2>/dev/null | grep -q "replicating to"; then ok "mubEZ replica su base al bucket (Litestream)"
  else fail "mubEZ no está replicando al bucket" "make logs S=mubez  (busca 'litestream')"; fi
fi
if has_profile core; then
  m="$(compose exec -T backup wget -qO- http://127.0.0.1:9101/metrics 2>/dev/null || true)"
  if [ -z "$m" ]; then warn "El primer respaldo todavía no corre (espera ~2 min tras arrancar)" "make backup  lo corre ya"
  elif printf '%s' "$m" | grep -q 'para_backup_last_run_success{[^}]*} 0'; then fail "El último respaldo falló" "make logs S=backup"
  else ok "Respaldos al bucket funcionando"; fi
fi

printf '\nResumen: %s ✅  %s ⚠️  %s ❌\n' "$OKS" "$WARNS" "$FAILS"
[ "$FAILS" -eq 0 ]
