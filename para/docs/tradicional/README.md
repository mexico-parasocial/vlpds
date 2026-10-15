# PARA tradicional: WatZappa con su PDS en TypeScript

Cómo corre PARA sin vlpds, tal como funcionaba antes del fork y como sigue
funcionando hoy en el repo WatZappa. Se conserva aquí porque las dos variantes
conviven mientras decidimos cuál usar ([VLPDS.md](../VLPDS.md)) y porque
para-platform, donde vivía parte de esta documentación, se va a archivar.

Libreto de comandos de esta variante: [LIBRETO.md](LIBRETO.md).

---

## Las piezas

```
                    Caddy (80/443) ── HTTPS y rutas públicas
   ┌───────────┬──────────────┼──────────────┬──────────────┬──────────────┐
  pds.       appview.        ozone.        bridge.       matrix./auth./   m8.
  PDS (TS)   AppView         Ozone         matrix-bridge idp./chat.       mubEZ
   │  firehose ▲ (dataplane)                 ▲ firehose   Synapse, MAS,   (broker M8)
   └───────────┴─────────────────────────────┘            para-idp, Element
        Postgres · Redis · bucket (blobs del PDS)
```

| Pieza | Repo | Qué hace |
|---|---|---|
| PDS | WatZappa (`packages/pds`) | Cuentas, repos, blobs, OAuth, y las rutas y reglas propias de PARA (`com.para.*`, boletas, delegaciones) |
| AppView | WatZappa (`packages/bsky`) | Feeds, perfiles, comunidades, gobierno, mapa cívico; `api.js` sirve, `dataplane.js` indexa el firehose del PDS |
| bsync | WatZappa | Mutes y notificaciones del AppView |
| Ozone | WatZappa | Moderación |
| Matrix | WatZappa (`docker-compose.matrix.yaml`, `deploy/matrix/`) | Synapse, MAS (logins), para-idp (prueba la identidad de PARA), Element |
| matrix-bridge | WatZappa (`services/matrix-bridge`) | Comunidades → espacios de Matrix, DMs con consentimiento, moderación |
| mubEZ | mubEZ | Broker M8: identidad, INE, ZKP, firmas para el bridge ([MUBEZ.md](../MUBEZ.md)) |
| iM8 | iM8 | La billetera que aprueba firmas |
| PARA | PARA | La app (iOS, Android, web) |

En esta variante el AppView y el bridge leen el firehose **directo del PDS**:
sus `seq` son consecutivos desde 1 y no hace falta relay.

## Tres formas de correrla

| Forma | Para qué | Dónde está |
|---|---|---|
| **Desarrollo** | Tu máquina: PDS, AppView, Ozone, chat y bridge en un proceso | `make run-dev-env` en WatZappa ([LIBRETO.md §2](LIBRETO.md#2-desarrollo)) |
| **WatZappa por su cuenta** | Producción en un host, con Ansible: dos stacks (app y Matrix) detrás de Caddy | `WatZappa/ansible/` ([LIBRETO.md §5](LIBRETO.md#5-producción-con-el-ansible-de-watzappa)) |
| **para-platform** (archivado) | La plataforma con perfiles, secretos como archivos, respaldos al bucket y Alloy, pensada para esta variante | [para-platform/](para-platform/README.md) (copia de su documentación) |

### Sobre la copia de para-platform

[para-platform/](para-platform/README.md) es su README y su `docs/` tal como
estaban en la rama `infra/vlpds-style` (commit `4d6658f`). Ojo con dos cosas:

- **Dependía de cambios en WatZappa que nunca se mezclaron** (el PR #10, que
  cerramos): imágenes con secretos `_FILE`, Caddyfiles de producción y
  `setup-appservice.sh`. Contra el `main` actual de WatZappa no arranca tal
  cual. Lo que sí funciona hoy está en el fork (`vlpds/para`), que trae esas
  piezas por su cuenta pero con vlpds como PDS.
- Sus enlaces a `../WatZappa/…` asumen que para-platform era hermano de
  WatZappa; desde aquí no resuelven.

Sirve como referencia de cómo operar la variante tradicional con las
mismas ideas (perfiles, preflight, verify, runbook) si algún día se decide
quedarse con el PDS en TypeScript.

## Diferencias con la variante vlpds

Resumen ([VLPDS.md](../VLPDS.md) tiene la tabla completa):

- **Funciona todo lo de PARA**: las rutas `com.para.*` de escritura (crear
  comunidad, unirse, votar, factor de correo) solo existen en este PDS.
- **El PDS no escala horizontalmente** y su estado (SQLite por repo, cuentas,
  llaves) vive en un volumen local que hay que respaldar.
- **No necesita relay** ni la KEK.
- **512 MB** de memoria para el PDS, frente a los 2 GB de vlpds.

## Documentos de WatZappa que conviene conocer

Están en el repo WatZappa:

| | |
|---|---|
| `ansible/README.md` | El despliegue de un host: qué instala, cómo verifica, secretos |
| `deploy/matrix/README.md`, `GUIA_ADMIN_ES.md`, `GUIA_USUARIO_ES.md`, `RUNBOOK_SECURITY_ES.md` | Matrix: puesta en marcha, administración, uso y seguridad |
| `deploy/bare-metal/README.md` | Sin Docker, con systemd |
| `services/bsky/README.md` | Cómo corren `api.js` y `dataplane.js` |
| `AGENTS.md` | Reglas del código: boletas, autoridad, Influence |
| `docs/QUARTER_PLAN_2026Q4.md` | El plan del piloto y su estado |
