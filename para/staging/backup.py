#!/usr/bin/env python3
"""Coherent snapshot/restore confined to volumes created by this staging project."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tarfile
import time

STATE = Path(__file__).resolve().parents[1] / '.dev/chat-staging'
PROJECT = 'para-chat-staging'
VOLUMES = ['postgres', 'minio', 'relay', 'synapse', 'm8']
COMPOSE = ['docker', 'compose', '-p', PROJECT, '-f', str(STATE / 'compose.json')]
if (STATE / 'monitoring.json').exists():
    COMPOSE += ['-f', str(STATE / 'monitoring.json')]
    VOLUMES += ['chat_prometheus', 'chat_alertmanager', 'chat_alert_receipts']
os.umask(0o077)


def run(args):
    subprocess.run(args, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


parser = argparse.ArgumentParser()
parser.add_argument('action', choices=['snapshot', 'restore'])
parser.add_argument('--snapshot', type=Path)
parser.add_argument('--confirm-staging-restore', action='store_true')
args = parser.parse_args()
config = json.loads((STATE / 'compose.json').read_text())
if (STATE / 'monitoring.json').exists():
    config['services'].update(json.loads((STATE / 'monitoring.json').read_text())['services'])
if config.get('name') != PROJECT:
    raise SystemExit('Refusing a non-staging project')

if args.action == 'snapshot':
    target = STATE / 'snapshots' / time.strftime('%Y%m%dT%H%M%SZ', time.gmtime())
    target.mkdir(mode=0o700)
    # Retain every immutable image before stopping containers. Moving a local
    # build tag must not garbage-collect the only reference to a snapshot image.
    image_ids = {}
    for image in {service['image'] for service in config['services'].values()}:
        value = subprocess.check_output(['docker', 'image', 'inspect', '--format', '{{.Id}}', image], text=True).strip()
        run(['docker', 'tag', value, f"para-chat-staging-snapshot:{value.removeprefix('sha256:')}"])
        image_ids[image] = value
    try:
        if (STATE / 'manifest.json').exists() and json.loads((STATE / 'manifest.json').read_text()).get('publicUrls'):
            from lan_https import stop_proxy
            stop_proxy()
        run(COMPOSE + ['stop', '--timeout', '60'])
        hashes = {}
        for name in VOLUMES:
            volume = f'{PROJECT}_{name}'
            owner = subprocess.check_output(['docker', 'volume', 'inspect', '--format',
                '{{index .Labels "com.docker.compose.project"}}', volume], text=True).strip()
            if owner != PROJECT:
                raise RuntimeError('Refusing a volume owned by another project')
            run(['docker', 'run', '--rm', '-v', f'{volume}:/data:ro', '-v', f'{target}:/backup',
                 'alpine:3.20', 'tar', '-czf', f'/backup/{name}.tar.gz', '-C', '/data', '.'])
            archive = target / f'{name}.tar.gz'
            archive.chmod(0o600)
            hashes[archive.name] = digest(archive)
        with tarfile.open(target / 'state.tar.gz', 'w:gz') as archive:
            for name in ['config', 'secrets', 'compose.json', 'manifest.json', 'fixtures.json', 'tls', 'monitoring.json', 'monitoring']:
                if (STATE / name).exists():
                    archive.add(STATE / name, arcname=name)
        hashes['state.tar.gz'] = digest(target / 'state.tar.gz')
        (target / 'snapshot.json').write_text(json.dumps({'version': 2, 'project': PROJECT,
            'sha256': hashes, 'images': image_ids, 'volumes': VOLUMES}, indent=2))
        for path in target.iterdir():
            path.chmod(0o600)
        print(f'Coherent staging snapshot: {target}')
    finally:
        if json.loads((STATE / 'manifest.json').read_text()).get('publicUrls'):
            from lan_https import start_proxy
            start_proxy()
        run(COMPOSE + ['up', '-d'])
else:
    if not args.confirm_staging_restore or args.snapshot is None:
        raise SystemExit('Restore requires --snapshot and --confirm-staging-restore')
    target = args.snapshot.resolve()
    if not target.is_relative_to((STATE / 'snapshots').resolve()):
        raise SystemExit('Snapshot must belong to this staging project')
    manifest = json.loads((target / 'snapshot.json').read_text())
    if manifest.get('version') not in (1, 2) or manifest.get('project') != PROJECT or manifest.get('volumes') != VOLUMES:
        raise SystemExit('Snapshot topology does not match staging')
    if manifest['version'] == 1 and json.loads((STATE / 'manifest.json').read_text()).get('publicUrls'):
        raise SystemExit('A legacy snapshot omits the active LAN certificate unit; restore it in its matching loopback profile')
    expected = {f'{name}.tar.gz' for name in VOLUMES} | {'state.tar.gz'}
    if set(manifest['sha256']) != expected:
        raise SystemExit('Snapshot member set does not match staging')
    for name, value in manifest['sha256'].items():
        if digest(target / name) != value:
            raise SystemExit('Snapshot integrity check failed')
        with tarfile.open(target / name) as archive:
            for member in archive.getmembers():
                path = Path(member.name)
                if path.is_absolute() or '..' in path.parts or not (member.isfile() or member.isdir()):
                    raise SystemExit('Unsafe snapshot archive member')
    for image, value in manifest['images'].items():
        actual = subprocess.check_output(['docker', 'image', 'inspect', '--format', '{{.Id}}', value], text=True).strip()
        if actual != value:
            raise SystemExit('Restore requires the same staging image versions')
    # Compose treats a bare sha256 manifest identifier as a repository name
    # after its original tag moves. Pin private local tags before touching data.
    restore_tags = {}
    for value in set(manifest['images'].values()):
        tag = f"para-chat-staging-restore:{value.removeprefix('sha256:')}"
        run(['docker', 'tag', value, tag])
        actual = subprocess.check_output(['docker', 'image', 'inspect', '--format', '{{.Id}}', tag], text=True).strip()
        if actual != value:
            raise SystemExit('Restored image tag does not match its immutable snapshot identifier')
        restore_tags[value] = tag
    if json.loads((STATE / 'manifest.json').read_text()).get('publicUrls'):
        from lan_https import stop_proxy
        stop_proxy()
    run(COMPOSE + ['down', '--timeout', '60', '--volumes'])
    # Mounted config is read-only for containers. Permit replacing our own
    # private host files before extracting the verified snapshot over them.
    for directory in ['config', 'secrets', 'tls', 'monitoring']:
        for path in (STATE / directory).rglob('*'):
            if path.is_file() and not path.is_symlink():
                path.chmod(0o600)
    with tarfile.open(target / 'state.tar.gz', 'r:gz') as archive:
        archive.extractall(STATE, filter='data')
    for name in ['compose.json', 'monitoring.json']:
        path = STATE / name
        if not path.exists(): continue
        restored = json.loads(path.read_text())
        for service in restored['services'].values():
            service['image'] = restore_tags[manifest['images'][service['image']]]
            service['pull_policy'] = 'never'
        path.write_text(json.dumps(restored, indent=2))
    run(COMPOSE + ['create'])
    for name in VOLUMES:
        volume = f'{PROJECT}_{name}'
        run(['docker', 'run', '--rm', '-v', f'{volume}:/data', '-v', f'{target}:/backup:ro',
             'alpine:3.20', 'tar', '-xzf', f'/backup/{name}.tar.gz', '-C', '/data'])
    if json.loads((STATE / 'manifest.json').read_text()).get('publicUrls'):
        from lan_https import start_proxy
        start_proxy()
    run(COMPOSE + ['up', '-d'])
    print('Restored isolated staging; verify prior accounts, records, room IDs, sessions and relay/bridge checkpoints.')
