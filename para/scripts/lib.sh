# shellcheck shell=bash disable=SC2034
# Funciones compartidas por los scripts de la plataforma (se carga con `.`).
# Define WATZAPPA_DIR, MUBEZ_DIR, PARA_DIR e IM8_DIR para quien la carga.
# Espera HERE = raíz de la plataforma (vlpds/para). Compatible con el bash 3.2 de macOS.

# env_value ARCHIVO LLAVE — valor de LLAVE en un .env, sin comillas.
env_value() {
  [ -f "$1" ] || return 0
  # Una llave ausente no es un error (con pipefail, el grep vacío lo sería).
  { grep -E "^$2=" "$1" 2>/dev/null || true; } | tail -1 | cut -d= -f2- | sed -e 's/^["'\'']//' -e 's/["'\'']$//'
}

abs_dir() { (cd "$HERE" && cd "$1" 2>/dev/null && pwd) || echo "$1"; }

# Prioridad: variable de entorno, luego .env de la plataforma, luego la carpeta
# hermana por defecto. El default se aplica antes de resolver la ruta: una
# ruta vacía haría que `cd` se quede en esta misma carpeta.
repo_dir() { # repo_dir VAR default
  local v="${!1:-}"
  [ -z "$v" ] && v="$(env_value "$HERE/.env" "$1")"
  [ -z "$v" ] && v="$2"
  abs_dir "$v"
}

WATZAPPA_DIR="$(repo_dir WATZAPPA_DIR ../../WatZappa)"
MUBEZ_DIR="$(repo_dir MUBEZ_DIR ../../mubEZ)"
PARA_DIR="$(repo_dir PARA_DIR ../../PARA)"
IM8_DIR="$(repo_dir IM8_DIR ../../iM8)"

# IP de esta máquina en la red local (la que un teléfono puede alcanzar).
lan_ip() {
  local iface
  iface="$(route -n get default 2>/dev/null | awk '/interface:/{print $2; exit}')"
  if [ -n "$iface" ]; then
    ipconfig getifaddr "$iface" 2>/dev/null && return 0
  fi
  # Linux
  ip route get 1.1.1.1 2>/dev/null | awk '{for (i = 1; i < NF; i++) if ($i == "src") {print $(i + 1); exit}}'
}

# with_node DIR CMD... — corre CMD con el Node del .nvmrc de DIR (vía nvm si
# está instalado; si no, con el node del PATH). Hace exec: úsalo en un subshell.
with_node() {
  local dir="$1"; shift
  if [ -s "${NVM_DIR:-$HOME/.nvm}/nvm.sh" ] && [ -f "$dir/.nvmrc" ]; then
    # shellcheck disable=SC2016
    exec bash -c 'export NVM_DIR="${NVM_DIR:-$HOME/.nvm}"; . "$NVM_DIR/nvm.sh" >/dev/null; nvm use --silent >/dev/null || nvm install; exec "$@"' _ "$@"
  fi
  exec "$@"
}
