# PARA y vlpds

Qué tomamos de [vlpds](https://vlpds.jazco.dev/docs/overview), qué no, cómo seguimos
su desarrollo con el fork, y qué hacer con **Spaces** (datos con permisos), que llegó
en vlpds 1.1.0.

Escrito el 2026-10-11. Nada de lo que describe está en commits todavía: son cambios
locales en ramas `infra/vlpds-style`.

---

## 1. En corto

- No corremos vlpds. PARA sigue usando su PDS en TypeScript (WatZappa). De vlpds
  tomamos **la forma de operar**: un solo comando para desarrollar, perfiles chicos,
  secretos como archivos, revisar antes de levantar y comprobar después, alertas con
  runbook.
- El fork `mexico-parasocial/vlpds` es una **copia de referencia** que se sincroniza
  sola cada día. Sirve para leer qué cambia arriba, no para compilar nada nuestro.
- **No implementé Spaces.** No existía cuando hice el trabajo de infraestructura, y
  aunque ahora existe, sigue en alfa (cambios incompatibles cada semana). Abajo está
  qué hay en WatZappa, por qué la opción del "sidecar" no funciona, y lo que
  recomiendo.

---

## 2. Repos

Cinco repos, un worktree por repo, todos en la rama `infra/vlpds-style`. Sin commits.

| Repo | Base | Cambios | Qué contiene |
|---|---|---|---|
| **para-platform** | `d6c23de` | 11 modificados, 58 nuevos | La plataforma al estilo vlpds: `make dev`, perfiles `tiny`/`standard`, compose por capas, Caddy, respaldos, Alloy y alertas, Ansible, docs en español, `make e2e-dm`. Remoto: `github.com/mexico-parasocial/para-platform` (vacío hasta el primer push). |
| **WatZappa** | `0caa89ea9` (main) | 137 modificados, 62 nuevos | Imágenes con secretos `_FILE`, workflow de imágenes, Caddyfiles de producción, scripts de Matrix (`setup-appservice.sh`), `/tls-check` en el PDS, solicitudes de DM en el bridge (`/api/dm-requests`), Spaces fijado al mismo commit que vlpds (fase 1) el PDS como repo host de sus cuentas (fase 2) y simplespace host de sus spaces (fase 3a, §7.4). Se borraron `ansible/` y `docker-compose.swarm.yml` (pasaron a para-platform). |
| **mubEZ** | `d25e62d` (main) | 6 modificados, 4 nuevos | Litestream y entrypoint con `_FILE`, imagen, audiencia `para-matrix-bridge/dm.v1` en el relay de firmas. |
| **iM8** | `161abe9` (`feat/holder-wallet`) | 1 modificado | El texto de aprobación para `dm.v1`. Parte de `feat/holder-wallet` porque esa pantalla solo existe ahí. |
| **PARA** | `1cf76f3bc` (main) | 12 modificados, 16 nuevos | Dockerfile web por dominio, DMs cifrados por Matrix detrás de `EXPO_PUBLIC_MATRIX_DMS`: pedir, aceptar, verificar la invitación por su intro, lista y vista de conversación (nativo). |

---

## 3. El fork

**Qué es.** `github.com/mexico-parasocial/vlpds` es un fork real de `jazware/vlpds`.
El repo vacío que se creó primero quedó renombrado como `vlpds-vacio-borrar`; se
puede borrar (`gh auth refresh -s delete_repo` y luego `gh repo delete`).

**Cómo se mantiene al día.** Un solo commit nuestro encima de upstream,
`.github/workflows/sync-upstream.yml`:

- corre diario (`17 6 * * *` UTC) y a mano (`workflow_dispatch`);
- llama a la API `merge-upstream` de GitHub sobre `main`;
- usa `secrets.SYNC_TOKEN` si existe, si no `GITHUB_TOKEN`.

**Estado hoy.** Al día: tiene `77b8897 vlpds 1.1.0` y el commit posterior de upstream
(`f2961b8`), más nuestro workflow y los merges de sincronización.

**Lo que hay que saber:**

| Detalle | Efecto | Qué hacer |
|---|---|---|
| `merge-upstream` no copia tags | El fork no tiene `v1.0.0` ni `v1.1.0` | Para leer una versión, usar el commit (`vlpds 1.1.0` = `77b8897`) o los releases de upstream |
| Con `GITHUB_TOKEN`, un cambio de upstream en `.github/workflows/` no se puede sincronizar | El sync fallaría ese día | Agregar el secreto `SYNC_TOKEN` (token con permiso `workflow`) |
| Los workflows de upstream también viven en el fork | Al subir nuestro commit corrieron `CI` (falló) e `Image` (publicó `ghcr.io/mexico-parasocial/vlpds`) | **Hecho (2026-10-11):** `CI` e `Image` deshabilitados en el fork; solo corre `sync-upstream`. La imagen ya publicada se puede borrar desde GitHub si no la queremos |

**Por qué un fork y no copiar código.** No construimos sobre vlpds: es un servidor en
Rust con su propio almacenamiento (SlateDB sobre un bucket). Lo que nos sirve es leer
sus decisiones a tiempo (como Spaces). El fork da eso sin costo: si algún día
queremos parchearlo, ya tiene historial compartido con upstream.

---

## 4. Lo que hicimos frente a vlpds

| Principio de vlpds | En vlpds | En PARA (para-platform) | Diferencia y por qué |
|---|---|---|---|
| Un estado durable | Todo en un bucket S3; los nodos no guardan nada | Postgres + Litestream (SQLite) + rclone hacia un bucket | PARA tiene varios servicios con bases propias (PDS, AppView, Synapse, MAS, mubEZ). El bucket es el destino de respaldos, no la base viva. |
| Nodos desechables | Se pierde un nodo y otro toma sus shards | Se reconstruye una máquina desde el bucket: `make restore`, `restore-media` | Sin failover automático: un servidor por perfil hasta que haga falta más. |
| Perfiles | Tamaños de nodo | `profiles/tiny.env` y `standard.env` (memoria, réplicas, workers) | Igual en espíritu. |
| Secretos | Archivos montados | Todo `_FILE`; `env-from-files.sh` en cada imagen, `make secrets` / `secrets-import` | Igual. Docker Desktop monta los secretos como root, así que en dev quedan `0444`. |
| Revisar antes, comprobar después | preflight y verify | `make preflight`, `make verify`, `make doctor`, `make check` | Igual. `check` valida compose, Caddy, alertas, Alloy y Ansible sin servidor. |
| Alertas con runbook | Una sección por alerta | `ops/alerts.yml` + `docs/RUNBOOK.md` | Igual. |
| Desarrollo en un comando | `just dev` | `make dev` (`APP=`, `MATRIX=1`, `FRESH=1`) | `FRESH=1` es el equivalente desechable. |
| Horizontalidad | Clúster con leases sobre el bucket | `PARA_ROLE=worker`: más workers de AppView detrás de Caddy | Sin coordinación de shards: escala lo que es sin estado. |
| Dominios de handle | Cualquier dominio, TLS bajo demanda | Cualquier `PARA_DOMAIN`; handles `alice.<dominio>`; Caddy `on_demand_tls` pregunta a `/tls-check` del PDS | `/tls-check` es nuevo en el PDS de WatZappa (6 pruebas). |
| Chat | No tiene | Synapse + MAS + para-idp + bridge, appservice generado por `make matrix-init` | Propio de PARA. |
| Mensajes directos | No tiene (ni `chat.bsky`) | DMs cifrados por Matrix con **solicitud con consentimiento** (ver `MATRIX.md`) | Propio de PARA. `make e2e-dm` lo prueba punta a punta. |
| **Spaces** | **1.1.0, en alfa, con `--spaces`** | **No implementado** | Ver §5 y §6. |

Lo que **no** tomamos a propósito: el almacenamiento de vlpds (SlateDB, log en el
bucket, leases). Exige que todo el servidor esté escrito para eso; adoptarlo sería
cambiar de PDS.

---

## 5. Spaces en vlpds 1.1.0

**Qué es un space.** Los datos privados de un grupo. Tiene una **autoridad** (un DID)
que decide quién lee y quién escribe. Cada miembro que escribe guarda un **space
repo** en **su propio PDS**, y el space es la unión de esos repos. Nada pasa por el
firehose. **No hay cifrado**: controla quién lee, pero los PDS que lo guardan sí
pueden leerlo.

- Dirección: `at://{autoridad}/space/{tipoDeSpace}/{skey}`. El tipo es un NSID con
  una declaración de léxico `"type": "space"`.
- Space repo: un conjunto plano `{colección}/{rkey}` → CID, con hash LtHash firmado
  por lector. Se jala con `listRepoOps`/`getRepo`; no se empuja.
- Leer el repo de otro: cadena de tres tokens. Tu PDS emite una delegación (60 s), la
  autoridad emite una credencial (10 min) atada a la llave del cliente, y cada
  petición va firmada (RFC 9421).
- Roles de vlpds: **repo host** (los space repos de sus cuentas) y **simplespace
  host** (spaces que gobiernan sus cuentas).

**Lo que vlpds agrega sobre la implementación de referencia:**

- **Solo OAuth.** Ni app passwords ni sesiones de contraseña leen o escriben spaces.
  La referencia sí los deja.
- Sin fugas: pruebas con centinelas en firehose, réplicas y backfill; métodos de
  spaces sin implementar responden 501 localmente y nunca se reenvían a otro host.
- Blobs: con Spaces activo, `sync.getBlob` solo sirve blobs que nombra un registro
  público.
- Acceso del operador auditado (cada lectura deja un registro con motivo) y takedowns
  de registro, cuenta y space.
- Límites: 100k registros por repo, oplog de 7 días.

**Estado.** Apagado por defecto (`--spaces`). Fijado al PR #5187 de atproto
(`5b95b2f2`). Sin compatibilidad hacia atrás de datos de Spaces, y la actualización
desde builds anteriores es en un solo sentido (detener todos los nodos y arrancar
todos con el build nuevo).

---

## 6. Spaces en WatZappa hoy

| Pieza | Estado |
|---|---|
| `packages/space` | Copia sin cambios de la referencia en `5b95b2f2`, el commit que sigue vlpds 1.1.0 (antes usaba DPoP, el protocolo anterior al 1 de octubre). Pasa su suite y los vectores de vlpds. Procedencia en `packages/space/SOURCE`. |
| Léxicos `com.atproto.space.*` y `simplespace.*` | En el mismo commit (`putMember` reemplaza a `addMember`, nuevo `notifyCredentialRevoked`). Cliente de `packages/api` regenerado. |
| Endpoints en el PDS | **Repo host propio (fase 2) y simplespace host de spaces propios (fase 3a):** con `PDS_SPACES_ENABLED`, cada cuenta escribe y lee sus propios space repos y crea, configura y borra sus spaces. Lo demás de Spaces responde 501. |
| Decisiones escritas | **CD-P3** (nueva, `docs/PERSONAL_CIVIC_TREE.md`): datos de spaces solo con sesiones OAuth. **CD-M5** (`docs/MATRIX_V2.md`): Spaces será la capa de "espacio con permisos", nunca reemplazo de Matrix, y la identidad anónima nunca escribe en un space. **CD-P2** (`docs/PERSONAL_CIVIC_TREE.md`): compartir es membresía de un space, nunca un campo `isPublic`. **OD-P1** (abierta): dónde viven las colecciones; candidatos "PDS alfa como sidecar" o "seguir el port a mano". |

Los dos casos que CD-P2 señala como errores vigentes: `com.para.raq.assessment` lleva
`isPublic` y `com.para.highlight.annotation` lleva `visibility`, en repos públicos.
Cualquiera que lea el PDS directo ve esos datos.

---

## 7. Qué recomiendo

### 7.1 El sidecar no sirve para lo que queremos

En Spaces, **cada registro vive en el PDS de quien lo escribe**. Un vlpds al lado
(sidecar) solo puede guardar space repos de cuentas que vivan en ese vlpds. Las
cuentas de PARA viven en el PDS de WatZappa, así que el sidecar solo podría ser la
**autoridad** de spaces cuyos miembros no escriben nada. Para OD-P1, donde la dueña
de la colección escribe, quedan dos caminos reales:

| Opción | Qué implica | Veredicto |
|---|---|---|
| **A. Repo host + simplespace host en el PDS de WatZappa** sobre `packages/space` | Re-fijar el port a `5b95b2f2` (el mismo pin que vlpds), escribir los handlers, todo detrás de una bandera como `--spaces` | **Recomendada**, por fases |
| B. Mudar las cuentas a vlpds | Perder lo propio del PDS de WatZappa, como la verificación de boletas cívicas (`verifyPublicBallotProof`) antes de preparar escrituras, y `/tls-check` | No |
| C. Esperar a que Spaces sea estable | Nada que mantener | Válida si no hay prisa por cerrar OD-P1 |

### 7.2 Primer léxico privado: las colecciones del árbol cívico personal

`com.para.collection.*` es el mejor primer caso:

- **Hoy no tiene léxico de registro**: es un `payload` opaco en el AppView. No hay
  datos públicos que migrar ni que borrar de repos públicos.
- El diseño ya está escrito (OD-P1): space de tipo `com.para.space.personal`,
  `skey: civic-tree`, `memberListPolicy`, la dueña como único miembro. El AppView
  queda como índice.
- Autoridad, escritora y lectora son la misma cuenta en el mismo PDS: no hace falta
  hablar con otros hosts para el primer paso.

**Después**, con el mismo mecanismo: `highlight.annotation` y `raq.assessment`
dejan `visibility`/`isPublic` y pasan a spaces (compartir = `putMember`).

**Nunca en un space:**

- Votos y delegaciones: cada commit de space va firmado con el DID del autor, así que
  un space oculta el registro a quien no es miembro pero no lo anonimiza ante los
  miembros. Esperan la identidad de boleta (OD-2).
- Nada de la identidad anónima (CD-M5).
- Chat y DMs: Spaces no cifra. Los DMs que hice siguen en Matrix, con E2EE, que es
  lo que pide CD-M5.

### 7.3 Requisitos antes de escribir código

1. **OAuth en la app.** PARA inicia sesión con `createSession` (contraseña). La
   referencia todavía lo acepta para spaces; vlpds no, y es la dirección que
   conviene: una app no debería poder leer los spaces de nadie con una sesión de
   contraseña. Sin cliente OAuth en la app, Spaces queda a medias.
2. **Re-fijar `packages/space`** al protocolo de `5b95b2f2` (firmas HTTP, credenciales
   de 10 minutos con revocación, `repoRev`/`spaceRev`) y actualizar los léxicos
   (`putMember`).
3. **Copiar las barreras de vlpds**, cada una con su prueba: sin firehose (pruebas con
   centinelas), 501 local para métodos no implementados (nunca reenviar), la regla de
   blobs, lectura del operador auditada, takedowns.
4. **Interop contra vlpds.** vlpds trae un arnés de interoperabilidad
   (`bench/spaces`) contra la referencia; usarlo con nuestro PDS como uno de los lados.
5. **Seguir el pin con el fork.** `lexicons/spaces-alpha/SOURCE` en el fork dice qué
   commit de la referencia sigue vlpds; cuando cambie, sabemos que hay cambio de
   protocolo.

### 7.4 Fases

| Fase | Entrega | Riesgo | Estado |
|---|---|---|---|
| 0 | Decidir OAuth en la app; deshabilitar CI/Image en el fork; `SYNC_TOKEN` | Bajo | **Hecha**, salvo `SYNC_TOKEN` (lo tiene que crear una persona: abajo) |
| 1 | `packages/space` y léxicos al pin `5b95b2f2`; su suite y los vectores de vlpds pasan | Bajo (código aislado) | **Hecha** |
| 2 | Repo host en el PDS detrás de `PDS_SPACES_ENABLED=true`: `space.createRecord/getRecord/listRecords/listRepoOps/getRepo` para la propia cuenta, con pruebas de fuga | Medio | **Hecha** |
| 3a | Simplespace host mínimo (autoridad = la propia cuenta, un solo miembro) y el registro `com.para.collection.record`, solo dentro de un space | Medio | **Hecha** |
| 3b | Cliente OAuth en la app (CD-P3); la app lee y escribe sus colecciones en su PDS y copia una vez las del AppView; se retiran `com.para.collection.*` del AppView | Alto: toca el login | Pendiente |
| 4 | Compartir (`putMember`) y mover highlights y RAQ a spaces | Alto: depende de que el protocolo se estabilice | Pendiente |

**Lo que hizo la fase 0 (2026-10-11):**

- **OAuth decidido** como requisito, registrado como CD-P3 en
  `WatZappa/docs/PERSONAL_CIVIC_TREE.md`. Construir el cliente OAuth en la app es
  trabajo aparte.
- `CI` e `Image` deshabilitados en el fork.
- `para-platform` tiene su remoto (`origin`), sin push.
- Falta, y solo puede hacerlo una persona: crear un token con permiso `workflow`
  sobre el fork y guardarlo con `gh secret set SYNC_TOKEN -R mexico-parasocial/vlpds`.
  Opcional: borrar `vlpds-vacio-borrar`.

**Lo que hizo la fase 1 (2026-10-11), en WatZappa:**

- `packages/space` y los léxicos `space`/`simplespace` copiados sin cambios de
  `5b95b2f2`. No había código propio que conservar ni consumidores fuera del código
  generado.
- Lo que la referencia cambió fuera del paquete y el paquete necesita: la opción
  `format` de `verifySignature` en `@atproto/crypto` y `isValidDid` con entrada de
  cualquier tipo en `@atproto/syntax`. Ambos compatibles hacia atrás.
- `tests/vectors.test.ts` corre los vectores de vlpds (LtHash, commits, firmas HTTP,
  tokens). Pasan los 19, así que la referencia, vlpds y nuestra copia producen y
  aceptan lo mismo.
- `pnpm codegen`: cambió solo el cliente de Spaces en `packages/api`. `api` y `pds`
  compilan; suites: space 153/153, crypto 20/20.
- Changeset `.changeset/spaces-alpha-pin.md`, procedencia en `packages/space/SOURCE`
  y una nota en `AGENTS.md`.
- Sin dependencias externas nuevas: `structured-headers` 1.0.1 ya estaba en el
  lockfile.

**Lo que hizo la fase 2 (2026-10-11), en WatZappa:**

- **El PDS como repo host de sus propias cuentas**, portado de la referencia y
  detrás de `PDS_SPACES_ENABLED` (apagado por defecto; en para-platform va
  comentado en `env/core.env.example`). Métodos: `createRecord`, `putRecord`,
  `deleteRecord`, `applyWrites`, `getRecord`, `listRecords`, `listRepoOps`,
  `getLatestCommit` y `getRepo`.
- **Almacenamiento:** las tablas y migraciones de la referencia, idénticas
  (`002`–`004`), para que las fases siguientes encajen. Una cuenta creada antes
  se migra la primera vez que usa Spaces (`ensureLatestSchema`).
- **Solo OAuth (CD-P3):** `authVerifier.spaceAuthorization` rechaza sesiones de
  contraseña y app passwords con `OAuthRequired`; cada handler exige el permiso
  `space:` que corresponde (acción y colección). Para eso porté a
  `oauth-scopes` los permisos `space:` de la referencia, y a `syntax` sus
  validadores que aceptan cualquier valor.
- **Límites deliberados de esta fase**, cada uno con su prueba:
  - solo se escribe en spaces cuya autoridad es la propia cuenta
    (`SpaceNotSupported`), porque escribir en el space de otra autoridad exige
    avisarle (`notifyWrite`) y eso es de la fase 3;
  - cada cuenta lee solo su propio repo (otro repo responde `RepoNotFound`),
    porque leer el de otro miembro requiere credenciales de space;
  - sin blobs (`SpaceBlobsNotSupported`): la referencia cambia `sync.getBlob`
    para no servir blobs que solo nombran registros de spaces, y aquí todavía no.
- **Barreras de vlpds:** cualquier método `com.atproto.space.*` o
  `simplespace.*` sin handler responde 501 en el propio PDS (con la bandera
  encendida o apagada) y nunca se reenvía a otro host; `getServiceAuth` no
  firma tokens para métodos de Spaces.
- **Pruebas:** `packages/pds/tests/spaces.test.ts`, 15 casos: lo que funciona,
  cada rechazo, la migración de una cuenta vieja, que el repo exportado verifica
  contra la llave de la cuenta, y una de fuga con centinelas (ni un evento en el
  firehose, el commit público no cambia, nada en `repo.listRecords` ni en el CAR
  de `sync.getRepo`). Siguen pasando las suites existentes de `getServiceAuth`,
  CRUD, borrado de cuentas, proxy y OAuth (83 pruebas), y la de `oauth-scopes`
  (360, incluidas 49 de `space:`).

**Lo que hizo la fase 3a (2026-10-11), en WatZappa:**

- **Simplespace host** para spaces propios: `createSpace`, `getSpace`,
  `updateSpace`, `deleteSpace` y `listMembers`, solo OAuth con permisos
  `manage:`. Solo acepta la política de lista de miembros (`UnsupportedPolicy`
  para pública o de app gestora): este servidor no emite credenciales, y una
  política abierta se volvería real el día que las emita. `putMember` y
  `removeMember` siguen en 501 (compartir es la fase 4). Borrar el space borra
  también el repo de la dueña en él.
- **El registro de colección:** `com.para.collection.record`, uno por colección
  (la clave es el id), con los mismos campos que ya usa el AppView. El space es
  `at://<did>/space/com.para.space.personal/civic-tree`. El NSID del tipo de
  space no tiene archivo de léxico: las herramientas de léxicos de WatZappa
  todavía no saben declarar `"type": "space"`, y el PDS no lo necesita.
- **Barrera:** el PDS rechaza `com.para.collection.record` en el repo público
  (`SPACE_ONLY_COLLECTIONS` en `prepare.ts`). Así una colección privada no
  puede terminar en el firehose por un error de la app.
- **Un error corregido de paso:** un registro inválido en un space devolvía 500;
  ahora 400, igual que en el repo público.
- **Pruebas:** `spaces.test.ts` pasa a 21 casos; con las suites de regresión
  (incluida `ballot-freeze`, que comparte `prepare.ts`), 141 pruebas en verde.

**Dos correcciones al plan original:**

- **El AppView no indexa el space.** No puede leerlo sin credenciales de space,
  y no hace falta: la dueña es la única lectora y la app lee directo de su PDS.
  Las tablas de colecciones del AppView se retiran en la 3b, no se degradan a
  índice.
- **La mudanza la hace la app.** Solo la dueña, por OAuth, puede escribir en su
  space; por eso la copia de las colecciones existentes es trabajo de la app, y
  la 3b empieza por el cliente OAuth.

Las fases 0 y 1 no comprometen datos. La 2 y la 3a tampoco mientras la bandera
siga apagada. De la 2 en adelante,
siempre detrás de la bandera y sin producción hasta que upstream deje de romper
compatibilidad cada semana.
