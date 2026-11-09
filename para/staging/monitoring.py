#!/usr/bin/env python3
"""Add private monitoring to the existing staging project; never recreate its core services."""
import datetime
import json
import os
from pathlib import Path
import secrets
import subprocess
import sys
import time
from operation_lock import operation

HERE = Path(__file__).resolve().parent
STATE = HERE.parent / '.dev/chat-staging'
BASE = STATE / 'compose.json'
MONITOR = STATE / 'monitoring.json'
PROJECT = 'para-chat-staging'
os.umask(0o077)


def compose(*args, **kwargs):
    return subprocess.run(['docker', 'compose', '-p', PROJECT, '-f', str(BASE), '-f', str(MONITOR), *args], check=True, capture_output=True, text=True, **kwargs)


def render():
    assert json.loads(BASE.read_text())['name'] == PROJECT
    config = STATE / 'monitoring'
    config.mkdir(exist_ok=True, mode=0o700)
    token = STATE / 'secrets/alert_token'
    if not token.exists():
        token.write_text(secrets.token_hex(32))
        token.chmod(0o444)  # Individually mounted; host parent is private.
    targets = {'pds': 'http://127.0.0.1:32583/xrpc/_health',
               'appview': 'http://127.0.0.1:32584/xrpc/_health',
               'relay': 'http://127.0.0.1:32470/xrpc/_health',
               'bridge': 'http://127.0.0.1:33001/healthz',
               'synapse': 'http://127.0.0.1:38008/_matrix/client/versions',
               'mas': 'http://127.0.0.1:38081/health',
               'idp': 'http://127.0.0.1:38090/healthz',
               'm8': 'http://127.0.0.1:38787/v1/health'}
    def write(name, value):
        path = config / name
        if path.exists(): path.chmod(0o600)
        path.write_text(json.dumps(value, indent=2))
        path.chmod(0o444)
    write('prometheus.json', {
        'global': {'scrape_interval': '5s', 'evaluation_interval': '5s'},
        'alerting': {'alertmanagers': [{'static_configs': [{'targets': ['127.0.0.1:39091']}]}]},
        'rule_files': ['/config/rules.json'],
        'scrape_configs': [
            {'job_name': 'chat-staging-bridge', 'static_configs': [{'targets': ['127.0.0.1:33001']}]},
            {'job_name': 'chat-staging-alert-probe', 'static_configs': [{'targets': ['127.0.0.1:39093']}]},
            {'job_name': 'chat-staging-health', 'metrics_path': '/probe', 'params': {'module': ['http_ok']},
             'static_configs': [{'targets': [url], 'labels': {'service': service}} for service, url in targets.items()],
             'relabel_configs': [{'source_labels': ['__address__'], 'target_label': '__param_target'},
                                 {'source_labels': ['__param_target'], 'target_label': 'instance'},
                                 {'target_label': '__address__', 'replacement': '127.0.0.1:39092'}]},
        ]})
    write('rules.json', {'groups': [{'name': 'chat-staging', 'rules': [
        {'alert': 'ParaChatStagingServiceUnavailable', 'expr': 'probe_success{job="chat-staging-health"} == 0', 'for': '30s',
         'labels': {'severity': 'critical'}, 'annotations': {'summary': 'Messaging staging endpoint is unavailable', 'description': 'Check the named service and dependencies; preserve identities and source-bound checkpoints.'}},
        {'alert': 'ParaChatStagingMetricsUnavailable', 'expr': 'up{job="chat-staging-bridge"} == 0', 'for': '30s',
         'labels': {'severity': 'warning', 'service': 'bridge'}, 'annotations': {'summary': 'Bridge metrics cannot be scraped'}},
        {'alert': 'ParaChatStagingAlertDeliveryProbe', 'expr': 'para_chat_staging_alert_probe == 0', 'for': '5s',
         'labels': {'severity': 'test', 'service': 'synthetic-probe'}, 'annotations': {'summary': 'Synthetic alert delivery drill'}},
    ]}]})
    write('blackbox.json', {'modules': {'http_ok': {'prober': 'http', 'timeout': '3s', 'http': {'method': 'GET', 'preferred_ip_protocol': 'ip4'}}}})
    write('alertmanager.json', {'global': {'resolve_timeout': '30s'},
        'route': {'receiver': 'private-staging', 'group_by': ['alertname', 'service'], 'group_wait': '1s', 'group_interval': '5s', 'repeat_interval': '1h'},
        'receivers': [{'name': 'private-staging', 'webhook_configs': [{'url': 'http://127.0.0.1:39093/alerts', 'send_resolved': True,
            'http_config': {'authorization': {'type': 'Bearer', 'credentials_file': '/run/secrets/alert_token'}}}]}]})
    def service(image, command, volumes):
        return {'image': image, 'network_mode': 'service:net', 'restart': 'unless-stopped', 'command': command,
                'volumes': volumes, 'cap_drop': ['ALL'], 'security_opt': ['no-new-privileges:true'], 'mem_limit': '256m'}
    def mount(name): return f'{config / name}:/config/{name}:ro'
    services = {
        'chat-prometheus': service('prom/prometheus:v3.5.0', ['--config.file=/config/prometheus.json', '--web.listen-address=0.0.0.0:39090', '--storage.tsdb.retention.time=7d', '--storage.tsdb.retention.size=256MB'], [mount('prometheus.json'), mount('rules.json'), 'chat_prometheus:/prometheus']),
        'chat-alertmanager': service('prom/alertmanager:v0.28.1', ['--config.file=/config/alertmanager.json', '--web.listen-address=0.0.0.0:39091', '--cluster.listen-address=', '--storage.path=/alertmanager'], [mount('alertmanager.json'), f'{token}:/run/secrets/alert_token:ro', 'chat_alertmanager:/alertmanager']),
        'chat-blackbox': service('prom/blackbox-exporter:v0.27.0', ['--config.file=/config/blackbox.json', '--web.listen-address=0.0.0.0:39092'], [mount('blackbox.json')]),
        'chat-alert-receiver': service('para-chat-staging-alert-receiver:local', ['python', '/app/alert_receiver.py'], [f'{token}:/run/secrets/alert_token:ro', 'chat_alert_receipts:/data']),
    }
    MONITOR.write_text(json.dumps({'services': services, 'volumes': {name: {} for name in ['chat_prometheus', 'chat_alertmanager', 'chat_alert_receipts']}}, indent=2))


def receiver(path, payload=None):
    code = """import json,urllib.request
from pathlib import Path
v=json.load(__import__('sys').stdin)
req=urllib.request.Request('http://127.0.0.1:39093'+v['path'],headers={'Authorization':'Bearer '+Path('/run/secrets/alert_token').read_text().strip(),'Content-Type':'application/json'},data=json.dumps(v['body']).encode() if v['body'] is not None else None)
print(urllib.request.urlopen(req,timeout=5).read().decode())"""
    result = compose('exec', '-T', 'chat-alert-receiver', 'python', '-c', code, input=json.dumps({'path': path, 'body': payload}))
    return json.loads(result.stdout)


def query(expression):
    import urllib.parse
    result = compose('exec', '-T', 'chat-prometheus', 'wget', '-qO-', 'http://127.0.0.1:39090/api/v1/query?query=' + urllib.parse.quote(expression))
    data = json.loads(result.stdout)
    assert data['status'] == 'success'
    return data['data']['result']


def wait(job, label):
    deadline = time.monotonic() + 90
    while time.monotonic() < deadline:
        try:
            value = job()
            if value: return value
        except (subprocess.CalledProcessError, ValueError): pass
        time.sleep(2)
    raise RuntimeError(f'Monitoring timeout: {label}')


def verify():
    def healthy():
        rows = query('probe_success{job="chat-staging-health"}')
        return rows if len(rows) == 8 and all(float(row['value'][1]) == 1 for row in rows) else None
    wait(healthy, 'eight successful health probes')
    assert float(query('up{job="chat-staging-bridge"}')[0]['value'][1]) == 1
    rules = compose('exec', '-T', 'chat-prometheus', 'promtool', 'check', 'rules', '/config/rules.json')
    assert 'SUCCESS' in rules.stdout
    print('PASS eight service probes, bridge scraping and validated alert rules; monitoring ports are not host-published')


def drill():
    before = max([row['id'] for row in receiver('/receipts')['receipts']] or [0])
    evidence = {'time': datetime.datetime.now(datetime.timezone.utc).isoformat(), 'synthetic': True}
    try:
        receiver('/probe', {'up': 0})
        fired = wait(lambda: next((r for r in receiver('/receipts')['receipts'] if r['id'] > before and r['name'] == 'ParaChatStagingAlertDeliveryProbe' and r['status'] == 'firing'), None), 'firing webhook receipt')
        evidence['firingReceiptId'] = fired['id']
    finally:
        receiver('/probe', {'up': 1})
    resolved = wait(lambda: next((r for r in receiver('/receipts')['receipts'] if r['id'] > fired['id'] and r['name'] == fired['name'] and r['status'] == 'resolved'), None), 'resolved webhook receipt')
    evidence['resolvedReceiptId'] = resolved['id']
    (STATE / 'logs/alert-delivery.json').write_text(json.dumps(evidence, indent=2))
    print('PASS Prometheus firing -> Alertmanager -> authenticated durable receipt -> resolved receipt (synthetic drill)')


if __name__ == '__main__':
    action = sys.argv[1] if len(sys.argv) > 1 else 'verify'
    if action == 'up':
        with operation(STATE):
            render()
            subprocess.run(['docker', 'build', '-f', str(HERE / 'Dockerfile.alert-receiver'), '-t', 'para-chat-staging-alert-receiver:local', str(HERE)], check=True)
            compose('config', '--quiet')
            compose('up', '-d', '--no-deps', 'chat-alert-receiver', 'chat-prometheus', 'chat-alertmanager', 'chat-blackbox')
            wait(lambda: receiver('/healthz'), 'receiver startup')
            verify()
    elif action == 'verify': verify()
    elif action == 'drill':
        with operation(STATE): verify(); drill()
    elif action == 'status': print(json.dumps(receiver('/receipts'), indent=2))
    else: raise SystemExit('Usage: monitoring.py up|verify|drill|status')
