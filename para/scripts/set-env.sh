#!/usr/bin/env bash
# set-env.sh LLAVE VALOR [LLAVE VALOR...] — fija valores en .env (los agrega
# si faltan). Lo usa `make upgrade`; rechaza tags mutables.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
f="$HERE/.env"
[ -f "$f" ] || { echo "No hay .env" >&2; exit 2; }
while [ $# -ge 2 ]; do
  key="$1"; val="$2"; shift 2
  case "$key:$val" in
    *_VERSION:main|*_VERSION:latest|*_VERSION:) echo "$key=$val no es un tag inmutable; usa sha-<commit> o X.Y.Z" >&2; exit 2 ;;
  esac
  if grep -qE "^$key=" "$f"; then
    tmp="$(mktemp)"; sed "s|^$key=.*|$key=$val|" "$f" > "$tmp" && cat "$tmp" > "$f" && rm -f "$tmp"
  else
    printf '%s=%s\n' "$key" "$val" >> "$f"
  fi
  echo "  $key=$val"
done
