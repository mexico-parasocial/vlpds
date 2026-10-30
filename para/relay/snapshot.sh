#!/usr/bin/env bash
# Invoked by make relay-snapshot. Copies the complete stopped relay volume.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$HERE"
OUTPUT="${RELAY_SNAPSHOT_DIR:-$HERE/.dev/relay-snapshots}"
mkdir -p "$OUTPUT"
OUTPUT="$(cd "$OUTPUT" && pwd)"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
NAME="rsky-relay-$STAMP.tar.gz"
[ ! -e "$OUTPUT/$NAME" ] || { echo "Snapshot already exists: $OUTPUT/$NAME" >&2; exit 2; }
container="$(make -s --no-print-directory compose ARGS='ps -q relay')"
[ -n "$container" ] || { echo 'The relay is not running' >&2; exit 2; }
image="$(docker inspect --format '{{.Config.Image}}' "$container")"
case "$image" in
  *rsky-relay:*) ;;
  *vlrelay*) echo 'vlRelay keeps its state in the bucket (S3_PREFIX/vlrelay); see docs/vlrelay.md' >&2; exit 2 ;;
  *) echo 'Snapshot requires the rsky relay; keep Indigo backups separate' >&2; exit 2 ;;
esac
make --no-print-directory stop S=relay
restart() { make --no-print-directory compose ARGS='start relay' >&2; }
trap restart EXIT
docker run --rm --user 0:0 --volumes-from "$container:ro" \
  -v "$OUTPUT:/snapshot" --entrypoint sh "$image" \
  -c 'umask 077; tar -C /data/relay -czf "$1" . && chown "$2:$3" "$1"' \
  sh "/snapshot/$NAME.tmp" "$(id -u)" "$(id -g)"
mv "$OUTPUT/$NAME.tmp" "$OUTPUT/$NAME"
printf '%s\n' "Snapshot: $OUTPUT/$NAME"
