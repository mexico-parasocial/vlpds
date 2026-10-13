#!/usr/bin/env bash
# make check — valida este repo sin tocar nada desplegado. Usa Docker para no
# exigir herramientas instaladas (shellcheck, caddy, promtool, alloy, ansible).
#
#   1. Scripts: sintaxis con el bash 3.2 de macOS y shellcheck.
#   2. Compose: la configuración final con .env.example y secretos de prueba,
#      en los dos perfiles de tamaño.
#   3. Caddyfile del edge (con el de WatZappa importado).
#   4. Alertas (promtool) y configuración de Alloy.
#   5. Ansible: sintaxis de los playbooks y ansible-lint.

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$HERE" || exit 2
# shellcheck source=scripts/lib.sh
. "$HERE/scripts/lib.sh"

FAILS=0
ok() { printf '  ✅ %s\n' "$1"; }
fail() { FAILS=$((FAILS + 1)); printf '  ❌ %s\n' "$1"; [ -n "${2:-}" ] && printf '%s\n' "$2" | sed 's/^/      /' | head -20; return 0; }
section() { printf '\n\033[1m%s\033[0m\n' "$1"; }
run() { # run DESCRIPCIÓN CMD...
  local out
  if out="$("${@:2}" 2>&1)"; then ok "$1"; else fail "$1" "$out"; fi
}

TMP="$(mktemp -d "${TMPDIR:-/tmp}/para-check.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT

section "Scripts"
for f in scripts/*.sh ops/backup/*.sh ops/postgres/init/*.sh; do
  run "sintaxis $f" /bin/bash -n "$f"
done
run "shellcheck" docker run --rm -v "$HERE:/w:ro" -w /w koalaman/shellcheck:v0.10.0 -S warning -x \
  scripts/*.sh ops/backup/*.sh ops/postgres/init/*.sh

section "Compose"
mkdir -p "$TMP/secrets"
for n in $("$HERE/scripts/secrets.sh" list | awk 'NR > 1 {print $1}'); do printf 'check-%s' "$n" > "$TMP/secrets/$n"; done
for profile in tiny standard; do
  run "make config ($profile, todos los perfiles)" env -i PATH="$PATH" HOME="$HOME" PARA_DIR="$PARA_DIR" \
    PARA_DOMAIN=example.org S3_BUCKET=check PARA_PROFILE="$profile" PARA_SECRETS_DIR="$TMP/secrets" \
    PROFILES=core,matrix,identity,edge,monitoring,local-s3,web WATZAPPA_DIR="$WATZAPPA_DIR" MUBEZ_DIR="$MUBEZ_DIR" \
    make -s -C "$HERE" config
done
replicas="$(env -i PATH="$PATH" HOME="$HOME" PARA_DOMAIN=example.org S3_BUCKET=check PARA_PROFILE=standard \
  PARA_SECRETS_DIR="$TMP/secrets" WATZAPPA_DIR="$WATZAPPA_DIR" MUBEZ_DIR="$MUBEZ_DIR" \
  make -s -C "$HERE" compose ARGS="config --format json" 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["services"]["bsky"]["deploy"]["replicas"])' 2>/dev/null)"
if [ "$replicas" = 2 ]; then ok "el perfil standard escala bsky a 2 réplicas"; else fail "standard debería dar 2 réplicas de bsky (dio ${replicas:-nada})"; fi

section "Edge"
caddy_validate() { # caddy_validate DESCRIPCIÓN SITIO_MATRIX SITIO_IDENTIDAD MANEJADOR_RAÍZ
  run "$1" docker run --rm \
    -e PARA_DOMAIN=example.org -e ACME_EMAIL=ops@example.org -e APPVIEW_WORKERS="10.0.0.2:2584" \
    -e PARA_HANDLE_DOMAIN=.example.org -e MATRIX_SERVER_NAME=matrix.example.org -e PARA_ROOT_HANDLER="$4" \
    -v "$HERE/edge/Caddyfile:/etc/caddy/Caddyfile:ro" \
    -v "$WATZAPPA_DIR/services/caddy/Caddyfile.prod:/etc/caddy/watzappa.Caddyfile:ro" \
    -v "$2:/etc/caddy/sites/matrix.Caddyfile:ro" -v "$3:/etc/caddy/sites/identity.Caddyfile:ro" \
    caddy:2.8.4 caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile
}
caddy_validate "caddy validate: todos los perfiles y app web" \
  "$WATZAPPA_DIR/services/caddy/Caddyfile.matrix.prod" "$HERE/edge/sites/identity.Caddyfile" "reverse_proxy web:8100"
caddy_validate "caddy validate: solo core (perfiles apagados, raíz 404)" \
  "$HERE/edge/sites/empty.Caddyfile" "$HERE/edge/sites/empty.Caddyfile" 'respond "Not Found" 404'

section "Monitoreo"
run "promtool check rules" docker run --rm -v "$HERE/ops:/ops:ro" --entrypoint promtool prom/prometheus:v2.54.1 check rules /ops/alerts.yml
run "alloy fmt" docker run --rm -v "$HERE/ops/alloy:/a:ro" grafana/alloy:v1.8.3 fmt /a/config.alloy

section "Ansible"
ansible() {
  docker run --rm -v "$HERE/deploy/ansible:/w" -w /w -e ANSIBLE_CONFIG=/w/ansible.cfg alpine/ansible:2.18.1 "$@"
}
run "galaxy + syntax-check" ansible sh -c '
  ansible-galaxy collection install -r requirements.yml -p /tmp/collections >/dev/null &&
  ANSIBLE_COLLECTIONS_PATH=/tmp/collections ansible-playbook -i inventories/example/hosts.yml --syntax-check playbooks/bootstrap.yml playbooks/site.yml'

printf '\n'
[ "$FAILS" -eq 0 ] && printf '\033[1;32m✓ Todo válido.\033[0m\n' || printf '%s comprobaciones fallaron.\n' "$FAILS"
[ "$FAILS" -eq 0 ]
