# Actualizaciones

## Cambiar de versión

```bash
make backup                                    # siempre antes: hay migraciones de una sola vía
make upgrade WATZAPPA_VERSION=sha-<commit>     # y/o MUBEZ_VERSION=...
```

`make upgrade` escribe el tag en `.env` (rechaza `main`, `latest` y vacío),
descarga, recrea solo los servicios cuya imagen cambió, espera a que estén
sanos y corre `make verify`. Cada servicio se detiene con gracia y Caddy
reintenta 10 s, así que una actualización se nota como unos segundos de
espera, no como errores.

Volver atrás es lo mismo con el tag anterior, **si** la versión nueva no
migró la base de forma incompatible. Si lo hizo, restaura el respaldo de antes
([Respaldos](RESPALDOS.md)).

Mantén las revisiones de los repos en `/opt/para` alineadas con las imágenes:
los compose y la configuración salen de ahí. Con Ansible, sube la `version`
de `para_repos` y los tags de `para_env` juntos.

Cambios que piden orden (los dice su nota de versión): por ejemplo, la
autoridad de comunidades exige detener el bridge, migrar el AppView y después
levantar el bridge nuevo:

```bash
make stop S=matrix-bridge
make upgrade WATZAPPA_VERSION=sha-<commit>     # el AppView migra al arrancar
make verify
```

## Migrar desde el despliegue anterior

Lo que cambia respecto del `para-platform` anterior y cómo pasar sin perder
datos. Hazlo en una ventana de mantenimiento; toma respaldos manuales antes
de empezar.

### 1. Secretos: de `.env` a archivos

```bash
make secrets-import      # de WatZappa/.env, env/identity.env y .env
make secrets             # los que no existían antes (p. ej. Ozone)
scripts/secrets.sh check # te dice qué llaves quitar de env/identity.env
```

La contraseña de Postgres importada es la del volumen existente: no la
cambies.

### 2. `.env`

Agrega `PARA_PROFILE`, `WATZAPPA_VERSION`, `MUBEZ_VERSION` y el bucket
(`S3_*`; antes eran `PDS_BLOBSTORE_S3_*` en `WatZappa/.env`). Quita
`PARA_AUTHORITY_SERVICE_TOKEN` (ahora es `secrets/authority_service_token`).

### 3. Datos del PDS: de volumen anónimo a `pds_data`

El compose anterior no le daba al PDS un volumen con nombre: sus datos
(cuentas, repos, llaves por repo) están en un volumen anónimo que un `down`
seguido de `up` habría reemplazado por uno vacío. Antes del primer `make up`
nuevo, cópialos (preflight se niega a seguir si detecta este caso):

```bash
old=$(docker inspect para-pds --format '{{range .Mounts}}{{if eq .Destination "/app/data"}}{{.Name}}{{end}}{{end}}')
docker volume create para_pds_data
docker run --rm -v "$old":/from:ro -v para_pds_data:/to alpine sh -c 'cp -a /from/. /to/ && chown -R 1000:1000 /to'
```

### 4. Synapse y MAS: al Postgres compartido

Antes tenían cada uno su Postgres (`synapse-db`, `mas-db`). Ahora usan bases
`matrix` y `mas` del Postgres de la plataforma; sus configuraciones no
cambian porque `synapse-db` y `mas-db` son alias de él.

```bash
make compose ARGS="--profile legacy-matrix-db up -d --no-deps synapse-db mas-db"
make compose ARGS="exec -T synapse-db pg_dump -U pg -Fc matrix" > matrix.dump
make compose ARGS="exec -T mas-db pg_dump -U pg -Fc mas" > mas.dump
make compose ARGS="--profile legacy-matrix-db rm -sf synapse-db mas-db"

make up PROFILES=core                      # Postgres crea matrix (collation C), mas y bridge
make db-init                               # por si el volumen ya existía
make compose ARGS="exec -T postgres pg_restore -U pg -d matrix --no-owner" < matrix.dump
make compose ARGS="exec -T postgres pg_restore -U pg -d mas --no-owner" < mas.dump
```

Los volúmenes viejos (`para_matrix_postgres_data`, `para_mas_postgres_data`)
quedan intactos; bórralos solo cuando la plataforma nueva lleve un tiempo bien.

### 5. Bridge: de SQLite a Postgres

El bridge ahora guarda su estado en la base `bridge`. Su SQLite sigue en el
volumen `bridge_data`; tráelo antes de que el bridge nuevo arranque:

```bash
make compose ARGS="run --rm --no-deps --entrypoint sh matrix-bridge -c 'DATABASE_URL=\"\$(cat /run/secrets/postgres_url_bridge)\" node dist/db/migrate-to-postgres.js --source /app/data/bridge.db'"
```

### 6. Matrix público y handles

- El edge anterior no publicaba Matrix: agrega los registros DNS `auth.`,
  `idp.` y `chat.` y revisa la config de MAS, que se generaba para
  `localhost` (docs/MATRIX.md, "MAS generado para localhost").
- Los handles: el PDS usaba por defecto `.pds.<dominio>`; la plataforma usa
  `.<dominio>`. Si ya hay cuentas con handles `x.pds.<dominio>`, pon
  `PARA_HANDLE_DOMAIN=.pds.<dominio>` (y su comodín DNS) para no romperlos.
- El AppView ahora recibe `BSKY_PUBLIC_URL`; sin ella no armaba las URLs de
  imágenes y video.

### 7. mubEZ

Nada que migrar: su volumen `mubez-data` se conserva y, al arrancar con la
imagen nueva, Litestream empieza a replicar esa base al bucket.

### 8. Levantar y verificar

```bash
make preflight
make up
make verify
```

### Qué se retiró de WatZappa

- `ansible/` y `docker-compose.swarm.yml`: el despliegue vive aquí
  (`deploy/ansible/`), con imágenes publicadas en vez de compilar en el
  servidor. Ninguno de los dos se había corrido contra una máquina real.
- `PDS_DB_POSTGRES_URL`: el PDS nunca la leyó.
