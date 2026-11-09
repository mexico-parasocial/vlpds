# Relay Rust de rsky

> El relay por defecto es ahora [vlRelay](vlrelay.md). rsky sigue disponible
> con `PARA_RELAY=rsky`, para las instalaciones que ya lo corren y el ensayo
> local de producción ([LIBRETO](LIBRETO.md#4-la-producción-en-tu-máquina)).

**Estado: listo para validación aislada en staging. La migración de usuarios
existentes sigue pendiente de validación integral y recuperación.** Construir y probar
este relay no completa la paridad PARA; ver las condiciones de salida en
[NATIVE-PARA-WRITES.md](NATIVE-PARA-WRITES.md).

El perfil VLPDS usa `VLPDS → rsky-relay → WatZappa AppView / Matrix bridge`.
El relay guarda el cursor original de VLPDS sin convertirlo a JavaScript y
publica su propia secuencia dentro del rango de enteros seguros. Las escrituras
nativas y sus políticas siguen en VLPDS; el relay no implementa reglas PARA.

## Fuente y construcción

`relay/source.env` fija rsky en
`e96634e18d3731d4f4a0380f620f2e1ea52f9a9f`. El Dockerfile aplica el parche
versionado `relay/patches/0001-para-single-pds.patch`, la corrección de control
WebSocket `0002-websocket-heartbeats.patch` y ejecuta los tests antes
de construir el binario. El tag actual es `e96634e-para2`.

```sh
make up BUILD=1
make verify
make relay-test
```

`relay-test` es una prueba aislada: necesita Rust 1.99, Node 22 o superior y
las dependencias de WatZappa, con `packages/sync/dist/lexicons` construido.
Usa puertos efímeros, cuentas de prueba y un directorio temporal; no toca los
contenedores ni las bases de un despliegue existente. Para probar la imagen:

```sh
docker build -t para-rsky-relay:e96634e-para2 relay
RELAY_IMAGE=para-rsky-relay:e96634e-para2 node relay/tests/smoke.mjs
```

## Cambios respecto a upstream

- Atiende pings/pongs y cierres aun sin eventos, con lectura acotada de
  controles WebSocket. El consumidor TypeScript conserva su heartbeat.
- Preserva `prev` en updates/deletes, `cid: null` en deletes y omite los
  campos opcionales ausentes. Conserva los bytes CAR y el commit firmado.
- Escucha en `RELAY_LISTEN`. Solo sigue `RELAY_PDS_HOST`; su transporte
  explícito `RELAY_UPSTREAM_URL` puede usar DNS de Docker. No descubre otros
  PDS ni sigue redirecciones. La identidad se valida con el hostname público.
- Arranca con resolución directa de DIDs, sin importar el PLC global.
  `RELAY_LENIENT_VALIDATION=false` es fijo en Compose.
- Fsync antes de publicar; los publicadores solo leen hasta la marca durable.
  Persiste el contador por separado para que la caducidad del replay no lo
  reinicie. El checkpoint upstream se confirma en Fjall junto con la cola/log;
  SQLite es un catálogo periódico. Fsync antes de avanzar su copia SQLite.
- Los eventos pendientes de identidad no caducan. La cola está acotada; si
  se llena, el proceso se detiene y vuelve desde el checkpoint durable. Se
  reintenta resolver aunque no lleguen nuevas escrituras.
- Una caída del validador detiene el relay; Docker lo reinicia. No queda un
  servidor sano que dejó de ingerir eventos.
- `OutdatedCursor` de VLPDS o un error de su suscripción detienen la ingestión
  explícitamente. Requieren reconciliación; no se convierten en una secuencia
  aparentemente completa que oculta una brecha upstream.

Los tamaños del parche son para un PDS: caché 32 MiB, buffer de escritura
16 MiB, presupuesto de intake 16 MiB y un worker de cada tipo. El perfil tiny
limita el contenedor a 256 MB. Son límites configurados, no un benchmark ni
una garantía de consumo bajo carga. El replay tiene un techo de 1 GiB y una
retención FIFO objetivo de 24 h; puede acortarse por volumen. Un cursor fuera
del rango retenido produce `OutdatedCursor` y exige reconciliación/backfill.

## Estado y recuperación

`rsky_relay_data:/data/relay` contiene SQLite, Fjall, el contador y las
obligaciones pendientes. Es estado durable. No se comparte con Indigo y no
se debe borrar para liberar espacio. Los respaldos Postgres ya no incluyen
una base `relay`; las bases/volúmenes antiguos se conservan para rollback.

Para una copia consistente, detener el relay, copiar **todo** `/data/relay`
y volverlo a arrancar. Restaurar siempre SQLite y Fjall juntos, con los
consumidores detenidos y sus checkpoints coherentes con esa copia. Una copia
vieja puede estar detrás del cursor de Matrix; restaurarla sola no basta.
El volumen requiere un respaldo/snapshot del operador independiente de los
`pg_dump` automáticos. Nunca copiar únicamente `relay.db` mientras escribe.

`make relay-snapshot` hace esa copia local con el relay detenido y lo vuelve
a iniciar incluso si falla la copia. Guarda el archivo en
`.dev/relay-snapshots/` (o `RELAY_SNAPSHOT_DIR`). Para un punto de recuperación
coherente, detener también dataplane/bridge y respaldar sus bases en el mismo
corte. Llevar estas copias fuera de la máquina con el procedimiento del operador.

Si se pierde el volumen y no hay una copia coherente, se crea otro espacio de
cursores: cambiar `RELAY_STREAM_ID` y reconstruir/reconciliar consumidores.
La copia autoritativa de los registros permanece en VLPDS; la asignación de
cursores de este relay no se puede reconstruir por una fórmula.

## Migrar desde Indigo

Un cursor de Indigo, uno de rsky y uno de VLPDS pertenecen a tres espacios
distintos. Nunca copiar, dividir, redondear ni reutilizar esos números.
Mantener el volumen `relay_data` y una copia de la base Postgres antigua.

1. Detener dataplane y Matrix bridge antes del corte; tomar una copia de sus
   bases y del estado Indigo. No borrar las proyecciones actuales.
2. Probar rsky en paralelo con un volumen y puerto propios. Comparar commits,
   CARs y estado reconstruido en consumidores aislados; atender las brechas
   de historia retenida mediante backfill.
3. Cambiar el endpoint de ingestión y definir un nuevo `RELAY_STREAM_ID`.
   Preparar/reconciliar el cursor Matrix en una copia de su base antes de
   promoverla. El bridge rechaza un source ID distinto o un cursor antiguo
   sin source ID: ese rechazo evita saltarse eventos o duplicar mensajes.
4. Arrancar consumidores y verificar lag, comunidades y publicaciones. Para
   rollback, usar la revisión/configuración y las copias anteriores de
   Indigo/consumidores; los cursores rsky no sirven para volver a Indigo.

Este cambio de repositorio no ejecuta un corte sobre servicios existentes.
Las instalaciones nuevas usan rsky por defecto. La paridad pendiente de las
escrituras nativas sigue documentada en [NATIVE-PARA-WRITES.md](NATIVE-PARA-WRITES.md).

## Actualizar

Actualizar el commit, parche y labels del Dockerfile juntos, fijar la fuente
en `relay/source.env`, correr
`make relay-test`, construir/probar la imagen y publicar otro tag inmutable.
Conservar `RELAY_STREAM_ID` y el volumen durante una actualización compatible;
el tag del software no es la identidad del espacio de cursores. `RELAY_COMMIT`
de Indigo ya no selecciona la fuente y preflight lo rechaza si sigue en `.env`.

## Validación de esta revisión

125 tests Rust pasan tanto en el host como durante la construcción Linux de
la imagen. La prueba contra VLPDS nativo y el decoder/schema real de WatZappa
verifica el commit PARA de ocho registros, create/update/delete, bytes CAR y
CIDs previos, cursores seguros, SIGKILL/replay, recuperación de cola tras caída
PLC + SIGKILL, apagado con gracia y rechazo explícito de una brecha upstream.
Clippy del relay y las configuraciones Compose dev, core y core+Matrix pasan.

La prueba usa fixtures para PLC y la consulta de nombres del AppView; valida
el transporte y contrato TypeScript. No sustituye las pruebas completas de
indexación AppView, efectos Matrix, M8 ni migración de datos de producción.
