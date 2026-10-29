# Respaldos y recuperación

## Qué es durable y dónde

| Dato | Dónde vive | Cómo está protegido | Lo que puede perderse |
| --- | --- | --- | --- |
| vlpds: cuentas, repos, blobs, llaves de firma (envueltas), estado de moderación | El bucket, directo (`S3_PREFIX/vlpds`) | Cada commit es durable en el bucket antes de responder | Nada |
| Base de mubEZ | Disco del contenedor, **solo caché** | Litestream, cada `LITESTREAM_SYNC_INTERVAL` (1 s) | ~1 s de cambios |
| Postgres: `para` (AppView, bsync, Ozone), `relay`, `matrix`, `mas`, `bridge` | Volumen `postgres_data` | `pg_dump` cada `BACKUP_INTERVAL` | Hasta un intervalo |
| Media de Synapse | Volumen `synapse_data` | Copia incremental cada `BACKUP_INTERVAL` | Hasta un intervalo |
| Caché de vlpds, archivos de eventos del relay, Redis | Volúmenes y memoria | No hace falta: se reconstruyen | Nada que importe |
| Las seis llaves offline | `secrets/` | **Nunca** al bucket | Todo, si no las respaldaste |

El AppView se puede reconstruir entero: su base sale de indexar el firehose.
Restaurarla es más rápido que volver a indexar, pero no es imprescindible.

En el bucket:

```
<S3_BUCKET>/<S3_PREFIX>/
  vlpds/                          todo vlpds (log, estado de SlateDB, blobs, cuentas)
  litestream/mubez/               base de mubEZ
  backups/postgres/<base>/<fecha>.dump
  synapse/media_store/
```

Activa en el bucket el versionado (o la retención de objetos) y una regla que
aborte subidas multiparte incompletas tras un día. Es lo que protege a vlpds
de un borrado accidental: vlpds no tiene otra copia.

## Las llaves offline

No viven en el bucket a propósito: con el bucket y estas llaves, alguien
tendría la plataforma entera. Respáldalas fuera de la máquina (un gestor de
contraseñas del equipo, o el inventario cifrado con sops):

| Archivo | Si se pierde |
| --- | --- |
| `vlpds_kek` | **Los repos del bucket no se pueden volver a firmar.** vlpds guarda cada llave de firma envuelta con la KEK |
| `vlpds_plc_rotation_key` | No se pueden actualizar los DIDs de las cuentas (migrar, rotar llaves). Va envuelta con la KEK: sirve junto con ella |
| `bsky_service_signing_key` | Cambia la identidad del AppView ante los clientes |
| `ozone_signing_key` | Las etiquetas de moderación firmadas dejan de verificarse |
| `m8_curp_pepper` | Ya no se puede deduplicar a nadie por CURP |
| `m8_issuer_private_jwk` | No se pueden emitir credenciales con la misma identidad |

Fuera de `secrets/`, también: la llave de firma de Synapse y los secretos de
MAS (`WatZappa/deploy/matrix/synapse/` y `deploy/matrix/mas/`, generados por
`make matrix-init`).

Para más seguridad, agrega una llave de recuperación del operador
(`VLPDS_PLC_RECOVERY_DID_KEY` en `env/vlpds.env`): vlpds la pone en las llaves
de rotación de cada DID, delante de la del servidor. Su parte privada no toca
nunca la máquina.

## Respaldos automáticos

El servicio `backup` (perfil `core`) respalda cada base de Postgres y
`backup-media` (perfil `matrix`) el media de Synapse, cada `BACKUP_INTERVAL`,
y borran lo más viejo que `BACKUP_RETENTION`. Exponen métricas que vigilan
`ParaBackupStale` y `ParaBackupFailing`.

```bash
make backup                        # uno ahora, además del automático
make restore ARGS="list"
make restore ARGS="list postgres/para"
```

La imagen de respaldos usa la misma versión mayor que Postgres (17):
`pg_dump` no respalda un servidor más nuevo que él.

## Restaurar

Siempre: detén primero lo que escribe en lo que vas a reemplazar.

**Una base de Postgres**

```bash
make stop S="bsky dataplane bsync ozone"          # lo que usa la base para
make restore ARGS="postgres para latest --force"  # o una fecha: 20261010T052901Z
make up
```

Sin `--force` se niega si la base tiene tablas. Para `relay`, detén el relay;
para `bridge`, el bridge.

**vlpds**: no hay nada que restaurar. Con el mismo bucket, la misma KEK y la
misma llave PLC envuelta, un vlpds nuevo arranca donde quedó el anterior (la
caché de disco se rellena sola). Si hubo un borrado en el bucket, recupera los
objetos con el versionado del proveedor; los procedimientos de vlpds están en
[docs/operations/backups-and-recovery.md](../../docs/operations/backups-and-recovery.md).

**mubEZ**: tampoco. Borra su volumen o muévelo de máquina y, al arrancar, se
restaura solo desde el bucket. Nunca corras dos brokers con el mismo
`S3_PREFIX`.

## Perder la máquina entera

1. Máquina nueva con Docker; los repos en la misma revisión (el fork en
   `para/platform`, WatZappa, mubEZ).
2. `.env` igual al de antes y los secretos de vuelta en `secrets/`: las llaves
   offline **y** los aleatorios (tenlos también en el inventario cifrado; si
   no, `make secrets` genera otros y habrá que cambiarlos donde se usan).
3. `make preflight`, luego `make up PROFILES=core` sin el edge todavía.
   vlpds retoma su estado del bucket solo.
4. Restaura Postgres (cada base). mubEZ se restaura solo.
5. Si usas Matrix, restaura su media: `make stop S=synapse` y
   `make restore-media`.
6. `make up` con todos los perfiles, apunta el DNS y `make verify`.

Tras una caída dura, vlpds tarda hasta un lease (60 s en `tiny`) en volver a
escribir: es normal.
