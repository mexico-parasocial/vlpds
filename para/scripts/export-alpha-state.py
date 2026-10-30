#!/usr/bin/env python3
"""Read a coherent WatZappa alpha SQLite snapshot; never modify the source."""
import argparse
import datetime as dt
import json
import os
from pathlib import Path
import sqlite3
import tempfile
from urllib.parse import quote


def timestamp(value):
    if value is None:
        return None
    parsed = dt.datetime.fromisoformat(str(value).replace('Z', '+00:00'))
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=dt.timezone.utc)
    return parsed.astimezone(dt.timezone.utc).isoformat(timespec='milliseconds').replace('+00:00', 'Z')


def snapshot(db_path):
    uri = 'file:' + quote(str(db_path.resolve())) + '?mode=ro'
    with sqlite3.connect(uri, uri=True) as db:
        db.row_factory = sqlite3.Row
        db.execute('PRAGMA query_only=ON')
        db.execute('BEGIN')
        rollouts = [dict(r) for r in db.execute('SELECT state, totalSlots, usedSlots, isOpen, openedAt FROM alpha_rollout ORDER BY state')]
        requests = [dict(r) for r in db.execute("SELECT did, state, status, createdAt, approvedAt, inviteCode FROM alpha_access_request WHERE status IN ('approved', 'pending') ORDER BY createdAt, did")]
        invites = [dict(r) for r in db.execute('SELECT code, state, did, createdBy, createdAt, usedAt FROM alpha_invite ORDER BY code')]
        db.rollback()
    for row in rollouts:
        row['isOpen'] = bool(row['isOpen'])
        row['openedAt'] = timestamp(row['openedAt'])
    for row in requests:
        row['createdAt'] = timestamp(row['createdAt'])
        row['approvedAt'] = timestamp(row['approvedAt'])
    for row in invites:
        row['createdAt'] = timestamp(row['createdAt'])
        row['usedAt'] = timestamp(row['usedAt'])
    # Older seeds omitted FRIEND. Preserve allocated friend access without
    # opening additional capacity; the operator can increase it explicitly.
    if not any(r['state'] == 'FRIEND' for r in rollouts):
        count = sum(r['state'] == 'FRIEND' and r['did'] is not None for r in invites)
        rollouts.append({'state': 'FRIEND', 'totalSlots': count, 'usedSlots': count, 'isOpen': False, 'openedAt': None})
    return {'version': 1, 'rollouts': rollouts, 'requests': requests, 'invites': invites}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--db', required=True, type=Path, help='WatZappa account-manager SQLite database')
    parser.add_argument('--output', required=True, type=Path, help='Private snapshot file (created with mode 0600)')
    args = parser.parse_args()
    if not args.db.is_file():
        parser.error('Source database does not exist')
    if args.output.exists():
        parser.error('Refusing to overwrite an existing snapshot')
    data = snapshot(args.db)
    content = json.dumps(data, separators=(',', ':')).encode() + b'\n'
    if len(content) > 8 << 20 or len(data['requests']) > 10_000 or len(data['invites']) > 20_000:
        parser.error('Snapshot exceeds native import bounds; split/review migration tooling before proceeding')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=args.output.parent, delete=False) as tmp:
        name = tmp.name
        os.fchmod(tmp.fileno(), 0o600)
        tmp.write(content)
        tmp.flush()
        os.fsync(tmp.fileno())
    try:
        # Atomic create, with no overwrite race after the existence check.
        os.link(name, args.output)
    finally:
        os.unlink(name)
    print(f"Created private alpha snapshot: {len(data['requests'])} requests, {len(data['invites'])} invites")


if __name__ == '__main__':
    main()
