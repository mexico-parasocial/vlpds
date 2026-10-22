# mubEZ: el broker M8

mubEZ es el servicio de identidad de PARA (el "broker M8"). Vale igual para
las dos variantes: con vlpds y con el PDS en TypeScript, mubEZ es el mismo
proceso y habla con el PDS solo por ATProto estándar.

## Qué hace

| Área | Rutas (bajo `/v1`) | Para qué |
|---|---|---|
| Sesiones | `/sessions/*`, `/oauth-client-metadata.json` | Login de PARA por OAuth contra el PDS de la cuenta, con handoff a la app (`para://m8-auth`) |
| Identidad | `/identity/*`, `/claims/*`, `/issuers` | Verificación con INE y pruebas ZKP (los artefactos `zkp-prover.*` y `nullifier-prover.*` se sirven desde aquí), credenciales firmadas por el emisor M8, revocación (`/identity/crl`) |
| Votos y delegaciones | autorizaciones de voto público, resolución de delegaciones | Las pruebas que el AppView verifica antes de indexar boletas (`PARA_CIVIC_VOTE_VERIFIER_URL`) |
| Identidades anónimas | `/anonymous/*` | Perfiles y publicaciones anónimas, contacto público |
| Firmas para Matrix | `/matrix/sign-requests` | Relay ligado a la sesión: PARA deposita un reto, la billetera iM8 lo firma y PARA lo recoge. Audiencias del bridge (`para-matrix-bridge/*.v1`) y `para-idp` |
| Comunidades, karma, ledger | `/communities`, `/karma`, `/ledger` | Lo que la app consulta del broker |
| Documentación | `/docs`, `/openapi.json` | La API completa |

El bridge de Matrix llama a mubEZ (`M8_BASE_URL`) para autenticar a quien usa
su API. El AppView comparte con mubEZ `civic_delegation_resolver_secret` para
resolver delegaciones cívicas.

## Desarrollo

Con Node 24.18 o superior y pnpm 11 (Corepack), desde `mubEZ/`:

```bash
pnpm install --frozen-lockfile
cp .env.example .env           # una vez; cambia JWT_SECRET, APP_KEY, COOKIE_SECRET y la llave del emisor
pnpm keys:generate             # la llave Ed25519 del emisor de credenciales
pnpm db:migrate
pnpm dev                       # http://localhost:8787
curl -s localhost:8787/v1/health
```

**Contra un PDS local**, pásale el PLC y el PDS por entorno; le ganan al
`.env`:

```bash
PLC_URL=http://localhost:2582 PDS_URL=http://localhost:2583 pnpm dev
```

Sin eso resuelve contra `plc.directory` y no encuentra los DIDs locales.
Sirve igual para `make run-dev-env` (tradicional) y para `make dev` (vlpds):
los dos usan el PLC en 2582 y el PDS en 2583.

**Pruebas:**

```bash
pnpm lint && pnpm build
pnpm test:integration          # con Node 24: el binario de better-sqlite3 está compilado para esa versión
pnpm test:e2e
```

## Producción: el perfil `identity`

En la plataforma, mubEZ es el perfil `identity` (`overrides/identity.yaml`
sobre el `docker-compose.yml` de mubEZ):

- **Reemplaza por completo la configuración de desarrollo.** El compose de
  mubEZ fija `NODE_ENV=development` y trae secretos por defecto. En
  development, el broker entrega un token M8 **para cualquier DID sin
  pruebas** (dev-token bootstrap). El override fuerza producción, apaga ese
  bootstrap, habilita el handoff OAuth y toma los secretos de `secrets/`.
- **No publica el puerto 8787.** Solo Caddy habla con el broker, en
  `https://m8.<dominio>`.
- **Su base vive en el bucket.** Litestream replica cada cambio en ~1 s
  (`LITESTREAM_SYNC_INTERVAL`) a `S3_PREFIX/litestream/mubez` y, si el disco
  está vacío, la restaura al arrancar. **Nunca levantes dos brokers con el
  mismo `S3_PREFIX`.**
- **Depende de mubEZ#6** (rama `infra/vlpds-style`), que agrega a la imagen la
  carga de secretos `_FILE` y Litestream. Con mubEZ en `main`, el perfil
  `identity` pasa la validación de Compose pero el broker no lee sus secretos
  ni replica. Ese PR sigue abierto a propósito.

### Secretos

| Archivo | Para qué | Offline |
|---|---|---|
| `m8_jwt_secret`, `m8_app_key`, `m8_cookie_secret` | Tokens M8, AdonisJS, cookies | |
| `m8_civic_vote_proof_secret` | Autorizaciones de voto público | |
| `civic_delegation_resolver_secret` | Compartido con el AppView | |
| `m8_curp_pepper` | Hash de las CURP. Perderlo rompe la deduplicación | **sí** |
| `m8_issuer_private_jwk`, `m8_issuer_public_jwk` | Emisor de credenciales (Ed25519) | **sí** (la privada) |
| `s3_access_key_id`, `s3_secret_access_key` | Litestream | |

`make secrets` los genera (el par del emisor necesita Node en la máquina; si
no hay, usa `pnpm keys:generate` en mubEZ). `make secrets-import` los trae de
un `env/identity.env` viejo. Si un secreto queda en `env/identity.env` y
también como archivo, el broker no arranca.

### Ajustes no secretos

En `.env` de la plataforma:

| Variable | Por defecto |
|---|---|
| `M8_CORS_ORIGIN` | `https://PARA_DOMAIN` |
| `M8_OAUTH_RETURN_TO_ALLOWLIST` | `im8://oauth/callback,para://m8-auth,https://PARA_DOMAIN/m8-auth` |
| `LITESTREAM_SYNC_INTERVAL` | `1s` (cada PUT cuesta; `10s` es diez veces más barato a cambio de poder perder 10 s) |

El resto, en `env/identity.env` (ver `env/identity.env.example`).

### El login de PARA

```bash
make backup                                            # antes de las migraciones 040–041 del broker
make login-preflight WEB_ORIGIN=https://ORIGEN_WEB_REAL
```

El preflight revisa el entorno efectivo, las migraciones en modo lectura, los
metadatos públicos, CORS y la ruta web `/m8-auth`. No crea sesiones ni emite
tokens.

## En la variante tradicional

Igual que arriba en desarrollo. El Ansible de WatZappa no despliega mubEZ:
corre aparte, con su propio compose, o con el perfil `identity` de la copia
archivada de para-platform ([tradicional/](tradicional/README.md)). Las mismas
advertencias valen: nunca con `NODE_ENV=development` en un servidor.
