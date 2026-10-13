# Actualizaciones

## Cambiar de versión

Siempre antes:

```bash
make backup          # Postgres: hay migraciones de una sola vía en el AppView, Ozone y mubEZ
```

**vlpds.** Cambia `VLPDS_IMAGE` en `.env` a la versión nueva (siempre un tag
fijo, nunca `latest`) y `make up`. vlpds se detiene en ~0.25 s y arranca en
~1.3 s; Caddy reintenta 10 s, así que se nota como unos segundos de espera.
Lee antes las notas de la versión: vlpds tiene **niveles de funciones**
(feature levels) que se activan después de actualizar todos los nodos, y
algunas subidas son de un solo sentido (por ejemplo, los datos de Spaces). El
procedimiento está en
[docs/operations/upgrades.md](../../docs/operations/upgrades.md).

**WatZappa** (AppView, bsync, Ozone, bridge, para-idp). Mientras no haya
imágenes publicadas, se construyen desde el checkout:

```bash
git -C ../../WatZappa pull          # o checkout de la revisión que quieras
make up BUILD=1
make verify
```

Con imágenes publicadas por CI sería `make upgrade WATZAPPA_VERSION=sha-<commit>`,
que escribe el tag en `.env` (rechaza `main`, `latest` y vacío), descarga,
recrea solo lo que cambió y corre `make verify`.

**El relay.** `RELAY_COMMIT` (un commit de indigo) y `RELAY_VERSION` (el tag
de la imagen local) en `.env`, luego `make up BUILD=1`. Su estado (hosts y
cursores en Postgres, 72 h de eventos en disco) se conserva.

**El fork.** `git pull` en el fork trae los cambios de `para/` y, por la
sincronización diaria, los de upstream.

Volver atrás es lo mismo con la versión anterior, **si** la nueva no migró
una base de forma incompatible. Si lo hizo, restaura el respaldo de antes
([Respaldos](RESPALDOS.md)).

Cambios que piden orden (los dice su nota de versión): por ejemplo, la
autoridad de comunidades exige detener el bridge, migrar el AppView y después
levantar el bridge nuevo:

```bash
make stop S=matrix-bridge
make up BUILD=1          # el AppView migra al arrancar
make verify
```

## De la variante tradicional a vlpds

Pasar un despliegue con el PDS en TypeScript a vlpds. **Hazlo solo cuando la
brecha de `com.para.*` esté resuelta** ([VLPDS.md §3](VLPDS.md#3-la-brecha-de-compara)):
con vlpds, hoy la app no puede crear comunidades, unirse ni votar.

### Qué se conserva y qué no

| | Qué pasa |
|---|---|
| Postgres (AppView, bsync, Ozone, Matrix, MAS, bridge) | Se conserva. El AppView reindexa lo que llegue por el relay; lo ya indexado sigue ahí |
| Matrix, mubEZ | Igual que antes: no dependen del PDS |
| Las cuentas y sus repos | **Se migran**, no se copian: vlpds guarda todo distinto (SlateDB en el bucket) |
| La llave PLC del PDS | Se conserva envuelta con la KEK: los DIDs que la listan siguen valiendo |
| Los handles | Se conservan si `PARA_HANDLE_DOMAIN` es el mismo |
| Los blobs | Se copian con cada cuenta |

### Pasos

1. **Respaldos completos** del despliegue tradicional: Postgres, el volumen
   del PDS y el bucket.
2. **Secretos.** En `vlpds/para`, `make secrets-import` trae los del
   despliegue anterior (`WatZappa/.env`, `env/identity.env`) y envuelve su
   llave PLC con una KEK nueva. `make secrets` crea lo que falte (los de
   vlpds y el relay). Guarda offline la KEK y la llave envuelta.
3. **`.env`** con el mismo `PARA_DOMAIN`, `PARA_HANDLE_DOMAIN` y bucket (con un
   `S3_PREFIX` nuevo o el mismo: vlpds usa `S3_PREFIX/vlpds`, separado de lo
   anterior).
4. **Migra las cuentas** con la herramienta de vlpds (`/migrate`): copia el
   repo, los blobs y las preferencias, y mueve la identidad con la firma del
   PDS viejo ([docs/migration.md](../../docs/migration.md)). Pruébala primero
   con una cuenta de prueba.

   **Pendiente antes de poder hacerlo:** la migración necesita los dos PDS en
   línea a la vez, con hostnames distintos, y la plataforma usa siempre
   `pds.<dominio>` para vlpds. Hace falta un hostname configurable (por
   ejemplo `pds2.<dominio>` para vlpds durante la migración) o levantar vlpds
   en otro dominio. No está hecho.
5. **Cuando todas las cuentas estén en vlpds**, apunta el DNS al servidor
   nuevo y `make verify`.
6. **Apaga el PDS viejo** cuando la plataforma lleve un tiempo bien. Conserva
   su volumen y su respaldo hasta estar seguro.

Una cuenta migrada emite `#identity` y `#sync`: el relay y el AppView se
enteran solos.

## Desde para-platform

Si ya operabas con la para-platform archivada (la variante tradicional con
perfiles), lo que cambia al pasar a `vlpds/para`:

- La plataforma vive en el fork (`/opt/para/vlpds/para`), con los repos como
  hermanos del fork. Con Ansible, `para_repos` apunta al fork en la rama
  `para/platform`.
- `overrides/core.yaml` ya no existe: el núcleo es `core.yaml`, propio.
- Los secretos del PDS (`pds_*`) se reemplazan por los de vlpds (`vlpds_*`),
  más `bsync_api_key`, `relay_admin_password` y `postgres_url_relay`.
- Ya no hay respaldo del PDS ni volumen `pds_data`.
- `make dev` ya no corre el entorno de WatZappa: levanta vlpds en Docker. El
  de WatZappa sigue en `make run-dev-env` ([tradicional/LIBRETO.md](tradicional/LIBRETO.md)).
- `make doctor-prod` es `make doctor`.
