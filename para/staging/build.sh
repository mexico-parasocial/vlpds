#!/usr/bin/env bash
# Sequential builds keep the local staging services responsive.
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
. "$HERE/scripts/lib.sh"
image_exists() { docker image inspect "$1" >/dev/null 2>&1; }
image_exists para-plc:9c8ea2f || docker build -t para-plc:9c8ea2f -f packages/server/Dockerfile \
  'https://github.com/did-method-plc/did-method-plc.git#9c8ea2fe23b89a5c1011246cbb4957dad9dbf7db'
image_exists para-bsync:ea85ae3 || docker build -t para-bsync:ea85ae3 -f services/bsync/Dockerfile \
  'https://github.com/mexico-parasocial/WatZappa.git#ea85ae3561e8d104fa46a7ec0fa7ee51cfc91cf9'
docker build -t para-vlpds:para-native-local "$HERE/.."
docker build -t para-rsky-relay:e96634e-para2 "$HERE/relay"
docker build -t vlpds-minio:chat-staging -f "$HERE/../build/Dockerfile.minio" "$HERE/.."
docker build -t para-chat-staging-bsky:local -f "$WATZAPPA_DIR/services/bsky/Dockerfile" "$WATZAPPA_DIR"
docker build -t para-chat-staging-bridge:local -f "$WATZAPPA_DIR/services/matrix-bridge/Dockerfile" "$WATZAPPA_DIR"
docker build -t watzappa-para-idp:latest -f "$WATZAPPA_DIR/services/para-idp/Dockerfile" "$WATZAPPA_DIR"
docker build -t para-chat-staging-m8:local "$MUBEZ_DIR"

docker build -t para-chat-staging-console:local -f "$HERE/staging/Dockerfile.console" "$HERE/.."
