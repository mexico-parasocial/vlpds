#!/usr/bin/env python3
"""Check staging endpoints and isolation without exposing generated credentials."""
from pathlib import Path
import json
import subprocess
import urllib.request

STATE = Path(__file__).resolve().parents[1] / '.dev/chat-staging'
manifest = json.loads((STATE / 'manifest.json').read_text())
assert manifest['project'] == 'para-chat-staging'
targets = {
    'PLC': 'http://localhost:32582/_health',
    'VLPDS': 'http://localhost:32583/xrpc/_health',
    'rsky': 'http://localhost:32470/xrpc/_health',
    'AppView': 'http://localhost:32584/xrpc/_health',
    'M8': 'http://localhost:38787/v1/health',
    'IdP': 'http://localhost:38090/healthz',
    'MAS': 'http://localhost:38080/.well-known/openid-configuration',
    'Synapse': 'http://localhost:38008/_matrix/client/versions',
    'bridge': 'http://localhost:33001/healthz',
    'MinIO': 'http://localhost:39000/minio/health/live',
}
errors = []
for name, url in targets.items():
    try:
        with urllib.request.urlopen(url, timeout=5) as response:
            if response.status != 200:
                raise RuntimeError(f'HTTP {response.status}')
        print(f'PASS {name}: {url}')
    except Exception as error:
        print(f'FAIL {name}: {url} ({type(error).__name__})')
        errors.append(name)
compose = ['docker', 'compose', '-p', 'para-chat-staging', '-f', str(STATE / 'compose.json')]
subprocess.run(compose + ['config', '--quiet'], check=True, capture_output=True)
if errors:
    raise SystemExit(1)
print('Persistent staging health passed; device, crypto and recovery acceptance are separate checks.')
