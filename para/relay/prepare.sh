#!/usr/bin/env bash
# Materialize exactly the audited upstream revision plus the checked-in patch.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
. "$HERE/source.env"
DEST="${1:?usage: prepare.sh EMPTY_CHECKOUT_DIRECTORY}"
if [ -e "$DEST" ]; then
  echo "prepare.sh: destination already exists; use an empty directory: $DEST" >&2
  exit 2
fi
mkdir -p "$DEST"
git -C "$DEST" init -q
git -C "$DEST" remote add origin "$RSKY_REPOSITORY"
git -C "$DEST" fetch -q --depth 1 origin "$RSKY_COMMIT"
git -C "$DEST" checkout -q --detach FETCH_HEAD
[ "$(git -C "$DEST" rev-parse HEAD)" = "$RSKY_COMMIT" ]
for patch in "$HERE"/patches/*.patch; do
  git -C "$DEST" apply --check "$patch"
  git -C "$DEST" apply "$patch"
done
printf '%s\n' "$RSKY_COMMIT+$RSKY_PATCH_VERSION" > "$DEST/.para-source"
