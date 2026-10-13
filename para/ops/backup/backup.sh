#!/bin/sh
# Respaldos de la plataforma PARA hacia el bucket.
#
#   backup.sh loop   respalda cada BACKUP_INTERVAL (el modo del servicio)
#   backup.sh once   un solo ciclo y sale (make backup)
#
# BACKUP_TARGETS elige qué respalda este contenedor:
#   postgres       pg_dump -Fc de cada base de BACKUP_DATABASES
#   pds            copia consistente (API de backup de SQLite) de los datos del
#                  PDS: cuentas, secuenciador y un SQLite + llave por repo
#   synapse-media  copia incremental del media_store de Synapse
#
# En el bucket:
#   $S3_PREFIX/backups/postgres/<base>/<fecha>.dump
#   $S3_PREFIX/backups/pds/<fecha>.tar.zst
#   $S3_PREFIX/synapse/media_store/...
#
# Las llaves que no deben vivir en el bucket (rotación PLC, firma de Synapse,
# emisor M8) NO se respaldan aquí: se guardan offline (docs/RESPALDOS.md).
#
# Expone métricas en :9101/metrics para Alloy (ParaBackupStale,
# ParaBackupFailing en ops/alerts.yml).

# pipefail: un pg_dump que falla no debe parecer un respaldo bueno solo porque
# la subida terminó. Las funciones corren dentro de `if`, donde set -e no
# aplica, así que cada paso revisa su propio resultado.
# shellcheck disable=SC3040  # el sh de la imagen (busybox ash) sí tiene pipefail
set -eu -o pipefail

: "${S3_BUCKET:?falta S3_BUCKET}"
: "${S3_PREFIX:?falta S3_PREFIX}"
BACKUP_TARGETS="${BACKUP_TARGETS:-postgres pds}"
BACKUP_DATABASES="${BACKUP_DATABASES:-para matrix mas bridge}"
BACKUP_INTERVAL="${BACKUP_INTERVAL:-6h}"
BACKUP_RETENTION="${BACKUP_RETENTION:-14d}"
METRICS_DIR=/srv/metrics

log() { echo "[backup $(date -u +%Y-%m-%dT%H:%M:%SZ)] $*"; }

read_secret() { # read_secret VARIABLE_CON_RUTA
  eval "f=\${$1:-}"
  [ -n "$f" ] && [ -r "$f" ] || { echo "backup: falta el archivo de $1" >&2; exit 64; }
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

DEST="B:$S3_BUCKET/$S3_PREFIX/backups"

seconds() { # 6h, 30m, 90s, 1d -> segundos
  case "$1" in
    *d) echo $(( ${1%d} * 86400 )) ;;
    *h) echo $(( ${1%h} * 3600 )) ;;
    *m) echo $(( ${1%m} * 60 )) ;;
    *s) echo "${1%s}" ;;
    *) echo "$1" ;;
  esac
}

# --- métricas -----------------------------------------------------------------
mkdir -p "$METRICS_DIR"
set_metric() { # set_metric TARGET OK(0|1) DURACION
  f="$METRICS_DIR/$1.prom"
  last_ok=0
  [ -f "$f" ] && last_ok="$(sed -n 's/^para_backup_last_success_timestamp_seconds{[^}]*} //p' "$f")"
  [ "$2" = 1 ] && last_ok="$(date +%s)"
  {
    echo "para_backup_interval_seconds{target=\"$1\"} $(seconds "$BACKUP_INTERVAL")"
    echo "para_backup_last_run_success{target=\"$1\"} $2"
    echo "para_backup_last_run_duration_seconds{target=\"$1\"} $3"
    echo "para_backup_last_success_timestamp_seconds{target=\"$1\"} ${last_ok:-0}"
  } > "$f.tmp"
  mv "$f.tmp" "$f"
  cat "$METRICS_DIR"/*.prom > "$METRICS_DIR/metrics"
}

# --- objetivos ----------------------------------------------------------------
backup_postgres() {
  PGPASSWORD="$(read_secret PGPASSWORD_FILE)"
  export PGPASSWORD
  existing="$(psql -h "${PGHOST:-postgres}" -U "${PGUSER:-pg}" -d postgres -tAc 'SELECT datname FROM pg_database')" || {
    log "postgres: no responde"
    return 1
  }
  for db in $BACKUP_DATABASES; do
    if ! printf '%s\n' "$existing" | grep -qx "$db"; then
      log "postgres/$db: no existe, se omite"
      continue
    fi
    obj="$DEST/postgres/$db/$STAMP.dump"
    if pg_dump -h "${PGHOST:-postgres}" -U "${PGUSER:-pg}" -Fc "$db" | rclone rcat "$obj"; then
      log "postgres/$db: $(rclone size --json "$obj" | sed -n 's/.*"bytes":\([0-9]*\).*/\1/p') bytes"
    else
      # Un pg_dump que falla a medias deja un objeto truncado que parece bueno.
      rclone deletefile "$obj" 2>/dev/null || true
      log "postgres/$db: FALLÓ"
      return 1
    fi
  done
  rclone delete --min-age "$BACKUP_RETENTION" "$DEST/postgres"
}

backup_pds() {
  src=/sources/pds
  [ -d "$src" ] || { log "pds: no hay $src, se omite"; return 0; }
  tmp="$(mktemp -d /tmp/pds.XXXXXX)"
  # La API de backup de SQLite da una copia consistente aunque el PDS esté
  # escribiendo. Los -wal/-shm no hacen falta en la copia; el resto (las
  # llaves por repo) se copia tal cual.
  if ! (cd "$src" && find . -type f) | while read -r f; do
    case "$f" in
      *-wal|*-shm) continue ;;
    esac
    mkdir -p "$tmp/$(dirname "$f")" || exit 1
    case "$f" in
      *.sqlite) sqlite3 "$src/$f" ".backup '$tmp/$f'" || exit 1 ;;
      *) cp -p "$src/$f" "$tmp/$f" || exit 1 ;;
    esac
  done; then
    rm -rf "$tmp"
    log "pds: FALLÓ la copia local"
    return 1
  fi
  obj="$DEST/pds/$STAMP.tar.zst"
  if tar -C "$tmp" -cf - . | zstd -q -T0 | rclone rcat "$obj"; then
    log "pds: $(du -sh "$tmp" | cut -f1) sin comprimir"
  else
    rclone deletefile "$obj" 2>/dev/null || true
    rm -rf "$tmp"
    log "pds: FALLÓ"
    return 1
  fi
  rm -rf "$tmp"
  rclone delete --min-age "$BACKUP_RETENTION" "$DEST/pds"
}

backup_synapse_media() {
  src=/sources/synapse/media_store
  [ -d "$src" ] || { log "synapse-media: no hay $src, se omite"; return 0; }
  rclone copy --transfers 4 "$src" "B:$S3_BUCKET/$S3_PREFIX/synapse/media_store" || {
    log "synapse-media: FALLÓ"
    return 1
  }
  log "synapse-media: copiado"
}

run_once() {
  STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
  failed=0
  for target in $BACKUP_TARGETS; do
    start="$(date +%s)"
    fn="backup_$(echo "$target" | tr '-' '_')"
    if "$fn"; then ok=1; else ok=0; failed=1; fi
    set_metric "$target" "$ok" $(( $(date +%s) - start ))
  done
  return "$failed"
}

case "${1:-loop}" in
  once)
    run_once
    ;;
  loop)
    httpd -p 9101 -h "$METRICS_DIR"
    interval="$(seconds "$BACKUP_INTERVAL")"
    # Deja arrancar a los servicios antes del primer respaldo.
    sleep "${BACKUP_FIRST_DELAY:-120}"
    while true; do
      run_once || log "hubo fallas; se reintenta en $BACKUP_INTERVAL"
      sleep "$interval"
    done
    ;;
  *)
    echo "uso: backup.sh [loop|once]" >&2
    exit 64
    ;;
esac
