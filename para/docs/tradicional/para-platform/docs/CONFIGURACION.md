# Configuración

Casi todo despliegue fija pocas cosas: el dominio, el bucket, las versiones y
el perfil. El resto tiene un valor por defecto pensado para ese perfil.

## De dónde sale cada cosa

| Fuente | Qué lleva |
| --- | --- |
| `.env` | Todo lo no secreto de la plataforma: dominio, perfil, versiones, bucket, rutas de los repos ([.env.example](../.env.example)). Le gana al perfil. |
| `profiles/<PARA_PROFILE>.env` | Tamaños: memoria, réplicas, Postgres, respaldos. |
| `secrets/<nombre>` | Un archivo por secreto. Llega al contenedor como `NAME_FILE=/run/secrets/<nombre>`. |
| `env/core.env`, `env/identity.env` | Ajustes finos no secretos de los servicios AT Protocol y de mubEZ (opcionales; ver los `.example`). |
| El Makefile | Valores que salen de `PARA_DOMAIN` si `.env` no los fija: `BSKY_SERVER_DID`, `OZONE_SERVER_DID`, `MATRIX_SERVER_NAME`… |

La plataforma ya no lee `WatZappa/.env`: era donde vivían los secretos en
claro. `make secrets-import` los trae a `secrets/`.

`make config` muestra la configuración final que verá Compose.

## Perfiles

Son techos de memoria, no consumo; la suma deja aire para el sistema.

| Servicio | `tiny` (4 vCPU / 8 GB) | `standard` (8 vCPU / 32 GB) |
| --- | --- | --- |
| Postgres | 1 GB · shared_buffers 256 MB · 100 conexiones | 8 GB · 2 GB · 300 |
| Redis (caché, sin disco) | 128 MB | 1 GB |
| PDS | 512 MB | 2 GB |
| AppView (bsky) | 768 MB × 1 | 2 GB × 2 |
| dataplane | 512 MB × 1 | 2 GB × 2 |
| bsync / Ozone | 192 MB / 384 MB | 512 MB / 1 GB |
| Synapse | 1 GB · cache factor 0.5 | 4 GB · 2.0 |
| MAS / para-idp / Element | 192 / 96 / 64 MB | 512 / 128 / 128 MB |
| matrix-bridge | 384 MB | 1 GB |
| mubEZ | 512 MB | 1 GB |
| Caddy / Alloy / respaldos | 128 / 256 / 256 MB | 512 MB / 512 MB / 1 GB |
| App web (bskyweb) | 128 MB | 256 MB |
| Respaldos | cada 6 h, se guardan 14 días | cada 1 h, 30 días |
| **Suma de techos** | **~6.3 GB** | **~29 GB** |

Por qué Postgres va aparte del archivo de WatZappa: `postgres-tuning.conf`
está afinado para la máquina de 128 GB (`shared_buffers = 8GB`); en una VM de
8 GB no arrancaría. El perfil fija lo que depende de la memoria con `-c`, y
además `listen_addresses=*`, sin lo cual Postgres con un `config_file` propio
solo escucha en localhost y ningún otro contenedor llega a él.

## Secretos

`scripts/secrets.sh list` es la referencia viva. Resumen:

| Archivo | Lo usa | Tipo | Offline |
| --- | --- | --- | --- |
| `postgres_password` | Postgres | aleatorio | |
| `postgres_url`, `postgres_url_bridge` | AppView, dataplane, bsync, Ozone / bridge | derivado | |
| `s3_access_key_id`, `s3_secret_access_key` | PDS, mubEZ, respaldos | del proveedor | |
| `pds_jwt_secret`, `pds_dpop_secret`, `pds_admin_password`, `pds_rate_limit_bypass_key` | PDS | aleatorio | |
| `pds_plc_rotation_key` | PDS | aleatorio (k256) | **sí** |
| `bsky_service_signing_key` | AppView | aleatorio | **sí** |
| `bsky_admin_passwords` | AppView | aleatorio | |
| `ozone_signing_key` | Ozone | aleatorio | **sí** |
| `ozone_admin_password` | Ozone | aleatorio | |
| `authority_service_token` | AppView y bridge | aleatorio | |
| `civic_delegation_resolver_secret` | AppView y mubEZ | aleatorio | |
| `m8_jwt_secret`, `m8_app_key`, `m8_cookie_secret`, `m8_civic_vote_proof_secret` | mubEZ | aleatorio | |
| `m8_curp_pepper` | mubEZ | aleatorio | **sí** |
| `m8_issuer_private_jwk`, `m8_issuer_public_jwk` | mubEZ | par Ed25519 | **sí** (la privada) |
| `matrix_admin_token` | bridge | `make matrix-admin-token` | |
| `matrix_appservice_token`, `matrix_hs_token` | bridge | del registro del appservice | |

Reglas que se cumplen solas:

- Si un secreto llega como archivo **y** como variable, el contenedor no
  arranca (`env-from-files`). Igual si el archivo falta o está vacío.
- `scripts/secrets.sh check` (y preflight) exige todos los de los perfiles
  activos, de 32+ caracteres los aleatorios, distintos entre sí y sin pinta de
  desarrollo (`change-me`, `example`, `dev-secret`…).
- En el servidor: `secrets/` es `root:10001 0750` y cada archivo `0440`. Los
  contenedores leen gracias a `group_add: [10001]`; ningún otro usuario de la
  máquina puede.
- Cambiar un secreto: reemplaza el archivo y `make up UP_ARGS=--force-recreate`
  (Compose no detecta cambios en el contenido de un archivo de secreto).

### Correo

La URL SMTP del PDS lleva contraseña, así que no va en `env/core.env`. Para
habilitar correo: agrega `pds_email_smtp_url` a `secrets/`, una entrada en
`secrets:` de `compose.yaml` y `PDS_EMAIL_SMTP_URL_FILE` al PDS en
`overrides/core.yaml`. Sin correo, las verificaciones solo van al log.

## Variables de `.env`

| Variable | Por defecto | Para qué |
| --- | --- | --- |
| `PARA_DOMAIN` | — | Dominio público; obligatorio |
| `ACME_EMAIL` | — | Avisos de Let's Encrypt |
| `PARA_PROFILE` | `tiny` | `tiny` o `standard` |
| `PROFILES` | `core,matrix,identity,edge` | Perfiles de Compose que levanta `make up` |
| `PARA_REGISTRY` | `ghcr.io/mexico-parasocial` | De dónde se descargan las imágenes |
| `WATZAPPA_VERSION`, `MUBEZ_VERSION` | `local` | Tag de imagen; en producción `sha-<commit>` o `X.Y.Z` |
| `S3_ENDPOINT`, `S3_REGION`, `S3_BUCKET`, `S3_PREFIX`, `S3_FORCE_PATH_STYLE` | —, `auto`, —, `prod`, `true` | El bucket. Un prefijo = una plataforma |
| `PLC_URL` | `https://plc.directory` | Directorio PLC |
| `ADMIN_DIDS` | — | Administradores de Ozone |
| `M8_CORS_ORIGIN` | `https://PARA_DOMAIN` | Quién llama al broker desde el navegador |
| `M8_OAUTH_RETURN_TO_ALLOWLIST` | `im8://…,para://m8-auth,https://PARA_DOMAIN/m8-auth` | Callbacks de login permitidos |
| `PARA_OPERATOR_CIDRS` | loopback y redes privadas | Quién llega a `com.atproto.admin.*` por el edge |
| `APPVIEW_WORKERS` | — | AppViews en otras máquinas (`ip:2584 …`) |
| `PARA_PRIVATE_BIND` | `127.0.0.1` | IP donde Postgres y bsync se publican para los workers |
| `PARA_ROLE` | `primary` | `appview-worker` en una máquina worker |
| `PARA_METRICS_URL`, `PARA_LOGS_URL` | — | Destino de Alloy (perfil `monitoring`) |
| `LITESTREAM_SYNC_INTERVAL` | `1s` | Cada cuánto llegan al bucket los cambios de mubEZ (ver abajo) |
| `BACKUP_INTERVAL`, `BACKUP_RETENTION` | del perfil | Respaldos |
| `PARA_SECRETS_DIR` | `secrets` | Dónde están los archivos de secretos (p. ej. un tmpfs) |
| `PARA_HANDLE_DOMAIN` | `.PARA_DOMAIN` | Sufijo de los handles de usuario; necesita DNS comodín |
| `MATRIX_SERVER_NAME` | `matrix.PARA_DOMAIN` | Nombre del homeserver; inmutable una vez que hay usuarios |
| `MATRIX_ENABLE_ENCRYPTION` | `true` | Salas nuevas cifradas (el chat nativo lo exige) |
| `PARA_WEB_VERSION` | `local` | Imagen de la app web (perfil `web`) |
| `PARA_GROWTHBOOK_API_HOST`, `PARA_GROWTHBOOK_CLIENT_KEY` | — | Banderas remotas de la app web; sin ellas, apagadas |

## Lo que cuesta el bucket

La cuenta depende de cuántas veces se escribe, no de cuánto:

- **mubEZ (Litestream)** sube un segmento por intervalo *solo si hubo cambios*.
  Sin actividad no cuesta nada. Con escrituras constantes, `1s` son hasta ~2.6
  millones de PUT al mes; `10s`, diez veces menos, a cambio de que un
  accidente pueda perder hasta 10 s de cambios del broker.
- **Respaldos**: unas pocas subidas por ciclo (una por base, una del PDS, y
  solo el media nuevo de Synapse). Con `tiny` son ~4 ciclos al día.
- **Blobs del PDS**: uno por imagen o video subido.

Revisa la tabla de precios de tu proveedor; para un piloto, R2 suele quedar
dentro de su capa gratuita.
