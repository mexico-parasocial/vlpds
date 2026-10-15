#!/bin/sh
# Loads secrets from files before starting the service.
#
# For every variable NAME_FILE whose value is a path under /run/secrets/, this
# reads the file into NAME and drops NAME_FILE, then execs the command. That is
# how Docker/Compose secrets reach services that only read plain environment
# variables, without the secret ever appearing in `docker inspect`.
#
# It refuses to start, instead of guessing, when:
#   - NAME and NAME_FILE are both set (which one would win is not obvious),
#   - the file is missing or unreadable,
#   - the file is empty.
#
# Variables ending in _FILE that point anywhere else are left alone, so a
# service's own *_FILE settings keep working.
#
# WatZappa's images don't ship it, so the platform mounts this file and makes
# it the entrypoint (core.yaml, overrides/matrix.yaml): no change to WatZappa.

set -eu

for name in $(env | sed -n 's/^\([A-Za-z_][A-Za-z0-9_]*\)_FILE=\/run\/secrets\/.*/\1/p'); do
  file="$(printenv "${name}_FILE")"
  if [ -n "$(printenv "$name" || true)" ]; then
    echo "env-from-files: $name and ${name}_FILE are both set; keep only one" >&2
    exit 64
  fi
  if [ ! -r "$file" ]; then
    echo "env-from-files: ${name}_FILE points to $file, which is missing or unreadable" >&2
    exit 64
  fi
  # $(cat) strips trailing newlines, which is what we want for a secret file.
  value="$(cat "$file")"
  if [ -z "$value" ]; then
    echo "env-from-files: $file (for $name) is empty" >&2
    exit 64
  fi
  export "$name=$value"
  unset "${name}_FILE"
done

exec "$@"
