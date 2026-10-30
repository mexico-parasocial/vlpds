#!/usr/bin/env python3
"""Vendor PARA's canonical schemas; --check catches drift without modifying files."""
import argparse
import json
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--source', type=Path, default=Path(__file__).resolve().parents[3] / 'WatZappa/lexicons/com/para')
parser.add_argument('--check', action='store_true')
args = parser.parse_args()
if not args.source.is_dir():
    parser.error(f'Lexicon directory does not exist: {args.source}')
docs = {}
for path in sorted(args.source.rglob('*.json')):
    doc = json.loads(path.read_text())
    nsid = doc['id']
    if not nsid.startswith('com.para.') or nsid in docs:
        parser.error(f'Invalid or duplicate PARA lexicon: {path}')
    docs[nsid] = doc
if not docs:
    parser.error('No PARA schemas found')
output = Path(__file__).resolve().parents[2] / 'lexicons/para.json'
content = json.dumps(docs, sort_keys=True, separators=(',', ':')) + '\n'
if args.check:
    if output.read_text() != content:
        parser.exit(1, 'PARA schemas have drifted; run para/scripts/sync-native-lexicons.py\n')
else:
    output.write_text(content)
print(f'{len(docs)} canonical PARA schemas verified' if args.check else f'{len(docs)} canonical PARA schemas bundled')
