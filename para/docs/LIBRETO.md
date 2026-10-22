# Libreto: PARA sobre vlpds

Los comandos para levantar y probar cada parte de la plataforma con vlpds como
PDS, en el orden en que conviene hacerlo, con lo que debe salir en cada paso.
Para la variante con el PDS en TypeScript está
[tradicional/LIBRETO.md](tradicional/LIBRETO.md).

Todos los comandos se corren desde `vlpds/para/` salvo que diga otra cosa.

```
0. Antes de empezar          requisitos, carpetas, lo que hay que saber cada vez
1. Validar sin levantar nada make check
2. Entorno local             make dev → make dev-smoke → make dev-down
3. Pruebas aisladas          make e2e-dm
4. Producción en tu máquina  el perfil core sobre un S3 local
5. Producción de verdad      secrets → matrix-init → preflight → up → verify
6. Cada perfil por separado  core, matrix, identity, edge, monitoring, web
7. Día a día                 logs, consola de vlpds, invitaciones, actualizar, respaldar
8. Herramientas de DX        qué hace cada comando
9. Problemas conocidos
```

---

## 0. Antes de empezar

**Herramientas:** Docker con Compose v2.24 o superior, `make`, `git`, `curl`,
`openssl` y `python3`. Para `make e2e-dm`, además Node (con nvm, cada repo usa
su `.nvmrc`) y pnpm.

**Carpetas.** La plataforma busca los otros repos como hermanos del fork:

```
~/code/
  vlpds/        el fork (mexico-parasocial/vlpds), rama para/platform
    para/       ← aquí corres todo
  WatZappa/     AppView, bsync, Ozone, bridge, para-idp (y su Matrix)
  mubEZ/        broker M8 (perfil identity)
  PARA/         la app (perfil web y make e2e-dm)
  iM8/          la billetera (opcional)
```

Si no están así, pon sus rutas en `.env` (`WATZAPPA_DIR`, `MUBEZ_DIR`,
`PARA_DIR`, `IM8_DIR`).

**Lo que hay que saber cada vez que la levantas:**

1. **Las escrituras `com.para.*` no funcionan con vlpds** (crear comunidad,
   unirse, votar, factor de correo…). Es una brecha conocida, no un fallo de
   tu entorno: [VLPDS.md §3](VLPDS.md#3-la-brecha-de-compara).
2. **El AppView y el bridge leen del relay, no de vlpds.** Si el AppView no
   indexa, lo primero es ver si el relay sigue a vlpds (pasos 2.3 y 5.6).
3. **Nada persiste en `make dev`**: vlpds corre en memoria y Postgres en
   tmpfs. `make dev-down` lo borra todo; las llaves de desarrollo se quedan en
   `.dev/dev.env`.
4. **En producción el estado es el bucket.** Nunca apuntes dos despliegues al
   mismo `S3_PREFIX`, ni levantes dos nodos de vlpds con el mismo
   `VLPDS_NODE_ID`: se bloquean entre sí.
5. **La KEK y la llave PLC envuelta van offline.** Sin la KEK, los repos del
   bucket no se pueden firmar; sin la llave PLC, no se pueden actualizar los
   DIDs. No se pueden recuperar de ningún otro lado.
6. **La imagen del AppView aún no se construye desde WatZappa** (el problema
   de rolldown; hay un arreglo en camino). Hasta que se mezcle, el entorno
   local y la prueba del paso 4 usan una imagen local con `api.js` y
   `dataplane.js` montados (`runtime/bsky/`).
7. **El perfil `identity` depende de mubEZ#6** (secretos como archivos y
   Litestream). Con mubEZ en `main`, ese perfil no arranca bien en producción.

---

## 1. Validar sin levantar nada

```bash
make check
```

Revisa, solo con Docker:

- la sintaxis y shellcheck de todos los scripts;
- la configuración de Compose con secretos falsos, en los dos perfiles de
  tamaño y con todos los perfiles de Compose;
- el Caddyfile, con y sin los sitios de Matrix e identidad;
- las alertas (promtool), Alloy y los playbooks de Ansible.

**Debe terminar en** `✓ Todo válido.` Córrelo antes de cada commit que toque
`para/`.

Para ver la configuración final que usaría Compose con tu `.env`:

```bash
make config
```

---

## 2. Entorno local

vlpds, un PLC local, el relay, el AppView (api + dataplane) y bsync en Docker.
Todos comparten la red de un contenedor `net`, así que `localhost:<puerto>` es
lo mismo dentro de cada servicio y desde tu Mac: las URLs que quedan en los
documentos DID funcionan en todas partes.

| Puerto | Servicio |
|---|---|
| 2582 | PLC |
| 2583 | vlpds |
| 2584 | AppView |
| 2470 | relay |
| 2585, 2586 | dataplane y bsync (solo dentro de la red) |

### 2.1 Levantar

```bash
make dev
```

La primera vez construye el PLC y el relay desde sus commits fijados (unos
minutos) y bsync desde WatZappa `main` (~2 min). Las siguientes, segundos.

**Debe salir:**

```
✓ PLC  http://localhost:2582/_health
✓ vlpds  http://localhost:2583/xrpc/_health
✓ AppView  http://localhost:2584/xrpc/_health
✓ relay  http://localhost:2470/xrpc/_health
✓ el relay sigue el firehose de vlpds
```

Si dice `Puertos ocupados`, hay otro entorno (de PARA tradicional o un
`make dev` viejo) usando 2470 o 2582–2584. Apágalo antes.

### 2.2 La prueba de punta a punta

```bash
make dev-smoke
```

Crea una cuenta de prueba en vlpds, publica un post, espera a que el AppView
lo indexe (vlpds → relay → AppView) y lo lee a través del proxy de vlpds al
AppView.

**Debe salir:**

```
✓ cuenta en vlpds: smoke<…>.test (did:plc:…)
✓ publicado en vlpds
✓ el AppView lo indexó (vlpds → relay → AppView)
✓ vlpds la sirve a través de su proxy al AppView
```

Se puede correr las veces que quieras: cada vez usa una cuenta nueva.

### 2.3 Mirar por dentro

```bash
# Estado de los contenedores
docker compose -f dev/compose.yaml --env-file .dev/dev.env ps

# Logs de un servicio (vlpds, relay, dataplane, appview, bsync, plc, postgres)
docker compose -f dev/compose.yaml --env-file .dev/dev.env logs -f dataplane

# ¿El relay sigue a vlpds? Debe listar localhost:2583 con status active
curl -s localhost:2470/xrpc/com.atproto.sync.listHosts

# ¿Qué indexó el AppView?
docker compose -f dev/compose.yaml --env-file .dev/dev.env exec -T postgres \
  psql -U postgres -d appview -c 'select count(*) from actor' -c 'select count(*) from record'

# Descripción del servidor vlpds
curl -s localhost:2583/xrpc/com.atproto.server.describeServer
```

La consola de vlpds en desarrollo está en `http://localhost:2583/admin`, con
el token `dev-admin-token` (solo en `--dev-mode`).

### 2.4 La brecha de `com.para.*`

Para ver con tus ojos lo que falta en vlpds (y comprobar cuándo se resuelva):

```bash
PDS=http://localhost:2583
h="prueba$(date +%s).test"; pw=$(openssl rand -hex 12)
jwt=$(curl -sf $PDS/xrpc/com.atproto.server.createAccount -H 'content-type: application/json' \
  -d "{\"handle\":\"$h\",\"email\":\"$h@example.invalid\",\"password\":\"$pw\"}" \
  | python3 -c 'import json,sys;print(json.load(sys.stdin)["accessJwt"])')
P='atproto-proxy: did:web:localhost%3A2584#bsky_appview'

# Una escritura de PARA: hoy 400 (el AppView no la tiene; sin la cabecera, 501)
curl -s -w ' %{http_code}\n' -X POST $PDS/xrpc/com.para.community.join \
  -H "authorization: Bearer $jwt" -H "$P" -H 'content-type: application/json' -d '{}'

# Una lectura de PARA: 200 (el AppView sí la tiene)
curl -s -w ' %{http_code}\n' $PDS/xrpc/com.para.feed.getTimeline -H "authorization: Bearer $jwt" -H "$P"
```

### 2.5 Apagar

```bash
make dev-down
```

Borra los contenedores y el volumen del relay. Lo demás ya vivía en memoria.

---

## 3. Pruebas aisladas

```bash
make e2e-dm
```

Mensajes directos cifrados por Matrix, de punta a punta: un Synapse
desechable, las rutas reales del bridge con pruebas de identidad reales y tres
clientes de matrix-js-sdk con cifrado Rust. No usa vlpds ni `make dev`, y al
terminar borra su contenedor. Necesita las dependencias instaladas en
`WatZappa/services/matrix-bridge` y en `PARA` (`pnpm install` en cada uno).

**Debe terminar en** `✓ Listo`. El detalle queda en `.dev/logs/e2e-dm.log`.

---

## 4. La producción en tu máquina

El perfil `core` tal como corre en producción (vlpds con KEK, llave PLC
envuelta, secretos como archivos, relay, respaldos), sobre un S3 local
(SeaweedFS) y sin el edge, que necesita DNS real. Sirve para probar cambios de
`core.yaml`, de los secretos o de los respaldos sin servidor.

> No crees cuentas en esta prueba: con `PLC_URL` por defecto, vlpds las
> registraría en el `plc.directory` público.

**4.1 Secretos de prueba, en una carpeta temporal:**

```bash
T=$(mktemp -d)/secrets; mkdir -p "$T"
PARA_SECRETS_DIR=$T PROFILES=core ./scripts/secrets.sh generate   # incluye la llave PLC envuelta (vw1.…)
openssl rand -hex 16 | tr -d '\n' > $T/s3_access_key_id
openssl rand -hex 32 | tr -d '\n' > $T/s3_secret_access_key
for n in matrix_admin_token matrix_appservice_token matrix_hs_token; do openssl rand -hex 16 | tr -d '\n' > $T/$n; done
chmod 0444 $T/*
PARA_SECRETS_DIR=$T PROFILES=core ./scripts/secrets.sh check      # debe dar 0 ❌
```

**4.2 Mientras la imagen del AppView no se construya** (rolldown), un override
con las imágenes locales. Guárdalo fuera del repo, por ejemplo en `$T/../override.yaml`:

```yaml
services:
  dataplane:
    image: para-core-bsky:latest        # tu imagen local del AppView
    volumes: ['${PLATFORM_DIR}/runtime/bsky/dataplane.js:/app/services/bsky/dataplane.js:ro']
  bsky:
    image: para-core-bsky:latest
    volumes: ['${PLATFORM_DIR}/runtime/bsky/api.js:/app/services/bsky/api.js:ro']
  bsync:
    image: para-bsync:ea85ae3           # la que construye make dev
  ozone:
    image: para-core-ozone:latest
```

**4.3 Una función para no repetir variables:**

```bash
O=$(dirname $T)/override.yaml
c() { env -i PATH="$PATH" HOME="$HOME" DOCKER_CONFIG="$HOME/.docker" \
  COMPOSE_PROJECT_NAME=para-prodtest PARA_DOMAIN=para.test \
  S3_BUCKET=para-test S3_ENDPOINT=http://s3-local:8333 S3_REGION=us-east-1 \
  PARA_PROFILE=tiny PARA_SECRETS_DIR=$T PROFILES=core,local-s3 \
  POSTGRES_PORT=55432 VLPDS_ADMIN_PORT=52620 PDS_CRAWLERS_EXTRA= \
  WATZAPPA_DIR=$(cd ../../WatZappa && pwd) \
  make -s compose ARGS="-f compose.yaml -f $O $*"; }
```

**4.4 Levantar:**

```bash
c up -d --wait s3-local postgres
docker run --rm --network para-prodtest_default \
  -e AWS_ACCESS_KEY_ID=$(cat $T/s3_access_key_id) -e AWS_SECRET_ACCESS_KEY=$(cat $T/s3_secret_access_key) \
  -e AWS_DEFAULT_REGION=us-east-1 amazon/aws-cli:2.27.0 \
  --endpoint-url http://s3-local:8333 s3 mb s3://para-test
c build relay backup                 # nunca --build en up: intentaría compilar WatZappa
c up -d --no-build pds relay redis bsync dataplane bsky ozone backup
c ps
```

**4.5 Lo que debe pasar:**

```bash
# pds, relay y bsky "healthy"; dataplane, bsync, ozone y backup "Up"
c ps

# vlpds escribe su estado en el bucket (prod/vlpds/…)
docker run --rm --network para-prodtest_default \
  -e AWS_ACCESS_KEY_ID=$(cat $T/s3_access_key_id) -e AWS_SECRET_ACCESS_KEY=$(cat $T/s3_secret_access_key) \
  -e AWS_DEFAULT_REGION=us-east-1 amazon/aws-cli:2.27.0 \
  --endpoint-url http://s3-local:8333 s3 ls s3://para-test/prod/ --recursive | head

# Un respaldo de Postgres: una línea por base (para, relay, matrix, mas, bridge)
docker exec para-prodtest-backup-1 backup.sh once

# El documento DID del AppView y la descripción de vlpds
docker exec para-prodtest-bsky-1 wget -qO- http://127.0.0.1:2584/.well-known/did.json
docker exec para-prodtest-pds-1 curl -s http://127.0.0.1:2583/xrpc/com.atproto.server.describeServer
```

En esta prueba vlpds **no** avisa al relay: `pds.para.test` termina en `.test`
y vlpds no pide que rastreen una dirección local. Es lo esperado; en
producción sí lo hace.

**4.6 Apagar y borrar todo:**

```bash
c down -v
```

---

## 5. Producción de verdad

Una máquina de cero. Más detalle en [Despliegue](DESPLIEGUE.md); con Ansible,
[deploy/ansible/README.md](../deploy/ansible/README.md).

**5.1 Lo que necesitas antes:**

- Una VM Linux con Docker: 4 vCPU / 12 GB para `tiny`, 8 vCPU / 32 GB / NVMe
  para `standard`.
- Un bucket privado (R2, S3, GCS…) con una llave solo para él y el versionado
  activado.
- DNS: registros A de `PARA_DOMAIN`, `pds.`, `appview.`, `ozone.`, `bridge.`,
  `m8.`, `matrix.`, `auth.`, `idp.` y `chat.`, más el comodín `*.PARA_DOMAIN`
  para los handles. **Elige el dominio para siempre**: queda en cada DID.
- Un lugar offline para las llaves (un gestor de contraseñas del equipo o el
  inventario cifrado con sops).

**5.2 Repos y configuración:**

```bash
mkdir -p /opt/para && cd /opt/para
git clone -b para/platform https://github.com/mexico-parasocial/vlpds.git
git clone https://github.com/mexico-parasocial/WatZappa.git
git clone https://github.com/mexico-parasocial/mubEZ.git      # perfil identity
cd vlpds/para
cp .env.example .env && $EDITOR .env      # PARA_DOMAIN, ACME_EMAIL, S3_*, PARA_PROFILE, PROFILES
```

**5.3 Secretos** (como root, para que queden `root:10001` con `0440`):

```bash
sudo make secrets                          # todo lo aleatorio, la KEK y la llave PLC envuelta
echo -n 'llave-del-bucket'   | sudo tee secrets/s3_access_key_id >/dev/null
echo -n 'secreto-del-bucket' | sudo tee secrets/s3_secret_access_key >/dev/null
sudo scripts/secrets.sh perms
sudo scripts/secrets.sh check
sudo scripts/secrets.sh list               # columna OFFLINE: guárdalos ahora
```

**5.4 Matrix, una vez** (si `PROFILES` incluye `matrix`):

```bash
make matrix-init      # Synapse, MAS, Element y el registro del appservice, en WatZappa/deploy/matrix
```

**5.5 Revisar, levantar, verificar:**

```bash
make preflight BUILD=1        # configuración, secretos, bucket (escribe, lee, lista, borra; no público), DNS
make up BUILD=1       # construye las imágenes de WatZappa y del relay aquí (hasta que haya CI)
make verify
```

**5.6 Lo que `make verify` debe confirmar** (y qué mirar si no):

| Comprobación | Si falla |
|---|---|
| Cada contenedor de `PROFILES` corriendo y sano | `make logs S=<servicio>` |
| Endpoints públicos con HTTPS válido | DNS y `make logs S=caddy` |
| `/admin`, `vlpds.admin.*` y `/metrics` de vlpds dan 404 desde fuera | Revisa `edge/Caddyfile` |
| vlpds responde `/tls-check` | `make logs S=pds` |
| El relay sigue a `pds.<dominio>` | `make relay-add-pds` y `make logs S=relay` |
| El último respaldo de Postgres está bien | `make logs S=backup` |

**5.7 Primeros pasos con la plataforma arriba:**

```bash
make invite                 # código de invitación (USES=5 para varios usos)
# crea la primera cuenta desde la app, pon su DID en ADMIN_DIDS (Ozone) y:
make up
make matrix-admin-token     # con Matrix: usuario admin y su token para el bridge
make up UP_ARGS=--force-recreate
make app-env > app.env      # las URLs para compilar la app contra este dominio
```

---

## 6. Cada perfil por separado

`PROFILES` en `.env` decide qué levanta `make up`. Puedes levantar uno solo
para probarlo: `make up PROFILES=core`.

| Perfil | Qué levanta | Cómo comprobarlo |
|---|---|---|
| `core` | vlpds (`pds`), relay, Postgres, Redis, dataplane, bsky, bsync, Ozone, backup | `make ps`; `make vlpds-admin ARGS="cluster status"`; `curl -s https://appview.<dominio>/xrpc/_health` |
| `edge` | Caddy (80/443) | `curl -sI https://pds.<dominio>/xrpc/_health`; `curl -s -o /dev/null -w '%{http_code}' https://pds.<dominio>/admin` debe dar 404 |
| `matrix` | Synapse, MAS, para-idp, Element, matrix-bridge, respaldo de media | `curl -s https://matrix.<dominio>/_matrix/client/versions`; `curl -s https://bridge.<dominio>/healthz` |
| `identity` | mubEZ (broker M8) | `curl -s https://m8.<dominio>/v1/health`; `make login-preflight WEB_ORIGIN=https://<dominio>` |
| `monitoring` | Alloy hacia tu Prometheus/Loki | Métricas con `instance=<dominio>` en tu Prometheus |
| `local-s3` | SeaweedFS como bucket (nunca en producción) | `curl -s localhost:8333` |
| `web` | La app web (bskyweb) en el dominio raíz | `curl -sI https://<dominio>/` |

`core` es el único imprescindible; `matrix` necesita `core` (Postgres) y
`edge` para que los clientes lleguen.

---

## 7. Día a día

**Estado y logs**

```bash
make ps
make logs S=pds              # o relay, dataplane, bsky, matrix-bridge, caddy…
make doctor                  # configuración y endpoints públicos, con el arreglo de cada fallo
```

**La consola de vlpds.** Solo escucha en `127.0.0.1:2620` del servidor. Desde
tu máquina:

```bash
ssh -L 2620:127.0.0.1:2620 tu-servidor
# abre http://localhost:2620/admin; el token está en secrets/vlpds_admin_token
```

**Comandos de operador de vlpds** (dentro del contenedor, con el token ya
cargado):

```bash
make vlpds-admin ARGS="cluster status"            # lease válido, shards, nivel de funciones
make vlpds-admin ARGS="account list"
make vlpds-admin ARGS="request-crawl bsky.network" # anunciar el servidor a la red de Bluesky
make invite USES=5
```

**Actualizar**

```bash
make backup                                   # antes de cualquier cambio de versión
# vlpds: cambia VLPDS_IMAGE en .env; WatZappa: git pull en ../../WatZappa (o WATZAPPA_VERSION)
make up BUILD=1
make verify
```

**Respaldar y restaurar Postgres**

```bash
make backup
make restore ARGS="list"
make restore ARGS="postgres para latest --force"   # detén antes lo que usa esa base
```

vlpds no tiene nada que restaurar: con el mismo bucket, la KEK y la llave PLC
envuelta, arranca donde quedó ([Respaldos](RESPALDOS.md)).

---

## 8. Herramientas de DX

| Comando | Qué hace |
|---|---|
| `make help` | Lista todos los comandos con su descripción |
| `make dev` | Entorno local con vlpds, PLC, relay, AppView y bsync; genera llaves de desarrollo en `.dev/dev.env` y registra vlpds en el relay |
| `make dev-smoke` | Cuenta → post → indexado por el AppView → leído por el proxy de vlpds |
| `make dev-down` | Apaga el entorno local y borra sus datos |
| `make e2e-dm` | DMs cifrados por Matrix de punta a punta, aislados |
| `make check` | Valida scripts, Compose, Caddy, alertas, Alloy y Ansible sin servidor |
| `make config` | La configuración final de Compose con tu `.env` |
| `make secrets` | Genera los secretos que falten (nunca pisa los existentes), incluida la llave PLC envuelta con la KEK |
| `make secrets-import` | Trae secretos de un despliegue tradicional (`WatZappa/.env`, `env/identity.env`); envuelve su llave PLC con la KEK |
| `scripts/secrets.sh check` / `list` / `perms` | Valida (presentes, largos, distintos, sin pinta de desarrollo) / qué es cada uno y cuáles van offline / permisos |
| `make preflight` | Antes de levantar: configuración, secretos, bucket, DNS, Compose (`BUILD=1` si las imágenes se construyen aquí) |
| `make up` / `make down` | Levanta o detiene `PROFILES` (`BUILD=1` construye aquí; `UP_ARGS=--force-recreate` tras cambiar un secreto) |
| `make verify` | Después de levantar: contenedores, HTTPS, rutas de operador cerradas, `/tls-check`, relay, respaldos |
| `make doctor` | Diagnóstico rápido de producción |
| `make ps` / `make logs S=…` / `make restart S=…` / `make stop S=…` | Lo de siempre, con la configuración de la plataforma |
| `make compose ARGS="…"` | Cualquier comando de Compose con la configuración de la plataforma |
| `make invite [USES=n]` | Código de invitación de vlpds |
| `make vlpds-admin ARGS="…"` | Cualquier comando de operador de vlpds |
| `make relay-add-pds` | Vuelve a pedir al relay que siga a vlpds |
| `make matrix-init` | Genera la configuración de Synapse, MAS, Element y el appservice para `PARA_DOMAIN` |
| `make matrix-admin-token` | Usuario admin de Synapse para el bridge y su token en `secrets/` |
| `make db-init` | Crea las bases compartidas (matrix, mas, bridge, relay) en un Postgres que ya existía |
| `make backup` / `make restore` / `make restore-media` | Respaldo inmediato / restaurar Postgres / restaurar el media de Synapse |
| `make app-env` | Las variables para compilar la app PARA contra este dominio |
| `make login-preflight WEB_ORIGIN=…` | Revisa el login de mubEZ sin emitir tokens |
| `make deploy INVENTORY=…` | Provisiona y despliega con Ansible |

Lo que trabaja por ti sin que lo pidas:

- `runtime/env-from-files.sh`: carga los secretos `NAME_FILE` en las imágenes
  de WatZappa, que no saben leerlos, y se niega a arrancar si un secreto falta,
  está vacío o llega por dos lados.
- vlpds se niega a arrancar con secretos débiles, repetidos o sin KEK.
- Caddy reintenta 10 s mientras un servicio reinicia, así que actualizar no
  produce 502.
- Los respaldos de Postgres corren solos cada `BACKUP_INTERVAL` y exponen
  métricas para las alertas `ParaBackupStale` y `ParaBackupFailing`.

---

## 9. Problemas conocidos

| Síntoma | Causa | Qué hacer |
|---|---|---|
| La app no puede crear comunidades, unirse ni votar | Las escrituras `com.para.*` no existen en vlpds | Ninguno por ahora: [VLPDS.md §3](VLPDS.md#3-la-brecha-de-compara) |
| El AppView no indexa nada | El relay no sigue a vlpds | `curl -s localhost:2470/xrpc/com.atproto.sync.listHosts` (local) o `make verify`; `make relay-add-pds` |
| `error in firehose event lexicon validation: Expected integer … at $.seq` | Algo lee el firehose de vlpds directo | Apúntalo al relay (`ws://relay:2470`) |
| `--crawlers entry ignored: … not a fully qualified hostname` | Un crawler sin punto en el nombre | Usa `relay.para.internal` (el alias del relay) |
| El relay responde `HostNotFound … not a public IP address` | El relay solo acepta hosts públicos | Normal en local: `make dev` lo registra por la base. En producción, revisa el DNS de `pds.` |
| `make up BUILD=1` falla en `oauth-provider-ui` con `Cannot find native binding` | El problema de rolldown en WatZappa | Espera el arreglo (rama `fix/pnpm-devengines-node-range`) o usa imágenes ya construidas |
| vlpds se reinicia con exit 2–9 | Se detiene a propósito ante fallos (lease perdido, bucket inaccesible) | `make logs S=pds`; Docker lo vuelve a levantar. Tras un SIGKILL no escribe durante ~un lease (60 s en tiny) |
| `budget is … MiB … Lower the cache flags` | Menos de 2 GB para vlpds | Sube `VLPDS_MEM` |
| El respaldo falla con `server version mismatch` | `pg_dump` más viejo que Postgres | La imagen de respaldos debe tener la misma versión mayor (17) |
