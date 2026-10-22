# Plataforma PARA sobre vlpds — un solo punto de entrada para desarrollar y
# operar. Vive en el fork de vlpds (para/); WatZappa, mubEZ y PARA son
# checkouts hermanos del fork (../../WatZappa, ...).
#
#   make help        lista los comandos
#
# Desarrollo:  make dev → make dev-smoke   (vlpds, PLC, relay y AppView locales)
# Producción:  make secrets → make preflight → make up → make verify

-include .env
export

PLATFORM_DIR := $(CURDIR)
export PLATFORM_DIR

WATZAPPA_DIR ?= ../../WatZappa
MUBEZ_DIR ?= ../../mubEZ
PARA_DIR ?= ../../PARA
PARA_PROFILE ?= tiny
PROFILES ?= core,matrix,identity,edge
comma := ,
PROFILE_FLAGS := $(foreach p,$(subst $(comma), ,$(PROFILES)),--profile $(p))

# Valores que salen de PARA_DOMAIN si .env no los fija. Se exportan para que
# los compose de cada repo los interpolen sin conocer la plataforma.
BSKY_SERVER_DID ?= did:web:appview.$(PARA_DOMAIN)
OZONE_SERVER_DID ?= did:web:ozone.$(PARA_DOMAIN)
MATRIX_SERVER_NAME ?= matrix.$(PARA_DOMAIN)
MATRIX_PUBLIC_HOMESERVER_URL ?= https://matrix.$(PARA_DOMAIN)
MATRIX_DOMAIN ?= chat.$(PARA_DOMAIN)
POSTGRES_USER ?= pg
PARA_SECRETS_GID ?= 10001
PARA_HANDLE_DOMAIN ?= .$(PARA_DOMAIN)
# vlpds quiere el dominio de handles sin el punto inicial.
VLPDS_HANDLE_DOMAIN ?= $(patsubst .%,%,$(PARA_HANDLE_DOMAIN))
# Salas nuevas cifradas: el chat nativo de PARA rechaza las que no lo están
# (ENCRYPTED_ROOM_REQUIRED). No cifra salas que ya existen.
MATRIX_ENABLE_ENCRYPTION ?= true
MATRIX_ADMIN_USER ?= para-bridge-admin

# El edge publica las rutas de cada perfil solo si el perfil está activo.
has_profile = $(findstring $(comma)$(1)$(comma),$(comma)$(PROFILES)$(comma))
PARA_EDGE_MATRIX_SITE := $(if $(call has_profile,matrix),$(PLATFORM_DIR)/edge/sites/matrix.Caddyfile,$(PLATFORM_DIR)/edge/sites/empty.Caddyfile)
PARA_EDGE_IDENTITY_SITE := $(if $(call has_profile,identity),$(PLATFORM_DIR)/edge/sites/identity.Caddyfile,$(PLATFORM_DIR)/edge/sites/empty.Caddyfile)
PARA_WEB_CONTEXT := $(abspath $(PARA_DIR))
PARA_ROOT_HANDLER := $(if $(call has_profile,web),reverse_proxy web:8100,respond "Not Found" 404)

# Los compose de Matrix y mubEZ interpolan estas variables, que en la
# plataforma llegan como archivos de secrets/. Vacías a propósito: así Compose
# no avisa que faltan y nada secreto viaja en el entorno.
MATRIX_ADMIN_TOKEN ?=
ADMIN_DIDS ?=
OZONE_SERVER_DID ?= did:web:ozone.$(PARA_DOMAIN)
MATRIX_DEPLOY_DIR := $(abspath $(WATZAPPA_DIR))/deploy/matrix
export MATRIX_DEPLOY_DIR

# El perfil da los tamaños; lo que ponga .env (ya exportado) le gana.
# Un worker de AppView (PARA_ROLE=appview-worker) usa compose.worker.yaml.
PARA_ROLE ?= primary
ifeq ($(PARA_ROLE),appview-worker)
COMPOSE := docker compose -f compose.worker.yaml --env-file profiles/$(PARA_PROFILE).env --profile appview-worker
else
COMPOSE := docker compose --env-file profiles/$(PARA_PROFILE).env $(PROFILE_FLAGS)
endif

.DEFAULT_GOAL := help

.PHONY: help
help: ## Muestra esta ayuda
	@awk 'BEGIN {FS = ":.*## "} /^##@/ {printf "\n\033[1m%s\033[0m\n", substr($$0, 5)} /^[a-z-]+:.*## / {printf "  \033[36m%-19s\033[0m %s\n", $$1, $$2}' $(MAKEFILE_LIST)
	@echo

##@ Desarrollo (tu máquina)

.PHONY: dev
dev: ## Entorno local: vlpds, PLC, relay, AppView y bsync en Docker
	@./dev/up.sh

.PHONY: dev-smoke
dev-smoke: ## Prueba punta a punta: cuenta → publicación → AppView → proxy de vlpds
	@./dev/smoke.sh

.PHONY: dev-down
dev-down: ## Apaga el entorno local y borra sus datos
	@./dev/down.sh

.PHONY: e2e-dm
e2e-dm: ## Prueba punta a punta de los mensajes directos por Matrix (aislada, solo Docker)
	@./scripts/e2e-dm.sh

.PHONY: app-env
app-env: ## Variables para compilar la app PARA contra esta plataforma (EAS / build web)
	@./scripts/app-env.sh

##@ Producción (el servidor)

.PHONY: secrets
secrets: ## Genera los secretos que falten en secrets/ (nunca pisa los existentes)
	@./scripts/secrets.sh generate

.PHONY: secrets-import
secrets-import: ## Trae los secretos de los .env de antes (WatZappa/.env, env/identity.env)
	@./scripts/secrets.sh import

.PHONY: preflight
preflight: ## Revisa configuración, secretos, bucket y DNS antes de levantar
	@./scripts/preflight.sh

.PHONY: up
up: ## Descarga las imágenes fijadas y levanta PROFILES (BUILD=1 construye aquí; UP_ARGS=--force-recreate tras cambiar un secreto)
ifeq ($(BUILD),1)
	$(COMPOSE) up -d --build --wait --wait-timeout 900 $(UP_ARGS)
else
	$(if $(filter primary,$(PARA_ROLE)),$(COMPOSE) build backup)
	$(COMPOSE) pull --ignore-pull-failures --quiet
	$(COMPOSE) up -d --no-build --wait --wait-timeout 900 $(UP_ARGS)
endif

.PHONY: verify
verify: ## Comprueba el despliegue: contenedores, endpoints públicos, respaldos
	@./scripts/verify.sh

# Solo las versiones que pasas en la línea de comandos se escriben en .env.
UPGRADE_SET := $(if $(filter command line,$(origin WATZAPPA_VERSION)),WATZAPPA_VERSION $(WATZAPPA_VERSION)) \
               $(if $(filter command line,$(origin MUBEZ_VERSION)),MUBEZ_VERSION $(MUBEZ_VERSION))

.PHONY: upgrade
upgrade: ## Cambia de versión: make upgrade WATZAPPA_VERSION=sha-... [MUBEZ_VERSION=...]
	@test -n "$(strip $(UPGRADE_SET))" || { echo 'Pasa WATZAPPA_VERSION=... y/o MUBEZ_VERSION=...'; exit 2; }
	@./scripts/set-env.sh $(UPGRADE_SET)
	$(COMPOSE) pull --ignore-pull-failures --quiet
	$(COMPOSE) up -d --no-build --wait --wait-timeout 900
	@./scripts/verify.sh

.PHONY: down
down: ## Detiene PROFILES (conserva volúmenes y el bucket)
	$(COMPOSE) down

.PHONY: stop
stop: ## Detiene un servicio sin borrarlo (make stop S=pds)
	$(COMPOSE) stop $(S)

.PHONY: restart
restart: ## Reinicia con gracia (make restart S=bsky)
	$(COMPOSE) restart $(S)

.PHONY: ps
ps: ## Estado de los contenedores
	$(COMPOSE) ps

.PHONY: logs
logs: ## Logs en vivo (make logs S=mubez para un solo servicio)
	$(COMPOSE) logs -f --tail=200 $(S)

.PHONY: config
config: ## Valida y muestra la configuración final de Compose
	$(COMPOSE) config

.PHONY: compose
compose: ## Cualquier comando de Compose con la config de la plataforma (ARGS="...")
	@$(COMPOSE) $(ARGS)

##@ Operación

.PHONY: doctor
doctor: ## Diagnóstico de producción: configuración y endpoints públicos
	@./scripts/doctor.sh prod

.PHONY: backup
backup: ## Corre un respaldo ahora (además del automático)
	$(COMPOSE) exec backup backup.sh once
	@case ",$(PROFILES)," in *,matrix,*) $(COMPOSE) exec backup-media backup.sh once ;; esac

.PHONY: restore
restore: ## Restaura Postgres del bucket (ARGS="list" | "postgres para latest"); ver docs/RESPALDOS.md
	$(COMPOSE) run --rm --no-deps backup restore.sh $(ARGS)

.PHONY: restore-media
restore-media: ## Restaura el media de Synapse desde el bucket (detén synapse antes)
	$(COMPOSE) run --rm --no-deps -v $${COMPOSE_PROJECT_NAME:-para}_synapse_data:/restore/synapse backup-media restore.sh synapse-media

.PHONY: db-init
db-init: ## Crea las bases compartidas (matrix, mas, bridge) en un Postgres que ya existía
	$(COMPOSE) exec -u postgres postgres sh /docker-entrypoint-initdb.d/10-para-databases.sh

# Las URLs públicas que ven los navegadores y la app: MAS en auth., para-idp
# en idp., Element en chat. La config de Element queda junto a la de Synapse
# (no versionada) para que el checkout del servidor siga limpio.
MATRIX_INIT_ENV = POSTGRES_PASSWORD="$$(cat secrets/postgres_password)" \
	MATRIX_SERVER_NAME="$(MATRIX_SERVER_NAME)" MATRIX_DOMAIN="$(MATRIX_DOMAIN)" \
	MAS_PUBLIC_BASE="https://auth.$(PARA_DOMAIN)/" PARA_IDP_PUBLIC_URL="https://idp.$(PARA_DOMAIN)" \
	ELEMENT_CONFIG_OUT="$(abspath $(WATZAPPA_DIR))/deploy/matrix/synapse/element-config.json" \
	PARA_PUBLIC_URL="https://$(PARA_DOMAIN)"

.PHONY: matrix-init
matrix-init: ## Genera la configuración de Synapse, MAS y Element para PARA_DOMAIN (una vez)
	@test -s secrets/postgres_password || { echo 'Primero: make secrets'; exit 2; }
	@case "$(PARA_DOMAIN)" in ''|example.org) echo 'Define PARA_DOMAIN en .env'; exit 2 ;; esac
	@$(MATRIX_INIT_ENV) ./matrix/setup.sh
	@$(MATRIX_INIT_ENV) ./matrix/setup-mas.sh
	@$(MATRIX_INIT_ENV) BRIDGE_APPSERVICE_URL=http://matrix-bridge:3001 ./matrix/setup-appservice.sh
	@./scripts/secrets.sh import | grep -E 'matrix_|importados' || true
	@echo
	@echo "Con la plataforma arriba falta el token de admin: make matrix-admin-token (docs/MATRIX.md)."

.PHONY: invite
invite: ## Crea un código de invitación de vlpds (USES=1)
	@$(COMPOSE) exec -T pds vlpds admin create-invite-code $(if $(USES),--uses $(USES))

.PHONY: vlpds-admin
vlpds-admin: ## Cualquier comando de operador de vlpds (ARGS="account list", "request-crawl bsky.network", ...)
	@$(COMPOSE) exec -T pds vlpds admin $(ARGS)

.PHONY: relay-add-pds
relay-add-pds: ## Pide al relay que siga a vlpds (lo hace vlpds solo al arrancar)
	@$(COMPOSE) exec -T pds vlpds admin request-crawl http://relay.para.internal:2470

.PHONY: matrix-admin-token
matrix-admin-token: ## Usuario admin de Synapse para el bridge (vía MAS) y su token en secrets/
	@$(COMPOSE) exec -T mas mas-cli manage register-user --config /config/config.yaml --yes --admin "$(MATRIX_ADMIN_USER)" >/dev/null 2>&1 || true
	@out="$$($(COMPOSE) exec -T mas mas-cli manage issue-compatibility-token --config /config/config.yaml \
	    --yes-i-want-to-grant-synapse-admin-privileges "$(MATRIX_ADMIN_USER)" 2>&1)"; \
	  token="$$(printf '%s' "$$out" | grep -oE 'mct_[A-Za-z0-9_]+' | tail -1)"; \
	  if [ -z "$$token" ]; then echo "$$out" | tail -5; echo 'No obtuve el token (ver arriba)'; exit 1; fi; \
	  chmod u+w secrets 2>/dev/null; printf '%s' "$$token" > secrets/matrix_admin_token; \
	  ./scripts/secrets.sh perms >/dev/null; \
	  echo "Token de $(MATRIX_ADMIN_USER) en secrets/matrix_admin_token. Ahora: make up UP_ARGS=--force-recreate"

.PHONY: login-preflight
login-preflight: ## Revisa login sin emitir tokens (WEB_ORIGIN=https://tu-app)
	@test -n "$$WEB_ORIGIN" || { echo 'Define WEB_ORIGIN con el origen web real de PARA'; exit 2; }
	$(COMPOSE) exec mubez node build/bin/para_login_preflight.js --web-origin "$$WEB_ORIGIN"

.PHONY: deploy
deploy: ## Provisiona y despliega con Ansible (INVENTORY=deploy/ansible/inventories/<nombre>)
	@test -n "$(INVENTORY)" || { echo 'Define INVENTORY=deploy/ansible/inventories/<nombre>'; exit 2; }
	cd deploy/ansible && ansible-playbook -i "$(abspath $(INVENTORY))/hosts.yml" playbooks/site.yml $(ANSIBLE_ARGS)

##@ Este repo

.PHONY: check
check: ## Valida scripts, Compose, Caddyfile, alertas, Alloy y Ansible (usa Docker)
	@./scripts/check.sh
