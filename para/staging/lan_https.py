#!/usr/bin/env python3
"""Private LAN TLS for the existing synthetic staging; retains keys, volumes and cursors."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import ssl
import subprocess
import time
import urllib.request
from operation_lock import operation

HERE = Path(__file__).resolve().parent
STATE = HERE.parent / '.dev/chat-staging'
TLS = STATE / 'tls'
PORTS = {'pds': (34443, 32583), 'appview': (34444, 32584), 'bridge': (34445, 33001),
         'm8': (34446, 38787), 'matrix': (34447, 38008), 'mas': (34448, 38080),
         'idp': (34449, 38090), 'web': (34450, 19006), 'element': (34451, 8082)}
PROJECT = 'para-chat-staging'
os.umask(0o077)


def dc(*args):
    return subprocess.run(['docker', 'compose', '-p', PROJECT, '-f', str(STATE / 'compose.json'), *args], check=True, capture_output=True, text=True)


def write(path, value, mounted=False):
    if path.exists(): path.chmod(0o600)
    path.write_text(value)
    path.chmod(0o444 if mounted else 0o600)


def urls():
    return json.loads((TLS / 'urls.json').read_text())


def prepare(host):
    host = host.lower()
    if not re.fullmatch(r'[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?\.local', host):
        raise SystemExit('Use this Mac\'s Bonjour .local hostname, without scheme or port')
    TLS.mkdir(exist_ok=True, mode=0o700)
    values = {name: f'https://{host}:{ports[0]}' for name, ports in PORTS.items()}
    previous = TLS / 'urls.json'
    if previous.exists() and {key: value.lower() for key, value in json.loads(previous.read_text()).items()} != values and (TLS / 'rollback').exists():
        raise SystemExit('LAN hostname already selected; rollback the existing profile before changing it')
    write(previous, json.dumps(values, indent=2))
    global_config = '{\n admin off\n skip_install_trust\n auto_https disable_redirects\n storage file_system {\n root ' + str(TLS / 'data') + '\n }\n}\n'
    blocks = []
    for name, public in values.items():
        routes = ['@outside not remote_ip private_ranges', 'respond @outside 403',
                  '@operator path /admin /admin/* /internal/* /metrics /chat-ops/* /xrpc/vlpds.admin.* /_synapse/*',
                  'respond @operator 404']
        if name == 'matrix':
            discovery = json.dumps({'m.homeserver': {'base_url': values['matrix']},
                                    'org.matrix.msc2965.authentication': {'issuer': values['mas'] + '/'}})
            routes += ['handle /.well-known/matrix/client {', 'header Content-Type application/json',
                       'header Access-Control-Allow-Origin *', "respond `" + discovery + "`", '}']
            routes += ['@compat path /_matrix/client/v3/login /_matrix/client/v3/logout /_matrix/client/v3/logout/all /_matrix/client/v3/refresh /_matrix/client/v1/login /_matrix/client/v1/logout /_matrix/client/v1/refresh',
                       'handle @compat {\n reverse_proxy 127.0.0.1:38080\n }']
            routes += ['@client path /_matrix/client/* /_matrix/media/* /health',
                       'handle @client {\n reverse_proxy 127.0.0.1:38008\n }', 'handle {\n respond 404\n }']
        elif name == 'element':
            element = json.dumps({'default_server_config': {'m.homeserver': {'base_url': values['matrix'], 'server_name': 'matrix.chat-staging.test'}},
                                  'disable_custom_urls': True, 'disable_guests': True, 'brand': 'PARA LAN Acceptance'})
            routes += ['@config path /config*.json', 'handle @config {', 'header Content-Type application/json', 'respond `' + element + '`', '}',
                       'handle {\n reverse_proxy 127.0.0.1:8082\n }']
        else:
            routes += ['handle {\n reverse_proxy 127.0.0.1:' + str(PORTS[name][1]) + '\n }']
        blocks.append(public + ' {\n tls internal\n route {\n' + '\n'.join(routes) + '\n }\n}\n')
    write(TLS / 'Caddyfile', global_config + '\n'.join(blocks))
    subprocess.run(['caddy', 'validate', '--config', str(TLS / 'Caddyfile'), '--adapter', 'caddyfile'], check=True, capture_output=True)
    print('Prepared private LAN HTTPS URLs; system certificate trust was not changed')


def start_proxy():
    pidfile = TLS / 'caddy.pid'
    fingerprint = hashlib.sha256((TLS / 'Caddyfile').read_bytes()).hexdigest()
    fingerprint_file = TLS / 'caddy.sha256'
    if pidfile.exists():
        pid = int(pidfile.read_text())
        command = subprocess.run(['ps', '-p', str(pid), '-o', 'command='], capture_output=True, text=True).stdout
        if str(TLS / 'Caddyfile') in command and 'caddy run' in command:
            if fingerprint_file.exists() and fingerprint_file.read_text() == fingerprint: return
            stop_proxy()
    with (TLS / 'caddy.log').open('ab') as log:
        process = subprocess.Popen(['caddy', 'run', '--config', str(TLS / 'Caddyfile'), '--adapter', 'caddyfile'],
                                   stdout=log, stderr=log, start_new_session=True)
    pidfile.write_text(str(process.pid))
    fingerprint_file.write_text(fingerprint)
    root = TLS / 'data/pki/authorities/local/root.crt'
    for _ in range(100):
        if process.poll() is not None: raise RuntimeError('LAN TLS proxy exited; inspect its private log')
        if root.exists():
            write(TLS / 'ca.crt', root.read_text(), mounted=True)
            return
        time.sleep(0.1)
    raise RuntimeError('LAN certificate generation timed out')


def stop_proxy():
    pidfile = TLS / 'caddy.pid'
    if not pidfile.exists(): return
    pid = int(pidfile.read_text())
    command = subprocess.run(['ps', '-p', str(pid), '-o', 'command='], capture_output=True, text=True).stdout
    if str(TLS / 'Caddyfile') in command and 'caddy run' in command:
        os.kill(pid, signal.SIGTERM)
        for _ in range(50):
            if not subprocess.run(['ps', '-p', str(pid)], capture_output=True).returncode == 0: break
            time.sleep(0.1)
        else: raise RuntimeError('LAN proxy did not stop; refusing to replace its listener')
    pidfile.unlink()


def apply():
    public = urls()
    compose_path = STATE / 'compose.json'
    config = json.loads(compose_path.read_text())
    assert config['name'] == PROJECT
    rollback = TLS / 'rollback'
    if rollback.exists():
        prior = json.loads((STATE / 'manifest.json').read_text()).get('publicUrls')
        if prior and {key: value.lower() for key, value in prior.items()} != public:
            raise SystemExit('A different LAN proposal exists; inspect it before applying again')
    else:
        rollback.mkdir(mode=0o700)
    names = ['compose.json', 'manifest.json', 'config/homeserver.json', 'config/mas.json']
    for name in names:
        target = rollback / name
        target.parent.mkdir(parents=True, exist_ok=True)
        if not target.exists(): shutil.copyfile(STATE / name, target)
    write(rollback / 'files.json', json.dumps(names))
    try:
        synapse = json.loads((STATE / 'config/homeserver.json').read_text())
        synapse['public_baseurl'] = public['matrix'] + '/'
        write(STATE / 'config/homeserver.json', json.dumps(synapse, indent=2), mounted=True)
        mas = json.loads((STATE / 'config/mas.json').read_text())
        mas['http']['public_base'] = public['mas'] + '/'
        mas['http']['issuer'] = public['mas'] + '/'
        provider = mas['upstream_oauth2']['providers'][0]
        provider['issuer'] = public['idp']
        provider['authorization_endpoint'] = public['idp'] + '/authorize'
        # Token and JWKS requests stay on the private container network.
        write(STATE / 'config/mas.json', json.dumps(mas, indent=2), mounted=True)
        services = config['services']
        services['idp']['environment']['PARA_IDP_ISSUER'] = public['idp']
        clients = json.loads(services['idp']['environment']['PARA_IDP_CLIENTS'])
        clients[0]['redirectUris'] = [public['mas'] + '/upstream/callback/' + provider['id']]
        services['idp']['environment']['PARA_IDP_CLIENTS'] = json.dumps(clients)
        services['m8']['environment']['SERVICE_URL'] = public['m8']
        services['m8']['environment']['MATRIX_IDP_ISSUER'] = public['idp']
        services['bridge']['environment']['MATRIX_PUBLIC_HOMESERVER_URL'] = public['matrix']
        services['pds']['environment']['VLPDS_PUBLIC_URL'] = public['pds']
        # Existing DID documents are intentionally retained. New test accounts
        # advertise this LAN endpoint; no real identity migration is performed.
        host = public['pds'].split('://')[1].split(':')[0]
        hosts = subprocess.check_output(['docker', 'exec', 'para-chat-staging-net-1', 'cat', '/etc/hosts'], text=True)
        gateway = subprocess.check_output(['docker', 'exec', 'para-chat-staging-net-1', 'getent', 'hosts', 'host.docker.internal'], text=True).split()[0]
        write(TLS / 'hosts', hosts + '\n' + gateway + ' ' + host + '\n', mounted=True)
        for name in ['pds', 'm8', 'bridge', 'dataplane', 'appview', 'synapse', 'mas', 'idp']:
            # Docker forbids extra_hosts with a shared service network. Mount
            # the local alias explicitly without replacing the network anchor.
            services[name].pop('extra_hosts', None)
            mounts = services[name].setdefault('volumes', [])
            for mount in [f'{TLS / "ca.crt"}:/run/para-lan-ca.crt:ro', f'{TLS / "hosts"}:/etc/hosts:ro']:
                if mount not in mounts: mounts.append(mount)
            if name in ('m8', 'bridge', 'dataplane', 'appview', 'idp'):
                services[name].setdefault('environment', {})['NODE_EXTRA_CA_CERTS'] = '/run/para-lan-ca.crt'
        write(compose_path, json.dumps(config, indent=2))
        manifest = json.loads((STATE / 'manifest.json').read_text())
        manifest['publicUrls'] = public
        write(STATE / 'manifest.json', json.dumps(manifest, indent=2))
        dc('config', '--quiet')
        dc('up', '-d', '--no-deps', 'idp', 'mas', 'synapse', 'm8', 'pds', 'dataplane', 'appview', 'bridge')
    except Exception:
        print('LAN configuration did not finish; original files are retained in tls/rollback')
        raise
    print('Applied LAN advertisements to synthetic staging; keys, databases and relay checkpoints retained')


def request(url):
    context = ssl.create_default_context(cafile=str(TLS / 'ca.crt'))
    with urllib.request.urlopen(url, context=context, timeout=8) as response:
        return json.load(response)


def verify():
    public = urls()
    paths = {'pds': '/xrpc/_health', 'appview': '/xrpc/_health', 'bridge': '/healthz', 'm8': '/v1/health',
             'matrix': '/_matrix/client/versions', 'mas': '/.well-known/openid-configuration', 'idp': '/.well-known/openid-configuration'}
    result = {}
    for name, path in paths.items():
        for attempt in range(30):
            try:
                body = request(public[name] + path)
                if name in ('mas', 'idp'): assert body['issuer'].rstrip('/') == public[name]
                result[name] = True
                break
            except Exception:
                if attempt == 29: raise
                time.sleep(1)
    metadata = request(public['matrix'] + '/_matrix/client/v1/auth_metadata')
    assert metadata['issuer'].rstrip('/') == public['mas']
    discovery = request(public['matrix'] + '/.well-known/matrix/client')
    assert discovery['m.homeserver']['base_url'] == public['matrix']
    element = request(public['element'] + '/config.json')
    assert element['default_server_config']['m.homeserver']['base_url'] == public['matrix']
    for name, path in [('pds', '/admin'), ('pds', '/metrics'), ('bridge', '/chat-ops/v1/status'), ('matrix', '/_synapse/admin/v1/server_version')]:
        try: request(public[name] + path)
        except urllib.error.HTTPError as error: assert error.code == 404
        else: raise AssertionError('Operator endpoint exposed through LAN TLS')
    (STATE / 'logs/lan-https.json').write_text(json.dumps({'services': result, 'issuerConsistent': True, 'operatorRoutesDenied': True,
                                                       'phoneVerified': False, 'systemTrustModified': False}, indent=2))
    print('PASS CA-validated TLS, seven endpoints, issuer/discovery consistency, Element configuration and operator route denial')


def app_env():
    public = urls()
    return {'EXPO_NO_DOTENV': '1', 'EXPO_PUBLIC_ENV': 'development',
            'EXPO_PUBLIC_USE_LOCAL_DEV_SERVICE': '1', 'EXPO_PUBLIC_DEFAULT_SERVICE': public['pds'],
            'EXPO_PUBLIC_LOCAL_DEV_SERVICE': public['pds'], 'EXPO_PUBLIC_LOCAL_APPVIEW_URL': public['appview'],
            'EXPO_PUBLIC_BLUESKY_PROXY_DID': 'did:web:localhost%3A32584',
            'EXPO_PUBLIC_LOCAL_BSKY_PROXY_DID': 'did:web:localhost%3A32584',
            'EXPO_PUBLIC_M8_BROKER_URL': public['m8'], 'EXPO_PUBLIC_MATRIX_BRIDGE_URL': public['bridge'],
            'EXPO_PUBLIC_MATRIX_IDP_URL': public['idp'],
            'EXPO_PUBLIC_MATRIX_PUSH_GATEWAY_URL': public['bridge'] + '/_matrix/push/v1/notify'}


def rollback():
    backup = TLS / 'rollback'
    for name in json.loads((backup / 'files.json').read_text()):
        write(STATE / name, (backup / name).read_text(), mounted=name.startswith('config/'))
    dc('up', '-d', '--no-deps', 'idp', 'mas', 'synapse', 'm8', 'pds', 'dataplane', 'appview', 'bridge')
    print('Restored original loopback advertisements and configuration; data volumes were not replaced')


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('action', choices=['prepare', 'up', 'verify', 'app-env', 'web', 'rollback'])
    parser.add_argument('--host', default=None)
    parser.add_argument('--client', type=Path, default=HERE.parents[3] / 'PARA')
    args = parser.parse_args()
    if args.action == 'prepare':
        host = args.host or subprocess.check_output(['scutil', '--get', 'LocalHostName'], text=True).strip() + '.local'
        prepare(host)
    elif args.action == 'up':
        with operation(STATE):
            start_proxy(); apply(); verify()
        print('Install and explicitly trust only the public tls/ca.crt on test devices; never distribute the CA private key')
    elif args.action == 'verify': verify()
    elif args.action == 'app-env': print('\n'.join(f'{key}={value}' for key, value in app_env().items()))
    elif args.action == 'web':
        environment = {**os.environ, **app_env()}
        # Use the already-installed CLI; do not install or rewrite a client checkout.
        subprocess.run(['node', str(args.client / 'node_modules/expo/bin/cli'), 'start', '--web', '--port', '8084'], cwd=args.client, env=environment, check=True)
    elif args.action == 'rollback':
        with operation(STATE): rollback()
