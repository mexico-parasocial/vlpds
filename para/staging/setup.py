#!/usr/bin/env python3
"""Render a persistent, loopback-only chat staging project without touching live config."""
import json
import os
from pathlib import Path
import re
import secrets
import subprocess

PLATFORM = Path(__file__).resolve().parents[1]
STATE = PLATFORM / '.dev/chat-staging'
ROOT = PLATFORM.parent.parent
PROJECT = 'para-chat-staging'
SERVER = 'matrix.chat-staging.test'
PROVIDER = '01K3ZQD8N7P0J5R2T9V4W6X8YB'
MAS_BRIDGE_CLIENT = '01K3ZQD8N7P0J5R2T9V4W6X8YD'
PORTS = {'plc': 32582, 'pds': 32583, 'appview': 32584, 'dataplane': 32585,
         'relay': 32470, 'bridge': 33001, 'm8': 38787, 'synapse': 38008,
         'mas': 38080, 'idp': 38090, 's3': 39000, 'console': 38000}

os.umask(0o077)
STATE.mkdir(parents=True, exist_ok=True, mode=0o700)
if (STATE / 'manifest.json').exists() and json.loads((STATE / 'manifest.json').read_text()).get('publicUrls'):
    raise SystemExit('LAN HTTPS is active. Use staging/lan_https.py rollback before regenerating the loopback profile; do not overwrite its issuer configuration.')
for name in ['config', 'secrets', 'logs', 'snapshots']:
    (STATE / name).mkdir(exist_ok=True, mode=0o700)


def write(path, text, container=False):
    if path.exists():
        path.chmod(0o600)
    path.write_text(text)
    # The parent is private on the host. Container readers get only individually
    # mounted files, which must be readable by their distinct service UIDs.
    path.chmod(0o444 if container else 0o600)


def secret(name):
    path = STATE / 'secrets' / name
    if not path.exists():
        write(path, secrets.token_hex(32), container=True)
    return path.read_text().strip()


def pds_key(name):
    path = STATE / 'secrets' / name
    if not path.exists():
        result = subprocess.run(['docker', 'run', '--rm', 'para-vlpds:para-native-local',
                                 '--generate-did-key'], capture_output=True, text=True, check=True)
        match = re.search(r'private key \(hex\): ([0-9a-f]{64})', result.stdout + result.stderr, re.I)
        if not match:
            raise RuntimeError('PDS key generation failed; key output is intentionally withheld')
        write(path, match.group(1), container=True)
    return path.read_text().strip()


def rsa(name):
    path = STATE / 'secrets' / name
    if not path.exists():
        subprocess.run(['openssl', 'genpkey', '-algorithm', 'RSA', '-pkeyopt',
                        'rsa_keygen_bits:2048', '-out', str(path)], check=True, capture_output=True)
        path.chmod(0o444)
    return path.read_text()


# All service bind mounts must be included in state.tar.gz; do not depend on
# mutable source files outside the snapshot during disaster recovery.
for source, target in [('env-from-files.sh', 'env-from-files.sh'),
                       ('bsky/api.js', 'bsky-api.js'), ('bsky/dataplane.js', 'bsky-dataplane.js')]:
    write(STATE / 'config' / target, (PLATFORM / 'runtime' / source).read_text(), container=True)

pg_password = secret('postgres_password')
idp_secret = secret('idp_client_secret')
mas_bridge_secret = secret('mas_bridge_admin_secret')
matrix_secret = secret('matrix_shared_secret')
as_token = secret('matrix_as_token')
hs_token = secret('matrix_hs_token')
secret('matrix_admin_token')  # Replaced only by the staging MAS bootstrap.
idp_rsa = rsa('idp_rsa')
mas_rsa = rsa('mas_rsa')
rotation = pds_key('plc_rotation_key')
appview_key = pds_key('appview_signing_key')
for name in ['pds_kek', 'pds_jwt', 'pds_admin', 's3_password', 'relay_admin',
             'm8_jwt', 'm8_app_key', 'm8_cookie', 'm8_pepper', 'm8_civic', 'bsync_key']:
    secret(name)


def db(name):
    return f'postgresql://pg:{pg_password}@127.0.0.1:5432/{name}'


def file_secret(env_name, name):
    return {f'{env_name}_FILE': f'/run/secrets/{name}'}


def private_value(name, value):
    write(STATE / 'secrets' / name, value, container=True)
    return name


# Synthetic operator authentication is confined to loopback staging. Public
# deployments need HTTPS and their reviewed authentication proxy.
ops_secret = secret('chat_ops_proxy_secret')
operator_accounts = []
for operator in ['staging', 'pds-only']:
    password = secret(f'console_{operator}_password')
    hash_file = STATE / 'secrets' / f'console_{operator}_bcrypt'
    if not hash_file.exists():
        hashed = subprocess.run(['docker', 'run', '--rm', '-i', 'caddy:2.10.2', 'caddy', 'hash-password'],
            input=password + '\n', capture_output=True, text=True, check=True).stdout.strip()
        if not hashed.startswith('$2'):
            raise RuntimeError('Cannot hash the private staging operator credential')
        write(hash_file, hashed, container=True)
    operator_accounts.append(f'        {operator} {hash_file.read_text().strip()}')
write(STATE / 'config/Caddyfile', """{
    admin off
    auto_https off
}
http://localhost:38000 {
    bind 0.0.0.0
    basic_auth {
""" + '\n'.join(operator_accounts) + """
    }
    handle /chat-ops/v1/* {
        reverse_proxy 127.0.0.1:33002 {
            header_up -Authorization
            header_up X-Para-Operator {http.auth.user.id}
            header_up X-Para-Ops-Key """ + ops_secret + """
        }
    }
    handle /xrpc/* {
        reverse_proxy 127.0.0.1:32590 {
            header_up -Authorization
            header_up X-Para-Operator {http.auth.user.id}
        }
    }
    handle {
        root * /srv
        try_files {path} /index.html
        file_server
    }
}
""", container=True)

write(STATE / 'config/postgres.sql', '\n'.join(f'CREATE DATABASE {name};' for name in
      ['plc', 'appview', 'bsync', 'synapse', 'mas', 'bridge']), container=True)
write(STATE / 'config/appservice.json', json.dumps({
    'id': 'para-chat-staging', 'url': 'http://localhost:33001',
    'as_token': as_token, 'hs_token': hs_token, 'sender_localpart': 'para_bridge_bot',
    'rate_limited': False, 'namespaces': {'users': [{'exclusive': False,
       'regex': r'@([a-z2-7]{32}):matrix\.chat-staging\.test'}], 'aliases': [], 'rooms': []},
}), container=True)
write(STATE / 'config/homeserver.json', json.dumps({
    'server_name': SERVER, 'public_baseurl': 'http://localhost:38008/',
    'report_stats': False, 'pid_file': '/data/homeserver.pid',
    'signing_key_path': '/data/signing.key',
    'database': {'name': 'psycopg2', 'args': {'host': '127.0.0.1', 'port': 5432,
                 'user': 'pg', 'password': pg_password, 'database': 'synapse'}},
    'media_store_path': '/data/media', 'trusted_key_servers': [],
    'federation_domain_whitelist': [], 'enable_registration': False,
    'enable_authenticated_media': True, 'push': {'include_content': False},
    'password_config': {'enabled': False}, 'suppress_key_server_warning': True,
    'app_service_config_files': ['/config/appservice.json'],
    'matrix_authentication_service': {'enabled': True, 'endpoint': 'http://localhost:38080/',
                                      'secret': matrix_secret},
    'listeners': [{'type': 'http', 'port': 38008, 'tls': False, 'bind_addresses': ['0.0.0.0'],
                   'resources': [{'names': ['client'], 'compress': False}]}],
    'rc_message': {'per_second': 100, 'burst_count': 100},
    'rc_registration': {'per_second': 100, 'burst_count': 100},
    'rc_room_creation': {'per_second': 100, 'burst_count': 100},
}), container=True)
write(STATE / 'config/mas.json', json.dumps({
    'http': {'public_base': 'http://localhost:38080/', 'issuer': 'http://localhost:38080/',
             'listeners': [{'name': 'web', 'resources': [{'name': x} for x in
               ['discovery', 'human', 'oauth', 'compat', 'graphql', 'assets']],
               'binds': [{'host': '0.0.0.0', 'port': 38080}], 'proxy_protocol': False},
               {'name': 'internal', 'resources': [{'name': 'health'}, {'name': 'adminapi'}],
                'binds': [{'host': '0.0.0.0', 'port': 38081}], 'proxy_protocol': False}]},
    'database': {'uri': db('mas')}, 'secrets': {'encryption': secret('mas_encryption'),
      'keys': [{'kid': 'chat-staging-v1', 'key': mas_rsa}]},
    'matrix': {'kind': 'synapse', 'homeserver': SERVER, 'secret': matrix_secret,
               'endpoint': 'http://localhost:38008'},
    'clients': [{'client_id': '0000000000000000000SYNAPSE',
      'client_auth_method': 'client_secret_basic', 'client_secret': secret('synapse_client_secret')},
      {'client_id': MAS_BRIDGE_CLIENT, 'client_auth_method': 'client_secret_basic', 'client_secret': mas_bridge_secret}],
    'policy': {'data': {'admin_clients': [MAS_BRIDGE_CLIENT]}},
    'upstream_oauth2': {'providers': [{'id': PROVIDER, 'human_name': 'PARA staging',
      'issuer': 'http://localhost:38090', 'authorization_endpoint': 'http://localhost:38090/authorize',
      'token_endpoint': 'http://localhost:38090/token', 'jwks_uri': 'http://localhost:38090/jwks.json',
      'client_id': 'mas', 'client_secret': idp_secret, 'token_endpoint_auth_method': 'client_secret_post',
      'scope': 'openid', 'discovery_mode': 'disabled', 'pkce_method': 'always',
      'claims_imports': {'localpart': {'action': 'force', 'template': '{{ user.sub }}'},
                        'displayname': {'action': 'ignore'}, 'email': {'action': 'ignore'}}}]},
    'passwords': {'enabled': False}, 'account': {'password_registration_enabled': False,
      'email_change_allowed': False, 'registration_token_required': False},
}), container=True)

services = {}
services['net'] = {'image': 'alpine:3.20', 'command': ['sleep', 'infinity'], 'init': True, 'restart': 'unless-stopped',
                   'ports': [f'127.0.0.1:{port}:{port}' for port in PORTS.values()]}


def service(name, image, **kwargs):
    value = {'image': image, 'network_mode': 'service:net', 'restart': 'unless-stopped',
             'depends_on': {'net': {'condition': 'service_started'}}, **kwargs}
    services[name] = value
    return value


def mount(name, target):
    return f'{STATE}/secrets/{name}:{target}:ro'


def health(url):
    return {'test': ['CMD', 'curl', '-fsS', url], 'interval': '3s', 'timeout': '3s', 'retries': 60}


def wrapped(name, image, env, secret_names, command, **kwargs):
    return service(name, image, entrypoint=['/bin/sh', '/runtime/env-from-files.sh'], command=command,
      environment=env, volumes=[f'{STATE}/config/env-from-files.sh:/runtime/env-from-files.sh:ro'] +
      [mount(x, f'/run/secrets/{x}') for x in secret_names] + kwargs.pop('volumes', []), **kwargs)


service('postgres', 'postgres:17-alpine', environment={'POSTGRES_USER': 'pg',
    'POSTGRES_PASSWORD_FILE': '/run/secrets/postgres_password',
    'POSTGRES_INITDB_ARGS': '--encoding=UTF8 --lc-collate=C --lc-ctype=C'},
    volumes=['postgres:/var/lib/postgresql/data', mount('postgres_password', '/run/secrets/postgres_password'),
             f'{STATE}/config/postgres.sql:/docker-entrypoint-initdb.d/10-chat.sql:ro'],
    healthcheck={'test': ['CMD', 'pg_isready', '-U', 'pg'], 'interval': '2s', 'retries': 60})
service('minio', 'vlpds-minio:chat-staging', command=['server', '/data', '--address', ':39000'],
    environment={'MINIO_ROOT_USER': 'chat-staging', 'MINIO_ROOT_PASSWORD_FILE': '/run/secrets/s3_password'},
    volumes=['minio:/data', mount('s3_password', '/run/secrets/s3_password')],
    healthcheck=health('http://localhost:39000/minio/health/live'))
service('bucket-init', 'vlpds-minio:chat-staging', restart='no',
    depends_on={'minio': {'condition': 'service_healthy'}},
    entrypoint=['/bin/sh', '-ec'], command=['mc alias set staging http://localhost:39000 chat-staging "$(cat /run/secrets/s3_password)" >/dev/null; mc mb --ignore-existing staging/para-chat-staging >/dev/null'],
    volumes=[mount('s3_password', '/run/secrets/s3_password')])
service('plc', 'para-plc:9c8ea2f', depends_on={'postgres': {'condition': 'service_healthy'}},
    environment={'DB_CREDS_JSON': json.dumps({'host': '127.0.0.1', 'username': 'pg', 'password': pg_password, 'database': 'plc'}),
                 'DB_MIGRATE_CREDS_JSON': json.dumps({'host': '127.0.0.1', 'username': 'pg', 'password': pg_password, 'database': 'plc'}),
                 'ENABLE_MIGRATIONS': 'true', 'PORT': '32582'},
    healthcheck={'test': ['CMD', 'wget', '-qO-', 'http://localhost:32582/_health'], 'interval': '3s', 'retries': 60})
pds_names = ['plc_rotation_key', 'pds_kek', 'pds_jwt', 'pds_admin', 's3_password']
service('pds', 'para-vlpds:para-native-local',
    depends_on={'plc': {'condition': 'service_healthy'}, 'bucket-init': {'condition': 'service_completed_successfully'}},
    command=['--dev-mode', '--no-rate-limits', '--crawlers=', '--shards=8', '--workers=2', '--io-threads=2'],
    environment={'VLPDS_PARA': 'true', 'VLPDS_LISTEN': '0.0.0.0:32583',
      'VLPDS_ADMIN_LISTEN': '127.0.0.1:32590', 'VLPDS_ADMIN_PROXY_HEADER': 'X-Para-Operator',
      'VLPDS_ADMIN_PROXY_FROM': '127.0.0.1/32', 'VLPDS_ADMIN_OPERATORS': 'staging,pds-only',
      'VLPDS_PUBLIC_URL': 'http://localhost:32583', 'VLPDS_HANDLE_DOMAIN': 'test',
      'VLPDS_PLC_URL': 'http://localhost:32582', 'VLPDS_PLC_MODE': 'directory',
      'VLPDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX': rotation,
      'VLPDS_APPVIEW': 'http://localhost:32584,did:web:localhost%3A32584',
      'VLPDS_PARA_VOTE_VERIFIER_URL': 'http://localhost:38787/v1/identity/civic-vote-proof/verify',
      'VLPDS_S3_ENDPOINT': 'http://localhost:39000', 'VLPDS_S3_BUCKET': 'para-chat-staging',
      'VLPDS_PREFIX': 'chat-staging/v1', 'VLPDS_S3_ACCESS_KEY': 'chat-staging',
      **file_secret('VLPDS_S3_SECRET_KEY', 's3_password'), **file_secret('VLPDS_KEK', 'pds_kek'),
      **file_secret('VLPDS_JWT_SECRET', 'pds_jwt'), **file_secret('VLPDS_ADMIN_TOKEN', 'pds_admin'),
      'VLPDS_CACHE_DIR': '/cache', 'VLPDS_LOG_FORMAT': 'text'},
    volumes=['pds_cache:/cache'] + [mount(x, f'/run/secrets/{x}') for x in pds_names],
    healthcheck=health('http://localhost:32583/xrpc/_health'), stop_grace_period='45s')
service('relay', 'para-rsky-relay:e96634e-para2',
    depends_on={'pds': {'condition': 'service_healthy'}}, command=['/usr/local/bin/rsky-relay', '--no-plc-export'],
    environment={'RELAY_LISTEN': '0.0.0.0:32470', 'RELAY_PLC_URL': 'http://localhost:32582',
      'RELAY_PDS_HOST': 'localhost:32583', 'RELAY_UPSTREAM_URL': 'http://localhost:32583',
      'RELAY_LENIENT_VALIDATION': 'false', 'RELAY_ADMIN_PASSWORD': secret('relay_admin'), 'RUST_LOG': 'rsky_relay=info'},
    volumes=['relay:/data/relay'], healthcheck=health('http://localhost:32470/xrpc/_health'), stop_grace_period='45s')

for name, db_name, port in [('dataplane', 'appview', 32585), ('appview', 'appview', 32584)]:
    key = private_value(f'{name}_db', db(db_name))
    env = {'BSKY_DB_POSTGRES_URL_FILE': f'/run/secrets/{key}', 'BSKY_DB_POOL_SIZE': '10',
      'BSKY_DID_PLC_URL': 'http://localhost:32582', 'BSKY_DISABLE_SSRF_PROTECTION': 'true',
      'BSKY_DATAPLANE_PORT': '32585', 'BSKY_REPO_PROVIDER': 'ws://localhost:32470',
      'BSKY_PORT': '32584', 'BSKY_PUBLIC_URL': 'http://localhost:32584',
      'BSKY_SERVER_DID': 'did:web:localhost%3A32584', 'BSKY_DATAPLANE_URLS': 'http://localhost:32585',
      'BSKY_DATAPLANE_HTTP_VERSION': '1.1', 'BSKY_SERVICE_SIGNING_KEY_FILE': '/run/secrets/appview_signing_key',
      'BSKY_ADMIN_PASSWORDS_FILE': '/run/secrets/pds_admin', 'MOD_SERVICE_DID': 'did:web:localhost%3A32584',
      'LOG_ENABLED': 'true', 'LOG_LEVEL': 'info'}
    env.update({'BSKY_BSYNC_URL': 'http://localhost:32586', 'BSKY_BSYNC_HTTP_VERSION': '1.1',
      'BSKY_BSYNC_API_KEY_FILE': '/run/secrets/bsync_key',
      'PARA_CIVIC_VOTE_VERIFIER_URL': 'http://localhost:38787/v1/identity/civic-vote-proof/verify'})
    wrapped(name, 'para-chat-staging-bsky:local', env, [key, 'appview_signing_key', 'pds_admin', 'bsync_key'],
      ['node', '--enable-source-maps', f'/app/services/bsky/{"dataplane" if name == "dataplane" else "api"}.js'],
      volumes=[f'{STATE}/config/bsky-{"dataplane" if name == "dataplane" else "api"}.js:/app/services/bsky/{"dataplane" if name == "dataplane" else "api"}.js:ro'],
      depends_on={'postgres': {'condition': 'service_healthy'}, 'relay': {'condition': 'service_healthy'}})

bsync_db = private_value('bsync_db', db('bsync'))
wrapped('bsync', 'para-bsync:ea85ae3', {'BSYNC_PORT': '32586', 'BSYNC_DB_MIGRATE': 'true',
      'BSYNC_DB_POSTGRES_URL_FILE': '/run/secrets/bsync_db', 'BSYNC_API_KEYS_FILE': '/run/secrets/bsync_key'},
      ['bsync_db', 'bsync_key'], ['node', '--enable-source-maps', 'index.js'],
      depends_on={'postgres': {'condition': 'service_healthy'}})

wrapped('idp', 'watzappa-para-idp:latest', {'PORT': '38090', 'PARA_IDP_ISSUER': 'http://localhost:38090', 'PARA_IDP_APP_SCHEME': 'social.para.app',
    'PARA_IDP_PRIVATE_KEY_FILE': '/run/secrets/idp_rsa',
    'PARA_IDP_CLIENTS': json.dumps([{'clientId': 'mas', 'clientSecret': idp_secret,
      'redirectUris': [f'http://localhost:38080/upstream/callback/{PROVIDER}']}])},
    ['idp_rsa'], ['node', 'dist/index.js'])
service('synapse', 'matrixdotorg/synapse:v1.161.0',
    depends_on={'postgres': {'condition': 'service_healthy'}, 'synapse-init': {'condition': 'service_completed_successfully'}},
    environment={'SYNAPSE_CONFIG_PATH': '/config/homeserver.json'},
    volumes=['synapse:/data', f'{STATE}/config/homeserver.json:/config/homeserver.json:ro',
             f'{STATE}/config/appservice.json:/config/appservice.json:ro'],
    healthcheck={'test': ['CMD', 'python3', '-c', 'import urllib.request; urllib.request.urlopen("http://localhost:38008/health")'],
                 'interval': '3s', 'timeout': '3s', 'retries': 60})
services['synapse-init'] = {'image': 'matrixdotorg/synapse:v1.161.0', 'restart': 'no',
    'entrypoint': ['/bin/sh', '-ec'], 'command': ['chown -R 991:991 /data'],
    'user': '0:0', 'volumes': ['synapse:/data']}
service('mas', 'ghcr.io/element-hq/matrix-authentication-service:1.24.0',
    depends_on={'postgres': {'condition': 'service_healthy'}, 'idp': {'condition': 'service_started'}},
    command=['server', '-c', '/config/mas.json'], volumes=[f'{STATE}/config/mas.json:/config/mas.json:ro'])

m8_names = ['m8_jwt', 'm8_app_key', 'm8_cookie', 'm8_pepper', 'm8_civic']
service('m8-init', 'alpine:3.20', user='0', restart='no',
    command=['sh', '-c', 'chown -R 1000:1000 /data'], volumes=['m8:/data'])
wrapped('m8', 'para-chat-staging-m8:local', {'NODE_ENV': 'development', 'PORT': '38787', 'HOST': '0.0.0.0',
    'SERVICE_URL': 'http://localhost:38787', 'DATABASE_PATH': '/data/mubez.db', 'DATABASE_AUTOMIGRATE': 'true',
    'PDS_URL': 'http://localhost:32583', 'PLC_URL': 'http://localhost:32582',
    'MATRIX_IDP_ISSUER': 'http://localhost:38090', 'PARA_API_BASE_URL': 'http://localhost:32584',
    'GROWTHBOOK_FEATURE_OVERRIDES': '{"m8:auth:dev_token_bootstrap":true}', 'RATE_LIMIT_ENABLED': 'false',
    **file_secret('JWT_SECRET', 'm8_jwt'), **file_secret('APP_KEY', 'm8_app_key'),
    **file_secret('COOKIE_SECRET', 'm8_cookie'), **file_secret('CURP_PEPPER', 'm8_pepper'),
    **file_secret('CIVIC_VOTE_PROOF_SECRET', 'm8_civic')},
    m8_names, ['node', 'build/bin/server.js'], volumes=['m8:/data'],
    tmpfs=['/app/build/.adonisjs:uid=1000,gid=1000,mode=0700'],
    depends_on={'m8-init': {'condition': 'service_completed_successfully'}},
    healthcheck={'test': ['CMD', 'node', '-e',
      "fetch('http://127.0.0.1:38787/v1/health', {signal: AbortSignal.timeout(2000)}).then(r => process.exit(r.ok ? 0 : 1)).catch(() => process.exit(1))"],
      'interval': '10s', 'timeout': '3s', 'retries': 3, 'start_period': '20s'})
bridge_db = private_value('bridge_db', db('bridge'))
wrapped('bridge', 'para-chat-staging-bridge:local', {'PORT': '33001',
    'CHAT_OPS_PORT': '33002', 'CHAT_OPS_ORIGIN': 'http://localhost:38000',
    'CHAT_OPS_PROXY_SECRET_FILE': '/run/secrets/chat_ops_proxy_secret',
    'CHAT_OPS_OPERATORS': json.dumps({'staging': 'operator'}),
    'CHAT_OPS_PROBES': json.dumps({'synapse': 'http://localhost:38008/_matrix/client/versions',
      'mas': 'http://localhost:38081/health', 'idp': 'http://localhost:38090/healthz',
      'broker': 'http://localhost:38787/v1/health', 'relay': 'http://localhost:32470/xrpc/_health',
      'appview': 'http://localhost:32584/xrpc/_health'}),
    'PDS_FIREHOSE_URL': 'ws://localhost:32470', 'BRIDGE_PDS_SOURCE_ID': 'chat-staging-rsky-v1',
    'BRIDGE_BACKFILL_FROM_START': '1', 'PLC_URL': 'http://localhost:32582',
    'DATABASE_URL_FILE': '/run/secrets/bridge_db', 'MATRIX_SERVER_NAME': SERVER,
    'MATRIX_HOMESERVER_URL': 'http://localhost:38008', 'MATRIX_PUBLIC_HOMESERVER_URL': 'http://localhost:38008',
    'MATRIX_ADMIN_TOKEN_FILE': '/run/secrets/matrix_admin_token',
    'MATRIX_APPSERVICE_TOKEN_FILE': '/run/secrets/matrix_as_token', 'MATRIX_HS_TOKEN_FILE': '/run/secrets/matrix_hs_token',
    'MATRIX_MAS_ADMIN_URL': 'http://localhost:38081', 'MATRIX_MAS_TOKEN_URL': 'http://localhost:38080/oauth2/token',
    'MATRIX_MAS_CLIENT_ID': MAS_BRIDGE_CLIENT, 'MATRIX_MAS_CLIENT_SECRET_FILE': '/run/secrets/mas_bridge_admin_secret',
    'MATRIX_ENABLE_ENCRYPTION': 'true', 'M8_BASE_URL': 'http://localhost:38787/v1', 'NODE_ENV': 'production'},
    ['bridge_db', 'matrix_admin_token', 'matrix_as_token', 'matrix_hs_token', 'mas_bridge_admin_secret', 'chat_ops_proxy_secret'], ['node', 'dist/index.js'],
    depends_on={'relay': {'condition': 'service_healthy'}, 'postgres': {'condition': 'service_healthy'},
                'mas': {'condition': 'service_started'}, 'synapse': {'condition': 'service_started'}})

service('console', 'para-chat-staging-console:local',
    command=['caddy', 'run', '--config', '/etc/caddy/Caddyfile', '--adapter', 'caddyfile'],
    volumes=[f'{STATE}/config/Caddyfile:/etc/caddy/Caddyfile:ro'])

write(STATE / 'compose.json', json.dumps({'name': PROJECT, 'services': services,
      'volumes': {name: {} for name in ['postgres', 'minio', 'relay', 'pds_cache', 'synapse', 'm8']}}, indent=2))
write(STATE / 'manifest.json', json.dumps({'version': 1, 'project': PROJECT, 'ports': PORTS,
      'serverName': SERVER, 'streamId': 'chat-staging-rsky-v1'}, indent=2))
print(f'Rendered {PROJECT}: persistent local state under {STATE}; credentials withheld.')
