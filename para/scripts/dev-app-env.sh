#!/usr/bin/env bash
# make dev-app-env — apunta la app PARA (PARA/.env.local) a esta máquina.
#
# Escribe solo estas llaves y deja el resto del archivo como está:
#   EXPO_PUBLIC_LOCAL_DEV_IP           IP de esta Mac en la red local
#   EXPO_PUBLIC_USE_LOCAL_DEV_SERVICE  1
#   EXPO_PUBLIC_M8_BROKER_URL          http://IP:8787
#   EXPO_PUBLIC_MATRIX_BRIDGE_URL      http://IP:3001
#   EXPO_PUBLIC_LOCAL_BSKY_PROXY_DID   el DID del AppView que está corriendo
#
# Es justo lo que `make doctor` revisa en "Configuración" y "DIDs". Córrelo
# cuando cambie la red (otra IP) o tras `make dev-reset` (otro DID), y luego
# reinicia Metro con --clear.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib.sh
. "$HERE/scripts/lib.sh"

[ -d "$PARA_DIR" ] || { echo "No encuentro PARA en $PARA_DIR (define PARA_DIR en .env)" >&2; exit 2; }
ip="${1:-$(lan_ip)}"
[ -n "$ip" ] || { echo "No pude saber la IP de esta máquina; pásala: make dev-app-env IP=192.168.x.y" >&2; exit 2; }

did="$(curl -s -m 3 http://127.0.0.1:2581/ 2>/dev/null | python3 -c 'import json,sys
try: print(json.load(sys.stdin)["bsky"]["did"])
except Exception: pass' 2>/dev/null || true)"

python3 - "$PARA_DIR/.env.local" "$ip" "$did" <<'PY'
import os, sys
path, ip, did = sys.argv[1], sys.argv[2], sys.argv[3]
want = {
    "EXPO_PUBLIC_LOCAL_DEV_IP": ip,
    "EXPO_PUBLIC_USE_LOCAL_DEV_SERVICE": "1",
    "EXPO_PUBLIC_M8_BROKER_URL": f"http://{ip}:8787",
    "EXPO_PUBLIC_MATRIX_BRIDGE_URL": f"http://{ip}:3001",
}
if did:
    want["EXPO_PUBLIC_LOCAL_BSKY_PROXY_DID"] = did
lines = open(path).read().splitlines() if os.path.exists(path) else []
seen = set()
out = []
for line in lines:
    key = line.split("=", 1)[0].strip()
    if key in want:
        if key in seen:
            continue
        out.append(f"{key}={want[key]}")
        seen.add(key)
    else:
        out.append(line)
missing = [k for k in want if k not in seen]
if missing:
    out.append("")
    out.append("# make dev-app-env (para-platform)")
    out.extend(f"{k}={want[k]}" for k in missing)
open(path, "w").write("\n".join(out) + "\n")
for k, v in want.items():
    print(f"  {k}={v}")
PY
[ -n "$did" ] || echo "  (el AppView no está corriendo: no actualicé EXPO_PUBLIC_LOCAL_BSKY_PROXY_DID; corre make dev y repite)"
echo "Escrito en $PARA_DIR/.env.local. Reinicia Metro con: npx expo start --clear"
