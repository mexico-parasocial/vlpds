# para-platform

Punto de entrada único para desarrollar, levantar y operar la plataforma PARA.
Este repo **no contiene código de servicios**: une los que ya viven en cada
repo y agrega solo lo que pertenece a la plataforma (perfiles, secretos, el
edge, respaldos, monitoreo y diagnóstico).

```
                         internet (80/443)
                               │
                            Caddy ── HTTPS · certificados de handles bajo demanda · rutas de operador cerradas
   ┌─────────┬──────────┬──────┴───┬─────────┬──────────┬──────────────────┐
 <dominio>  pds.     appview. ×N  ozone.   bridge.     m8.        matrix. auth. idp. chat.
 app web    PDS      AppView     Ozone   bridge     mubEZ     Synapse  MAS  para-idp Element
 (bskyweb)  + handles                                │          │
               │          │          │       │       Litestream    │
               └──────────┴────┬─────┴───────┴───────────┼─────────┘
                    Postgres (una sola)   Redis (caché)  │
                          │ respaldos                    │ WAL en ~1 s
                          └──────────────────────────────┴──► bucket (S3/R2/GCS)
```

| | |
| --- | --- |
| **2** lugares con estado | Postgres y un bucket. Todo lo demás es caché o se reconstruye: perder un contenedor o su disco no pierde nada. |
| **1** comando para desarrollar | `make dev` levanta PLC, PDS, AppView, chat, bridge y el broker M8 en una terminal. |
| **1** comando para desplegar | `make up` descarga imágenes ya construidas por CI; nada compila en el servidor. |
| **2** tamaños | `tiny` (una VM de 4 vCPU / 8 GB) y `standard` (8 vCPU / 32 GB, AppView en réplicas). |
| **0** secretos en variables | Cada secreto es un archivo; si llega por dos lados, el servicio no arranca. |
| **5** llaves para guardar offline | El resto vive en el bucket o se regenera. |

## Desarrollo

```bash
make setup     # una vez: revisa herramientas, instala dependencias, crea mubEZ/.env
make dev       # todo el backend en una terminal; Ctrl-C lo detiene todo
```

`make dev APP=web` (o `ios`, `android`) arranca también la app. `MATRIX=1`
agrega Synapse/MAS; `FRESH=1` usa un perfil desechable. Cuando algo falla:
`make doctor`, que dice qué está mal y el comando que lo arregla. Más en
[docs/DESARROLLO.md](docs/DESARROLLO.md).

## Producción

```bash
cp .env.example .env     # PARA_DOMAIN, bucket, versiones fijadas
make secrets             # genera los secretos (o make secrets-import desde los .env de antes)
make matrix-init         # una vez, si usas Matrix (docs/MATRIX.md)
make preflight           # configuración, secretos, bucket y DNS: antes de levantar nada
make up                  # descarga y levanta
make verify              # contenedores, endpoints públicos, respaldos
```

Con Ansible es lo mismo, de una máquina vacía a verificada:
[deploy/ansible/README.md](deploy/ansible/README.md).

Lo propio de PARA tiene su comando: `make invite` (primera cuenta),
`make matrix-admin-token` (admin del chat), `make app-env` (URLs para
compilar la app).

| Perfil de Compose | Servicios |
| --- | --- |
| `core` | Postgres, Redis, PDS, AppView (bsky + dataplane), bsync, Ozone, respaldos |
| `matrix` | Synapse, MAS, para-idp, Element, matrix-bridge, respaldo de media |
| `identity` | mubEZ (broker M8), replicando su base al bucket |
| `edge` | Caddy |
| `monitoring` | Alloy: métricas y logs a tu Prometheus/Loki |
| `local-s3` | Un S3 local para probar sin nube |
| `web` | La app web de PARA (bskyweb) en el dominio raíz |

## Documentación

| | |
| --- | --- |
| [Desarrollo](docs/DESARROLLO.md) | `make dev`, la app en el teléfono, datos de prueba, problemas comunes |
| [Operaciones](docs/OPERACIONES.md) | El ciclo del operador y por dónde empezar |
| [Despliegue](docs/DESPLIEGUE.md) | Una máquina de cero, a mano o con Ansible |
| [Configuración](docs/CONFIGURACION.md) | Perfiles, memoria, cada secreto, cada variable |
| [Escalado](docs/ESCALADO.md) | Réplicas, workers de AppView, cuándo crecer |
| [Respaldos](docs/RESPALDOS.md) | Qué es durable y dónde, restaurar, las llaves offline |
| [Actualizaciones](docs/ACTUALIZACIONES.md) | Cambiar de versión y migrar desde el despliegue anterior |
| [Matrix](docs/MATRIX.md) | Synapse, MAS, para-idp y Element: hosts, login, primera vez |
| [App](docs/APP.md) | Las URLs que la app hornea al compilar y la app web |
| [Runbook](docs/RUNBOOK.md) | Una sección por alerta |
| [vlpds](docs/VLPDS.md) | Qué tomamos de vlpds, el fork, y qué hacer con Spaces |

## Cómo está armado

- `compose.yaml` incluye `WatZappa/docker-compose.prod.yaml`,
  `WatZappa/docker-compose.matrix.yaml` y `mubEZ/docker-compose.yml`, cada uno
  con un override de `overrides/`, más `ops/compose.yaml`. No se copia nada:
  si un repo cambia su compose, la plataforma lo toma.
- Todo queda en un solo proyecto de Compose (`para`), así que los servicios se
  ven por nombre (el bridge llama a `http://mubez:8787/v1`).
- El tamaño sale de `profiles/<PARA_PROFILE>.env`; cualquier valor se
  sobrescribe en `.env`.
- `make check` valida este repo (scripts, Compose en los dos perfiles,
  Caddyfile, alertas, Alloy, Ansible) usando solo Docker.
- Requiere Docker Compose v2.24 o superior (`include` con varios archivos y
  `!override`/`!reset`).

### Seguridad de mubEZ

El `docker-compose.yml` de mubEZ es de desarrollo: fija `NODE_ENV=development`
y trae secretos por defecto. En development el broker entrega un token M8 para
**cualquier DID sin pruebas** (dev-token bootstrap). `overrides/identity.yaml`
reemplaza por completo ese bloque: fuerza producción, apaga el dev-token
bootstrap, toma los secretos de `secrets/` y no publica el puerto 8787 en el
host (solo Caddy habla con el broker).

### Autoridad de comunidades

`secrets/authority_service_token` es la credencial dedicada que comparten el
AppView y el bridge. El bridge consulta `https://appview.PARA_DOMAIN` por
defecto (`BRIDGE_AUTHORITY_APPVIEW_URL` lo cambia); el resolver exige HTTPS
salvo loopback. Durante una actualización, detén el bridge anterior, aplica
las migraciones de autoridad del AppView y después levanta el bridge nuevo.
Una respuesta de salud no acredita autoridad: el procedimiento y el estado
están en
[el registro de autoridad](../WatZappa/docs/TRUSTED-AUTHORITY-EXECUTION-2026-10-05.md).

### Aceptación del login de PARA

La plataforma ya habilita el handoff OAuth y apaga el bootstrap de
desarrollo. Ajusta `M8_CORS_ORIGIN` y `M8_OAUTH_RETURN_TO_ALLOWLIST` en `.env`
si el origen web no es `https://PARA_DOMAIN`. Aplica primero las migraciones
040–041 del broker, después de respaldar su base (`make backup`). Antes de
probar los clientes:

```bash
make login-preflight WEB_ORIGIN=https://ORIGEN_WEB_REAL
```

El preflight revisa el entorno efectivo, las migraciones en modo lectura, los
metadatos públicos, CORS y la ruta web `/m8-auth`; no crea sesiones ni emite
tokens. El estado del piloto está en
[la cola canónica](../WatZappa/docs/QUARTER_PLAN_2026Q4.md), con evidencia en
[el registro de login](../WatZappa/docs/M8-LOGIN-EXECUTION-2026-10-05.md).
