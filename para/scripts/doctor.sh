#!/usr/bin/env bash
# Diagnóstico de la plataforma PARA.
#
#   scripts/doctor.sh dev    entorno local (procesos de desarrollo)   [default]
#   scripts/doctor.sh prod   configuración de producción de esta máquina
#
# Cada revisión imprime ✅ / ⚠️ / ❌ y, cuando algo falla, el comando que lo
# arregla. Sale con código 1 si hubo algún ❌. Nunca imprime secretos: de los
# .env solo lee llaves con URLs, IPs o DIDs.
#
# Compatible con el bash 3.2 de macOS (sin arrays asociativos).

set -uo pipefail

MODE="${1:-dev}"
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

# Primer valor definido de KEY en los .env de PARA, en el orden en que Expo
# los aplica (.env.local le gana a .env).
para_env() {
  local v
  v="$(env_value "$PARA_DIR/.env.local" "$1")"
  [ -z "$v" ] && v="$(env_value "$PARA_DIR/.env" "$1")"
  printf '%s' "$v"
}

# Valor por defecto de una constante en src/lib/constants.ts de PARA.
para_const() {
  grep -E "export const $1 =" "$PARA_DIR/src/lib/constants.ts" 2>/dev/null |
    grep -oE "did:[a-z]+:[a-zA-Z0-9._:-]+" | head -1
}

plc_has() { [ "$(http_code "http://localhost:2582/$1")" = "200" ]; }

# Hora de modificación más reciente de un árbol (sin node_modules), epoch.
newest_mtime() {
  python3 - "$1" <<'PY'
import os, sys
root = sys.argv[1]
best = 0
for dirpath, dirnames, filenames in os.walk(root):
    dirnames[:] = [d for d in dirnames if d not in ("node_modules", ".git")]
    for f in filenames:
        try:
            best = max(best, os.stat(os.path.join(dirpath, f)).st_mtime)
        except OSError:
            pass
print(int(best))
PY
}

# Epoch en que arrancó el proceso que escucha en PORT (vacío si no hay).
process_start() {
  local pid etime
  pid="$(lsof -tiTCP:"$1" -sTCP:LISTEN 2>/dev/null | head -1)"
  [ -z "$pid" ] && return 0
  etime="$(ps -o etime= -p "$pid" 2>/dev/null | tr -d ' ')"
  python3 - "$etime" <<'PY'
import sys, time
e = sys.argv[1]
days = 0
if "-" in e:
    d, e = e.split("-", 1); days = int(d)
parts = [int(p) for p in e.split(":")]
while len(parts) < 3:
    parts.insert(0, 0)
h, m, s = parts
print(int(time.time() - (((days * 24 + h) * 60 + m) * 60 + s)))
PY
}

fmt_age() { python3 -c "import sys,time;d=int(time.time())-int(sys.argv[1]);print(f'{d//86400} d' if d>=86400 else f'{d//3600} h' if d>=3600 else f'{d//60} min')" "$1"; }

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

section_services_dev() {
  section "Servicios locales"
  local devenv="make dev  (desde $HERE; levanta todo)"
  check_service "PLC"              http://localhost:2582/_health        200 "$devenv"
  check_service "PDS"              http://localhost:2583/xrpc/_health   200 "$devenv"
  check_service "AppView"          http://localhost:2584/xrpc/_health   200 "$devenv"
  check_service "Chat (dev)"       http://localhost:2590/xrpc/_health   any "$devenv"
  check_service "Introspección"    http://127.0.0.1:2581/               200 "$devenv"
  check_service "Matrix bridge"    http://localhost:3001/healthz        200 "$devenv  (con FRESH=1 no hay bridge)"
  check_service "Broker M8 (mubEZ)" http://localhost:8787/v1/health     200 "$devenv"
}

# ---------------------------------------------------------------------------
section_dids() {
  section "Identidades de servicio (DIDs)"
  local manifest live configured chat
  manifest="$(curl -s -m 4 http://127.0.0.1:2581/ 2>/dev/null || true)"
  live="$(printf '%s' "$manifest" | python3 -c 'import json,sys
try: print(json.load(sys.stdin)["bsky"]["did"])
except Exception: pass' 2>/dev/null)"

  configured="$(para_env EXPO_PUBLIC_LOCAL_BSKY_PROXY_DID)"
  local source=".env.local/.env de PARA"
  if [ -z "$configured" ]; then configured="$(para_const DEV_ENV_APPVIEW_DID)"; source="default en constants.ts"; fi

  if [ -z "$live" ]; then
    warn "No pude leer el DID vivo del AppView (introspección apagada)"
  elif [ "$configured" = "$live" ]; then
    ok "PARA usa el DID del AppView en ejecución ($live)"
  else
    fail "PARA hace proxy a $configured ($source), pero el AppView es $live" \
      "make dev-app-env  (pone EXPO_PUBLIC_LOCAL_BSKY_PROXY_DID=$live en PARA/.env.local) y reinicia Metro con --clear"
  fi
  if [ -n "$configured" ] && ! plc_has "$configured"; then
    fail "$configured no existe en el PLC local: el login fallará con 'could not resolve proxy did'" \
      "Usa el DID de: curl http://127.0.0.1:2581 (campo bsky.did)"
  fi

  chat="$(para_env EXPO_PUBLIC_LOCAL_CHAT_PROXY_DID)"
  [ -z "$chat" ] && chat="$(para_const DEV_ENV_CHAT_DID)"
  if [ -z "$chat" ]; then warn "No encontré el DID del chat configurado en PARA"
  elif plc_has "$chat"; then ok "DID del chat registrado en el PLC ($chat)"
  else fail "DID del chat $chat no existe en el PLC local" "Revisa EXPO_PUBLIC_LOCAL_CHAT_PROXY_DID en $PARA_DIR/.env.local"; fi
}

# ---------------------------------------------------------------------------
section_config_dev() {
  section "Configuración"
  local ip para_ip
  ip="$(lan_ip)"
  para_ip="$(para_env EXPO_PUBLIC_LOCAL_DEV_IP)"
  if [ -z "$para_ip" ]; then ok "PARA usa localhost (sin EXPO_PUBLIC_LOCAL_DEV_IP): válido para web y simulador"
  elif [ "$para_ip" = "$ip" ]; then ok "IP de la Mac coincide con EXPO_PUBLIC_LOCAL_DEV_IP ($ip)"
  else fail "EXPO_PUBLIC_LOCAL_DEV_IP=$para_ip pero la Mac hoy es ${ip:-desconocida}" \
    "make dev-app-env  (actualiza IP, M8_BROKER_URL y MATRIX_BRIDGE_URL) y reinicia Metro con --clear"; fi

  for key in EXPO_PUBLIC_M8_BROKER_URL EXPO_PUBLIC_MATRIX_BRIDGE_URL; do
    local url; url="$(para_env $key)"
    if [ -z "$url" ]; then warn "$key no está definido: PARA usará su valor por defecto"; continue; fi
    local host="${url#*://}"; host="${host%%[:/]*}"
    # Un nombre (p. ej. Mac-mini.local por mDNS) sobrevive a cambios de IP;
    # solo importa que hoy resuelva a esta Mac.
    local resolved; resolved="$(python3 -c 'import socket,sys
try: print(socket.gethostbyname(sys.argv[1]))
except Exception: pass' "$host" 2>/dev/null)"
    case "$resolved" in
      "$ip"|127.*) ok "$key → $url" ;;
      "") fail "$key apunta a $host, que no resuelve" "make dev-app-env" ;;
      *) fail "$key apunta a $host ($resolved), que no es esta Mac ($ip)" "make dev-app-env" ;;
    esac
  done

  if [ ! -f "$MUBEZ_DIR/.env" ]; then
    fail "mubEZ no tiene .env" "make setup"
  else
    # make dev le pasa PLC_URL por entorno (le gana a .env); si está corriendo,
    # manda lo que ve el proceso.
    local plc mpid
    mpid="$(lsof -tiTCP:8787 -sTCP:LISTEN 2>/dev/null | head -1)"
    [ -n "$mpid" ] && plc="$(ps eww -p "$mpid" 2>/dev/null | tr ' ' '\n' | grep '^PLC_URL=' | cut -d= -f2-)"
    [ -z "${plc:-}" ] && plc="$(env_value "$MUBEZ_DIR/.env" PLC_URL)"
    case "$plc" in
      http://localhost:2582*|http://127.0.0.1:2582*) ok "mubEZ resuelve DIDs contra el PLC local" ;;
      *) fail "mubEZ usa PLC_URL=${plc:-default} y no resolverá cuentas locales (inventa DIDs para handles)" \
           "Arráncalo con make dev (le pasa el PLC local) o pon PLC_URL=http://localhost:2582 en $MUBEZ_DIR/.env" ;;
    esac
  fi

  local bpid bm8
  bpid="$(lsof -tiTCP:3001 -sTCP:LISTEN 2>/dev/null | head -1)"
  if [ -n "$bpid" ]; then
    bm8="$(ps eww -p "$bpid" 2>/dev/null | tr ' ' '\n' | grep '^M8_BASE_URL=' | cut -d= -f2-)"
    bm8="${bm8:-http://localhost:8787/v1}"
    case "$bm8" in
      *:8787*) ok "El bridge valida tokens M8 contra $bm8" ;;
      *) warn "El bridge valida tokens M8 contra $bm8 (¿es el broker local?)" ;;
    esac
  fi
}

# ---------------------------------------------------------------------------
check_build() { # nombre dir_paquete puerto
  local name="$1" dir="$2" port="$3" src dist started
  if [ ! -d "$dir/src" ] || [ ! -d "$dir/dist" ]; then
    warn "$name: sin src/ o dist/ en $dir"; return
  fi
  src="$(newest_mtime "$dir/src")"; dist="$(newest_mtime "$dir/dist")"
  if [ "$src" -gt "$dist" ]; then
    fail "$name: el código fuente es más nuevo que el build (build de hace $(fmt_age "$dist"))" \
      "cd $WATZAPPA_DIR && make build   (luego reinicia el servicio)"
    return
  fi
  started="$(process_start "$port")"
  if [ -n "$started" ] && [ "$started" -lt "$dist" ]; then
    fail "$name: corre código anterior al último build (arrancó hace $(fmt_age "$started"))" \
      "Reinicia el proceso que escucha en :$port"
  else ok "$name: build al día"; fi
}

section_builds() {
  section "Builds (¿está corriendo el código que crees?)"
  check_build "dev-env (PLC/PDS/AppView/chat)" "$WATZAPPA_DIR/packages/dev-env"      2583
  check_build "pds"                            "$WATZAPPA_DIR/packages/pds"          2583
  check_build "bsky (AppView)"                 "$WATZAPPA_DIR/packages/bsky"         2584
  check_build "matrix-bridge"                  "$WATZAPPA_DIR/services/matrix-bridge" 3001
}

# ---------------------------------------------------------------------------
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
  dev)
    printf 'Plataforma PARA — diagnóstico local\n'
    section_repos
    section_services_dev
    section_dids
    section_config_dev
    section_builds
    ;;
  prod)
    printf 'Plataforma PARA — diagnóstico de producción\n'
    section_repos
    section_prod
    ;;
  *) echo "Uso: $0 [dev|prod]"; exit 2 ;;
esac

printf '\nResumen: %s ✅  %s ⚠️  %s ❌\n' "$OKS" "$WARNS" "$FAILS"
[ "$FAILS" -eq 0 ]
