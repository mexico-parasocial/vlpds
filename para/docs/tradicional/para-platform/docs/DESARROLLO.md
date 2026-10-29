# Desarrollo

Todo el entorno local con un comando, en una terminal.

```
make dev
  ├─ m8       mubEZ (pnpm dev)               :8787   PLC/PDS locales por entorno
  └─ atproto  WatZappa (make run-dev-env)    :2581 introspección · :2582 PLC
                                             :2583 PDS · :2584 AppView · :2590 chat
                                             :3001 matrix-bridge
              Postgres :5434 y Redis :6381 en Docker (se quedan arriba al salir)
```

## Primera vez

```bash
git clone … para-platform && git clone … WatZappa && git clone … mubEZ   # carpetas hermanas
cd para-platform
make setup
make dev
```

`make setup` revisa Docker, Node (con nvm cada repo usa su `.nvmrc`: WatZappa
22, mubEZ 24), pnpm, jq y compañía; instala dependencias donde falten o el
lockfile cambió; y crea `mubEZ/.env` con secretos de desarrollo. Se puede
repetir: no toca lo que ya está bien.

Si los repos no son carpetas hermanas, pon sus rutas en `.env`
(`WATZAPPA_DIR`, `MUBEZ_DIR`, `PARA_DIR`).

## El día a día

| Comando | Qué hace |
| --- | --- |
| `make dev` | Levanta todo, espera a que cada servicio responda, imprime las URLs y muestra los logs de todos con prefijo. Ctrl-C detiene todo. |
| `make dev APP=web` | Además la app (`web`, `ios`, `android` o `expo`). La app se queda con la terminal; los logs del backend van a `.dev/logs/`. |
| `make dev MATRIX=1` | Además Synapse, MAS y Element en Docker. |
| `make dev FRESH=1` | Perfil desechable: nada persiste al salir (como `just dev` de vlpds). Sin bridge. |
| `make seed` | Cuentas de prueba (`WatZappa/test-accounts.json`) y memes. |
| `make dev-logs S=atproto` | Sigue un log (`atproto`, `m8`; sin `S`, todos). |
| `make doctor` | Qué está mal y el comando que lo arregla. |
| `make e2e-dm` | Prueba punta a punta de los mensajes directos por Matrix: Synapse desechable, rutas reales del bridge, cifrado real. No necesita `make dev`. |
| `make dev-app-env` | Apunta `PARA/.env.local` a esta Mac: IP, URLs del broker y del bridge, DID del AppView. |
| `make dev-down` | Apaga Postgres/Redis de desarrollo. |
| `make dev-reset` | Borra los datos locales persistentes (pide confirmación). |

`make dev` no edita ningún `.env`: a mubEZ le pasa `PLC_URL` y `PDS_URL`
locales por entorno, que le ganan a su `.env`.

## La app en un teléfono físico

En el teléfono, `localhost` es el teléfono. Con `make dev` arriba:

```bash
make dev-app-env               # escribe la IP y el DID en PARA/.env.local
cd ../PARA && npx expo start --clear
```

Repítelo cuando cambies de red (otra IP) o después de `make dev-reset` (otro
DID). `make doctor` avisa en ambos casos.

## Si algo falla

| Síntoma | Causa y arreglo |
| --- | --- |
| `Puertos ocupados` al arrancar | Otro `make dev` (o un proceso viejo). El mensaje trae el `lsof` para verlo. |
| `could not resolve proxy did` al hacer login | La app hace proxy a un DID que no es el del AppView en ejecución. `make dev-app-env` y Metro con `--clear`. |
| La app no conecta desde el teléfono | IP vieja en `.env.local`: `make dev-app-env`. |
| mubEZ inventa DIDs para los handles | Está resolviendo contra el PLC público. Arráncalo con `make dev` (le pasa el local). |
| Corre código viejo después de cambiar algo | `make doctor` → "Builds". `make dev` recompila lo que WatZappa necesita al arrancar. |
| Un proceso se cae | `make dev` detiene el resto y muestra sus últimas 25 líneas; el log completo está en `.dev/logs/`. |

## Probar la plataforma de producción en tu máquina

Sin nube, con el perfil `local-s3` (un S3 de SeaweedFS):

```bash
cp .env.example .env    # PARA_DOMAIN=para.test, PROFILES=core,identity,local-s3,
                        # S3_ENDPOINT=http://s3-local:8333, S3_BUCKET=para-test
make secrets            # y crea a mano s3_access_key_id / s3_secret_access_key (cualquier valor)
make up BUILD=1         # construye las imágenes aquí (tarda: compila WatZappa)
```

El edge necesita DNS real, así que en local se deja fuera. Si en esta máquina
ya corre otro stack con los mismos puertos, usa otro nombre de proyecto y
puerto de Postgres: `COMPOSE_PROJECT_NAME=paratest POSTGRES_PORT=15432 make up …`.
