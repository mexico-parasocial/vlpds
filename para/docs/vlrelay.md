# vlRelay

[vlRelay](https://github.com/jazware/vlrelay) es el relay de jazware, hecho
con los mismos crates que vlpds (vlsync, vlatproto). Sirve `seq` densos
(1, 2, 3…) que `@atproto/sync` acepta sin cambios. Esto es lo que antes
resolvía el rsky parchado de `relay/`. **Es el relay por defecto de la
plataforma.** rsky sigue disponible con `PARA_RELAY=rsky`, para las
instalaciones que ya lo corren y para el ensayo local de producción.

Versión fijada: `1.0.0` (commit `5fcc251`, 2026-10-10), construida desde ese
commit como `para-vlrelay:1.0.0`.

## Por qué

- Elimina todo lo que mantiene `relay/`: no hay parche, fork ni build propio
  de rsky. Los `seq` densos y la respuesta a los pings del consumidor vienen
  de upstream, no de un parche nuestro.
- Verifica más que rsky: firma, inversión del MST contra `prevData`, orden de
  `rev` y autoridad del host, con estado de cadena por cuenta.
- Su estado durable vive en el bucket. Perder el disco no cambia el espacio
  de cursores (con rsky sí: hay que cambiar `RELAY_STREAM_ID`). Está probado
  abajo.
- Replay de 72 h por defecto (rsky: 24 h y 1 GiB).

## Pruebas

Corridas el 2026-10-30 en esta Mac (arm64), con el entorno de `make dev` en
puertos alternos. Las hizo un arnés propio, no una prueba versionada; ver
[Lo que falta](#lo-que-falta). La imagen se construyó desde el commit
fijado: `docker build https://github.com/jazware/vlrelay.git#5fcc251…`.

| Prueba | Resultado |
|---|---|
| `make dev-smoke` con vlRelay en memoria (el `make dev` por defecto) | Pasa: cuenta, post, AppView lo indexa vía vlRelay, proxy de vlpds, commit nativo PARA (comunidad + membresía) y rechazo de `validate=false` |
| Lo mismo, con vlRelay sobre un bucket MinIO y commitlog en disco (como en producción) | Pasa |
| `@atproto/sync` de WatZappa (0.4.11) desde el cursor 0, con verificación completa de commits (firma y MST) | 0 errores en todo el stream, PARA nativo incluido; cada `seq` es un entero seguro de JS |
| Consumidor detenido, dos posts, reanuda con su cursor | Recibe exactamente los eventos posteriores (seq 48–49), en orden, ninguno repetido |
| `kill -9` del relay | Al volver, los 84 eventos entregados se repiten con el mismo `seq`; el siguiente post sale en un `seq` nuevo; el dataplane se reconecta solo y el AppView lo indexa |
| Volumen del relay borrado (disco perdido) | Se recupera del bucket. Los 85 eventos se repiten con su `seq` original, los nuevos siguen arriba de 8,640,050, y un consumidor con el cursor viejo (50) recibe solo lo nuevo |
| Cursor por delante de la cabeza | `FutureCursor` y cierre |
| Memoria | 179 MiB tras las pruebas, bajo `VLRELAY_MEM=768m` |

Notas de las pruebas:

- Después de una recuperación desde el bucket, `/xrpc/_health` responde 200
  unos segundos antes que `subscribeRepos`, que da 503 mientras tanto. Los
  consumidores reintentan; no se perdió nada.
- El `IdResolver` endurecido de WatZappa no resuelve contra un PLC `http://`.
  El arnés le pasa `fetch` normal. El dataplane real corre con
  `unauthenticatedCommits` y no lo necesita.
- `bsync` no lee el firehose: el consumidor es el dataplane.

## Lo que falta

- **Producción real.** El hairpin hacia `pds.<dominio>` (abajo) no se probó en
  un servidor; `make verify` lo comprueba. Tampoco se probó sobre R2 ni
  SeaweedFS (perfil `local-s3`); la prueba usó MinIO.
- **El Matrix bridge** no está en el entorno dev. Su cursor (con
  `RELAY_STREAM_ID=vlrelay-v1`) no se probó contra vlRelay.
- **La imagen se construye aquí.** `ghcr.io/jazware/vlrelay` responde
  `unauthorized` a un pull anónimo con cualquier tag (`1.0.0`, `1.0`,
  `sha-5fcc251`, `main`), aunque upstream la publica en cada push. Parece un
  paquete privado. `make up` construye `para-vlrelay:1.0.0` (unos minutos la
  primera vez). Si upstream la hace pública o nuestro CI la publica, basta
  `VLRELAY_IMAGE`.
- **Upstream es nuevo.** Su README dice que no ha corrido en producción. Los
  arreglos de OOM en VPS chicos llegaron después de `1.0.0`
  (`--record-cache-entries` y `--qlog-flush-segment-mb`, 2026-10-18).
- **Prueba repetible.** El arnés vive fuera del repo. Convertirlo en
  `make relay-test` para vlRelay (como `relay/tests/smoke.mjs` para rsky)
  queda pendiente.
- **El staging de chat** (`staging/`) y el ensayo de producción del
  [LIBRETO](LIBRETO.md#4-la-producción-en-tu-máquina) siguen con rsky. El
  ensayo usa `para.test`, un TLD que vlRelay rechaza fuera de `--dev-mode`.

## Cómo cubre lo que hacía rsky

| Lo que hace el rsky parchado | vlRelay |
|---|---|
| Renumera los `seq` de vlpds (> 2^53) para TypeScript | Sí, de serie: `seq` densos del líder, los mismos tras reinicios. Una recuperación desde el bucket los adelanta (nunca los reusa) |
| Sigue un solo PDS (`RELAY_PDS_HOST`) | `--host pds.<dominio>` sin `--crawl` ni `--plc-export`: no admite ni descubre otros hosts. Un `--host` empieza en el tier `trusted` (sin tope de cuentas nuevas) |
| Transporte por Docker DNS, identidad pública (`RELAY_UPSTREAM_URL`) | **No existe.** Ver abajo |
| `--no-plc-export` | Es el default: sin `--plc-export` resuelve cada DID contra `VLRELAY_PLC_URL` (50 consultas/s) |
| Límites de frame de 64 KiB, heartbeats (parche `0002`) | Lee los frames de control del consumidor y responde pings (vlsync). Un consumidor lento recibe `ConsumerTooSlow` a 128 MiB de atraso y reconecta con su cursor |
| Fsync antes de publicar | Un nodo solo corre en modo `fsync` con `--qlog-dir` (el override lo pone) |
| `OutdatedCursor` de vlpds detiene la ingestión | **Distinto:** lo registra (`upstream info`) y sigue. Las cuentas con hueco quedan `desynchronized` hasta un `#sync`; vlRelay pide `getLatestCommit` + `getBlocks` a vlpds para resincronizarlas (`src/node/resync.rs`) |
| `FutureCursor` del consumidor | Espera 2 s a que la cabeza llegue y manda el error `FutureCursor` |
| Admin con `RELAY_ADMIN_PASSWORD` | `/admin` (consola y API, `admin:<token>`) con el mismo secreto `relay_admin_password` |
| Métricas en `:2471` | `/metrics` sin auth en `:2470`; el label `para.metrics.port` cambia a `2470` |

## Identidad y transporte

vlRelay compara el host del que llega un evento con el PDS del documento
DID (`https://pds.<dominio>`). Hay tres formas de leer vlpds:

**A. Por el edge público (lo que hace el override).**
`--host pds.<dominio>`, sin `--dev-mode`. vlRelay resuelve el nombre por DNS
público, conecta a la IP pública del servidor, Caddy termina TLS y pasa
`subscribeRepos` a vlpds. La identidad cuadra sin trucos. Necesita que el
servidor alcance su propia IP pública desde un contenedor (hairpin), que el
perfil `edge` esté arriba y que el certificado de `pds.<dominio>` exista.
Mientras tanto vlRelay reintenta. `make verify` lo comprueba con:

```bash
make compose ARGS='exec -T relay curl -fsS https://pds.<dominio>/xrpc/_health'
```

**B. Por Caddy dentro de Docker, con `--dev-mode`.** Si no hay hairpin, darle
a `caddy` el alias de red `pds.<dominio>` y agregar `--dev-mode` al relay en
un override local:

```yaml
services:
  caddy:
    networks:
      default:
        aliases: ['pds.${PARA_DOMAIN}']
  relay:
    command: [--dev-mode, --host, 'pds.${PARA_DOMAIN}', ...]  # el resto igual
```

La conexión sigue siendo `wss://` contra el certificado real de Caddy, pero
`--dev-mode` quita el filtro de IPs públicas en todo vlRelay. Eso abarca
los documentos DID, las consultas a `did:web` y las resincronizaciones.
También acepta `ws://`, IPs, puertos y TLDs reservados, y sube el
presupuesto de consultas de DID a 1000/s. Sin `--crawl` los únicos hosts que
lee son los de `--host`, pero un documento DID o un `did:web` que apunte a
una dirección interna sí se consultaría. Es un riesgo SSRF menor que no
existe en A.

**C. `--dev-mode --host http://pds:2583` más un alias de operador.**
`{"action":"alias","of":...}` hace que dos nombres hablen por el mismo PDS,
pero los dos tienen que estar ya en la tabla de hosts, y el alias deja de
leer su socket. Habría que registrar `pds.<dominio>` (que lo leería una vez)
y luego marcarlo alias de `pds:2583` a mano en la API de admin. Es estado
en el bucket que nadie ve en Compose. No lo recomiendo.

En `make dev` no hay problema: todo comparte la red de `net`, vlpds se
anuncia como `http://localhost:2583` y `--host http://localhost:2583`
implica `--dev-mode`, que ahí corresponde.

## Almacenamiento y retención

- **Bucket:** el mismo de la plataforma (`S3_ENDPOINT`, `S3_BUCKET`, las llaves
  de `secrets/`) bajo `S3_PREFIX/vlrelay`, junto a `S3_PREFIX/vlpds`.
  No hace falta otro MinIO. vlRelay exige `S3_ENDPOINT` aunque el bucket sea
  de AWS; `VLRELAY_S3_ENDPOINT` lo cambia solo para el relay. Ahí van los
  segmentos del log (`log/qlog/`), el estado (SlateDB: registros de cuentas,
  tabla de hosts, cursores de cada PDS), la política y el manifiesto. El
  líder hace flush cada 30 s.
- **Disco local:** `vlrelay_data:/var/lib/vlrelay`, con el commitlog en `qlog/`
  y fsync por entrada. Si se pierde, el nodo se recupera del último flush
  (≤ 30 s) y vlpds reenvía el resto desde el cursor guardado. Los `seq`
  saltan hacia adelante y los cursores de los consumidores siguen valiendo
  (probado).
- **Replay:** `VLRELAY_RETAIN_HOURS` (72 por defecto) en el bucket. Los
  últimos 32 MiB salen de memoria, 512 MiB del disco y lo demás del bucket.
  Un cursor más viejo recibe `#info OutdatedCursor` y sigue desde lo más
  antiguo que queda. Con el volumen de PARA, 168 h cuesta poco.
- **Respaldos:** no hay `make relay-snapshot` (el script lo rechaza con
  vlRelay). Lo durable está en el bucket, igual que vlpds.
- **Memoria:** `VLRELAY_MEM=768m` y `--ingest-mem-mb 384`, con anillo, cachés
  y buffers reducidos para un PDS (su guía "A small box"). En dev usó
  179 MiB. La release `1.0.0` no tiene `--record-cache-entries` ni
  `--qlog-flush-segment-mb`. Con un PDS el caché de registros es chico, y un
  segmento de 64 MiB tarda en llenarse. Si se fija un `sha-` posterior al
  2026-10-18, agregar ambos (`65536` y `16`).

## Uso

Sin configurar nada, `make up`, `make compose`, `make dev` y los demás
comandos agregan `-f overrides/relay-vlrelay.yaml`
(`dev/compose.vlrelay.yaml` en dev). `make up` construye
`para-vlrelay:1.0.0` desde el commit fijado, como hace con `backup`.
`.env.example` trae:

```bash
PARA_RELAY=vlrelay
RELAY_STREAM_ID=vlrelay-v1
# VLRELAY_IMAGE=para-vlrelay:1.0.0   # o una imagen publicada
```

El servicio se sigue llamando `relay`, escucha en `2470` y conserva el
alias `relay.para.internal`. El dataplane, `make verify` (que busca
`pds.<dominio>` en `listHosts`) y Alloy no cambian. El bridge toma
`BRIDGE_PDS_SOURCE_ID=${RELAY_STREAM_ID:-vlrelay-v1}`.

`make preflight` falla en tres casos:

- La imagen no tiene un tag fijo.
- Falta `S3_ENDPOINT`.
- `RELAY_STREAM_ID` sigue siendo `rsky-*`, o la máquina tiene un volumen
  `para_rsky_relay_data` y `PARA_RELAY` no está fijado. Así una instalación
  de rsky no se pasa sola de espacio de cursores al actualizar.

`make verify` además revisa el hairpin. `make relay-add-pds` reabre el
socket de `pds.<dominio>` con la API de admin en vez de `requestCrawl`. El
override quita el relay local de `VLPDS_CRAWLERS`, porque vlRelay no acepta
`requestCrawl`.

Para seguir con rsky: `PARA_RELAY=rsky` (y `RELAY_STREAM_ID=rsky-relay-v1`)
en `.env`, o en `para_env` del inventario de Ansible.

La consola de vlRelay está en `/admin` del puerto 2470, que no se publica.
La API se alcanza desde el contenedor
(`make compose ARGS='exec -T relay curl -u admin:<token> http://127.0.0.1:2470/admin/api/hosts'`).
Para la consola web, publica `127.0.0.1:2470` en un override local y usa un
túnel SSH.

## Cambiar de relay

Los cursores de rsky y los de vlRelay son espacios distintos. Nunca copiar
uno en otro.

- **Dataplane:** usa un `MemoryRunner` sin cursor guardado: cada vez que
  arranca empieza en la cabeza del relay, con rsky también. Se cambia solo,
  pero lo escrito mientras no corre no le llega.
- **Matrix bridge:** guarda su cursor junto con `BRIDGE_PDS_SOURCE_ID`
  (`RELAY_STREAM_ID`). Con el ID viejo le pediría a vlRelay un `seq` de
  rsky, que suele estar por delante de la cabeza. Recibiría `FutureCursor`,
  se cerraría y reconectaría sin fin. Por eso preflight exige otro
  `RELAY_STREAM_ID`. Seguir el procedimiento de
  [RSKY-RELAY.md](RSKY-RELAY.md#migrar-desde-indigo) (detener, copiar la base,
  reconciliar el cursor).
- **Primera suscripción:** un `--host` nuevo empieza en la cabeza de vlpds,
  no en el cursor 0, igual que indigo. Para que vlRelay tenga historia antes
  del corte, arrancarlo en paralelo (otro nombre de servicio y volumen)
  antes de mover los consumidores. Si no, usar
  `{"action":"set-backfill","backfill":true}` en el host, que lee todo lo
  que vlpds conserve.
- **Volver a rsky:** `PARA_RELAY=rsky`, restaurar `RELAY_STREAM_ID` y la base
  del bridge de antes del corte. `rsky_relay_data` no se toca mientras corre
  vlRelay.

## Archivos

| Archivo | Qué |
|---|---|
| `overrides/relay-vlrelay.yaml` | El servicio `relay` con vlRelay (build fijado), el `RELAY_STREAM_ID` del bridge y `VLPDS_CRAWLERS` sin el relay local |
| `dev/compose.vlrelay.yaml` | Lo mismo para `make dev`, en memoria |
| `Makefile` | `PARA_RELAY` (`vlrelay` por defecto, o `rsky`), la construcción en `make up` y `relay-add-pds` para cada uno |
| `dev/up.sh`, `dev/down.sh` | Usan el override salvo con `PARA_RELAY=rsky`; con vlRelay solo comprueban `listHosts` |
| `scripts/preflight.sh`, `scripts/verify.sh` | Las comprobaciones de arriba |
| `relay/snapshot.sh` | Rechaza vlRelay con un mensaje claro |
| `core.yaml`, `relay/` | rsky, sin cambios: la definición base que el override reemplaza |
