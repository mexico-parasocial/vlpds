#!/usr/bin/env python3
"""Provision only the isolated staging bridge operator through MAS."""
import json
from pathlib import Path
import re
import subprocess
import time
import urllib.request

STATE = Path(__file__).resolve().parents[1] / '.dev/chat-staging'
COMPOSE = ['docker', 'compose', '-p', 'para-chat-staging', '-f', str(STATE / 'compose.json')]


def wait(url):
    deadline = time.monotonic() + 180
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen(url, timeout=3) as response:
                if response.status == 200:
                    return
        except Exception:
            pass
        time.sleep(2)
    raise RuntimeError(f'Staging endpoint unavailable: {url}')


wait('http://localhost:38080/.well-known/openid-configuration')
wait('http://localhost:38008/_matrix/client/versions')
token_file = STATE / 'secrets/matrix_admin_token'
token = token_file.read_text().strip()
request = urllib.request.Request('http://localhost:38008/_matrix/client/v3/account/whoami',
                                 headers={'authorization': f'Bearer {token}'})
try:
    with urllib.request.urlopen(request, timeout=5) as response:
        identity = json.load(response)
        if identity.get('user_id') != '@para_bridge_operator:matrix.chat-staging.test':
            raise RuntimeError('Unexpected staging operator identity')
except urllib.error.HTTPError:
    create = subprocess.run(COMPOSE + ['exec', '-T', 'mas', 'mas-cli', 'manage', 'register-user',
      '-c', '/config/mas.json', '--yes', '--admin', 'para_bridge_operator'], capture_output=True, text=True)
    if create.returncode and not ('already' in (create.stderr + create.stdout).lower()):
        raise RuntimeError('Cannot bootstrap the staging MAS operator; inspect private staging logs')
    issued = subprocess.run(COMPOSE + ['exec', '-T', 'mas', 'mas-cli', 'manage', 'issue-compatibility-token',
      '-c', '/config/mas.json', '--yes-i-want-to-grant-synapse-admin-privileges',
      'para_bridge_operator', 'STAGING_BRIDGE_OPERATOR'], capture_output=True, text=True, check=True)
    # The CLI prints a token for this synthetic operator. Keep it out of stdout.
    candidates = re.findall(r'\b(?:mct_|syt_)[A-Za-z0-9_\-]+', issued.stdout + issued.stderr)
    if not candidates:
        log = STATE / 'logs/mas-token.private.log'
        log.write_text(issued.stdout + issued.stderr)
        log.chmod(0o600)
        raise RuntimeError('Cannot parse MAS token; output saved privately, not printed')
    token_file.chmod(0o600)
    token_file.write_text(candidates[-1])
    token_file.chmod(0o444)
print('Staging MAS bridge operator provisioned; no user or operator credential printed.')
