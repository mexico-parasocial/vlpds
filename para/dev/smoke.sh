#!/usr/bin/env bash
# make dev-smoke: comprueba el entorno local de punta a punta. Crea una cuenta
# de prueba en vlpds, publica, espera a que el AppView la indexe (firehose de
# vlpds → relay → AppView) y lee el feed a través del proxy de vlpds al AppView.
set -euo pipefail

PDS=http://localhost:2583
APPVIEW=http://localhost:2584
suffix="$(date +%s)"
handle="smoke${suffix}.test"
password="$(openssl rand -hex 16)"

json() { python3 -c "import json,sys; print(json.load(sys.stdin)$1)"; }

session="$(curl -sf "$PDS/xrpc/com.atproto.server.createAccount" \
  -H 'content-type: application/json' \
  -d "{\"handle\":\"$handle\",\"email\":\"$handle@example.invalid\",\"password\":\"$password\"}")"
did="$(echo "$session" | json "['did']")"
jwt="$(echo "$session" | json "['accessJwt']")"
echo "✓ cuenta en vlpds: $handle ($did)"

text="hola desde vlpds $suffix"
curl -sf "$PDS/xrpc/com.atproto.repo.createRecord" \
  -H 'content-type: application/json' -H "authorization: Bearer $jwt" \
  -d "{\"repo\":\"$did\",\"collection\":\"app.bsky.feed.post\",\"record\":{\"\$type\":\"app.bsky.feed.post\",\"text\":\"$text\",\"createdAt\":\"$(date -u +%Y-%m-%dT%H:%M:%S.000Z)\"}}" >/dev/null
echo "✓ publicado en vlpds"

for _ in $(seq 1 30); do
  if curl -sf "$APPVIEW/xrpc/app.bsky.feed.getAuthorFeed?actor=$did" | grep -q "$text"; then
    echo "✓ el AppView lo indexó (vlpds → relay → AppView)"
    break
  fi
  sleep 1
done
curl -sf "$APPVIEW/xrpc/app.bsky.feed.getAuthorFeed?actor=$did" | grep -q "$text" \
  || { echo "✗ el AppView no indexó la publicación" >&2; exit 1; }

curl -sf "$PDS/xrpc/app.bsky.feed.getAuthorFeed?actor=$did" \
  -H "authorization: Bearer $jwt" | grep -q "$text" \
  || { echo "✗ el proxy de vlpds al AppView no devolvió la publicación" >&2; exit 1; }
echo "✓ vlpds la sirve a través de su proxy al AppView"
