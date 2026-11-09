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
bridge de Matrix, para-idp y mubEZ salen de sus repos. Los contratos PARA del
fork se actualizan en los lexicons compartidos de WatZappa.

---

## 1. En corto

- **vlpds funciona como PDS de PARA para lo genérico de ATProto**: cuentas,
  sesiones, OAuth, repos, blobs, handles, el firehose y el proxy al AppView.
  Probado de punta a punta en `make dev-smoke`.
- **El fork implementa PARA en Rust**, con políticas antes del commit, invitaciones
  firmadas por el creador, factores iM8 independientes y estado alpha durable.
  Incluye imports revisados; no migrar usuarios hasta validar los clientes, la
  recuperación y los servicios reales (§3).
- **vlpds exige un relay.** Numera su firehose con valores mayores que 2^53,
  que los consumidores en TypeScript (el AppView, el bridge) rechazan.
  vlRelay los renumera (§4).
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
| Rutas propias de PARA (`com.para.*`) | En el PDS | Comandos nativos, invitaciones autorizadas, iM8 independiente y alpha privado; validación integral pendiente |
| Reglas de PARA al escribir | El PDS rechaza boletas congeladas, votos y delegaciones sin prueba, reposts | El fork las aplica antes de create/put/applyWrites, incluso con validate=false; AppView sigue validando al indexar |
| Consola de operador | `com.atproto.admin.*` con contraseña | `/admin` y `vlpds.admin.*` con token, en un puerto solo de loopback |
| Certificados de handles | `/tls-check` (añadido en WatZappa) | `/tls-check` nativo |
| Memoria | 512 MB (tiny) | 2 GB como mínimo |
| Spaces (datos con permisos) | Port parcial en WatZappa (fases 1–3a) | Completo en alfa, con `--spaces` (`VLPDS_SPACES`) |

## 3. Escrituras nativas y brechas pendientes

Con el fork compilado y `VLPDS_PARA=true`, las rutas registradas de escritura se
resuelven en el PDS aunque la app incluya `atproto-proxy`. Las lecturas continúan
en el AppView. La imagen upstream `ghcr.io/jazware/vlpds:1.1.0` no incluye este port.

Ya están implementados `createBoard`, `join`, `leave`, `createBriefingPack`,
`updateBriefingPack`, `shareContent`, `removeSharedContent`, `restoreSharedContent`
y `com.para.civic.castVote`. Todos usan el worker y log durable de VLPDS y verifican
los permisos de colección. Las escrituras genéricas ejecutan la misma política.

`acceptDraftInvite` exige invitación firmada por el creador; la activación con nueve
fundadores la firma el creador. `account.getAuthFactor` y `setAuthFactor` usan firmas
del wallet iM8 que funcionan sin sesión del broker. Alpha tiene cuotas e invitaciones
de un solo uso en el log privado; la administración exige credenciales de operador.
Los imports sin revisión se cierran con `ParaImportReviewRequired`; la ruta revisada
fija CAR, firma original y head del destino, y alpha tiene una migración privada.
Las obligaciones de cliente, revisión y validación están en
[Escrituras nativas](NATIVE-PARA-WRITES.md). **No es aún paridad completa para
producción.**

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

[vlRelay](https://github.com/jazware/vlrelay) (Rust, de jazware, con los
mismos crates que vlpds) consume esos enteros y publica `seq` densos
(1, 2, 3…) que `@atproto/sync` acepta. AppView y bridge leen
`ws://relay:2470`. Sigue solo a `pds.<dominio>` (`--host`, sin `--crawl`),
leído por el edge público. Su estado durable está en el bucket
(`S3_PREFIX/vlrelay`) con un commitlog local; el replay es de 72 h.
Ver [vlRelay](vlrelay.md).

El rsky parchado de `relay/` sigue disponible con `PARA_RELAY=rsky` para las
instalaciones que ya lo corren ([Relay Rust de rsky](RSKY-RELAY.md)). Un
consumidor TypeScript adaptado a los cursores de 64 bits de VLPDS podría
eliminar el relay en el futuro.

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

- Mantener el port Rust de PARA y cerrar las brechas pendientes (§3).
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
- Las imágenes de upstream no incorporan PARA. Compose compila este fork desde
  su Dockerfile y usa `para-vlpds:para-native-local` en desarrollo; producción
  requiere una imagen propia con tag inmutable.
- `para/` contiene la plataforma; `src/para.rs` y `src/xrpc/para/` contienen las
  políticas y comandos nativos. Los esquemas se sincronizan desde WatZappa.
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
