# Escalado

```
              Caddy (principal)
     ┌──────────┼──────────────┬──────────────┐
  bsky ×N    bsky (worker 1)  bsky (worker 2)   ← AppView: sin estado propio
     │            │               │
     └────────────┴───── red privada ─────┐
                                          │
   PDS · bridge · mubEZ · Synapse ── Postgres ── bucket   ← estado: una sola principal
```

Crecer es como en vlpds: arrancar otro proceso apuntando al mismo estado. La
diferencia honesta es qué parte de PARA puede hacerlo:

| Servicio | ¿Escala horizontal? | Por qué |
| --- | --- | --- |
| AppView (bsky + dataplane) | **Sí**: réplicas y workers | Todo su estado está en Postgres |
| bsync, Ozone, para-idp | Sí en principio; hoy una instancia | Sin estado propio; aún no hace falta |
| PDS | No | El PDS de referencia guarda cuentas y repos en SQLite en disco y tiene un solo secuenciador |
| matrix-bridge | No | Consume el firehose; dos instancias procesarían todo dos veces |
| mubEZ | No, a propósito | Dos réplicas escribirían la misma réplica de Litestream |
| Synapse | No (sin workers de Synapse) | Una instancia alcanza para el piloto |

La carga que crece con los usuarios es sobre todo de lectura (timelines,
perfiles, búsquedas), y esa es justo la del AppView.

## En la misma máquina: réplicas

`BSKY_REPLICAS` y `DATAPLANE_REPLICAS` (el perfil `standard` usa 2 de cada
uno). Caddy reparte entre todas las réplicas (Docker DNS las resuelve) y salta
por 30 s una que rechace conexiones.

```bash
echo BSKY_REPLICAS=3 >> .env && make up
```

## En otra máquina: workers de AppView

Un worker corre bsky, dataplane y un Redis propio (es solo caché) contra el
Postgres de la principal, por la red privada (Tailscale o WireGuard).

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
WATZAPPA_VERSION=sha-…                        # la misma que la principal
```

y en su `secrets/` solo: `postgres_url` (apuntando a `PARA_PRIMARY_IP:5432`),
`bsky_service_signing_key`, `bsky_admin_passwords`, `authority_service_token`
y `civic_delegation_resolver_secret`. Luego `make up` en el worker y `make up`
en la principal (Caddy toma la nueva lista).

Con Ansible es agregar el host al grupo `para_appview` con su
`para_private_ip`: el rol escribe todo lo anterior, incluida la `postgres_url`
del worker, y no le manda ningún otro secreto.

Para quitar un worker: sácalo de `APPVIEW_WORKERS`, `make up` en la
principal, y `make down` en el worker.

La red privada es requisito: Postgres y bsync quedan publicados solo en la IP
privada de la principal. Limita en la ACL de Tailscale quién llega a los
puertos 5432 y 2586 de la principal (solo los workers).

## Cuándo crecer

- **Memoria**: `ParaMemoryLow` o contenedores reiniciando por OOM. Primero
  sube el techo del servicio en `.env`; si la máquina no da, `standard` o más
  máquina.
- **CPU del AppView** sostenida arriba de ~60 %: una réplica más o un worker,
  de modo que perder una instancia no sature al resto.
- **Conexiones de Postgres** (`ParaPostgresConnectionsHigh`): cada réplica y
  worker abre su propio pool. Sube `PG_MAX_CONNECTIONS` junto con la memoria
  de Postgres, o pon un PgBouncer delante si pasan de unos cientos.
- **Postgres mismo** es el límite final de esta forma: cuando lo sea, la
  siguiente palanca es un Postgres administrado más grande (con réplicas de
  lectura para el AppView), no más contenedores.
