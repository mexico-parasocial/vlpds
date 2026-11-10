# PARA lexicon source

- Canonical source: WatZappa `lexicons/com/para/`.
- Source commit: `cf6dc45164f1a245cb3c4826de2320412d29c1c1` on
  `feat/od10-policy-ballots` (not yet on WatZappa `main`).
- Bundle: `para.json`, 156 documents.
- Bundle SHA-256: `8127d1b77662f1de20c099a0ceb89305b66eae2a74e6d8e8ac00613960b93040`.
- The source retires the horizontal governance model (`governanceMode`,
  `governanceConfig`) and the legacy `community.proposal`, `deliberation` and
  `decision` records, adds cabildeo `kind` / `protectedAction`, conclusions and
  follow-through, `community.roleOffer`, and states that a policy ballot's
  `signal` is never 0.
- Regeneration: `python3 para/scripts/sync-native-lexicons.py --source
  <path-to-watzappa>/lexicons/com/para`.
- Drift check: append `--check`. The script's default still uses the adjacent
  WatZappa checkout, so specify `--source` until that checkout is consolidated.

Schema changes belong in WatZappa first. Update this receipt when regenerating.
