# PARA sobre vlpds frente a WatZappa tradicional

Hay dos formas de correr el backend de PARA y las dos siguen vivas mientras
decidimos cuál usar:

| | **Tradicional** | **vlpds** (este fork) |
|---|---|---|
| PDS | El de WatZappa, en TypeScript | [vlpds](https://vlpds.jazco.dev/docs/overview) 1.1.0, en Rust |
| Dónde vive la infraestructura | El repo WatZappa (Ansible, compose) | `vlpds/para/` en `mexico-parasocial/vlpds` |
| Documentación | [tradicional/](tradicional/README.md) | Esta carpeta |
| Libreto de comandos | [tradicional/LIBRETO.md](tradicional/LIBRETO.md) | [LIBRETO.md](LIBRETO.md) |

Todo lo demás es el mismo código en las dos: el AppView, bsync, Ozone, el
bridge de Matrix, para-idp y mubEZ salen de sus repos. El fork no cambia nada
de WatZappa.

---

## 1. En corto

- **vlpds funciona como PDS de PARA para lo genérico de ATProto**: cuentas,
  sesiones, OAuth, repos, blobs, handles, el firehose y el proxy al AppView.
  Probado de punta a punta en `make dev-smoke`.
- **Bloqueo: las escrituras propias de PARA no existen en vlpds.** Unas doce
  rutas `com.para.*` (crear comunidad, unirse, votar, el factor de correo…) las
  implementa el PDS de TypeScript. En vlpds responden 501, y reenviadas al
  AppView responden 400. Con vlpds, hoy la app no puede hacer esas acciones
  (§3).
- **vlpds exige un relay.** Numera su firehose con valores mayores que 2^53,
  que los consumidores en TypeScript (el AppView, el bridge) rechazan. Un relay
  de indigo los renumera (§4).
- **Operarlo es más simple**: todo su estado está en el bucket, el disco es
  caché, no hay respaldos que correr para el PDS y escala a varios nodos sin
  cambiar nada (§5).

---

## 2. Qué cambia, pieza por pieza

| | Tradicional | vlpds |
|---|---|---|
| Estado del PDS | SQLite por repo y cuentas en un volumen local; blobs en el bucket | Todo en el bucket (`S3_PREFIX/vlpds`), con SlateDB; el disco es caché |
| Respaldo del PDS | Copia de los SQLite cada `BACKUP_INTERVAL`: se puede perder hasta un intervalo | Nada que copiar: cada commit es durable en el bucket antes de responder |
| Llaves de firma | En el volumen del PDS | En el bucket, envueltas con la KEK (`vlpds_kek`, offline) |
| Llave de rotación PLC | Hex en claro | Envuelta con la KEK (`make secrets` lo hace) |
| Escalar el PDS | No escala: un secuenciador, SQLite en disco | Varios nodos sobre el mismo bucket, con leases y shards |
| Firehose | `seq` consecutivos desde 1; el AppView lo lee directo | `seq = unix_micros × 256 + escritor`, más de 2^53; hace falta el relay |
| Rutas propias de PARA (`com.para.*`) | En el PDS | **No existen**; solo las lecturas que también tiene el AppView |
| Reglas de PARA al escribir | El PDS rechaza boletas congeladas, votos y delegaciones sin prueba, reposts | No las aplica; solo el AppView al indexar |
| Consola de operador | `com.atproto.admin.*` con contraseña | `/admin` y `vlpds.admin.*` con token, en un puerto solo de loopback |
| Certificados de handles | `/tls-check` (añadido en WatZappa) | `/tls-check` nativo |
| Memoria | 512 MB (tiny) | 2 GB como mínimo |
| Spaces (datos con permisos) | Port parcial en WatZappa (fases 1–3a) | Completo en alfa, con `--spaces` (`VLPDS_SPACES`) |

## 3. La brecha de `com.para.*`

La app manda todas las rutas `com.para.*` con el cliente del AppView, es
decir, con la cabecera `atproto-proxy: <AppView>#bsky_appview`. En la variante
tradicional el PDS de TypeScript resuelve las de escritura él mismo. vlpds no
las conoce y reenvía todo lo que trae esa cabecera al AppView, que no las
implementa.

| Ruta | Tradicional | vlpds |
|---|---|---|
| `com.para.community.createBoard`, `join`, `leave`, `acceptDraftInvite` | PDS | ✗ |
| `com.para.community.createBriefingPack`, `updateBriefingPack` | PDS | ✗ |
| `com.para.community.shareContent`, `sharedContentAction` | PDS | ✗ |
| `com.para.civic.castVote` | PDS (verifica la prueba de M8) | ✗ |
| `com.para.account.getAuthFactor`, `setAuthFactor` | PDS | ✗ |
| `com.para.alpha.requestAccess`, `createInvite` | PDS | ✗ |
| `com.para.alpha.getAccess`, `getRolloutStatus` | AppView | ✓ |
| `com.para.feed.getTimeline`, `com.para.actor.getProfileStats`, `com.para.social.getPostMeta` | AppView | ✓ |

Comprobado contra el entorno local: `com.para.community.join` y
`com.para.civic.castVote` responden 501 en vlpds, y 400 cuando se reenvían al
AppView; `com.para.feed.getTimeline` responde 200. El libreto lo repite en
[LIBRETO.md, paso 2.4](LIBRETO.md#24-la-brecha-de-compara).

Las salidas posibles, para decidir antes de mover usuarios:

1. **Que la app escriba los registros directamente** con
   `com.atproto.repo.createRecord`, y que el AppView valide al indexar. El
   AppView ya verifica las boletas antes de indexarlas, incluidas las de otros
   repos. Es el camino más ATProto, pero cambia la app y pierde el rechazo
   inmediato (un voto inválido se escribe y se ignora en vez de rechazarse).
2. **Un servicio pequeño de PARA** que implemente esas rutas. Solo sirve para
   las que no escriben en el repo del usuario: un servicio no puede escribir en
   un repo ajeno.
3. **Implementarlas en el fork de vlpds** (Rust). Contradice la idea de no
   construir sobre vlpds y nos ata a mantener un parche.

Mientras no se resuelva, vlpds sirve para probar operación, rendimiento y
costos, no para usuarios reales.

## 4. Por qué el relay es obligatorio

vlpds no tiene un secuenciador global: cada nodo numera los eventos de su log
con `unix_micros × 256 + escritor` y todos los nodos mezclan los logs en el
mismo orden. Eso es lo que le permite escalar a varios nodos sin coordinarse.
Su documentación lo dice claro: esos números pasan de 2^53, así que hay que
guardarlos como enteros de 64 bits, no como números de JavaScript.

`@atproto/sync`, la biblioteca con la que el AppView lee el firehose, valida
el `seq` como entero seguro de JavaScript y descarta los eventos de vlpds
(`Expected integer value type (got 458708000931978140n) at $.seq`). El bridge
de Matrix tiene el mismo problema.

El relay de [indigo](https://github.com/bluesky-social/indigo) (en Go) lee el
firehose de vlpds y lo vuelve a publicar con `seq` consecutivos propios, como
hace la red de ATProto. El AppView y el bridge leen `ws://relay:2470`. Cuesta
un proceso pequeño (256 MB en tiny), una base en Postgres y unos GB de disco
para 72 h de eventos.

Dos detalles de la puesta en marcha:

- En producción, vlpds avisa solo al relay al arrancar (`VLPDS_CRAWLERS`), y
  el relay comprueba `https://pds.<dominio>` por la red pública antes de
  seguirlo. `make verify` revisa que lo esté siguiendo; `make relay-add-pds`
  lo vuelve a pedir.
- El relay solo da de alta hosts con IP pública, incluso por su ruta de admin.
  En el entorno local, donde vlpds es `localhost:2583`, `make dev` lo registra
  directo en la base del relay.

## 5. Lo que se gana operando vlpds

- **Un respaldo menos y nada que perder.** Cada commit está en el bucket antes
  de responder. Perder la máquina no pierde ningún post: otra máquina con el
  mismo bucket, la KEK y la llave PLC envuelta sigue donde quedó.
- **Actualizaciones de ~1 s.** vlpds se detiene en ~0.1–0.25 s y arranca en
  ~1.3 s; Caddy reintenta la conexión 10 s, así que los clientes esperan en vez
  de ver errores.
- **Escala horizontal.** El perfil `standard` usa 64 shards. Otro nodo con el
  mismo prefijo toma los shards de uno caído en segundos (en este fork aún no
  hay un compose de segundo nodo: ver [Escalado](ESCALADO.md)).
- **Seguridad por defecto.** Se niega a arrancar con secretos cortos,
  repetidos o vacíos, sin KEK o con las credenciales por defecto de MinIO; la
  consola no sale por el edge.
- **Spaces completo** (en alfa), con OAuth obligatorio y acceso de operador
  auditado.

## 6. Lo que se pierde o cambia

- Las rutas y reglas propias de PARA en el PDS (§3).
- `com.para.account.*`: el segundo factor por correo de PARA vive en el PDS de
  TypeScript. vlpds tiene su propio 2FA ([docs/oauth-2fa.md](../../docs/oauth-2fa.md)).
- Más memoria: 2 GB como mínimo, y `tiny` pasa a pedir una VM de 12 GB.
- Un proceso más: el relay.
- La migración de cuentas existentes es la estándar de ATProto (exportar el
  repo, importarlo y actualizar el DID), no una copia de archivos:
  [docs/migration.md](../../docs/migration.md) de vlpds y
  [Actualizaciones](ACTUALIZACIONES.md).

## 7. El fork

`mexico-parasocial/vlpds` es un fork real de `jazware/vlpds`:

- `main` se sincroniza con upstream cada día (`.github/workflows/sync-upstream.yml`,
  con `SYNC_TOKEN` si existe, si no `GITHUB_TOKEN`). `merge-upstream` no copia
  tags: la versión 1.1.0 es el commit `77b8897`.
- Los workflows `CI` e `Image` de upstream están deshabilitados en el fork;
  la imagen que usamos es la de upstream, `ghcr.io/jazware/vlpds:1.1.0`.
- `para/` es lo único nuestro. No compilamos vlpds: usamos su imagen. Si algún
  día hiciera falta un parche, el fork ya comparte historial con upstream.
- Con `GITHUB_TOKEN`, un cambio de upstream en `.github/workflows/` no se puede
  sincronizar. Agrega el secreto `SYNC_TOKEN` (un token con permiso `workflow`).

## 8. Spaces

vlpds 1.1.0 trae Spaces completo en alfa, detrás de `--spaces`
(`VLPDS_SPACES=true` en `.env`): repo host y simplespace host, solo por OAuth,
sin fugas al firehose y con acceso de operador auditado. WatZappa tiene un port
parcial (fases 1–3a: el PDS como repo host y simplespace host de sus propias
cuentas), fijado al mismo commit de la referencia.

Las decisiones de producto siguen en WatZappa: **CD-P3** (datos de spaces
solo con OAuth), **CD-P2** (compartir es ser miembro de un space, nunca un
campo `isPublic`), **CD-M5** (Spaces nunca reemplaza a Matrix y la identidad
anónima nunca escribe en un space) y **OD-P1** (abierta: dónde viven las
colecciones del árbol cívico personal). No lo actives en producción mientras
el protocolo cambie cada semana: no hay compatibilidad hacia atrás de los
datos de Spaces.
