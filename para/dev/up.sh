#!/usr/bin/env bash
# make dev: levanta el entorno local de PARA con vlpds (compose.yaml) y espera
# a que el PDS y el AppView respondan. Las llaves de desarrollo se generan una
# vez en .dev/dev.env (fuera del repo).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
STATE="$HERE/../.dev"
ENV_FILE="$STATE/dev.env"
VLPDS_IMAGE="${VLPDS_IMAGE:-ghcr.io/jazware/vlpds:1.1.0}"

mkdir -p "$STATE"
if [ ! -s "$ENV_FILE" ]; then
  key() { docker run --rm "$VLPDS_IMAGE" --generate-did-key 2>&1 | grep -io 'private key (hex): [0-9a-f]*' | awk '{print $NF}'; }
  umask 077
  {
    echo "DEV_PLC_ROTATION_KEY=$(key)"
    echo "DEV_BSKY_SIGNING_KEY=$(key)"
  } > "$ENV_FILE"
fi

busy=""
for port in 2470 2582 2583 2584; do
  if lsof -nP -tiTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1; then busy="$busy $port"; fi
done
if [ -n "$busy" ]; then
  echo "Puertos ocupados:$busy (¿otro entorno de PARA?)" >&2
  exit 2
fi

compose() { docker compose -f "$HERE/compose.yaml" --env-file "$ENV_FILE" "$@"; }
compose up -d --build

wait_http() { # wait_http NOMBRE URL SEGUNDOS
  local t=0
  until curl -sf -o /dev/null "$2"; do
    t=$((t + 2))
    if [ "$t" -ge "$3" ]; then echo "✗ $1 no respondió en $3 s ($2)" >&2; exit 1; fi
    sleep 2
  done
  echo "✓ $1  $2"
}
wait_http PLC http://localhost:2582/_health 180
wait_http vlpds http://localhost:2583/xrpc/_health 120
wait_http AppView http://localhost:2584/xrpc/_health 120
wait_http relay http://localhost:2470/xrpc/_health 60

# El relay solo da de alta hosts que alcanza por IP pública, también por su
# ruta de admin, así que vlpds (localhost) se registra en su base de datos.
# Al reiniciar, el relay se suscribe a cada host activo desde last_seq.
added="$(compose exec -T postgres psql -U postgres -d relay -Atc \
  "INSERT INTO host (created_at, updated_at, hostname, no_ssl, account_limit, trusted, status, last_seq)
   VALUES (now(), now(), 'localhost:2583', true, 1000, true, 'active', 0)
   ON CONFLICT (hostname) DO NOTHING RETURNING id")"
if [ -n "$added" ]; then
  compose restart relay >/dev/null
  wait_http relay http://localhost:2470/xrpc/_health 60
fi
echo "✓ el relay sigue el firehose de vlpds"
