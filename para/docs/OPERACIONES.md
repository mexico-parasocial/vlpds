# Operaciones

Para quien opera la plataforma. Lee [Despliegue](DESPLIEGUE.md) primero, y
[Respaldos](RESPALDOS.md) y el [Runbook](RUNBOOK.md) antes de necesitarlos.
El día a día con ejemplos está en el [libreto, paso 7](LIBRETO.md#7-día-a-día).

```
 preflight ──► up ──► verify ──► (alertas) ──► runbook
     ▲                                            │
     └──── actualizar / escalar / restaurar ◄─────┘
```

## Tres cosas que distinguen esta plataforma

1. **El estado vive en dos lugares: el bucket y Postgres.** vlpds guarda en el
   bucket cada commit antes de responder; mubEZ manda cada cambio con
   Litestream en ~1 s; los respaldos copian Postgres y el media de Synapse cada
   `BACKUP_INTERVAL`. Los contenedores son reemplazables.
2. **Lo que no está bien no arranca.** Un secreto que falta, vacío, débil o
   que llega por dos lados detiene el contenedor con un mensaje claro. vlpds
   además se detiene a propósito (exit 2–9) si pierde su lease o el bucket, y
   espera que Docker lo levante de nuevo. Es más fácil arreglar un servicio que
   no arrancó que uno que arrancó mal.
3. **Hay un relay en medio.** El AppView y el bridge no leen a vlpds: leen al
   relay. Si el AppView deja de indexar, el relay es lo primero que se revisa.

## Comandos

| | |
| --- | --- |
| `make preflight` | Antes de levantar: configuración, secretos, bucket (escribe, lee, lista, borra; no público), DNS, Compose |
| `make up` | Levanta y espera a que todo esté sano |
| `make verify` | Después: contenedores, HTTPS, rutas de operador cerradas, `/tls-check`, relay, respaldos |
| `make doctor` | Diagnóstico rápido de configuración y endpoints |
| `make ps` / `make logs S=…` | Estado y logs |
| `make vlpds-admin ARGS="…"` | Comandos de operador de vlpds (`cluster status`, `account list`, `request-crawl …`) |
| `make invite` | Código de invitación |
| `make relay-add-pds` | Vuelve a pedir al relay que siga a vlpds |
| `make backup` / `make restore ARGS=…` | Respaldo inmediato de Postgres / restaurar del bucket |
| `make compose ARGS="…"` | Cualquier comando de Compose con la configuración de la plataforma |

## La consola de vlpds

vlpds trae una consola web de operador (`/admin`): cuentas, moderación,
firehose, relays, cuotas, layout de shards. **Nunca sale por el edge**: Caddy
responde 404 a `/admin`, `/xrpc/vlpds.admin.*`, `/metrics` e `/internal/*`
para todos. Detrás de Docker, Caddy puede ver a cualquier cliente con la IP del
bridge de Docker, así que filtrar por red no protegería nada.

La consola escucha en su propio puerto, publicado solo en el loopback del
servidor (`127.0.0.1:2620`, `VLPDS_ADMIN_PORT`):

```bash
ssh -L 2620:127.0.0.1:2620 tu-servidor
# http://localhost:2620/admin, con el token de secrets/vlpds_admin_token
```

o con `tailscale serve` en la red privada, como recomienda vlpds.

## Monitoreo

Con el perfil `monitoring`, Alloy manda a tu stack compatible con
Prometheus/Loki (`PARA_METRICS_URL`, `PARA_LOGS_URL`) las métricas del host,
de Postgres, del edge, del bridge, de los respaldos, de vlpds (`:9583`) y del
relay (`:2471`), más los logs de todos los contenedores. Descubre las
métricas por la etiqueta `para.metrics.port` de cada servicio.

Carga [ops/alerts.yml](../ops/alerts.yml) en tu Prometheus/Mimir; cada alerta
tiene su sección en el [Runbook](RUNBOOK.md). vlpds documenta las suyas
(`VlpdsFirehoseConsumersTooSlow`, leases, bucket) en
[docs/operations/monitoring.md](../../docs/operations/monitoring.md).

Lo que conviene mirar en un tablero:

- `caddy_reverse_proxy_upstreams_healthy` por servicio: lo que ven los usuarios.
- Tasa de 5xx del edge.
- `para_matrix_firehose_lag_seconds`: qué tan atrás va el bridge del relay.
- `time() - para_backup_last_success_timestamp_seconds` por objetivo.
- Memoria y disco del host; conexiones de Postgres contra su máximo.

Una sonda externa (un servicio de uptime contra
`https://pds.PARA_DOMAIN/xrpc/_health`) cubre el caso que nada de adentro ve:
la máquina entera caída.
