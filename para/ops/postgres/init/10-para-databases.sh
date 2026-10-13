#!/bin/sh
# Crea las bases que comparten el Postgres de la plataforma. Idempotente:
# corre sola en el primer arranque de un volumen nuevo y `make db-init` la
# repite sobre uno que ya existía.
#
#   matrix  Synapse. Exige collation C, y eso solo se fija al crear la base.
#   mas     Matrix Authentication Service.
#   bridge  matrix-bridge.
set -eu

run_psql() {
  psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname "$POSTGRES_DB" "$@"
}

create_db() { # create_db NOMBRE [OPCIONES...]
  name="$1"; shift
  if [ "$(run_psql -tAc "SELECT 1 FROM pg_database WHERE datname = '$name'")" = "1" ]; then
    echo "para-databases: $name ya existe"
    return 0
  fi
  run_psql -c "CREATE DATABASE \"$name\" OWNER \"$POSTGRES_USER\" $*"
  echo "para-databases: $name creada"
}

create_db "${MATRIX_DB_NAME:-matrix}" ENCODING "'UTF8'" LC_COLLATE "'C'" LC_CTYPE "'C'" TEMPLATE template0
create_db mas
create_db bridge
