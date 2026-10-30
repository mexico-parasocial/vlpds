#!/usr/bin/env python3
"""Exercise the actual trusted proxy and a recoverable Synapse provisioning outage."""
import base64
import json
import os
from pathlib import Path
import subprocess
import time
import urllib.error
import urllib.request
import uuid

os.umask(0o077)
STATE = Path(__file__).resolve().parents[1] / '.dev/chat-staging'
COMPOSE = ['docker', 'compose', '-p', 'para-chat-staging', '-f', str(STATE / 'compose.json')]
assert json.loads((STATE / 'manifest.json').read_text())['project'] == 'para-chat-staging'


def request(path, body=None, actor='staging', expected=200, headers=None, base='http://localhost:38000'):
    password = (STATE / 'secrets' / f'console_{actor}_password').read_text().strip()
    auth = base64.b64encode(f'{actor}:{password}'.encode()).decode()
    values = {'authorization': f'Basic {auth}', 'origin': 'http://localhost:38000',
              'content-type': 'application/json', 'x-para-chat-action': '1', **(headers or {})}
    req = urllib.request.Request(base + path, data=json.dumps(body).encode() if body is not None else None, headers=values)
    try:
        with urllib.request.urlopen(req, timeout=60) as response:
            assert response.status == expected
            return json.load(response)
    except urllib.error.HTTPError as error:
        assert error.code == expected, f'Unexpected HTTP status: {error.code}'
        return json.load(error)


def until(check, label):
    for _ in range(90):
        try:
            result = check()
            if result:
                return result
        except (urllib.error.URLError, TimeoutError):
            pass
        time.sleep(1)
    raise RuntimeError(f'Staging operations timeout: {label}')


status = request('/chat-ops/v1/status')
assert all(value['ok'] for value in status['services'].values())
assert isinstance(status['checkpoint']['cursor'], str)
assert request('/xrpc/vlpds.admin.getSession', actor='pds-only')['operator'] == 'pds-only'
request('/chat-ops/v1/status', actor='pds-only', expected=403,
        headers={'x-para-operator': 'staging', 'x-para-ops-key': (STATE / 'secrets/chat_ops_proxy_secret').read_text().strip()})
print('PASS one authenticated gateway; PDS-only operator denied chat even with spoofed headers')

for device in status['devices']:
    assert not {'did', 'mxid', 'accessToken', 'refreshToken'} & device.keys()
config = json.loads((STATE / 'compose.json').read_text())
assert not any(':33002:' in port or ':32590:' in port for port in config['services']['net']['ports'])
print('PASS private operations listener, source-bound text cursor and identity-free device responses')

fixture = json.loads((STATE / 'fixtures.json').read_text())
subprocess.run(COMPOSE + ['stop', '--timeout', '20', 'synapse'], check=True, capture_output=True)
try:
    assert not request('/chat-ops/v1/status')['services']['synapse']['ok']
    board = request('/xrpc/com.para.community.createBoard',
                    {'name': f'Outage rehearsal {uuid.uuid4().hex[:10]}', 'quadrant': 'MX-NL'},
                    headers={'authorization': 'Bearer ' + fixture['pdsToken']}, base='http://localhost:32583')
    def failed_job():
        return next((job for job in request('/chat-ops/v1/status')['jobs']
                     if job['communityUri'] == board['uri'] and job['state'] == 'failed'), None)
    job = until(failed_job, 'durable failed community job')
    print('PASS homeserver outage is visible and preserves a durable provisioning job')
finally:
    subprocess.run(COMPOSE + ['up', '-d', 'synapse'], check=True, capture_output=True)

until(lambda: request('/chat-ops/v1/status')['services']['synapse']['ok'], 'Synapse recovery')
operation = {'requestId': str(uuid.uuid4()), 'reason': 'Synthetic homeserver outage recovered'}
path = f"/chat-ops/v1/jobs/{job['id']}/retry"
result = request(path, operation)
assert result['result'] == 'success'
replayed = request(path, operation)
assert replayed['replay']
status = request('/chat-ops/v1/status')
assert next(j for j in status['jobs'] if j['id'] == job['id'])['state'] == 'done'
assert next(a for a in status['audit'] if a['requestId'] == operation['requestId'])['result'] == 'success'
assert any(c['communityUri'] == board['uri'] for c in status['communities'])
print('PASS authenticated retry, completed room mapping, replay protection and durable operator audit')

community = next(c for c in status['communities'] if c['communityUri'] == board['uri'])
fixture['opsEvidence'] = {'jobId': job['id'], 'retryRequestId': operation['requestId'],
                          'communityUri': board['uri'], 'spaceId': community['spaceId']}
temporary = STATE / 'fixtures.json.ops.tmp'
temporary.write_text(json.dumps(fixture, indent=2))
temporary.replace(STATE / 'fixtures.json')
