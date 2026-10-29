# Runbook

La primera página que se abre cuando suena una alerta. Una sección por alerta
de [ops/alerts.yml](../ops/alerts.yml), en el mismo orden. Para cualquier
cosa, primero:

```bash
make ps            # quién no está running/healthy
make logs S=<svc>  # qué dice
make verify        # el estado completo
```

**Lo que no se hace:** `docker kill` (corta sin gracia: el bridge y mubEZ
dejan trabajo a medias), correr dos mubEZ con el mismo `S3_PREFIX`, `make
down -v` (borra los volúmenes), editar secretos sin `UP_ARGS=--force-recreate`,
o "arreglar" en el servidor algo que el inventario va a pisar en el siguiente
despliegue.

---

## ParaEdgeDown

**Página.** Caddy no entrega métricas: probablemente nada público responde.

1. `make ps` — ¿`caddy` está running? `make logs S=caddy`.
2. Si reinicia en bucle: casi siempre es el Caddyfile o un certificado.
   `make check` valida el Caddyfile; un error de ACME en el log suele ser DNS
   que no apunta aquí o el puerto 80 cerrado.
3. Si corre pero Alloy no lo ve: `make logs S=alloy` (red, proxy de Docker).

## ParaUpstreamDown

**Página.** Caddy no alcanza un servicio (la etiqueta `upstream` dice cuál:
`pds:2583` (vlpds), `bsky:2584`, `ozone:3000`, `matrix-bridge:3001`,
`mubez:8787`, o un worker).

1. `make ps` y `make logs S=<servicio>`.
2. Si el contenedor reinicia y el log dice `env-from-files: …`: falta un
   secreto, está vacío o llega dos veces. `scripts/secrets.sh check`.
   vlpds dice lo mismo con sus propias palabras (secreto corto, repetido, sin
   KEK) y se detiene.
3. vlpds que sale con código 2–9 se detuvo a propósito (perdió su lease, no
   alcanza el bucket, otro nodo con su `VLPDS_NODE_ID`); Docker lo levanta de
   nuevo. Tras un SIGKILL u OOM no escribe durante un lease (60 s en `tiny`).
   Si no se recupera: `make logs S=pds` y que el bucket responda
   (`make preflight`).
4. mubEZ que no arranca con un error de Litestream: no alcanza el bucket
   (llaves, endpoint, red). Es a propósito: sin bucket no hay durabilidad, así
   que no sirve. Arregla el bucket (`make preflight` lo prueba).
4. Un worker de AppView: ¿responde por la red privada? `curl
   http://<ip-privada>:2584/xrpc/_health` desde la principal.

## ParaEdge5xxHigh

**Ticket.** Más del 5 % de las respuestas son errores del servidor.

1. Qué host: el tablero de Caddy por `host`. `make logs S=<servicio>`.
2. Coincide con un despliegue: vuelve a la versión anterior
   (`make upgrade WATZAPPA_VERSION=<anterior>`), si no hubo migración
   incompatible.
3. Coincide con `ParaPostgresConnectionsHigh` o `ParaMemoryLow`: atiende esa.

## ParaPostgresDown

**Página.** Todo lo que no es mubEZ depende de Postgres.

1. `make ps`, `make logs S=postgres`.
2. `could not resize shared memory` o reinicios por OOM: el perfil no alcanza
   para la máquina. Baja `PG_SHARED_BUFFERS` o sube `POSTGRES_MEM`.
3. Disco lleno: ver `ParaDiskAlmostFull`.
4. Corrupción o volumen perdido: restaura cada base
   ([Respaldos](RESPALDOS.md)).

## ParaPostgresConnectionsHigh

**Ticket.** Más del 80 % de `max_connections` en uso.

1. Quién: `make compose ARGS="exec -T -u postgres postgres psql -U pg -c 'select datname, usename, count(*) from pg_stat_activity group by 1,2 order by 3 desc'"`.
2. Cada réplica y worker del AppView abre su propio pool. Sube
   `PG_MAX_CONNECTIONS` (y `POSTGRES_MEM`) o reduce réplicas.
3. Si sigue creciendo sin carga que lo explique: una fuga en un servicio;
   reinícialo (`make restart S=…`) y abre un issue con el log.

## ParaBackupStale

**Página.** Un respaldo no termina bien desde hace más de 2.5 intervalos: si
se pierde la máquina ahora, se pierde más de lo esperado.

1. `make logs S=backup` (o `backup-media`): el error está ahí.
2. Bucket: llaves rotadas, cuota, red. `make preflight` lo prueba de punta a
   punta.
3. Postgres no responde: ver `ParaPostgresDown`.
4. Arreglado: `make backup` y confirma que la alerta se limpia.

## ParaBackupFailing

**Ticket.** El último intento falló (todavía no es grave; `ParaBackupStale`
avisa si sigue). Mismos pasos que arriba.

## ParaDiskAlmostFull

**Página** con menos del 10 %, ticket (`ParaDiskFilling`) con menos del 20 %.

1. Qué crece: `docker system df -v` y `du -sh /var/lib/docker/volumes/*`.
2. Imágenes viejas: `docker image prune -a` (solo las que no usa ningún
   contenedor).
3. Logs: Docker tiene tope por contenedor (Ansible lo configura: 5 × 50 MB).
4. La caché de disco de vlpds ocupa hasta `VLPDS_DISK_CACHE_MB` (4 GB en
   `tiny`) y el relay guarda 72 h de eventos: los dos se pueden borrar sin
   perder nada (sus volúmenes `vlpds_cache` y `relay_data`).
5. Postgres o el media de Synapse crecen de verdad: más disco.

## ParaMemoryLow

**Ticket.** Queda menos del 10 % de memoria en la máquina.

1. `docker stats --no-stream`: quién usa qué contra su techo.
2. La suma de techos del perfil no debe pasar la memoria de la máquina menos
   ~1.5 GB. Si la máquina es más chica que el perfil, baja techos en `.env`.
3. Un servicio pegado a su techo: súbelo, o escala (docs/ESCALADO.md).

## ParaHostClockSkew

**Ticket.** El reloj se desvía más de 50 ms. Los tokens, certificados y
firmas dependen de la hora.

1. `chronyc tracking`. ¿Corre chrony? `systemctl status chrony`.
2. El proveedor bloquea NTP: configura los servidores de hora del proveedor
   en `/etc/chrony/chrony.conf`.

## ParaTargetDown

**Ticket.** Un servicio con métricas (`job`: `pds`, `relay`,
`matrix-bridge`, `backup`, `backup-media`) dejó de entregarlas.

1. `make ps`: ¿está corriendo? Si no, sus logs.
2. Si corre: su puerto de métricas (`para.metrics.port` en los overrides) y
   `make logs S=alloy`.

## ParaBridgeFirehoseLagging

**Ticket.** El bridge procesa el firehose (que lee del relay) con más de
5 min de retraso: lo que pasa en las comunidades tarda en llegar a Matrix.

1. `make logs S=matrix-bridge`: errores contra Synapse (token de admin
   vencido → `secrets/matrix_admin_token`) o contra el relay.
2. ¿El relay sigue a vlpds? `make verify` (sección Firehose) o
   `make compose ARGS="exec -T relay wget -qO- http://127.0.0.1:2470/xrpc/com.atproto.sync.listHosts"`.
   Si no: `make relay-add-pds`.
3. ¿Synapse está sano? `make logs S=synapse`.
4. Después de una caída larga, el retraso baja solo mientras se pone al día;
   vigila que la curva baje.

---

## Sin alerta propia

### El AppView no muestra lo nuevo

Las publicaciones se escriben en vlpds pero no aparecen en los feeds.

1. ¿El relay sigue a vlpds? Igual que en `ParaBridgeFirehoseLagging`, paso 2.
2. `make logs S=dataplane`: un `error in firehose event lexicon validation …
   at $.seq` quiere decir que algo apunta al firehose de vlpds directo; el
   dataplane debe leer `ws://relay:2470` (`BSKY_REPO_PROVIDER`).
3. Un solo dataplane indexa: si hay un worker de AppView, el suyo no debe
   tener `BSKY_REPO_PROVIDER` (`overrides/worker.yaml`).

### La app no puede crear comunidades, unirse ni votar

No es una caída: esas rutas `com.para.*` no existen en vlpds
([VLPDS.md §3](VLPDS.md#3-la-brecha-de-compara)).
