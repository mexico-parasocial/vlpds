#!/usr/bin/env python3
"""List communities founded under the retired horizontal governance model.

WatZappa retired horizontal governance. Its AppView migration
`20261105T120000000Z-drop-board-governance-mode` rebuilds the authority of every
board whose record says `governanceMode: "horizontal"` and leaves it WITHOUT AN
OWNER: the creator was founded as a 90-day moderator, and that grant is not a
founding owner grant under the remaining rules. VLPDS cannot repair this: an
authority event must be issued by the repository owner, and an owner is restored
only by an `owner.recover` decision (a cabildeo that carries a `protectedAction`
and closes with quorum, voting open for at least 72 hours).

This script is read-only. It uses public XRPC (listRepos, listRecords) and writes
nothing, so run it before the AppView migration to know which communities need a
recovery cabildeo.

    python3 para/scripts/list-horizontal-boards.py --pds https://pds.example.org
"""
import argparse
import json
import sys
import urllib.error
import urllib.parse
import urllib.request

BOARD = 'com.para.community.board'
AUTHORITY = 'com.para.community.authorityEvent'

parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
parser.add_argument('--pds', required=True, help='base URL of the VLPDS node (https, or loopback http)')
parser.add_argument('--timeout', type=float, default=15.0)
parser.add_argument('--output', help='write the JSON report here instead of stdout')
args = parser.parse_args()
base = args.pds.rstrip('/')
host = urllib.parse.urlsplit(base)
if host.scheme not in ('https', 'http') or (host.scheme == 'http' and host.hostname not in ('localhost', '127.0.0.1', '::1')):
    parser.error('--pds must be https, or http on a loopback host')


def get(method, **params):
    query = urllib.parse.urlencode({k: v for k, v in params.items() if v is not None})
    with urllib.request.urlopen(f'{base}/xrpc/{method}?{query}', timeout=args.timeout) as response:
        return json.load(response)


def pages(method, key, **params):
    cursor = None
    while True:
        result = get(method, cursor=cursor, **params)
        yield from result.get(key, [])
        cursor = result.get('cursor')
        if not cursor:
            return


def records(did, collection):
    try:
        yield from pages('com.atproto.repo.listRecords', 'records', repo=did, collection=collection, limit=100)
    except urllib.error.HTTPError as error:
        if error.code not in (400, 404):  # an inactive or empty repository has nothing to list
            raise


def scan():
    for repo in pages('com.atproto.sync.listRepos', 'repos', limit=1000):
        yield repo


affected = []
scanned = 0
try:
    repos = list(scan())
except (urllib.error.URLError, OSError, ValueError) as error:
    sys.exit(f'cannot list repositories on {base}: {getattr(error, "reason", error)}')
for repo in repos:
    if repo.get('active') is False:
        continue
    did = repo['did']
    scanned += 1
    try:
        boards = [r for r in records(did, BOARD) if r['value'].get('governanceMode') == 'horizontal']
        events = list(records(did, AUTHORITY)) if boards else []
    except (urllib.error.URLError, OSError, ValueError) as error:
        sys.exit(f'cannot read records of {did}: {getattr(error, "reason", error)}')
    if not boards:
        continue
    for board in boards:
        mine = [e['value'] for e in events if e['value'].get('community') == board['uri']]
        affected.append(
            {
                'board': board['uri'],
                'name': board['value'].get('name'),
                'creator': did,
                'status': board['value'].get('status'),
                'authority': [
                    {k: e.get(k) for k in ('action', 'subject', 'basis', 'effectiveAt', 'expiresAt')} for e in mine
                ],
                # The founding grant of a horizontal board is `moderator.grant` with an expiry.
                'hasOwnerGrant': any(e.get('action') == 'owner.grant' for e in mine),
            }
        )

report = {'scannedRepos': scanned, 'horizontalBoards': affected}
text = json.dumps(report, indent=2, sort_keys=True) + '\n'
if args.output:
    with open(args.output, 'w') as out:
        out.write(text)
else:
    sys.stdout.write(text)
print(f'{len(affected)} horizontal board(s) in {scanned} repositories', file=sys.stderr)
