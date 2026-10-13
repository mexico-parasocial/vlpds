#!/bin/sh
# Restaura desde el bucket lo que backup.sh respaldó.
#
#   restore.sh list [postgres/<base>|pds]
#   restore.sh postgres <base> [<fecha>|latest] [--force]
#   restore.sh pds [<fecha>|latest] [--force]
#   restore.sh synapse-media          (con el volumen de Synapse en /restore/synapse)
#
# Se usa con `make restore ARGS="..."` (ver docs/RESPALDOS.md), que corre este
# script en el contenedor de respaldos. Antes de restaurar, detén los
# servicios que escriben en lo que vas a reemplazar.
#
# Sin --force se niega a pisar datos: una base con tablas o un directorio del
# PDS que no está vacío.

# shellcheck disable=SC3040  # el sh de la imagen (busybox ash) sí tiene pipefail
set -eu -o pipefail

: "${S3_BUCKET:?falta S3_BUCKET}"
: "${S3_PREFIX:?falta S3_PREFIX}"

read_secret() {
  eval "f=\${$1:-}"
  [ -n "$f" ] && [ -r "$f" ] || { echo "restore: falta el archivo de $1" >&2; exit 64; }
  cat "$f"
}

# Sin archivo de configuración: el remoto B sale de estas variables.
export RCLONE_CONFIG=/dev/null
export RCLONE_CONFIG_B_TYPE=s3
export RCLONE_CONFIG_B_PROVIDER=Other
export RCLONE_CONFIG_B_ENDPOINT="${S3_ENDPOINT:-}"
export RCLONE_CONFIG_B_REGION="${S3_REGION:-auto}"
export RCLONE_CONFIG_B_FORCE_PATH_STYLE="${S3_FORCE_PATH_STYLE:-true}"
export RCLONE_CONFIG_B_NO_CHECK_BUCKET=true
RCLONE_CONFIG_B_ACCESS_KEY_ID="$(read_secret S3_ACCESS_KEY_ID_FILE)"
RCLONE_CONFIG_B_SECRET_ACCESS_KEY="$(read_secret S3_SECRET_ACCESS_KEY_FILE)"
export RCLONE_CONFIG_B_ACCESS_KEY_ID RCLONE_CONFIG_B_SECRET_ACCESS_KEY

SRC="B:$S3_BUCKET/$S3_PREFIX/backups"

pick() { # pick DIRECTORIO FECHA|latest SUFIJO -> nombre del objeto
  if [ "$2" = latest ]; then
    rclone lsf "$SRC/$1" --include "*$3" | sort | tail -1
  else
    echo "$2$3"
  fi
}

force=0
for a in "$@"; do [ "$a" = --force ] && force=1; done

case "${1:-}" in
  list)
    rclone lsl "$SRC/${2:-}" | sort -k2,3
    ;;

  postgres)
    db="${2:?uso: restore.sh postgres <base> [<fecha>|latest] [--force]}"
    when="${3:-latest}"; [ "$when" = --force ] && when=latest
    obj="$(pick "postgres/$db" "$when" .dump)"
    [ -n "$obj" ] || { echo "restore: no hay respaldos de postgres/$db" >&2; exit 1; }
    PGPASSWORD="$(read_secret PGPASSWORD_FILE)"
    export PGPASSWORD
    tables="$(psql -h "${PGHOST:-postgres}" -U "${PGUSER:-pg}" -d "$db" -tAc \
      "SELECT count(*) FROM pg_tables WHERE schemaname NOT IN ('pg_catalog','information_schema')")"
    if [ "$tables" != 0 ] && [ "$force" = 0 ]; then
      echo "restore: $db tiene $tables tablas. Detén sus servicios y repite con --force para reemplazarla." >&2
      exit 1
    fi
    echo "restore: postgres/$db/$obj -> $db"
    rclone cat "$SRC/postgres/$db/$obj" |
      pg_restore -h "${PGHOST:-postgres}" -U "${PGUSER:-pg}" -d "$db" --clean --if-exists --no-owner
    echo "restore: listo"
    ;;

  pds)
    when="${2:-latest}"; [ "$when" = --force ] && when=latest
    obj="$(pick pds "$when" .tar.zst)"
    [ -n "$obj" ] || { echo "restore: no hay respaldos del PDS" >&2; exit 1; }
    dst=/sources/pds
    if [ -n "$(ls -A "$dst" 2>/dev/null)" ] && [ "$force" = 0 ]; then
      echo "restore: $dst no está vacío. Detén el PDS (make stop S=pds) y repite con --force." >&2
      exit 1
    fi
    echo "restore: pds/$obj -> $dst"
    find "$dst" -mindepth 1 -delete
    rclone cat "$SRC/pds/$obj" | zstd -dq | tar -C "$dst" -xf -
    # El PDS corre como node (uid 1000) en su imagen.
    chown -R 1000:1000 "$dst"
    echo "restore: listo"
    ;;

  synapse-media)
    [ -d /restore/synapse ] || { echo "restore: monta el volumen de Synapse en /restore/synapse (make restore-media)" >&2; exit 64; }
    mkdir -p /restore/synapse/media_store
    echo "restore: synapse/media_store -> volumen de Synapse"
    rclone copy --transfers 4 "B:$S3_BUCKET/$S3_PREFIX/synapse/media_store" /restore/synapse/media_store
    # Synapse corre como uid 991 en su imagen.
    chown -R 991:991 /restore/synapse/media_store
    echo "restore: listo"
    ;;

  *)
    sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'
    exit 64
    ;;
esac
