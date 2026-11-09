#!/usr/bin/env bash
set -euo pipefail
umask 077
HERE="$(cd "$(dirname "$0")" && pwd)"
STATE="$HERE/../.dev/chat-staging"
compose() { docker compose -p para-chat-staging -f "$STATE/compose.json" "$@"; }
case "${1:-}" in
  build|setup|up|down|smoke|ops|snapshot|restore)
    mkdir -p "$STATE"
    LOCK="$STATE/operation.lock"
    if ! mkdir "$LOCK" 2>/dev/null; then
      previous="$(cat "$LOCK/pid" 2>/dev/null || true)"
      if [[ "$previous" =~ ^[0-9]+$ ]] && ! kill -0 "$previous" 2>/dev/null; then
        rm -f "$LOCK/pid"
        rmdir "$LOCK"
        mkdir "$LOCK"
      else
        echo 'Another staging operation is active; run build, smoke, snapshot and restore sequentially.' >&2
        exit 2
      fi
    fi
    printf '%s\n' "$$" > "$LOCK/pid"
    trap 'rm -f "$LOCK/pid"; rmdir "$LOCK"' EXIT
    ;;
esac
case "${1:-}" in
  build) "$HERE/build.sh" ;;
  setup) python3 "$HERE/setup.py" ;;
  up)
    python3 "$HERE/setup.py"
    compose stop bridge >/dev/null 2>&1 || true
    compose up -d --force-recreate postgres minio bucket-init plc pds relay bsync dataplane appview idp synapse mas m8
    python3 "$HERE/bootstrap.py"
    compose up -d --no-deps --force-recreate bridge console
    ;;
  down) compose down ;;
  status) compose ps ;;
  logs) compose logs --tail=80 "${2:?service required}" ;;
  verify) python3 "$HERE/verify.py" ;;
  smoke)
    PLATFORM="$(cd "$HERE/.." && pwd)"
    if [[ -f "$STATE/tls/ca.crt" ]]; then export NODE_EXTRA_CA_CERTS="$STATE/tls/ca.crt"; fi
    ( HERE="$PLATFORM"; . "$PLATFORM/scripts/lib.sh"; cd "$MUBEZ_DIR"; with_node "$MUBEZ_DIR" pnpm exec tsx "$PLATFORM/staging/smoke.mts" "${@:2}" )
    ;;
  ops) python3 "$HERE/ops.py" ;;
  app-env)
    if python3 -c 'import json,sys;sys.exit(not bool(json.load(open(sys.argv[1])).get("publicUrls")))' "$STATE/manifest.json"; then
      python3 "$HERE/lan_https.py" app-env
      exit 0
    fi
    cat <<'EOF'
EXPO_PUBLIC_ENV=development
EXPO_PUBLIC_DEFAULT_SERVICE=http://localhost:32583
EXPO_PUBLIC_BLUESKY_PROXY_DID=did:web:localhost%3A32584
EXPO_PUBLIC_M8_BROKER_URL=http://localhost:38787
EXPO_PUBLIC_MATRIX_BRIDGE_URL=http://localhost:33001
EXPO_PUBLIC_MATRIX_IDP_URL=http://localhost:38090
EOF
    ;;
  snapshot) python3 "$HERE/backup.py" snapshot ;;
  restore) python3 "$HERE/backup.py" restore --snapshot "${2:?snapshot directory required}" --confirm-staging-restore ;;
  *) echo 'Usage: staging/stage.sh build|setup|up|down|status|logs SERVICE|verify|smoke|ops|app-env|snapshot|restore SNAPSHOT' >&2; exit 2 ;;
esac
