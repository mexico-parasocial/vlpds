# PARA sobre vlpds

El backend de PARA con [vlpds](https://vlpds.jazco.dev/docs/overview) como
PDS, en el fork `mexico-parasocial/vlpds`. Esta carpeta no contiene código de
servicios: une los de cada repo (WatZappa, mubEZ, PARA) y agrega solo lo que
pertenece a la plataforma: perfiles, secretos, el edge, el relay, respaldos,
monitoreo y diagnóstico. No cambia nada de WatZappa.

> **Dos variantes conviven.** Esta es la de vlpds. La tradicional, con el PDS
> en TypeScript de WatZappa, está en [docs/tradicional/](docs/tradicional/README.md).
> En qué se diferencian y qué falta en vlpds: [docs/VLPDS.md](docs/VLPDS.md).
> Lo más importante: **las escrituras propias de PARA (`com.para.*`) todavía no
> funcionan con vlpds**.

```
                         internet (80/443)
                               │
                            Caddy ── HTTPS · certificados de handles bajo demanda · rutas de operador cerradas
   ┌─────────┬──────────┬──────┴───┬─────────┬──────────┬──────────────────┐
 <dominio>  pds.     appview. ×N  ozone.   bridge.     m8.        matrix. auth. idp. chat.
 app web    vlpds    AppView     Ozone   bridge     mubEZ     Synapse  MAS  para-idp Element
            │  firehose ▲ (dataplane)      ▲          │
            └──► relay ─┴──────────────────┘          │ Litestream
            │   (renumera los seq)                     │
            │                Postgres (una) · Redis (caché)
            │                     │ respaldos           │
            └─────────────────────┴─────────────────────┴──► bucket (S3/R2/GCS)
```

| | |
| --- | --- |
| **2** lugares con estado | El bucket (vlpds entero, mubEZ, respaldos) y Postgres. Perder un contenedor o su disco no pierde nada. |
| **1** comando para desarrollar | `make dev` levanta vlpds, PLC, relay, AppView y bsync; `make dev-smoke` lo prueba de punta a punta. |
| **1** comando para desplegar | `make up`, con `make preflight` antes y `make verify` después. |
| **2** tamaños | `tiny` (4 vCPU / 12 GB) y `standard` (8 vCPU / 32 GB / NVMe, AppView en réplicas, 64 shards de vlpds). |
| **0** secretos en variables | Cada secreto es un archivo; si llega por dos lados, el servicio no arranca. |
| **6** llaves para guardar offline | La KEK y la llave PLC envuelta de vlpds, más las de AppView, Ozone y mubEZ. |

## Empezar

```bash
make check        # valida todo sin levantar nada
make dev          # el entorno local
make dev-smoke    # cuenta → post → AppView → proxy de vlpds
make dev-down
```

El recorrido completo, con lo que debe salir en cada paso, está en
**[docs/LIBRETO.md](docs/LIBRETO.md)**.

## Producción

```bash
cp .env.example .env     # PARA_DOMAIN, bucket, perfil
sudo make secrets        # incluida la KEK y la llave PLC envuelta; guarda las offline
make matrix-init         # una vez, si usas Matrix
make preflight BUILD=1
make up BUILD=1          # construye las imágenes de WatZappa y del relay aquí
make verify
make invite              # la primera cuenta
```

| Perfil de Compose | Servicios |
| --- | --- |
| `core` | vlpds (`pds`), relay, Postgres, Redis, AppView (bsky + dataplane), bsync, Ozone, respaldos |
| `matrix` | Synapse, MAS, para-idp, Element, matrix-bridge, respaldo de media |
| `identity` | mubEZ (broker M8), replicando su base al bucket |
| `edge` | Caddy |
| `monitoring` | Alloy: métricas y logs a tu Prometheus/Loki |
| `local-s3` | Un S3 local para probar sin nube |
| `web` | La app web de PARA (bskyweb) en el dominio raíz |

## Documentación

| | |
| --- | --- |
| **[Libreto](docs/LIBRETO.md)** | Cada comando, en orden, con lo que debe salir; lo que hay que saber cada vez; las herramientas de DX |
| [vlpds frente a tradicional](docs/VLPDS.md) | Qué cambia, la brecha de `com.para.*`, por qué el relay, el fork, Spaces |
| [Desarrollo](docs/DESARROLLO.md) | `make dev`, mubEZ y la app contra el entorno local |
| [Despliegue](docs/DESPLIEGUE.md) | Una máquina de cero, a mano o con Ansible |
| [Configuración](docs/CONFIGURACION.md) | Perfiles, memoria, cada secreto, cada variable |
| [Operaciones](docs/OPERACIONES.md) | El ciclo del operador, la consola de vlpds, monitoreo |
| [Escalado](docs/ESCALADO.md) | Réplicas, workers de AppView, nodos de vlpds |
| [Respaldos](docs/RESPALDOS.md) | Qué es durable y dónde, las llaves offline, restaurar |
| [Actualizaciones](docs/ACTUALIZACIONES.md) | Cambiar de versión y migrar desde la variante tradicional |
| [Matrix](docs/MATRIX.md) | Synapse, MAS, para-idp y Element |
| [mubEZ](docs/MUBEZ.md) | El broker M8, en desarrollo y en producción |
| [App](docs/APP.md) | Las URLs que la app hornea al compilar y la app web |
| [Runbook](docs/RUNBOOK.md) | Una sección por alerta |
| [Tradicional](docs/tradicional/README.md) | La variante con el PDS en TypeScript, con [su libreto](docs/tradicional/LIBRETO.md) |

## Cómo está armado

- `core.yaml` define el núcleo. vlpds se llama `pds` para que el resto (edge,
  mubEZ, bridge) no tenga que saber qué PDS es. El AppView, bsync y Ozone se
  construyen desde el checkout de WatZappa (`WATZAPPA_DIR`, por defecto
  `../../WatZappa`).
- `compose.yaml` incluye `core.yaml`, `secrets.yaml`, el compose de Matrix de
  WatZappa y el de mubEZ, cada uno con su override de `overrides/`, más
  `ops/compose.yaml` y `web/compose.yaml`.
- Las imágenes de WatZappa no saben leer secretos `NAME_FILE`:
  `runtime/env-from-files.sh` se monta como su entrypoint y lo hace por ellas.
- El AppView y el bridge leen el firehose del relay, no de vlpds
  ([por qué](docs/VLPDS.md#4-por-qué-el-relay-es-obligatorio)).
- `matrix/` trae los scripts que generan la configuración de Matrix (escriben
  en `WatZappa/deploy/matrix`, de donde la monta su compose) y
  `edge/sites/matrix.Caddyfile` sus rutas.
- `dev/` es el entorno local; `runtime/bsky/` lleva las entradas del AppView
  que se montan mientras su imagen no se pueda construir.
- `make check` valida todo con Docker. Requiere Docker Compose v2.24 o
  superior (`include` con varios archivos y `!override`/`!reset`).
