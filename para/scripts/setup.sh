#!/usr/bin/env bash
# make setup — deja una máquina nueva lista para `make dev`.
#
# Revisa herramientas, encuentra los repos hermanos, instala dependencias
# donde falten (cada repo con su versión de Node) y crea mubEZ/.env con
# secretos de desarrollo apuntando al PLC y PDS locales. Se puede repetir:
# no toca lo que ya está bien.

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$HERE" || exit 2
# shellcheck source=scripts/lib.sh
. "$HERE/scripts/lib.sh"

FAILS=0
ok() { printf '  ✅ %s\n' "$1"; }
warn() { printf '  ⚠️  %s\n' "$1"; [ -n "${2:-}" ] && printf '      → %s\n' "$2"; return 0; }
fail() { FAILS=$((FAILS + 1)); printf '  ❌ %s\n' "$1"; [ -n "${2:-}" ] && printf '      → %s\n' "$2"; return 0; }
section() { printf '\n\033[1m%s\033[0m\n' "$1"; }

section "Herramientas"
if command -v docker >/dev/null && docker info >/dev/null 2>&1; then ok "Docker corriendo"
else fail "Docker no está corriendo" "Instala/abre Docker Desktop, OrbStack o colima"; fi
for t in git curl openssl python3 jq lsof; do
  if command -v "$t" >/dev/null; then ok "$t"; else fail "Falta $t" "brew install $t"; fi
done
if [ -s "${NVM_DIR:-$HOME/.nvm}/nvm.sh" ]; then ok "nvm (cada repo usará su .nvmrc)"
elif command -v node >/dev/null; then warn "Sin nvm: todos los repos usarán node $(node -v)" "WatZappa pide $(cat "$WATZAPPA_DIR/.nvmrc" 2>/dev/null) y mubEZ $(cat "$MUBEZ_DIR/.nvmrc" 2>/dev/null); instala nvm"
else fail "No hay Node" "Instala nvm: https://github.com/nvm-sh/nvm"; fi
if command -v corepack >/dev/null || command -v pnpm >/dev/null; then ok "pnpm/corepack"
else fail "Falta pnpm" "corepack enable  (viene con Node)"; fi

section "Repositorios"
check_repo() { # nombre dir requerido clon
  if [ -d "$2/.git" ] || [ -f "$2/.git" ]; then ok "$1 en $2"; return 0; fi
  if [ "$3" = yes ]; then fail "$1 no está en $2" "$4   (o define $(echo "$1" | tr "[:lower:]" "[:upper:]")_DIR en .env)"
  else warn "$1 no está en $2 (opcional)" "$4"; fi
  return 1
}
check_repo WatZappa "$WATZAPPA_DIR" yes "git clone https://github.com/mexico-parasocial/WatZappa.git $WATZAPPA_DIR"
check_repo mubEZ "$MUBEZ_DIR" yes "git clone git@github.com:mexico-parasocial/mubEZ.git $MUBEZ_DIR"
HAVE_PARA=0
check_repo PARA "$PARA_DIR" no "Clona la app en $PARA_DIR para usar make dev APP=..." && HAVE_PARA=1

[ "$FAILS" -eq 0 ] || { printf '\nArregla lo de arriba y repite make setup.\n'; exit 1; }

section "Dependencias"
install_deps() { # nombre dir
  local stamp="$2/node_modules/.modules.yaml"
  if [ -f "$stamp" ] && [ ! "$2/pnpm-lock.yaml" -nt "$stamp" ]; then ok "$1: al día"; return 0; fi
  printf '  ⏳ %s: pnpm install...\n' "$1"
  if (cd "$2" && with_node "$2" pnpm install --frozen-lockfile) >"$HERE/.dev/setup-$1.log" 2>&1; then
    ok "$1: instaladas"
  else
    fail "$1: falló pnpm install" "Revisa .dev/setup-$1.log"
  fi
}
mkdir -p "$HERE/.dev"
install_deps WatZappa "$WATZAPPA_DIR"
install_deps mubEZ "$MUBEZ_DIR"
[ "$HAVE_PARA" = 1 ] && install_deps PARA "$PARA_DIR"

section "Configuración local"
if [ -f "$MUBEZ_DIR/.env" ]; then
  ok "mubEZ/.env existe (no lo toco)"
else
  python3 - "$MUBEZ_DIR/.env.example" "$MUBEZ_DIR/.env" <<'PY'
import secrets, sys
src, dst = sys.argv[1], sys.argv[2]
local = {
    "NODE_ENV": "development",
    "PLC_URL": "http://localhost:2582",
    "PDS_URL": "http://localhost:2583",
    "JWT_SECRET": secrets.token_hex(32),
    "APP_KEY": secrets.token_hex(16),
    "COOKIE_SECRET": secrets.token_hex(32),
    "CURP_PEPPER": secrets.token_hex(32),
    "CIVIC_VOTE_PROOF_SECRET": secrets.token_hex(32),
    "CIVIC_DELEGATION_RESOLVER_SECRET": secrets.token_hex(32),
}
out = []
for line in open(src).read().splitlines():
    key = line.lstrip("# ").split("=", 1)[0]
    if key in local and "=" in line:
        out.append(f"{key}={local.pop(key)}")
    else:
        out.append(line)
out += [f"{k}={v}" for k, v in local.items()]
open(dst, "w").write("\n".join(out) + "\n")
PY
  chmod 600 "$MUBEZ_DIR/.env"
  ok "mubEZ/.env creado: secretos de desarrollo y PLC/PDS locales"
fi
if [ -f "$HERE/.env" ]; then ok ".env de la plataforma existe"
else cp "$HERE/.env.example" "$HERE/.env"; ok ".env creado desde .env.example (solo importa para make up)"; fi

if [ "$FAILS" -eq 0 ]; then
  printf '\n\033[1;32m✓ Listo.\033[0m Ahora: make dev   (o make dev APP=web)\n'
else
  printf '\nHubo %s problemas; arréglalos y repite make setup.\n' "$FAILS"
  exit 1
fi
