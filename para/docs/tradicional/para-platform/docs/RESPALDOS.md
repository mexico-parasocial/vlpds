# Respaldos y recuperación

## Qué es durable y dónde

| Dato | Dónde vive | Cómo llega al bucket | Lo que puede perderse |
| --- | --- | --- | --- |
| Blobs del PDS (imágenes, video) | El bucket, directo | Ya están ahí | Nada |
| Base de mubEZ (identidad, llaves envueltas, auditoría) | Disco del contenedor, **solo caché** | Litestream, cada `LITESTREAM_SYNC_INTERVAL` (1 s) | ~1 s de cambios |
| Postgres: `para`, `matrix`, `mas`, `bridge` | Volumen `postgres_data` | `pg_dump` cada `BACKUP_INTERVAL` | Hasta un intervalo |
| PDS: cuentas, secuenciador, un SQLite y una llave por repo | Volumen `pds_data` | Copia consistente (API de backup de SQLite) cada `BACKUP_INTERVAL` | Hasta un intervalo |
| Media de Synapse | Volumen `synapse_data` | Copia incremental cada `BACKUP_INTERVAL` | Hasta un intervalo |
| Redis | Memoria | No hace falta: es caché | Nada que importe |
| Las cinco llaves offline | `secrets/` | **Nunca** al bucket | Todo, si no las respaldaste |

En el bucket:

```
<S3_BUCKET>/
  blocks/ tmp/ quarantine/          blobs del PDS (el PDS no admite prefijo)
  <S3_PREFIX>/litestream/mubez/     base de mubEZ
  <S3_PREFIX>/backups/postgres/<base>/<fecha>.dump
  <S3_PREFIX>/backups/pds/<fecha>.tar.zst
  <S3_PREFIX>/synapse/media_store/
```

Usa un bucket por plataforma (los blobs no llevan prefijo). Activa en el
bucket lo que el proveedor ofrezca para protegerlo de borrados: versionado o
retención de objetos, y una regla que aborte subidas multiparte incompletas
después de un día.

## Las llaves offline

No viven en el bucket a propósito: con el bucket y estas llaves, alguien
tendría la plataforma entera. Respáldalas fuera de la máquina (un gestor de
contraseñas del equipo, o el inventario cifrado con sops):

| Archivo | Si se pierde |
| --- | --- |
| `pds_plc_rotation_key` | No se pueden actualizar los DIDs de las cuentas (migrar, rotar llaves) |
| `bsky_service_signing_key` | Cambia la identidad del AppView ante los clientes |
| `ozone_signing_key` | Las etiquetas de moderación firmadas dejan de verificarse |
| `m8_curp_pepper` | Ya no se puede deduplicar a nadie por CURP |
| `m8_issuer_private_jwk` | No se pueden emitir credenciales verificables con la misma identidad |

Fuera de `secrets/`, también: la llave de firma de Synapse y los secretos de
MAS (`WatZappa/deploy/matrix/synapse/` y `deploy/matrix/mas/`, generados por
`make matrix-init`).

## Respaldos automáticos

El servicio `backup` (perfil `core`) y `backup-media` (perfil `matrix`)
respaldan cada `BACKUP_INTERVAL` y borran lo más viejo que
`BACKUP_RETENTION`. Exponen métricas que vigilan `ParaBackupStale` y
`ParaBackupFailing`.

```bash
make backup                        # uno ahora, además del automático
make restore ARGS="list"           # qué hay en el bucket
make restore ARGS="list postgres/para"
```

## Restaurar

Siempre: detén primero lo que escribe en lo que vas a reemplazar.

**Una base de Postgres**

```bash
make stop S="bsky dataplane bsync ozone"          # lo que usa la base para
make restore ARGS="postgres para latest --force"  # o una fecha: 20261010T052901Z
make up
```

Sin `--force` se niega si la base tiene tablas.

**El PDS**

```bash
make stop S=pds
make restore ARGS="pds latest --force"
make up
```

**mubEZ**: no hay nada que hacer. Borra su volumen o muévelo de máquina y, al
arrancar, se restaura solo desde el bucket (probado: un registro escrito antes
de destruir el contenedor y su disco vuelve en el contenedor nuevo). Nunca
corras dos brokers con el mismo `S3_PREFIX`.

## Perder la máquina entera

1. Máquina nueva con Docker; los tres repos en la misma revisión.
2. `.env` igual al de antes y las llaves offline de vuelta en `secrets/`
   (`make secrets` genera el resto; `postgres_password` y el resto de los
   aleatorios conviene tenerlos también en el inventario cifrado, o tendrás
   que cambiarlos donde se usan).
3. `make preflight`, luego `make up PROFILES=core` sin el edge todavía.
4. Restaura Postgres (cada base) y el PDS como arriba. mubEZ se restaura solo.
5. Si usas Matrix, restaura su media: `make stop S=synapse` y
   `make restore-media`.
6. `make up` con todos los perfiles, apunta el DNS y `make verify`.
