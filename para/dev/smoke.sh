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

if [ "${VLPDS_PARA:-true}" != false ]; then
  community="$(curl -sf "$PDS/xrpc/com.para.community.createBoard" \
    -H 'content-type: application/json' -H "authorization: Bearer $jwt" \
    -H 'atproto-proxy: did:web:localhost%3A2584#bsky_appview' \
    -d "{\"name\":\"Smoke native $suffix\",\"quadrant\":\"MX-NL\"}")"
  board_uri="$(echo "$community" | json "['uri']")"
  board_key="${board_uri##*/}"
  curl -sf "$PDS/xrpc/com.atproto.repo.getRecord?repo=$did&collection=com.para.community.membership&rkey=$board_key" \
    | python3 -c 'import json,sys; r=json.load(sys.stdin); assert r["value"]["membershipState"] == "active"'
  echo '✓ comunidad nativa y membresía del creador en el repo durable'
  status="$(curl -s -o /dev/null -w '%{http_code}' "$PDS/xrpc/com.atproto.repo.createRecord" \
    -H 'content-type: application/json' -H "authorization: Bearer $jwt" \
    -d "{\"repo\":\"$did\",\"collection\":\"com.para.community.vote\",\"validate\":false,\"record\":{}}")"
  [ "$status" = 400 ] || { echo "✗ la política PARA no rechazó la boleta congelada ($status)" >&2; exit 1; }
  echo '✓ la política previa al commit no se elude con validate=false'
fi
