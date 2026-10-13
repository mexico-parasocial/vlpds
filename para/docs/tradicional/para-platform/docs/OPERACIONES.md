# Operaciones

Para quien opera una plataforma PARA, desde un piloto en una VM chica hasta
varias máquinas. Lee [Despliegue](DESPLIEGUE.md) primero, y
[Respaldos](RESPALDOS.md) y el [Runbook](RUNBOOK.md) antes de necesitarlos.

```
 preflight ──► up ──► verify ──► (alertas) ──► runbook
     ▲                                            │
     └──── upgrade / escalar / restaurar ◄────────┘
```

El ciclo es corto a propósito. Desplegar es `make up` con imágenes fijadas;
actualizar es cambiar el tag y repetir; escalar es subir réplicas o sumar un
worker; una alerta trae el nombre de su sección en el runbook.

## Dos cosas que distinguen esta plataforma

1. **El estado vive en dos lugares: Postgres y el bucket.** Los contenedores
   son reemplazables. mubEZ ni siquiera depende de su disco: Litestream manda
   cada cambio de su base al bucket en ~1 s y la restaura sola en una máquina
   nueva. El PDS y Synapse sí escriben en disco, y los respaldos los copian al
   bucket cada `BACKUP_INTERVAL` (ver [Respaldos](RESPALDOS.md)).
2. **Lo que no está bien no arranca.** Un secreto que falta, vacío o que llega
   por dos lados detiene el contenedor con un mensaje claro. Preflight se
   niega a levantar sin bucket, sin versiones fijadas o con un bucket público.
   Es más fácil arreglar un servicio que no arrancó que uno que arrancó mal.

## Comandos

| | |
| --- | --- |
| `make preflight` | Antes de levantar: configuración, secretos, bucket (escribe/lee/lista/borra, no público), DNS, Compose |
| `make up` | Descarga y levanta; espera a que todo esté sano |
| `make verify` | Después: contenedores, endpoints públicos con HTTPS, replicación de mubEZ, respaldos |
| `make upgrade WATZAPPA_VERSION=sha-…` | Cambia de versión y verifica |
| `make ps` / `make logs S=…` | Estado y logs |
| `make backup` / `make restore ARGS=…` | Respaldo inmediato / restaurar del bucket |
| `make doctor-prod` | Diagnóstico rápido de configuración y endpoints |
| `make compose ARGS="…"` | Cualquier comando de Compose con la configuración de la plataforma |

## Monitoreo

Con el perfil `monitoring`, Alloy manda a tu stack compatible con
Prometheus/Loki (`PARA_METRICS_URL`, `PARA_LOGS_URL`) las métricas del host,
de Postgres, del edge (incluida la salud de cada servicio detrás de Caddy),
del bridge y de los respaldos, más los logs de todos los contenedores. Lee los
logs de Docker por un proxy que solo permite listar contenedores y leer logs.

Carga [ops/alerts.yml](../ops/alerts.yml) en tu Prometheus/Mimir: 13 alertas,
6 de página y 7 de ticket, cada una con su sección en el
[Runbook](RUNBOOK.md).

Lo que conviene mirar en un tablero:

- `caddy_reverse_proxy_upstreams_healthy` por servicio: lo que ven los usuarios.
- Tasa de 5xx del edge.
- `time() - para_backup_last_success_timestamp_seconds` por objetivo.
- Memoria y disco del host; conexiones de Postgres contra su máximo.
- `para_matrix_firehose_lag_seconds`: qué tan atrás va el bridge.

Una sonda externa (un servicio de uptime contra
`https://pds.PARA_DOMAIN/xrpc/_health`) cubre el caso que nada de adentro ve:
la máquina entera caída.
