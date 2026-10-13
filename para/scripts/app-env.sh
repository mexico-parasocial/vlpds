#!/usr/bin/env bash
# make app-env — las variables con que se compila la app PARA para esta
# plataforma (EAS para iOS/Android, o el build web).
#
# La app hornea estas URLs al compilar; sin ellas usa sus valores por defecto,
# que apuntan a los servicios de Bluesky. Salen de PARA_DOMAIN igual que las
# publica el edge. Imprime un .env listo para EAS (eas.json "env", o
# `eas env:create`) o para --build-arg.
#
#   make app-env               a la terminal
#   make app-env > app.env     a un archivo

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib.sh
. "$HERE/scripts/lib.sh"

cfg() { local v="${!1:-}"; [ -z "$v" ] && v="$(env_value "$HERE/.env" "$1")"; printf '%s' "$v"; }
d="$(cfg PARA_DOMAIN)"
case "$d" in ''|example.org) echo "Define PARA_DOMAIN en .env" >&2; exit 2 ;; esac
appview_did="$(cfg BSKY_SERVER_DID)"; appview_did="${appview_did:-did:web:appview.$d}"

cat <<ENV
# App PARA para https://$d (generado por para-platform: make app-env)
EXPO_PUBLIC_ENV=production
EXPO_PUBLIC_DEFAULT_SERVICE=https://pds.$d
EXPO_PUBLIC_BLUESKY_PROXY_DID=$appview_did
EXPO_PUBLIC_M8_BROKER_URL=https://m8.$d
EXPO_PUBLIC_MATRIX_BRIDGE_URL=https://bridge.$d
EXPO_PUBLIC_MATRIX_IDP_URL=https://idp.$d
EXPO_PUBLIC_MATRIX_PUSH_GATEWAY_URL=https://bridge.$d/_matrix/push/v1/notify
# Sin servicio chat.bsky propio: la app manda esas peticiones al de Bluesky
# (did:web:api.bsky.chat) salvo que pongas aquí otro DID. Ver docs/APP.md.
# EXPO_PUBLIC_CHAT_PROXY_DID=
# Mensajes directos cifrados por Matrix (solicitud con consentimiento; docs/MATRIX.md).
# EXPO_PUBLIC_MATRIX_DMS=1
ENV
