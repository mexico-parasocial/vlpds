# Matrix

El chat de comunidades de PARA es Matrix, cerrado a PARA: sin federación,
sin registro abierto, con el login delegado en la identidad PARA del usuario.

```
 app PARA / Element                         navegador
      │  OIDC + PKCE                              │
      ▼                                           ▼
 auth.<dominio>  MAS ──── upstream OIDC ────► idp.<dominio>  para-idp
      │  (emite los tokens Matrix)                (prueba la llave PARA)
      ▼
 matrix.<dominio>  Synapse ◄──── appservice ──── matrix-bridge ◄── firehose (relay)
      ▲                                              │
 chat.<dominio>  Element Web                    bridge.<dominio>  (API de la app)
```

| Host | Servicio | Para qué |
| --- | --- | --- |
| `matrix.<dominio>` (`MATRIX_SERVER_NAME`) | Synapse | API de cliente. Login, logout y refresh van a MAS. `/_synapse/admin` solo desde la red privada. |
| `auth.<dominio>` | MAS | Emisor OIDC: la app y Element inician sesión aquí. `/api/admin` solo desde la red privada. |
| `idp.<dominio>` | para-idp | MAS le pregunta quién es el usuario; la app firma con su llave PARA (`para://idp-login`). |
| `chat.<dominio>` | Element Web | Cliente web de respaldo. |
| `bridge.<dominio>` | matrix-bridge | API de comunidades de la app y push gateway (`/_matrix/push/v1/notify`). |

La app web completa el login en `https://<dominio>/matrix-auth` (bskyweb ya
registra esa ruta; ver [App](APP.md)).

## Primera vez

```bash
make secrets
make matrix-init           # Synapse, MAS, Element y el appservice del bridge para PARA_DOMAIN
```

`make matrix-init` genera, en carpetas no versionadas de WatZappa
(`WatZappa/deploy/matrix/synapse/`, `WatZappa/deploy/matrix/mas/`):

- la config de Synapse y su llave de firma (respáldala offline),
- la config de MAS con `public_base` = `https://auth.<dominio>/` y el
  proveedor para-idp en `https://idp.<dominio>`,
- la delegación de autenticación de Synapse a MAS,
- la config de Element apuntando a `MATRIX_SERVER_NAME`,
- el registro del appservice del bridge (abajo).

Luego, con la plataforma arriba:

```bash
make compose ARGS="exec mas mas-cli database migrate --config /config/config.yaml"   # la primera vez
make matrix-admin-token    # usuario admin en MAS y su token en secrets/matrix_admin_token
make up UP_ARGS=--force-recreate
```

### Registro del appservice

El bridge recibe de Synapse los eventos de las salas como appservice, y
registra las cuentas derivadas (localparts de 32 caracteres base32, CD-M1).
`make matrix-init` genera su registro con `matrix/setup-appservice.sh` (traído
aquí del PR #10 de WatZappa, que no se mezcló; escribe en
`WatZappa/deploy/matrix`, de donde lo monta su compose):

- `url: http://matrix-bridge:3001` (el servicio en la plataforma),
- `as_token` y `hs_token` nuevos, que `make secrets-import` copia a
  `secrets/matrix_appservice_token` y `secrets/matrix_hs_token`,
- namespace de usuarios **exclusivo** `@[a-z2-7]{32}:<MATRIX_SERVER_NAME>`:
  solo el bridge puede registrar esos nombres,
- y el overlay `zz-para-appservice.yaml` para que Synapse lo cargue.

Si ya existe un registro, no lo toca. Rotar los tokens:
`deploy/matrix/RUNBOOK_SECURITY_ES.md` de WatZappa (Synapse y bridge juntos).

## Mensajes directos

El servidor no puede saber qué cuenta de Matrix es de cada DID (a propósito:
la cuenta de chat no se vincula con la identidad AT Protocol). Por eso un
mensaje directo empieza como **solicitud con consentimiento**
(`/api/dm-requests` del bridge):

1. A pide un DM al DID de B y aprueba en iM8 "Compartir tu identidad de chat
   para un mensaje directo" (audiencia `para-matrix-bridge/dm.v1`).
2. B ve la solicitud (solo el DID de A, nunca su cuenta de chat).
3. Si B acepta, el bridge le entrega una sola vez la cuenta de A, la borra de
   la solicitud y la marca como aceptada (A lo ve). El cliente de B crea la
   sala cifrada, invita a A y manda, cifrado, `com.para.dm.intro` con el id de
   la solicitud y su DID.
4. Mientras A tenga una solicitud aceptada, su cliente revisa las invitaciones
   directas: entra, descifra la primera y solo conserva la sala si ese mensaje
   trae el id de su solicitud aceptada y el DID de B. Compartir una comunidad
   ya deja ver cuentas de chat, así que una invitación sola no prueba nada;
   de las que no cuadran, sale. Al verificarla, quita la solicitud del bridge.
5. Cada cliente recuerda, **solo en el dispositivo**, qué DID hay del otro
   lado de cada sala verificada; ni el homeserver ni el bridge guardan ese par.

Si B rechaza, o A cancela, o pasan 14 días, la solicitud se borra sin entregar
nada.

Es la única vez que el bridge guarda un DID junto a una cuenta de chat, solo
mientras la solicitud está pendiente. Límite: 20 solicitudes pendientes por
persona. En la app va detrás de `EXPO_PUBLIC_MATRIX_DMS=1` ([App](APP.md)).

## Cifrado

`MATRIX_ENABLE_ENCRYPTION=true` por defecto en la plataforma: el bridge crea
salas cifradas, que es lo que exige el chat nativo de PARA
(`ENCRYPTED_ROOM_REQUIRED`). No cifra salas que ya existen.

## Sin chat

PARA puede salir sin Matrix y conservar la gobernanza. El matrix-bridge no es
solo chat: también sirve constitución, propuestas y decisiones, sorteo (drand),
reportes, sanciones y panel de moderación, insignias y extracción. Con el
perfil `governance` en lugar de `matrix` corre solo:

```bash
PROFILES=core,governance,identity,edge   # más web si la sirves aquí
make up
```

- No se levantan Synapse, MAS, para-idp, Element ni el respaldo de su media.
  No hace falta `make matrix-init` ni los secretos `matrix_*` /
  `mas_bridge_admin_secret`.
- El Makefile aplica `overrides/bridge-chat-off.yaml`: el bridge arranca con
  `MATRIX_CHAT_ENABLED=false`, sin cliente de admin, sin sondeo de salas, sin
  trabajos de Matrix en el reintento y sin barrido de leases.
- Las rutas de chat (`/api/matrix-*`, `/api/community-join`, `/api/rooms`,
  `/api/unread`, `/api/mark-read`, `/api/space-for-community`,
  `/api/dm-requests*`, `/api/devices*`, el push gateway y las transacciones del
  appservice) responden `503 {"error":"CHAT_DISABLED"}`. `/api/push-token`
  sigue guardando el token de Expo: con él se avisa a quien sale sorteado.
- La app web que construye la plataforma recibe `EXPO_PUBLIC_CHAT_ENABLED=false`
  sola (sale de `PROFILES`). Las builds nativas lo necesitan en su entorno de
  EAS: ocultan chat, mensajes directos y contadores de no leídos, y dejan las
  pantallas de gobernanza.
- La imagen del bridge (`WATZAPPA_VERSION`) tiene que incluir este modo. Con
  `BUILD=1` se construye desde `WATZAPPA_DIR`, que tiene que apuntar a un
  checkout que lo tenga; `make preflight` lo revisa (ver `.env.example`).

### Encender el chat después

1. Prepara Matrix como en [Primera vez](#primera-vez): `make matrix-init`, la
   migración de MAS y `make matrix-admin-token`.
2. Pon `matrix` en `PROFILES` (`governance` puede quedarse o salir) y
   `make up UP_ARGS=--force-recreate`.
3. **Cada comunidad creada con el chat apagado necesita su espacio y sus
   salas.** El bridge las guardó con una fila de gobernanza en
   `community_space_map` (`space_id` = `para-unprovisioned:<uri>`, sin salas)
   y una entrada `create_space` fallida en `sync_log`. Con el chat encendido,
   el trabajo de reintento de creación de salas (`RetryWorker`, cada 60 s, sin
   límite de intentos para `create_space`) las provisiona desde el registro
   actual en el PDS. Comprueba que no quede ninguna:

   ```sql
   -- en la base bridge
   SELECT community_uri FROM community_space_map
   WHERE space_id LIKE 'para-unprovisioned:%';
   ```

   Si alguna queda sin entrada en `sync_log`, provisiónala con
   `services/matrix-bridge/src/backfill.ts` de WatZappa, que también toma las
   filas sin salas. Mientras no tenga salas, sus rutas de chat responden
   `404 Space not found`.
4. Reconstruye la app con `EXPO_PUBLIC_CHAT_ENABLED=true` (la web, con el
   siguiente `make up`; las nativas, con una build nueva). Los miembros
   entran a las salas con el join verificado al abrir el chat.

## Problemas conocidos

### MAS generado para localhost

`setup-mas.sh` generaba `public_base: http://localhost:8080/`, que solo sirve
en una máquina de desarrollo; preflight lo detecta. `make matrix-init` ya pasa
las URLs públicas, pero no regenera una config existente (cambiar el
proveedor dejaría huérfanas las cuentas vinculadas). Para corregir una
existente, edita a mano, conservando los secretos:

- `WatZappa/deploy/matrix/mas/config.yaml`: `public_base` e `issuer` →
  `https://auth.<dominio>/`; en el proveedor, `issuer` →
  `https://idp.<dominio>` y `authorization_endpoint` →
  `https://idp.<dominio>/authorize`.
- `WatZappa/deploy/matrix/mas/para-idp.env`: `PARA_IDP_ISSUER=https://idp.<dominio>` y
  el `redirectUris` → `https://auth.<dominio>/upstream/callback/<id>`.

Después: `make up UP_ARGS=--force-recreate`.

### La app pide el SDK de Matrix a chat.para.social

Cuando el bundle de matrix-js-sdk no va incluido, el WebView de la app lo
pide a `https://chat.para.social/static/matrix-js-sdk.bundle.txt`
(`PARA/src/screens/Communities/matrix-client.ts`): un dominio fijo que Element
no sirve. Mientras se incluya el bundle (el caso actual) no importa.
