# La app PARA

La app (iOS, Android y web) no se configura en el servidor: hornea las URLs
de la plataforma al compilarse. Sin ellas usa sus valores por defecto, que
apuntan a los servicios de **Bluesky** (`did:web:api.bsky.app`, …).

## Las variables

```bash
make app-env            # las imprime para PARA_DOMAIN
```

| Variable | Valor |
| --- | --- |
| `EXPO_PUBLIC_DEFAULT_SERVICE` | `https://pds.<dominio>` |
| `EXPO_PUBLIC_BLUESKY_PROXY_DID` | `did:web:appview.<dominio>` (el AppView de la plataforma) |
| `EXPO_PUBLIC_M8_BROKER_URL` | `https://m8.<dominio>` |
| `EXPO_PUBLIC_MATRIX_BRIDGE_URL` | `https://bridge.<dominio>` |
| `EXPO_PUBLIC_MATRIX_IDP_URL` | `https://idp.<dominio>` |
| `EXPO_PUBLIC_MATRIX_PUSH_GATEWAY_URL` | `https://bridge.<dominio>/_matrix/push/v1/notify` |
| `EXPO_PUBLIC_CHAT_PROXY_DID` | sin valor (ver abajo) |
| `EXPO_PUBLIC_MATRIX_DMS` | `1` para activar los mensajes directos cifrados ([Matrix](MATRIX.md)) |

- **iOS / Android (EAS):** ponlas en el perfil de `eas.json` (`env`) o con
  `eas env:create`.
- **Web:** el Dockerfile de PARA las deriva de un solo argumento,
  `PARA_DOMAIN`; cualquier `EXPO_PUBLIC_*` explícito le gana.

## La app web

Perfil `web`: bskyweb (el servidor Go con el build web adentro) en
`https://<dominio>`. El dominio raíz sigue respondiendo `/.well-known/*` desde
el PDS; todo lo demás va a la app. bskyweb ya registra `/matrix-auth`, donde
MAS devuelve el login del chat web.

La imagen es por dominio (lleva las URLs horneadas):

- **CI:** `.github/workflows/para-web-image.yml` de PARA publica
  `ghcr.io/mexico-parasocial/para-web:sha-<commit>` con la variable de
  repositorio `PARA_DOMAIN`. En `.env`: `PARA_WEB_VERSION=sha-<commit>`.
- **Aquí:** `make up BUILD=1` la construye con el `PARA_DOMAIN` de `.env`
  desde `PARA_DIR`.

El `.dockerignore` de PARA deja fuera `node_modules`, las carpetas nativas y
los `.env`: antes, el build copiaba el `.env.local` del desarrollador (IPs de
su máquina y valores privados) dentro de la imagen.

bskyweb trae por defecto las banderas remotas de Bluesky (GrowthBook);
la plataforma las apaga (`FEATURE_GATE_BOOTSTRAP=false`) salvo que definas
`PARA_GROWTHBOOK_API_HOST` y `PARA_GROWTHBOOK_CLIENT_KEY`.

## Mensajes directos

Los mensajes directos de la app siguen hoy en `chat.bsky.*`, que la plataforma
no sirve: el PDS reenvía esas peticiones al DID de `EXPO_PUBLIC_CHAT_PROXY_DID`
(por defecto, el chat de Bluesky, donde las cuentas de PARA no existen).

El reemplazo son **mensajes directos cifrados por Matrix**, detrás de
`EXPO_PUBLIC_MATRIX_DMS=1` (ver [Matrix](MATRIX.md), "Mensajes directos"):

- En un perfil, el candado pide un mensaje cifrado (se confirma en la app y en
  iM8).
- En Mensajes, el candado abre "Mensajes cifrados": las conversaciones
  verificadas en este dispositivo y las solicitudes recibidas y enviadas.
- Al aceptar, el cliente crea la sala cifrada, invita a quien pidió y le
  manda, cifrado, el mensaje de presentación con el id de la solicitud.
- Quien pidió, mientras tenga una solicitud aceptada, revisa sus invitaciones
  directas: conserva la que trae ese mensaje y sale de las demás. La
  conversación aparece en la lista y se abre con el chat nativo.

Hoy todo esto funciona en iOS/Android con el motor de chat nativo
(`EXPO_PUBLIC_CHAT_ENGINE=native`). En web, pedir, rechazar y cancelar ya
funcionan; aceptar y abrir conversaciones llegan con la vista de conversación
web, porque ahí solo el cliente del iframe del chat cifra.

Falta: esa vista web, y que Mensajes deje de mostrar las conversaciones de
`chat.bsky` cuando la bandera está activa.
