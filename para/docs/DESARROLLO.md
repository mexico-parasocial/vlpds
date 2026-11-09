# Desarrollo

El entorno local con vlpds, en Docker, con un comando. El paso a paso con lo
que debe salir está en el [libreto](LIBRETO.md#2-entorno-local); aquí, cómo
está armado y cómo conectarle mubEZ y la app.

```
make dev
  net        un contenedor que solo presta su red: localhost:<puerto> es lo
             mismo dentro de cada servicio y desde tu Mac
  ├─ plc        :2582  did-method-plc, construido desde el commit que usan los bancos de prueba de vlpds
  ├─ vlpds      :2583  --memory --dev-mode (nada persiste)
  ├─ relay      :2470  vlRelay (en memoria), renumera el firehose de vlpds
  ├─ dataplane  :2585  indexa el firehose del relay en Postgres
  ├─ appview    :2584  api.js
  ├─ bsync      :2586  construido desde WatZappa main
  └─ postgres          tmpfs, una base por servicio
```

| Comando | Qué hace |
| --- | --- |
| `make dev` | Genera las llaves de desarrollo en `.dev/dev.env` la primera vez, levanta todo, espera a que responda y registra vlpds en el relay |
| `make dev-smoke` | Cuenta → post → AppView → proxy de vlpds |
| `make dev-down` | Apaga y borra todo |
| `make e2e-dm` | DMs cifrados por Matrix de punta a punta, aislados (no usan vlpds) |
| `make check` | Valida la plataforma sin levantar nada |

Variables útiles al levantar:

| Variable | Por defecto | Para qué |
| --- | --- | --- |
| `VLPDS_IMAGE` | `ghcr.io/jazware/vlpds:1.1.0` | Probar otra versión de vlpds |
| `BSKY_IMAGE` | `para-core-bsky:latest` | La imagen local del AppView (hasta que se construya desde WatZappa) |
| `BSYNC_IMAGE` | `para-bsync:<commit>` | La imagen de bsync |
| `DATAPLANE_LOG_LEVEL` | `info` | `debug` para ver cada evento indexado |

## Por qué así

- **Una sola red para todos.** Las URLs que quedan en los documentos DID
  (`http://localhost:2583`, `did:web:localhost%3A2584`) tienen que funcionar
  desde vlpds, el AppView, el relay y tu navegador. Con un contenedor que
  presta su red, `localhost` es el mismo en todas partes, como en los bancos de
  prueba de vlpds.
- **El AppView desactiva la protección SSRF** (`BSKY_DISABLE_SSRF_PROTECTION`)
  porque si no, se niega a resolver DIDs contra el PLC en `localhost`. Solo en
  desarrollo.
- **El relay sigue el PDS configurado.** vlRelay lee `localhost:2583`
  (`--host`, `dev/compose.vlrelay.yaml`) y resuelve DIDs contra el PLC local.
  La primera vez `make dev` construye su imagen desde el commit fijado.
  `make dev PARA_RELAY=rsky` usa rsky ([RSKY-RELAY.md](RSKY-RELAY.md)).
- **Nada persiste.** Es más fácil empezar de cero que reparar un estado
  local. Las llaves (rotación PLC y firma del AppView) sí se guardan en
  `.dev/dev.env` para no cambiar de identidad en cada arranque.

## mubEZ contra el entorno local

Desde `mubEZ/`, con Node 24 ([MUBEZ.md](MUBEZ.md)):

```bash
PLC_URL=http://localhost:2582 PDS_URL=http://localhost:2583 pnpm dev
```

## La app contra el entorno local

La app necesita el PDS y el DID del AppView. En `PARA/.env.local`:

```bash
EXPO_PUBLIC_LOCAL_DEV_SERVICE=http://<IP de tu Mac>:2583
EXPO_PUBLIC_LOCAL_BSKY_PROXY_DID=did:web:localhost%3A2584
```

En un teléfono físico, `localhost` es el teléfono: usa la IP de tu Mac
(`ipconfig getifaddr en0`) y reinicia Metro con `--clear` cuando cambie. Ojo:
el DID del AppView dice `localhost`, así que desde el teléfono las peticiones
que vlpds reenvía funcionan (vlpds está en tu Mac), pero no las que la app
haga directo al AppView por su DID.

El fork nativo permite crear comunidades, unirse y votar (con verificador M8).
Para invitaciones de fundadores, factores iM8 y alpha, aún hace falta la variante
tradicional. Consulta [Escrituras nativas](NATIVE-PARA-WRITES.md).

## Probar la producción en tu máquina

El perfil `core` de producción sobre un S3 local:
[libreto, paso 4](LIBRETO.md#4-la-producción-en-tu-máquina).
