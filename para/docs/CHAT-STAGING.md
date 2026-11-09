# Staging persistente de chat sobre VLPDS

Este ensayo pertenece a `vlpds/para`. WatZappa aporta AppView, bridge e IdP;
VLPDS conserva las escrituras PARA, políticas, firma y commits durables.
El recorrido es **VLPDS → rsky → consumidores TypeScript**. Indigo no forma
parte de este staging. La secuencia que consumen AppView y bridge es la de
rsky; el checkpoint original de VLPDS permanece en Rust.

## Alcance e identidad

El proyecto Docker es `para-chat-staging`. Sus puertos se publican únicamente
en `127.0.0.1`; PostgreSQL, MinIO, relay, Matrix y M8 tienen volúmenes propios.
Las cuentas y semillas creadas por `staging/smoke.mts` son sintéticas. M8 usa
el bootstrap de desarrollo solo en este ensayo aislado; esa prueba no acredita
el login OAuth real de un usuario de producción.

`para/.dev/chat-staging` guarda configuración, llaves, fixture y snapshots.
El directorio es privado y está excluido de Git y del contexto de la imagen
VLPDS. No copiar sus secretos a producción.

El esquema `social.para.app` coincide con el dominio de `clientUri=https://para.social`;
MAS exige un callback nativo sin autoridad (`:/matrix-auth`). La
[política de MAS 1.24](https://github.com/element-hq/matrix-authentication-service/blob/v1.24.0/policies/client_registration/client_registration.rego)
verifica ambas condiciones. La app mantiene `para` para navegación y registra
`social.para.app` para Matrix. Esto requiere un nuevo binario nativo y un
runtime de release compatible; una actualización JavaScript sola no registra
el esquema en un binario instalado.

El cliente nativo recibe el enlace intermedio `social.para.app://idp-login`, exige el
emisor configurado en `EXPO_PUBLIC_MATRIX_IDP_URL`, solicita aprobación a iM8
mediante el mailbox de la sesión M8 y verifica la firma en el IdP. Reabre el
callback de MAS conservando sus cookies; el SDK consume el retorno final
`social.para.app:/matrix-auth` y valida su estado OAuth. MAS crea la cuenta durante el primer login. El bridge deriva el MXID sin
precrearlo; su namespace de observación es no exclusivo. Una reserva exclusiva
o un usuario creado antes por appservice bloquean ese primer login. Las
cuentas appservice existentes exigen migración revisada, no adopción automática.
El flag `MATRIX_ALLOW_APPSERVICE_LOGIN=true` queda reservado a despliegues
legacy sin MAS y no se activa en este staging.

El cliente comprueba que la
clave aprobada derive el MXID esperado y registra el **device ID real del
SDK** mediante una prueba nueva antes del join. El bridge comprueba en
Synapse que ese dispositivo exista para la cuenta derivada; un ID inventado
se rechaza antes de crear el registro. Esto también se aplica a los DM.

El mailbox separa desafíos del bridge (64 caracteres hex) y del IdP
(43 caracteres base64url y emisor configurado). Verifica la firma antes de
devolverla y exige la misma sesión. iM8 rechaza solicitudes caducadas,
audiencias desconocidas y emisores inseguros; la identidad de boleta sigue
excluida del firmante Matrix.

## Uso

Desde `vlpds/para`, con los checkouts hermanos y sus dependencias instaladas:

```sh
make chat-staging-build
make chat-staging-up
make chat-staging-verify
make chat-staging-smoke
./staging/stage.sh smoke --mas --crypto --revoke --ops
make chat-staging-ops
# Prueba del panel real con Puppeteer ya instalado en WatZappa/packages/pds:
node ./staging/ui.mjs
./staging/stage.sh status
./staging/stage.sh app-env
```

Las construcciones son secuenciales. PLC y bsync usan las revisiones fijadas
del entorno de desarrollo; VLPDS, AppView, bridge, M8 e IdP se construyen desde
los checkouts locales. El relay aplica las dos correcciones versionadas de
`relay/patches` y ejecuta sus pruebas durante la construcción. Registrar los
IDs de imágenes del ensayo aceptado antes de moverlo a otro host.

| Servicio | Dirección local |
| --- | --- |
| VLPDS | http://localhost:32583 |
| AppView | http://localhost:32584 |
| rsky | http://localhost:32470 |
| bridge | http://localhost:33001 |
| M8 | http://localhost:38787 |
| Synapse | http://localhost:38008 |
| MAS | http://localhost:38080 |
| IdP | http://localhost:38090 |
| Consola con Chat | http://localhost:38000/admin/chat |

La configuración loopback sirve para esta Mac y un simulador iOS. Un teléfono
físico necesita URLs alcanzables con TLS y una configuración coherente de
issuer, callbacks y documentos DID; no basta con sustituir `localhost` por
una IP en una sola variable.

## Respaldo y restauración

```sh
make chat-staging-snapshot
./staging/stage.sh restore /ruta/absoluta/para/.dev/chat-staging/snapshots/SNAPSHOT
make chat-staging-verify
make chat-staging-smoke
```

El snapshot detiene los servicios del proyecto propio, copia todos sus
volúmenes durables y su configuración/llaves, calcula SHA-256 y registra IDs
de imágenes. Incluye bucket VLPDS, PLC/AppView/bridge/MAS en PostgreSQL,
estado de rsky, Synapse y SQLite M8. Todos los archivos montados por los
servicios se copian a la configuración privada incluida en el snapshot; no
dependen de scripts mutables fuera del respaldo. El disco/cache de VLPDS no se requiere
para reconstruir sus repositorios desde el bucket. Al finalizar vuelve a
iniciar los servicios.

La restauración comprueba proyecto, miembros, hashes e imágenes antes de
recrear exclusivamente los volúmenes `para-chat-staging_*`. Restaura las
llaves que conservan las identidades. El comando `restore` es una orden
explícita de sustituir los datos de **este staging** por el snapshot elegido.
`chat-staging-down` conserva los volúmenes. El snapshot retiene etiquetas locales propias para cada ID de imagen antes de
detener servicios. La restauración crea etiquetas privadas desde esos IDs
verificados y usa `pull_policy: never`, aunque las etiquetas de build hayan
cambiado. No eliminar esas imágenes mientras existan snapshots que las usen. El snapshot
**no empaqueta las imágenes**: conservarlas/exportarlas por separado antes de
recuperar en otro host. No ejecutar `up` entre `restore` y la aceptación:
`up` regenera configuración desde el código actual.

```sh
./staging/stage.sh smoke --mas --crypto --revoke --ops --expect-restored
```

## Consola y operaciones privadas

`/admin/chat` usa el layout existente de VLPDS y el API del bridge
`/chat-ops/v1`. Muestra disponibilidad individual, checkpoint ligado a su
fuente, último evento/lag, cola de provisioning, fallos de proyección,
dispositivos atestados y auditoría. Las listas están limitadas a 100 entradas
recientes. La disponibilidad HTTP y la frescura de ingestión se muestran
por separado; ninguna reemplaza la prueba de publicar un evento nuevo.

El gateway Caddy autentica al operador, elimina Authorization antes de
reenviar a VLPDS y fija los headers de identidad. Para el bridge agrega además
un secreto privado de proxy. El bridge exige ese secreto y su propia lista de
operadores: abrir la consola PDS no concede permisos de chat. Los POST exigen
Origin exacto, header de acción, razón y request ID. El registro durable se crea
antes de revocar o reintentar; repetir el mismo request ID devuelve el resultado
sin repetir la acción. Una operación interrumpida con resultado `running`
requiere revisión antes de emitir una nueva orden.

Solo están habilitados reintentos de jobs de infraestructura y revocación.
El job se guarda antes de llamar a Synapse, tiene lease de diez minutos,
protección por número de intento y cinco intentos automáticos; el operador
puede reintentarlo con razón auditada. Reutilizar una sala exige URI, creador,
parentesco y cifrado correctos. El reintento nunca vuelve a conceder roles o
membresías guardadas. Los fallos legacy sin estos jobs necesitan revisión;
no se adoptan alias o salas existentes a ciegas.

Los dispositivos se presentan por ID operativo, nombre y fechas; las
respuestas excluyen DID, MXID y tokens. El panel no entrega mensajes ni llaves.
Las credenciales sintéticas de entrada están en archivos privados
`secrets/console_staging_password` y `secrets/console_pds-only_password`, bajo
el directorio del ensayo. `staging` puede operar chat; `pds-only` abre el PDS y
recibe 403 en chat, incluso intentando falsificar los headers del proxy.
No publicar los listeners internos 32590/33002 ni usar Basic sobre HTTP fuera
de este loopback. Producción requiere TLS y un proxy de autenticación revisado.
El listener del bridge está desactivado por defecto: se activa expresamente
con `CHAT_OPS_PORT`, `CHAT_OPS_ORIGIN`, `CHAT_OPS_PROXY_SECRET` y
`CHAT_OPS_OPERATORS` (`viewer` u `operator`), según WatZappa `.env.example`.

## Revocación y configuración existente

Borrar solo el dispositivo en Synapse deja utilizable el refresh token MAS.
El bridge ahora termina todas sus sesiones OAuth/compat en MAS y después
borra el dispositivo Synapse. Repetir una revocación vuelve a imponer esta
regla incluso si una fila legacy ya decía `revoked`.

La configuración nueva incluye el cliente confidencial
`01K3ZQD8N7P0J5R2T9V4W6X8YD`, permitido en `policy.data.admin_clients`, y el
recurso `adminapi` solo en el listener interno MAS. El secreto
`mas_bridge_admin_secret` se monta como archivo en el bridge. La autoridad
administrativa de MAS nunca se entrega a una app o al navegador.

**No borrar ni regenerar el directorio MAS existente para actualizarlo**:
contiene claves de cifrado/firma y vínculos con cuentas. `setup-mas.sh`
conserva una instalación existente. Para preparar un cambio revisable:

```sh
# Desde vlpds, con dependencias de ui instaladas (npm ci --prefix ui):
node para/matrix/upgrade-mas-admin.mjs prepare /ruta/mas/config.yaml /ruta/secrets/mas_bridge_admin_secret
# Revisar localmente la propuesta privada, sin copiar llaves a incidencias/logs.
node para/matrix/upgrade-mas-admin.mjs apply /ruta/mas/config.yaml.bridge-admin.proposal.json
```

La herramienta conserva claves, base de datos, proveedores y clientes
existentes; rechaza colisiones y cambios del archivo durante la revisión,
y deja un respaldo privado antes de aplicar. Reiniciar MAS y bridge en una
ventana de mantenimiento y verificar revocación de acceso **y** renovación.
En WatZappa tradicional se suministra el mismo secreto por
`MAS_BRIDGE_ADMIN_SECRET` al generador y `MATRIX_MAS_CLIENT_SECRET` al bridge.
Esto no migra cuentas appservice existentes a MAS ni cambia sus identidades.

## Evidencia y salida a producción

La prueba de protocolo verifica una cuenta persistente, publicación con el
mismo CID en AppView, proxy del PDS, comunidad nativa, membresía del creador y
rechazo de `validate=false` para las escrituras prohibidas. Después comprueba
M8, identidad Matrix derivada y el recorrido de firma iM8 → mailbox → IdP con
rechazo de replay. Puede repetirse contra el mismo fixture después de reiniciar
o restaurar; conserva DID, URI, CID y MXID esperados.

Un `/healthz` correcto no acredita ingestión. El ensayo detectó y corrigió
lecturas PostgreSQL con campos snake_case, contadores int8 como texto, cursor
INTEGER de 32 bits y falta de respuestas WebSocket pong en rsky. El cursor
local ahora usa BIGINT y conversiones comprobadas de enteros seguros, sin
convertir el cursor upstream de VLPDS a Number.

Aceptación automatizada local del 2026-10-15 (278 pruebas del bridge en
SQLite/PostgreSQL, 15 pruebas de callback/admisión y typechecks iOS/Android/web):

- Login MAS real con PKCE y firma del código iM8 vía mailbox; creación de
  cuenta, atestación del device real y join verificado.
- Dos sesiones MAS con Matrix SDK/Rust crypto: Megolm en ambos sentidos,
  servidor con ciphertext, comparación SAS y verificación criptográfica.
- Recuperación del historial con un tercer device y backup de llaves;
  una clave incorrecta se rechaza antes de importar historial.
- Revocación vía usuario y operador: acceso anterior/nuevo y refresh rechazados.
- Caída de Synapse: indisponibilidad visible, job durable, recuperación,
  reintento autorizado, replay del request ID y auditoría.
- Restauración coordinada de volúmenes, configuración y claves, sin caché PDS:
  mismo DID/CID/MXID/room IDs, sesiones MAS/M8 renovables, checkpoint estable
  e historial cifrado recuperable. Un ensayo cambió deliberadamente una
  etiqueta de imagen y el restore estricto recuperó el ID original guardado.
- Consola en navegador a tamaño escritorio y móvil; un operador de PDS sin
  permiso de chat recibe un error de autorización y no datos de dispositivos.

Estas pruebas usan clientes SDK en la Mac y contenedores Linux. **No acreditan
la app instalada en un iPhone ni un host Linux con HTTPS**. Antes de migrar
usuarios siguen pendientes el binario nativo con nuevo esquema, aceptación
real de app/iM8, SAS/QR y recuperación desde sus pantallas, interoperabilidad
Element, chat web cifrado, medios y push en release firmado, roles/remoción
con actores separados y migración/import/rollback revisados. No se han movido
cuentas reales ni activado un rollout de producción.

La paridad del PDS y las condiciones del import se describen en
[NATIVE-PARA-WRITES.md](NATIVE-PARA-WRITES.md); el relay y su recuperación en
[RSKY-RELAY.md](RSKY-RELAY.md). Las decisiones de privacidad e identidad del
chat siguen en [WatZappa MATRIX_V2](../../../WatZappa/docs/MATRIX_V2.md).
