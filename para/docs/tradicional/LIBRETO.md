# Libreto: PARA tradicional (WatZappa con su PDS en TypeScript)

Los comandos para levantar y probar cada parte de la variante tradicional, en
el orden en que conviene hacerlo, con lo que debe salir en cada paso. Para la
variante con vlpds está [../LIBRETO.md](../LIBRETO.md).

Salvo que diga otra cosa, los comandos se corren desde la raíz de **WatZappa**.

```
0. Antes de empezar           requisitos, carpetas, lo que hay que saber cada vez
1. Preparar WatZappa          dependencias y build
2. Desarrollo                 make run-dev-env, cuentas de prueba, diagnóstico
3. Matrix en desarrollo       make matrix-up y compañía
4. mubEZ y la app             contra el entorno local
5. Producción                 con el Ansible de WatZappa
6. Pruebas                    unitarias, Matrix, DMs, producción
7. Herramientas de DX         qué hace cada comando
8. Problemas conocidos
```

---

## 0. Antes de empezar

**Herramientas:** Docker, Node 22 (WatZappa) y 24 (mubEZ) con nvm (cada repo
trae su `.nvmrc`), pnpm 11 por Corepack, `make`, `jq`, `curl`, `openssl`.

```bash
corepack enable
nvm install            # en cada repo, lee su .nvmrc
```

**Carpetas** (hermanas):

```
~/code/
  WatZappa/   PDS, AppView, bsync, Ozone, bridge, para-idp, Matrix
  mubEZ/      broker M8
  PARA/       la app
  iM8/        la billetera
```

**Lo que hay que saber cada vez que la levantas:**

1. **El entorno de desarrollo es persistente.** Cuentas, repos, el PLC local y
   el bridge viven en `WatZappa/.dev-env-data/`, y Postgres y Redis en
   contenedores que se quedan arriba al salir. No mezcles carpetas de datos:
   rompe la identidad de los DIDs y los cursores. Para empezar de cero, ver
   §2.6.
2. **Los DIDs son del PLC local** (`localhost:2582`), no de `plc.directory`.
   Si la app o mubEZ resuelven contra el público, verán DIDs inexistentes.
3. **`make run-dev-env` recompila el PDS y el AppView** cada vez que arranca
   (para no correr migraciones viejas). La primera vez tarda.
4. **La imagen Docker del AppView aún no se construye** (el problema de
   rolldown; arreglo en la rama `fix/pnpm-devengines-node-range`). Afecta al
   despliegue de producción, no a `make run-dev-env`.
5. **El bridge en desarrollo corre en el host**, no en Docker: es parte de
   `make run-dev-env` (puerto 3001). No levantes también el del compose.

---

## 1. Preparar WatZappa

```bash
nvm use
pnpm install --frozen-lockfile
pnpm build          # codegen + TypeScript; también lo hace run-dev-env para pds, bsky y dev-env
```

Si después de cambiar de rama algo corre código viejo: `pnpm build` en el
paquete que cambió (los servicios de `dev-env` se cargan desde `dist`).

---

## 2. Desarrollo

### 2.1 Levantar

```bash
make run-dev-env               # o make run-dev-env-logged, con logs de cada servicio
```

| Puerto | Servicio |
|---|---|
| 2581 | Introspección de dev-env |
| 2582 | PLC local |
| 2583 | PDS |
| 2584 | AppView |
| 2587 | Ozone |
| 2590 | Chat (`chat.bsky`) |
| 3001 | matrix-bridge (`BRIDGE_PORT`) |
| 5434, 6381 | Postgres y Redis en Docker (`packages/dev-infra`) |

Al arrancar imprime `Persistent dev profile: …/.dev-env-data (PostgreSQL schema: para_local)`
y, si un puerto está ocupado, se niega con `Port … is already in use`.

### 2.2 Comprobar que responde

```bash
curl -s localhost:2583/xrpc/_health                       # PDS
curl -s localhost:2584/xrpc/_health                       # AppView
curl -s localhost:2583/xrpc/com.atproto.server.describeServer
curl -s localhost:3001/healthz                            # bridge
make doctor                                               # todos los chequeos de salud
```

### 2.3 Cuentas de prueba y datos

```bash
./scripts/seed-test-accounts.sh     # idempotente; credenciales en test-accounts.json (gitignored)
make seed-memes-persistent          # imágenes de memes en el PDS local
```

Las cuentas viven en el PDS y el PLC locales: nada es público y no se manda
correo (dev-env instala un mailer de prueba).

### 2.4 Una prueba de punta a punta a mano

```bash
PDS=http://localhost:2583
h="prueba$(date +%s).test"; pw=$(openssl rand -hex 12)
s=$(curl -sf $PDS/xrpc/com.atproto.server.createAccount -H 'content-type: application/json' \
  -d "{\"handle\":\"$h\",\"email\":\"$h@example.invalid\",\"password\":\"$pw\"}")
did=$(echo "$s" | jq -r .did); jwt=$(echo "$s" | jq -r .accessJwt)

curl -sf $PDS/xrpc/com.atproto.repo.createRecord -H "authorization: Bearer $jwt" -H 'content-type: application/json' \
  -d "{\"repo\":\"$did\",\"collection\":\"app.bsky.feed.post\",\"record\":{\"\$type\":\"app.bsky.feed.post\",\"text\":\"hola\",\"createdAt\":\"$(date -u +%Y-%m-%dT%H:%M:%S.000Z)\"}}"

sleep 2; curl -s "localhost:2584/xrpc/app.bsky.feed.getAuthorFeed?actor=$did" | jq '.feed | length'   # 1
```

A diferencia de vlpds, aquí funcionan también las rutas de PARA, por ejemplo
`com.para.community.listBoards` o `com.para.community.createBoard`.

### 2.5 Una demo desechable

```bash
make run-demo-env     # dev-env completo con datos de demo; nada persiste, sin salas de Matrix
```

### 2.6 Apagar y empezar de cero

`Ctrl-C` detiene el PDS, el AppView y el bridge. Postgres y Redis se quedan
arriba a propósito:

```bash
docker compose -f packages/dev-infra/docker-compose.yaml stop db redis     # apagarlos
# empezar de cero (borra cuentas, posts y la base local):
docker compose -f packages/dev-infra/docker-compose.yaml rm -sf db redis
docker volume rm -f dev-infra_atp_db dev-infra_atp_redis
rm -rf .dev-env-data
```

---

## 3. Matrix en desarrollo

Synapse, MAS, para-idp y Element en Docker. El script resuelve la raíz del
repo desde su propia ubicación, así que da igual desde dónde lo llames, y se
niega a arrancar si falta un archivo de configuración (antes, Docker creaba
carpetas vacías y Synapse quedaba en un bucle de reinicios).

```bash
cp .env.example .env            # si no existe: setup.sh lee de ahí MATRIX_* y POSTGRES_*
deploy/matrix/setup.sh          # una vez: genera la configuración de Synapse
deploy/matrix/setup-mas.sh      # una vez: MAS y para-idp
make matrix-up                  # el bridge sigue en el host (make run-dev-env)
make matrix-status              # contenedores y una prueba a cada endpoint
make matrix-doctor              # qué tan lejos llegó comunidad → espacio → evento
make matrix-logs SERVICE=synapse
make matrix-smoke               # arranca Synapse + bridge y prueba las rutas del appservice
make matrix-down                # conserva los volúmenes
```

`make matrix-up-with-bridge` usa el bridge en contenedor: detén antes el del
host, porque no pueden compartir el puerto 3001.

---

## 4. mubEZ y la app contra el entorno local

**mubEZ** (desde `mubEZ/`, con Node 24):

```bash
pnpm install --frozen-lockfile
cp .env.example .env            # una vez; cambia los secretos de ejemplo
pnpm db:migrate
PLC_URL=http://localhost:2582 PDS_URL=http://localhost:2583 pnpm dev
curl -s localhost:8787/v1/health
```

Pasarle `PLC_URL` y `PDS_URL` locales es lo que evita que resuelva handles
contra el PLC público. Más en [../MUBEZ.md](../MUBEZ.md).

**La app** (desde `PARA/`): las URLs locales van en `.env.local` (ver
`PARA/.env.example`). En un teléfono físico, `localhost` es el teléfono: usa la
IP de tu Mac (`ipconfig getifaddr en0`) y reinicia Metro con `--clear`
después de cambiarla.

---

## 5. Producción con el Ansible de WatZappa

Un host con dos stacks detrás de Caddy (`WatZappa/ansible/README.md`):

| Stack | Compose | Servicios |
|---|---|---|
| app | `docker-compose.prod.yaml` | postgres, redis, pds, dataplane, bsky, bsync, ozone, caddy |
| matrix | `docker-compose.matrix.yaml` | synapse, synapse-db, para-idp, mas, mas-db, element-web, matrix-bridge |

**5.1 Antes:** un host de 8 vCPU / 16 GB / 200 GB como mínimo, DNS de
`pds.`, `appview.`, `ozone.`, `bridge.`, `matrix.`, `chat.` y el dominio raíz.

**5.2 Secretos:**

```bash
./infrascripts/generate-secrets.sh > .env    # luego edita dominios y contraseñas de admin
```

El `.env` se sube tal cual al servidor (`mode 0600`, sin pasar por Jinja). Los
secretos de Ansible (S3, Postgres) van en un Vault:
`ansible/group_vars/all/vault.yml.example`.

**5.3 Desplegar:**

```bash
cd ansible
ansible-galaxy collection install -r requirements.yml     # una vez
$EDITOR inventory/production.yml                          # el host real (trae un placeholder que falla a propósito)
ansible-playbook playbooks/site.yml                       # aprovisiona y levanta los dos stacks
```

**5.4 Redesplegar código** (menos de un minuto):

```bash
ansible-playbook playbooks/deploy.yml                    # los dos stacks
ansible-playbook playbooks/deploy.yml --tags matrix      # solo Matrix
ansible-playbook playbooks/deploy.yml -e compose_build_no_cache=true
```

**5.5 Lo que el playbook verifica solo**, y falla si no:

- los dos stacks en `running` y cada contenedor con healthcheck en `healthy`;
- cada servicio de Matrix en su puerto de loopback;
- `mas-cli doctor` con sus tres comprobaciones de delegación;
- `https://matrix.<dominio>/_matrix/client/v3/login` respondiendo vía MAS.

**5.6 Después, a mano:**

```bash
make doctor          # en el servidor, contra los contenedores
make smoke-test      # pruebas de punta a punta contra producción
```

Respaldos: `infrascripts/backup-postgres.sh` para Postgres y
`deploy/matrix/backup.sh` para Matrix. En esta variante **el volumen del PDS
también hay que respaldarlo**: guarda las cuentas, un SQLite por repo y sus
llaves.

---

## 6. Pruebas

```bash
cd packages/pds && pnpm test          # o packages/bsky, packages/ozone…
pnpm run build:ts                     # el check "Build" de CI
make matrix-smoke                     # appservice de Matrix
```

Los mensajes directos cifrados de punta a punta (Synapse desechable, bridge
real, tres clientes) se prueban desde el fork, porque no dependen del PDS:

```bash
cd ../vlpds/para && make e2e-dm
```

---

## 7. Herramientas de DX

| Comando (en WatZappa) | Qué hace |
|---|---|
| `make help` | Lista los comandos |
| `make run-dev-env` / `run-dev-env-logged` | El entorno persistente: PLC, PDS, AppView, Ozone, chat y bridge |
| `make run-demo-env` | dev-env desechable con datos de demo |
| `./scripts/seed-test-accounts.sh` | Cuentas de prueba idempotentes en `test-accounts.json` |
| `make seed-memes-persistent` | Imágenes de memes en el PDS local |
| `make doctor` y `make pds-doctor`, `bsky-doctor`, `dataplane-doctor`, `bsync-doctor`, `ozone-doctor`, `postgres-doctor`, `redis-doctor`, `caddy-doctor`, `index-doctor` | Chequeos de salud, en local o contra los contenedores |
| `make matrix-up` / `matrix-up-with-bridge` / `matrix-down` / `matrix-status` / `matrix-doctor` / `matrix-logs` / `matrix-smoke` | El stack de Matrix |
| `make build` / `make test` / `make lint` / `make fmt` | Compilar, probar, revisar estilo |
| `make codegen` | Regenera el código tras tocar `lexicons/` o un `.proto` |
| `make deps` / `make clean-deps` / `make clean-build` | Dependencias y artefactos |
| `make pre-deploy` / `make smoke-test` | Antes y después de desplegar |
| `infrascripts/generate-secrets.sh` | El `.env` de producción con secretos nuevos |

En el entorno de desarrollo, además:

- `dev-env-persistent.sh` repara las migraciones del PDS de desarrollo y
  revisa las del AppView antes de arrancar.
- `with-matrix-bridge.sh` arranca el bridge cuando el PDS acepta conexiones y
  detiene los dos juntos.
- `matrix-stack.sh` se niega a arrancar si falta una configuración montada.

---

## 8. Problemas conocidos

| Síntoma | Causa | Qué hacer |
|---|---|---|
| `Port … is already in use` | Otro entorno (de PARA tradicional o de vlpds) usa 2581–2590 o 3001 | Apágalo: `make dev-down` en `vlpds/para`, o el proceso viejo |
| `Cannot find native binding` al construir imágenes o correr vitest | rolldown: pnpm se salta su binario | Espera el arreglo (rama `fix/pnpm-devengines-node-range`) |
| Corre código viejo después de cambiar de rama | `dist` desactualizado | `pnpm build` en el paquete, o reinicia `make run-dev-env` (recompila pds y bsky) |
| Synapse en bucle con `Must supply a config file` | Se arrancó el compose de Matrix desde otra carpeta | Usa `make matrix-up` (o `scripts/matrix-stack.sh`) |
| mubEZ inventa DIDs para los handles | Resuelve contra el PLC público | Arráncalo con `PLC_URL` y `PDS_URL` locales |
| `could not resolve proxy did` en la app | La app hace proxy a otro DID de AppView | Corrige `.env.local` y reinicia Metro con `--clear` |
