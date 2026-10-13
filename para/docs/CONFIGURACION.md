# Configuración

Casi todo despliegue fija pocas cosas: el dominio, el bucket y el perfil. El
resto tiene un valor por defecto pensado para ese perfil.

## De dónde sale cada cosa

| Fuente | Qué lleva |
| --- | --- |
| `.env` | Todo lo no secreto de la plataforma: dominio, perfil, imágenes, bucket, rutas de los repos ([.env.example](../.env.example)). Le gana al perfil. |
| `profiles/<PARA_PROFILE>.env` | Tamaños: memoria, réplicas, Postgres, vlpds (shards, lease, cachés), respaldos. |
| `secrets/<nombre>` | Un archivo por secreto. Llega al contenedor como `NAME_FILE=/run/secrets/<nombre>`. |
| `env/vlpds.env` | Ajustes finos de vlpds: correo, marca, enlaces legales, llave de recuperación del operador ([.example](../env/vlpds.env.example)). |
| `env/core.env` | Ajustes finos del AppView, bsync y Ozone ([.example](../env/core.env.example)). |
| `env/identity.env` | Ajustes finos de mubEZ ([.example](../env/identity.env.example)). |
| El Makefile | Lo que sale de `PARA_DOMAIN` si `.env` no lo fija: `BSKY_SERVER_DID`, `OZONE_SERVER_DID`, `MATRIX_SERVER_NAME`, `VLPDS_HANDLE_DOMAIN`… |

`make config` muestra la configuración final que verá Compose.

## Perfiles

Son techos de memoria, no consumo.

| Servicio | `tiny` (4 vCPU / 12 GB) | `standard` (8 vCPU / 32 GB / NVMe) |
| --- | --- | --- |
| Postgres | 1 GB · shared_buffers 256 MB · 100 conexiones | 6 GB · 2 GB · 300 |
| Redis (caché, sin disco) | 128 MB | 1 GB |
| **vlpds** | 2 GB · 1 shard · lease 60 s · caché de disco 4 GB | 6 GB · 64 shards · lease 10 s · 64 GB |
| **relay** | 256 MB | 1 GB |
| AppView (bsky) | 768 MB × 1 | 2 GB × 2 |
| dataplane (indexa; siempre uno) | 512 MB | 2 GB |
| bsync / Ozone | 192 MB / 384 MB | 512 MB / 1 GB |
| Synapse | 1 GB · cache factor 0.5 | 4 GB · 2.0 |
| MAS / para-idp / Element | 192 / 96 / 64 MB | 512 / 128 / 128 MB |
| matrix-bridge | 384 MB | 1 GB |
| mubEZ | 512 MB | 1 GB |
| Caddy / Alloy / respaldos | 128 / 256 / 256 MB | 512 MB / 512 MB / 1 GB |
| App web (bskyweb) | 128 MB | 256 MB |
| Respaldos | cada 6 h, 14 días | cada 1 h, 30 días |
| **Suma de techos** | **~8.2 GB** | **~30.5 GB** |

**vlpds no baja de 2 GB.** Sus cachés se dimensionan con el límite del
contenedor y, con menos, se niega a arrancar (`vlpds --memory-plan` lo
comprueba). Por eso `tiny` pide 12 GB de máquina: con 8 no queda margen.

**El lease de vlpds** es lo que un nodo tarda en volver a escribir tras una
caída dura (OOM, SIGKILL): ~60 s en `tiny`. Un apagado con gracia tarda ~1 s
con cualquier lease. En `standard` el lease es corto para que otro nodo tome
los shards rápido ([Escalado](ESCALADO.md)).

Las opciones de tamaño de vlpds que no tienen variable de entorno
(`--firehose-ring-mb` y similares) van como argumentos en `core.yaml`, con los
valores `VLPDS_*` del perfil.

## Secretos

`scripts/secrets.sh list` es la referencia viva.

| Archivo | Lo usa | Tipo | Offline |
| --- | --- | --- | --- |
| `postgres_password` | Postgres | aleatorio | |
| `postgres_url`, `postgres_url_bridge`, `postgres_url_relay` | AppView, dataplane, bsync, Ozone / bridge / relay | derivado | |
| `s3_access_key_id`, `s3_secret_access_key` | vlpds, mubEZ, respaldos | del proveedor | |
| `vlpds_jwt_secret`, `vlpds_admin_token`, `vlpds_internal_token`, `vlpds_rate_limit_bypass_key` | vlpds | aleatorio (32+ bytes, distintos) | |
| `vlpds_kek` | vlpds: envuelve las llaves de firma de cada repo | aleatorio | **sí** |
| `vlpds_plc_rotation_key` | vlpds: controla los DIDs | k256 envuelta con la KEK (`vw1.…`) | **sí** |
| `relay_admin_password` | relay | aleatorio | |
| `bsky_service_signing_key` | AppView | aleatorio | **sí** |
| `bsky_admin_passwords` | AppView | aleatorio | |
| `bsync_api_key` | AppView ↔ bsync | aleatorio | |
| `ozone_signing_key` | Ozone | aleatorio | **sí** |
| `ozone_admin_password` | Ozone | aleatorio | |
| `authority_service_token` | AppView y bridge | aleatorio | |
| `civic_delegation_resolver_secret` | AppView y mubEZ | aleatorio | |
| `m8_jwt_secret`, `m8_app_key`, `m8_cookie_secret`, `m8_civic_vote_proof_secret` | mubEZ | aleatorio | |
| `m8_curp_pepper` | mubEZ | aleatorio | **sí** |
| `m8_issuer_private_jwk`, `m8_issuer_public_jwk` | mubEZ | par Ed25519 | **sí** (la privada) |
| `matrix_admin_token` | bridge | `make matrix-admin-token` | |
| `matrix_appservice_token`, `matrix_hs_token` | bridge | del registro del appservice | |

**La llave PLC envuelta.** `make secrets` crea la KEK y después le pide a la
imagen de vlpds una llave nueva envuelta con ella
(`--wrap-plc-rotation-key`); la KEK entra como archivo montado, nunca como
argumento. `make secrets-import` hace lo mismo con la llave de un PDS
tradicional (`PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX`), así que los DIDs
que la listan siguen siendo válidos. Las dos necesitan Docker.

Reglas que se cumplen solas:

- vlpds se niega a arrancar con un secreto vacío, de menos de 32 bytes, dos
  tokens iguales, sin KEK o con las credenciales por defecto de MinIO.
- Para los servicios de WatZappa, `runtime/env-from-files.sh` detiene el
  contenedor si un secreto falta, está vacío o llega como archivo **y** como
  variable.
- `scripts/secrets.sh check` (y preflight) exige todos los de los perfiles
  activos, de 32+ caracteres los aleatorios, distintos entre sí y sin pinta de
  desarrollo.
- En el servidor, `secrets/` es `root:10001 0750` y cada archivo `0440`. Los
  contenedores leen gracias a `group_add: [10001]` (vlpds corre como uid
  10001).
- Cambiar un secreto: reemplaza el archivo y `make up UP_ARGS=--force-recreate`.

### Correo

La URL SMTP lleva contraseña, así que va como secreto: crea
`secrets/vlpds_email_smtp_url`, agrega la entrada en `secrets.yaml`, y en
`core.yaml` añade `VLPDS_EMAIL_SMTP_URL_FILE: /run/secrets/vlpds_email_smtp_url`
y el secreto a la lista del servicio `pds`. El remitente y la marca van en
`env/vlpds.env`. Sin correo, los mensajes van al log.

## Variables de `.env`

| Variable | Por defecto | Para qué |
| --- | --- | --- |
| `PARA_DOMAIN` | — | Dominio público; obligatorio y para siempre |
| `ACME_EMAIL` | — | Avisos de Let's Encrypt |
| `PARA_PROFILE` | `tiny` | `tiny` o `standard` |
| `PROFILES` | `core,matrix,identity,edge` | Perfiles de Compose que levanta `make up` |
| `VLPDS_IMAGE` | `ghcr.io/jazware/vlpds:1.1.0` | La imagen de vlpds (siempre una versión fija) |
| `PARA_REGISTRY`, `WATZAPPA_VERSION`, `MUBEZ_VERSION` | `ghcr.io/mexico-parasocial`, `local`, `local` | Nombre y tag de las imágenes de WatZappa y mubEZ; con `local`, `make up BUILD=1` las construye |
| `RELAY_COMMIT`, `RELAY_VERSION` | `162dca4…` | El commit de indigo del que se construye el relay |
| `S3_ENDPOINT`, `S3_REGION`, `S3_BUCKET`, `S3_PREFIX`, `S3_FORCE_PATH_STYLE` | —, `auto`, —, `prod`, `true` | El bucket. Un prefijo = una plataforma |
| `PLC_URL` | `https://plc.directory` | Directorio PLC |
| `PDS_INVITE_REQUIRED` | `true` | vlpds exige invitación |
| `PDS_CRAWLERS_EXTRA` | `,https://bsky.network` | Relays externos que vlpds avisa además del local. Vacío: una red cerrada |
| `VLPDS_SPACES` | `false` | Spaces (alfa) |
| `VLPDS_NODE_ID` | `single` | Identidad del nodo; estable entre reinicios y única por prefijo |
| `VLPDS_ADMIN_PORT` | `2620` | Puerto de la consola de vlpds en el loopback del servidor |
| `ADMIN_DIDS` | — | Administradores de Ozone |
| `M8_CORS_ORIGIN`, `M8_OAUTH_RETURN_TO_ALLOWLIST` | ver [MUBEZ.md](MUBEZ.md) | Login de mubEZ |
| `PARA_OPERATOR_CIDRS` | loopback y redes privadas | Quién llega a las rutas de operador de Matrix por el edge |
| `APPVIEW_WORKERS` | — | AppViews en otras máquinas (`ip:2584 …`) |
| `PARA_PRIVATE_BIND` | `127.0.0.1` | IP donde Postgres y bsync se publican para los workers |
| `PARA_ROLE` | `primary` | `appview-worker` en una máquina worker |
| `PARA_METRICS_URL`, `PARA_LOGS_URL` | — | Destino de Alloy (perfil `monitoring`) |
| `LITESTREAM_SYNC_INTERVAL` | `1s` | Cada cuánto llegan al bucket los cambios de mubEZ |
| `BACKUP_INTERVAL`, `BACKUP_RETENTION` | del perfil | Respaldos de Postgres |
| `PARA_SECRETS_DIR` | `secrets` | Dónde están los secretos (p. ej. un tmpfs) |
| `PARA_HANDLE_DOMAIN` | `.PARA_DOMAIN` | Sufijo de los handles; necesita DNS comodín |
| `MATRIX_SERVER_NAME` | `matrix.PARA_DOMAIN` | Nombre del homeserver; inmutable una vez que hay usuarios |
| `MATRIX_ENABLE_ENCRYPTION` | `true` | Salas nuevas cifradas |
| `PARA_WEB_VERSION` | `local` | Imagen de la app web (perfil `web`) |

## Lo que cuesta el bucket

La cuenta depende de cuántas veces se escribe, no de cuánto:

- **vlpds** escribe segmentos de su log (agrupa los commits; hasta 8 MB por
  segmento) y compactaciones. Un nodo `tiny` sin actividad cuesta ~$0 en R2;
  `standard` cuesta del orden de $50 al mes solo por sondear sus 64 shards
  (medido por vlpds). Guarda 72 h de firehose para cursores viejos.
- **mubEZ (Litestream)** sube un segmento por intervalo *solo si hubo
  cambios*. Con escrituras constantes, `1s` son hasta ~2.6 millones de PUT al
  mes; `10s`, diez veces menos.
- **Respaldos de Postgres**: unas pocas subidas por ciclo.
- **Blobs**: uno por imagen o video subido.

Para un piloto, R2 suele quedar dentro de su capa gratuita.
