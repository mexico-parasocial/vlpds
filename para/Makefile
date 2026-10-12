# PARA sobre vlpds. Ver para/dev/compose.yaml.
.PHONY: dev dev-smoke dev-down

dev: ## Levanta el entorno local (vlpds, PLC, relay, AppView, bsync)
	./dev/up.sh

dev-smoke: ## Cuenta → publicación → AppView → proxy de vlpds
	./dev/smoke.sh

dev-down: ## Apaga el entorno local y borra sus datos
	./dev/down.sh
