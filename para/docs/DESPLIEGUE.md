# Despliegue

Una máquina de cero hasta verificada. El ejemplo es el perfil `tiny`: una VM
de 4 vCPU / 12 GB con Ubuntu LTS, un bucket en R2 y DNS en cualquier
proveedor. Los comandos, con lo que debe salir en cada uno, también están en
el [libreto, paso 5](LIBRETO.md#5-producción-de-verdad).

> Antes de mover usuarios reales a vlpds, lee
> [VLPDS.md §3](VLPDS.md#3-la-brecha-de-compara): las escrituras propias de
> PARA todavía no funcionan con vlpds.

## Qué necesitas

| | Notas |
| --- | --- |
| Máquina | Linux con Docker y Compose v2.24+, 4 vCPU / 12 GB (`tiny`) u 8 vCPU / 32 GB / NVMe (`standard`), ~40 GB de disco (más la caché de vlpds) |
| Reloj | NTP (chrony) activo: el firehose de vlpds avanza por reloj |
| Bucket | S3, R2, GCS o compatible, privado, con una llave solo para él (lectura, escritura, listado, borrado) y el versionado activado |
| DNS | Registros A de `PARA_DOMAIN`, `pds.`, `appview.`, `ozone.`, `bridge.`, `m8.`, `matrix.`, `auth.`, `idp.` y `chat.`, más el comodín `*.PARA_DOMAIN` para los handles, todos hacia la máquina |
| Llaves offline | Un lugar fuera de la máquina para las seis de [Respaldos](RESPALDOS.md) |

> **Elige `PARA_DOMAIN` para siempre.** Está en el DID de cada servicio
> (`did:web:pds.<dominio>`, `did:web:appview.<dominio>`), en cada handle y en
> cada MXID de Matrix.

## Con Ansible

De una máquina vacía a `make verify`, más el endurecimiento del host:
[deploy/ansible/README.md](../deploy/ansible/README.md). El rol clona el fork
de vlpds (rama `para/platform`), WatZappa y mubEZ en `/opt/para` y opera desde
`/opt/para/vlpds/para`.

La llave PLC va envuelta con la KEK: genera las dos en tu máquina con
`make secrets` y copia los archivos al inventario cifrado con sops.

## A mano

```bash
# 1. Docker, git y make en la máquina; los repos como hermanos del fork.
mkdir -p /opt/para && cd /opt/para
git clone -b para/platform https://github.com/mexico-parasocial/vlpds.git
git clone https://github.com/mexico-parasocial/WatZappa.git
git clone https://github.com/mexico-parasocial/mubEZ.git
cd vlpds/para

# 2. Configuración no secreta.
cp .env.example .env && $EDITOR .env       # PARA_DOMAIN, ACME_EMAIL, bucket, perfil

# 3. Secretos (como root: quedan root:10001 con 0440).
sudo make secrets                           # aleatorios, KEK y llave PLC envuelta
echo -n 'llave-del-bucket'   | sudo tee secrets/s3_access_key_id >/dev/null
echo -n 'secreto-del-bucket' | sudo tee secrets/s3_secret_access_key >/dev/null
sudo scripts/secrets.sh perms

# 4. Matrix, una vez (docs/MATRIX.md).
make matrix-init

# 5. Revisa, levanta, verifica.
make preflight BUILD=1
make up BUILD=1
make verify
```

Guarda ahora las llaves offline (`scripts/secrets.sh list`, columna OFFLINE).

`BUILD=1` construye en la máquina las imágenes de WatZappa (desde el checkout
de `../../WatZappa`) y la del relay (desde el commit fijado de indigo). La
imagen del AppView todavía no se construye por el problema de rolldown de
WatZappa; hasta que se arregle, la plataforma no levanta completa en
producción ([libreto, problemas conocidos](LIBRETO.md#9-problemas-conocidos)).

## El relay

vlpds le pide al relay que lo rastree al arrancar (`VLPDS_CRAWLERS`), y el
relay comprueba `https://pds.<dominio>` por la red pública antes de seguirlo.
Por eso el DNS de `pds.` tiene que resolver desde la propia máquina. Si `make
verify` dice que el relay no sigue a vlpds:

```bash
make relay-add-pds
make logs S=relay
```

Para federar con la red de Bluesky, deja `PDS_CRAWLERS_EXTRA=,https://bsky.network`
(es el valor por defecto) y anuncia el servidor cuando la primera cuenta
funcione: `make vlpds-admin ARGS="request-crawl bsky.network"`.

## Handles de usuario

Las cuentas reciben handles `alice<PARA_HANDLE_DOMAIN>` (por defecto
`alice.PARA_DOMAIN`). Un handle se verifica en
`https://alice.PARA_DOMAIN/.well-known/atproto-did`, así que cada uno necesita
HTTPS. Caddy emite su certificado la primera vez que alguien lo visita, solo
después de preguntarle a vlpds (`/tls-check`) que la cuenta existe.

Let's Encrypt limita los certificados por dominio (unos 50 por semana) y el
primer acceso a un handle nuevo espera unos segundos. Si esperas muchas
altas, cambia a un certificado comodín con reto DNS-01 (un Caddy con el módulo
de tu proveedor DNS, como hace el rol de Caddy de vlpds).

## Primera cuenta

vlpds exige invitación (`PDS_INVITE_REQUIRED=true`):

```bash
make invite            # USES=5 para un código de varios usos
```

Corre `vlpds admin create-invite-code` dentro del contenedor, con el token de
`secrets/vlpds_admin_token`. Cuando la primera cuenta funcione, pon su DID en
`ADMIN_DIDS` (para Ozone) y `make up`.

## Qué hace `make up`

1. Construye la imagen de respaldos (y con `BUILD=1`, las de WatZappa y el relay).
2. Descarga las imágenes fijadas (vlpds, Postgres, Caddy…).
3. `docker compose up -d --wait`: solo recrea lo que cambió y espera a que
   cada servicio con healthcheck esté sano (hasta 15 min la primera vez; vlpds
   tiene 120 s de gracia para abrir sus shards).

Los reinicios son con gracia: vlpds tiene 90 s para detenerse y Caddy
reintenta la conexión 10 s, así que durante una actualización las peticiones
esperan en vez de recibir 502. **No uses `docker kill`**: tras un SIGKILL,
vlpds no escribe durante un lease.

## Verificar

`make verify` comprueba tras cualquier cambio:

- cada contenedor de `PROFILES` corriendo y sano;
- los endpoints públicos con certificados válidos: vlpds, `describeServer`,
  el DID del AppView, Ozone, el bridge, `/login` de Matrix vía MAS, el broker
  M8 y la app web;
- `/admin`, `vlpds.admin.*` y `/metrics` de vlpds responden 404 desde fuera;
- vlpds responde `/tls-check`;
- el relay sigue a `pds.<dominio>`;
- mubEZ replicando al bucket y el último respaldo bien.
