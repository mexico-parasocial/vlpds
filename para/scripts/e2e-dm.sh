#!/usr/bin/env bash
# make e2e-dm — prueba punta a punta de los mensajes directos por Matrix.
#
# Aislada: un Synapse desechable en Docker (la imagen fijada en WatZappa), las
# rutas reales del bridge con pruebas de identidad reales y tres clientes de
# matrix-js-sdk con cifrado Rust (de PARA). No toca make dev ni otros stacks;
# al terminar borra el contenedor. Detalle completo en .dev/logs/e2e-dm.log.

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$HERE" || exit 2
# shellcheck source=scripts/lib.sh
. "$HERE/scripts/lib.sh"

die() { printf '\033[31m✗\033[0m %s\n' "$1" >&2; [ -n "${2:-}" ] && printf '  → %s\n' "$2" >&2; exit 2; }

BRIDGE_DIR="$WATZAPPA_DIR/services/matrix-bridge"
command -v docker >/dev/null && docker info >/dev/null 2>&1 || die "Docker no está corriendo" "Abre Docker Desktop (o colima start) y repite"
[ -d "$BRIDGE_DIR/node_modules" ] || die "El bridge de WatZappa no tiene dependencias" "make setup"
[ -d "$PARA_DIR/node_modules/matrix-js-sdk" ] || die "PARA no tiene dependencias instaladas en $PARA_DIR" "make setup"
SYNAPSE_IMAGE="$(sed -n 's/^ *image: *\(matrixdotorg\/synapse:[^ ]*\).*/\1/p' "$WATZAPPA_DIR/docker-compose.matrix.yaml" | head -n1)"
[ -n "$SYNAPSE_IMAGE" ] || die "No encuentro la imagen de Synapse en $WATZAPPA_DIR/docker-compose.matrix.yaml"

LOG="$HERE/.dev/logs/e2e-dm.log"
mkdir -p "$(dirname "$LOG")"
printf '\033[1m▸\033[0m Mensajes directos por Matrix, punta a punta (%s)\n' "$SYNAPSE_IMAGE"
(
  export BRIDGE_DIR PARA_DIR SYNAPSE_IMAGE
  cd "$BRIDGE_DIR" && with_node "$BRIDGE_DIR" pnpm exec tsx "$HERE/scripts/e2e/dm.mts"
) >"$LOG" 2>&1
status=$?
grep '^\[dm-e2e\]' "$LOG" | sed 's/^\[dm-e2e\] /  /'
if [ "$status" -ne 0 ]; then
  printf '\n\033[31m✗ Falló.\033[0m Últimas líneas (%s):\n' "$LOG"
  grep -v '^\[dm-e2e\]' "$LOG" | grep -iE 'error|assert|fail' | tail -n 15 | sed 's/^/    /'
  exit 1
fi
printf '\033[32m✓ Listo\033[0m\n'
