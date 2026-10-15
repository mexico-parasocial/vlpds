#!/usr/bin/env bash
# make dev-down: apaga el entorno local y borra lo que guardó (el volumen
# anónimo del relay; lo demás ya vivía en memoria).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
docker compose -f "$HERE/compose.yaml" --env-file "$HERE/../.dev/dev.env" down -v
