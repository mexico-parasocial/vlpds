# Despliegue

Una máquina de cero hasta verificada. El ejemplo es el perfil `tiny`: una VM
de 4 vCPU / 8 GB con Ubuntu LTS, un bucket en R2 y DNS en cualquier proveedor.

## Qué necesitas

| | Notas |
| --- | --- |
| Máquina | Linux con Docker, 4+ vCPU, 8+ GB (`tiny`) o 8 vCPU / 32 GB (`standard`), ~40 GB de disco |
| Bucket | S3, R2, GCS o compatible, privado, con una llave solo para él (lectura, escritura, listado, borrado) |
| DNS | Registros A de `PARA_DOMAIN`, `pds.`, `appview.`, `ozone.`, `bridge.`, `m8.`, `matrix.`, `auth.`, `idp.` y `chat.`, más un comodín `*.PARA_DOMAIN` para los handles de usuario (`alice.PARA_DOMAIN`), todos hacia la máquina |
| Imágenes | Las que CI publicó en GHCR: `WATZAPPA_VERSION` y `MUBEZ_VERSION` = `sha-<commit>` o `X.Y.Z` |
| Llaves offline | Un lugar fuera de la máquina para las cinco de [Respaldos](RESPALDOS.md) |

> **Elige `PARA_DOMAIN` para siempre.** Está en el DID de cada servicio, en
> cada handle y en cada MXID de Matrix (`MATRIX_SERVER_NAME`). Cambiarlo
> después rompe identidades.

## Con Ansible

Todo lo de abajo, de una máquina vacía a `make verify`, más el endurecimiento
del host: [deploy/ansible/README.md](../deploy/ansible/README.md).

## A mano

```bash
# 1. Docker, git, make en la máquina; los tres repos como carpetas hermanas,
#    cada uno en la revisión que corresponde a las imágenes.
git clone … /opt/para/para-platform && git clone … /opt/para/WatZappa && git clone … /opt/para/mubEZ
cd /opt/para/para-platform

# 2. Configuración no secreta.
cp .env.example .env && $EDITOR .env      # PARA_DOMAIN, bucket, versiones

# 3. Secretos (como root: quedan root:10001 con 0440).
sudo make secrets                          # genera los aleatorios
echo -n 'llave-del-bucket' | sudo tee secrets/s3_access_key_id >/dev/null
echo -n 'secreto-del-bucket' | sudo tee secrets/s3_secret_access_key >/dev/null
sudo scripts/secrets.sh perms

# 4. Matrix, una vez: config de Synapse, MAS y Element para PARA_DOMAIN, y el
#    registro del appservice del bridge (docs/MATRIX.md).
make matrix-init

# 5. Revisa, levanta, verifica.
make preflight
make up
make verify
```

Guarda ahora las llaves offline (`scripts/secrets.sh list`, columna OFFLINE).

Con Matrix, ya arriba: `make matrix-admin-token` y
`make up UP_ARGS=--force-recreate` (docs/MATRIX.md).

## Handles de usuario

Las cuentas reciben handles `alice<PARA_HANDLE_DOMAIN>` (por defecto
`alice.PARA_DOMAIN`). Un handle se verifica en
`https://alice.PARA_DOMAIN/.well-known/atproto-did`, así que cada uno necesita
HTTPS. Caddy emite su certificado la primera vez que alguien lo visita, solo
después de preguntarle al PDS (`/tls-check`) que la cuenta existe. Hace falta
el registro DNS comodín; preflight lo comprueba.

Let's Encrypt limita los certificados por dominio registrado (del orden de 50
por semana). Si esperas muchas altas a la vez, cambia a un certificado
comodín con reto DNS-01, que requiere un Caddy con el módulo de tu proveedor
DNS.

## Primera cuenta

El PDS exige invitación (`PDS_INVITE_REQUIRED=1`):

```bash
make invite            # {"code":"..."}; USES=5 para un código de varios usos
```

Corre dentro del contenedor del PDS con `secrets/pds_admin_password`: las
rutas `com.atproto.admin.*` responden 404 desde internet. Cuando la primera
cuenta funcione, pon su DID en `ADMIN_DIDS` (para Ozone) y `make up`.

## Qué hace `make up`

1. Crea la red `para-edge` si falta.
2. Construye la única imagen local (los respaldos, chica).
3. Descarga las imágenes fijadas.
4. `docker compose up -d --wait`: solo recrea lo que cambió y espera a que
   cada servicio con healthcheck esté sano (hasta 15 min la primera vez).

Los reinicios son con gracia: cada servicio tiene su `stop_grace_period` y
Caddy reintenta la conexión 10 s, así que durante una actualización las
peticiones esperan en vez de recibir 502. No uses `docker kill`.

## Verificar

`make verify` repite lo que hay que comprobar tras cualquier cambio:

- cada contenedor de `PROFILES` corriendo y sano;
- `https://pds.…/xrpc/_health`, `describeServer`, AppView, Ozone, bridge,
  `/login` de Matrix vía MAS y el broker M8, con certificados válidos;
- las rutas de operador del PDS cerradas desde fuera;
- mubEZ replicando al bucket y el último respaldo bien.
