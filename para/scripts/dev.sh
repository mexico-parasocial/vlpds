#!/usr/bin/env bash
# make dev — todo el entorno local de PARA con un comando, en una terminal.
#
#   make dev                 backend: PLC, PDS, AppView, chat y bridge
#                            (WatZappa) + broker M8 (mubEZ)
#   make dev APP=web         además la app (web | ios | android | expo)
#   make dev MATRIX=1        además Synapse/MAS/Element (Docker)
#   make dev FRESH=1         perfil desechable: nada persiste al salir
#
# Arranca cada proceso con la versión de Node de su repo (.nvmrc, vía nvm si
# está instalado), espera a que cada uno responda su health check, imprime
# las URLs y se queda mostrando los logs de todos con prefijo. Ctrl-C detiene
# todo. Si un proceso muere, detiene los demás y muestra sus últimas líneas.
# Los logs completos quedan en .dev/logs/<servicio>.log (make dev-logs S=pds).
#
# No edita ningún .env: mubEZ recibe PLC_URL y PDS_URL locales por entorno.
# Postgres y Redis de desarrollo (Docker) se quedan corriendo al salir, como
# en WatZappa; `make dev-down` los apaga.
#
# Compatible con el bash 3.2 de macOS.

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$HERE" || exit 2
# shellcheck source=scripts/lib.sh
. "$HERE/scripts/lib.sh"

APP="${APP:-}"
MATRIX="${MATRIX:-0}"
FRESH="${FRESH:-0}"
LOG_DIR="$HERE/.dev/logs"
mkdir -p "$LOG_DIR"

PIDS=""
NAMES=""
STARTED_AT="$(date +%s)"

# --- antes de arrancar ----------------------------------------------------------
die() { printf '\033[31m✗\033[0m %s\n' "$1" >&2; [ -n "${2:-}" ] && printf '  → %s\n' "$2" >&2; exit 2; }
step() { printf '\033[1m▸\033[0m %s\n' "$1"; }

command -v docker >/dev/null && docker info >/dev/null 2>&1 || die "Docker no está corriendo" "Abre Docker Desktop (o colima start) y repite"
[ -d "$WATZAPPA_DIR" ] || die "No encuentro WatZappa en $WATZAPPA_DIR" "Define WATZAPPA_DIR en .env"
[ -d "$MUBEZ_DIR" ] || die "No encuentro mubEZ en $MUBEZ_DIR" "Define MUBEZ_DIR en .env"
[ -d "$WATZAPPA_DIR/node_modules" ] || die "WatZappa no tiene dependencias instaladas" "make setup"
[ -d "$MUBEZ_DIR/node_modules" ] || die "mubEZ no tiene dependencias instaladas" "make setup"
[ -f "$MUBEZ_DIR/.env" ] || die "mubEZ no tiene .env" "make setup (lo crea con secretos de desarrollo)"
if [ -n "$APP" ]; then
  [ -d "$PARA_DIR/node_modules" ] || die "PARA no tiene dependencias instaladas en $PARA_DIR" "make setup"
  case "$APP" in web|ios|android|expo|1) ;; *) die "APP=$APP no existe" "Usa APP=web, ios, android o expo" ;; esac
fi

busy=""
for port in 2581 2582 2583 2584 2587 2590 3001 8787; do
  if lsof -nP -tiTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1; then busy="$busy $port"; fi
done
[ -z "$busy" ] || die "Puertos ocupados:$busy (¿otro make dev?)" "lsof -nP -iTCP:${busy# } -sTCP:LISTEN  para ver quién"

# --- procesos -------------------------------------------------------------------
COLORS="36 35 33 32 34 31"
start() { # start NOMBRE DIR CMD...
  local name="$1" dir="$2"; shift 2
  local color; color="$(echo "$COLORS" | cut -d' ' -f$(( ($(echo "$NAMES" | wc -w) % 6) + 1 )))"
  local tag; tag="$(printf '\033[%sm%-7s\033[0m│ ' "$color" "$name")"
  : > "$LOG_DIR/$name.log"
  if [ -n "$APP" ]; then
    # La app se queda con la terminal; el backend solo escribe a su log.
    ( cd "$dir" && with_node "$dir" "$@" ) >>"$LOG_DIR/$name.log" 2>&1 &
  else
    ( cd "$dir" && with_node "$dir" "$@" ) > >(tee -a "$LOG_DIR/$name.log" | awk -v t="$tag" '{ print t $0; fflush() }') 2>&1 &
  fi
  PIDS="$PIDS $!"
  NAMES="$NAMES $name"
}

kill_tree() { # kill_tree PID SEÑAL
  local child
  for child in $(pgrep -P "$1" 2>/dev/null); do kill_tree "$child" "$2"; done
  kill -"$2" "$1" 2>/dev/null || true
}

STOPPING=0
cleanup() {
  [ "$STOPPING" = 1 ] && return
  STOPPING=1
  printf '\n\033[1m▸\033[0m Deteniendo...\n'
  local pid
  for pid in $PIDS; do kill_tree "$pid" TERM; done
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    alive=0
    for pid in $PIDS; do kill -0 "$pid" 2>/dev/null && alive=1; done
    [ "$alive" = 0 ] && break
    sleep 1
  done
  for pid in $PIDS; do kill_tree "$pid" KILL; done
  [ "$FRESH" = 1 ] && [ -n "${FRESH_DIR:-}" ] && rm -rf "$FRESH_DIR"
  echo "Listo. Postgres/Redis de desarrollo siguen arriba (make dev-down los apaga)."
}
trap cleanup EXIT
trap 'exit 130' INT TERM

nth() { echo "$1" | tr -s ' ' '\n' | sed '/^$/d' | sed -n "$2p"; }

dead_process() { # imprime el nombre del primer proceso muerto, si hay
  local i=1 pid
  for pid in $PIDS; do
    if ! kill -0 "$pid" 2>/dev/null; then nth "$NAMES" "$i"; return 0; fi
    i=$((i + 1))
  done
  return 1
}

fail_with_log() { # fail_with_log NOMBRE
  printf '\n\033[31m✗ %s se detuvo.\033[0m Últimas líneas (%s):\n' "$1" "$LOG_DIR/$1.log"
  tail -n 25 "$LOG_DIR/$1.log" | sed 's/^/    /'
  exit 1
}

wait_http() { # wait_http NOMBRE URL SEGUNDOS
  local name="$1" url="$2" limit="$3" t=0 code dead
  while [ "$t" -lt "$limit" ]; do
    code="$(curl -s -m 2 -o /dev/null -w '%{http_code}' "$url" 2>/dev/null || true)"
    case "$code" in 2??) printf '  \033[32m✓\033[0m %-14s %s\n' "$name" "$url"; return 0 ;; esac
    if dead="$(dead_process)"; then fail_with_log "$dead"; fi
    sleep 2; t=$((t + 2))
  done
  printf '  \033[31m✗\033[0m %-14s %s no respondió en %ss\n' "$name" "$url" "$limit"
  exit 1
}

# --- arranque -------------------------------------------------------------------
if [ "$MATRIX" = 1 ]; then
  step "Matrix (Synapse, MAS, Element) en Docker"
  (cd "$WATZAPPA_DIR" && make matrix-up) || die "No arrancó el stack de Matrix" "cd $WATZAPPA_DIR && make matrix-status"
fi

if [ "$FRESH" = 1 ]; then
  FRESH_DIR="$(mktemp -d "${TMPDIR:-/tmp}/para-dev.XXXXXX")"
  step "Perfil desechable (nada persiste): $FRESH_DIR"
  start m8 "$MUBEZ_DIR" env PLC_URL=http://localhost:2582 PDS_URL=http://localhost:2583 \
    DATABASE_PATH="$FRESH_DIR/mubez.db" DATABASE_AUTOMIGRATE=true pnpm dev
  start atproto "$WATZAPPA_DIR" env NODE_ENV=development make run-demo-env
else
  step "Perfil persistente (los datos sobreviven entre sesiones; make dev-reset los borra)"
  start m8 "$MUBEZ_DIR" env PLC_URL=http://localhost:2582 PDS_URL=http://localhost:2583 pnpm dev
  start atproto "$WATZAPPA_DIR" make run-dev-env
fi

step "Esperando a cada servicio (la primera vez compila WatZappa: unos minutos)"
wait_http "Broker M8" http://localhost:8787/v1/health 180
wait_http "PLC" http://localhost:2582/_health 900
wait_http "PDS" http://localhost:2583/xrpc/_health 300
wait_http "AppView" http://localhost:2584/xrpc/_health 300
[ "$FRESH" = 1 ] || wait_http "Matrix bridge" http://localhost:3001/healthz 300

ip="$(lan_ip)"
printf '\n\033[1;32m✓ Todo arriba en %ss\033[0m\n\n' "$(( $(date +%s) - STARTED_AT ))"
cat <<EOF
  PLC            http://localhost:2582
  PDS            http://localhost:2583
  AppView        http://localhost:2584
  Chat (dev)     http://localhost:2590
  Bridge         http://localhost:3001
  Broker M8      http://localhost:8787
  Introspección  http://127.0.0.1:2581   (DIDs y cuentas de prueba)

  Teléfono físico: make dev-app-env  (pone ${ip:-la IP de esta Mac} y el DID del AppView en PARA/.env.local)
  Datos de prueba: make seed    Diagnóstico: make doctor    Logs: make dev-logs S=atproto

EOF

if [ -n "$APP" ]; then
  [ "$APP" = 1 ] && APP=expo
  step "App PARA ($APP). El backend sigue en segundo plano; sus logs: make dev-logs"
  case "$APP" in
    web) app_cmd="npx expo start --web" ;;
    ios) app_cmd="npx expo run:ios" ;;
    android) app_cmd="npx expo run:android" ;;
    *) app_cmd="npx expo start" ;;
  esac
  # En primer plano, con la terminal: Expo necesita su menú interactivo.
  (cd "$PARA_DIR" && with_node "$PARA_DIR" $app_cmd) || true
  exit 0
fi

# Sin wait -n en bash 3.2: revisa cada 2 s si alguien murió.
while true; do
  if dead="$(dead_process)"; then fail_with_log "$dead"; fi
  sleep 2
done
