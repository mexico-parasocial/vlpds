# PARA lexicon source

- Canonical source: WatZappa `lexicons/com/para/`.
- Source commit: `c3b0b6425d9df8312c7ed0bf66b3994104eb4897` on
  `watzappa-permissioned-spaces`.
- Bundle: `para.json`, 153 documents.
- Bundle SHA-256: `55add45e0f75916fe94386a22463cfc9254fc7fc3519fab8fa47f2dfcb7ba998`.
- The source includes optional wallet-factor and founder-finalization fields,
  the newer community `region` contract, and Spaces collection schemas.
- Regeneration: `python3 para/scripts/sync-native-lexicons.py --source
  <path-to-watzappa>/lexicons/com/para`.
- Drift check: append `--check`. The script's default still uses the adjacent
  WatZappa checkout, so specify `--source` until that checkout is consolidated.

Schema changes belong in WatZappa first. Update this receipt when regenerating.
