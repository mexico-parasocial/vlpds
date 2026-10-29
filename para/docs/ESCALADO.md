# Escalado

```
              Caddy (principal)
     ┌──────────┼──────────────┬──────────────┐
  bsky ×N    bsky (worker 1)  bsky (worker 2)   ← AppView: sin estado propio
     │            │               │
     └────────────┴───── red privada ─────┐
                                          │
   vlpds · relay · dataplane · bridge · mubEZ · Synapse ── Postgres ── bucket
```

| Servicio | ¿Escala horizontal? | Por qué |
| --- | --- | --- |
| AppView (bsky) | **Sí**: réplicas y workers | Todo su estado está en Postgres |
| vlpds | **Sí, por diseño** (aún sin compose para un segundo nodo) | Varios nodos sobre el mismo bucket, con leases y shards ([abajo](#vlpds-con-más-de-un-nodo)) |
| dataplane | Uno que indexa; los workers llevan otro que solo sirve consultas | Desde WatZappa#13, el dataplane también indexa el firehose: dos indexarían todo dos veces |
| bsync, Ozone, para-idp | Sí en principio; hoy una instancia | Sin estado propio; aún no hace falta |
| relay | No hace falta | Un proceso chico aguanta el firehose de un PDS |
| matrix-bridge | No | Consume el firehose; dos instancias procesarían todo dos veces |
| mubEZ | No, a propósito | Dos réplicas escribirían la misma réplica de Litestream |
| Synapse | No (sin workers de Synapse) | Una instancia alcanza para el piloto |

La carga que crece con los usuarios es sobre todo de lectura (timelines,
perfiles, búsquedas), y esa es justo la del AppView.

## En la misma máquina: réplicas

`BSKY_REPLICAS` (el perfil `standard` usa 2). Caddy reparte entre todas las
réplicas (el DNS de Docker las resuelve) y salta por 30 s una que rechace
conexiones.

```bash
echo BSKY_REPLICAS=3 >> .env && make up
```

## En otra máquina: workers de AppView

Un worker corre bsky, un dataplane que **solo sirve consultas** y un Redis
propio, contra el Postgres de la principal, por la red privada (Tailscale o
WireGuard). El dataplane que indexa el firehose sigue siendo el de la
principal.

**En la principal** (`.env`):

```bash
PARA_PRIVATE_BIND=100.64.0.10                 # su IP privada: ahí se publican Postgres y bsync
APPVIEW_WORKERS=100.64.0.11:2584 100.64.0.12:2584
```

**En cada worker** (`.env`, con la misma revisión de los repos):

```bash
PARA_ROLE=appview-worker
PARA_DOMAIN=example.org
PARA_PRIVATE_BIND=100.64.0.11                 # su IP privada
PARA_PRIMARY_IP=100.64.0.10
```

y en su `secrets/` solo: `postgres_url` (apuntando a `PARA_PRIMARY_IP:5432`),
`bsync_api_key`, `bsky_service_signing_key`, `bsky_admin_passwords`,
`authority_service_token` y `civic_delegation_resolver_secret`. Luego
`make up` en el worker y `make up` en la principal (Caddy toma la nueva
lista).

Con Ansible es agregar el host al grupo `para_appview` con su
`para_private_ip`: el rol escribe todo lo anterior y no le manda ningún otro
secreto.

Para quitar un worker: sácalo de `APPVIEW_WORKERS`, `make up` en la
principal, y `make down` en el worker. Limita en la ACL de la red privada
quién llega a los puertos 5432 y 2586 de la principal (solo los workers).

## vlpds con más de un nodo

vlpds está hecho para correr en varios nodos sobre el mismo bucket: cada nodo
tiene un `VLPDS_NODE_ID` estable, los shards se reparten con leases en el
bucket y, si un nodo cae, otro toma sus shards en lo que dura un lease (10 s
en `standard`). Los nodos se hablan por mTLS (`--peer-listen`,
`--peer-tls-dir`, `--advertise-url`) y cualquiera sirve el firehose con los
mismos cursores.

El fork todavía no trae un compose para el segundo nodo: hoy el perfil
`standard` deja un nodo con 64 shards, listo para crecer. El procedimiento
completo está en
[docs/operations/scaling-and-clustering.md](../../docs/operations/scaling-and-clustering.md).
Dos reglas que no cambian:

- **Nunca dos nodos con el mismo `VLPDS_NODE_ID`** sobre el mismo prefijo: se
  bloquean entre sí.
- **NTP en cada máquina**: un reloj atrasado retrasa el firehose de todos.

## Cuándo crecer

- **Memoria**: `ParaMemoryLow` o contenedores reiniciando por OOM. Primero
  sube el techo del servicio en `.env`; vlpds dimensiona sus cachés con el
  suyo (`VLPDS_MEM`). Si la máquina no da, `standard` o más máquina.
- **CPU del AppView** sostenida arriba de ~60 %: una réplica más o un worker.
- **Conexiones de Postgres** (`ParaPostgresConnectionsHigh`): cada réplica y
  worker abre su propio pool. Sube `PG_MAX_CONNECTIONS` junto con la memoria
  de Postgres, o pon un PgBouncer delante.
- **Postgres mismo** es el límite final de esta forma: la siguiente palanca
  es un Postgres administrado más grande, con réplicas de lectura para el
  AppView.
