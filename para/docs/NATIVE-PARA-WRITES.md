# Native PARA writes in VLPDS

**Production readiness: existing-user migration remains NO-GO.** The previously
disabled founder, iM8 and alpha endpoints now have native implementations, along
with reviewed public-repository and private-alpha imports. The release still needs
full PARA client-flow validation, a real migration rehearsal, wallet/device and
authentication-protocol review, and real AppView/Matrix/M8/recovery validation.
All nodes must use the compatible native fork; upstream or mixed-version rollback
is unsupported once independent wallet factors are enrolled.

The PARA fork implements repository commands in Rust. VLPDS owns authentication,
OAuth/app-password scopes, signing and durable commits. There is no second PDS,
command sidecar or alternate write database. The application can keep its existing
XRPC calls and `atproto-proxy` header: registered commands run locally before the
proxy fallback. Read methods continue through the AppView.

## Implemented commands

- `com.para.community.createBoard`: board, owner membership, initial governance,
  two authority events, founder list, list item and starter pack in one commit.
  Repeating the same creator/name/quadrant returns the existing board.
- `join` and `leave`: update only the authenticated member's repository, preserve
  roles, refuse locked memberships and owner departure. Assembly admission creates
  a pending membership rather than bypassing approval. Membership keys cannot alias
  boards in different repositories.
- `createBriefingPack` and `updateBriefingPack`: preserve ownership and creation
  metadata; honor the caller's CID precondition.
- `shareContent`, `removeSharedContent`, `restoreSharedContent`: require active
  membership and, for moderation, a steward role. Authors cannot forge `sharedBy`.
- `com.para.civic.castVote`: checks phase, deadline, options, membership and edit
  window, then verifies M8 authorization before writing. The edit-window check also
  reads the author's local votes, so ingestion lag cannot reset the window.
- `acceptDraftInvite`: accepts a creator-signed founder-list invitation and writes
  only the invitee's membership. The creator uses `vlpds.para.inviteFounder` and
  `vlpds.para.finalizeFounders`; nine accepted founders, including the creator, are
  required for activation. Acceptance is idempotent. This workflow requires all
  founders to use this PDS, including different nodes of the same cluster.
- `account.getAuthFactor` / `setAuthFactor`: independent iM8 wallet challenges,
  purpose/epoch/account/PDS binding, durable single use and atomic session
  revocation. Password, app-password, passkey and OAuth login paths enforce it.
  Factor removal requires the password and a fresh wallet removal approval.
- `alpha.requestAccess`, `createInvite`, `getAccess`, `getRolloutStatus`: private
  durable approvals, FIFO waitlists, single-use invitations and quota allocation.
  Administration requires the admin credential. Fresh states start closed;
  `vlpds.para.setAlphaRollout` configures capacity. Invite issuance cannot waive
  capacity, including the separate FRIEND quota.

PARA lexicons are bundled from WatZappa, including request and response contracts.
Regenerate with `python3 para/scripts/sync-native-lexicons.py`; verify with `--check`.
Business-policy changes must be reviewed against WatZappa's common policy and PDS
handlers. The Rust and TypeScript implementations are now separate maintenance
obligations, so upstream updates require the Rust regression suite. The
`PARA native writes` workflow runs formatting, clippy and the native, CRUD and
app-password regression tests on fork changes.

## Commit and policy guarantees

A command captures the authenticated repository's head before reading state. Every
planned record is encoded and validated, its OAuth/app-password scope is checked,
and the records are submitted together to the existing worker with `swap_commit`.
The worker signs and logs the commit using VLPDS's normal durability guarantees.
Concurrent changes return `InvalidSwap`; the client must retry from current state.
No handler writes into the community creator's repository on behalf of an invitee.

Generic `createRecord`, `putRecord` and `applyWrites` also run the policy gate when
PARA mode is enabled. `validate: false` cannot waive it. Frozen public ballots,
proof-bearing public reactions and reposts are refused. Civic ballots and civic
delegations require the same M8 claim shapes as the TypeScript implementation and
successful remote verification. Authority events cannot be updated or deleted.
Membership deletion is refused so a client cannot erase locked membership state;
leaving preserves the authorization history. Raw board creation and identity or
governance-mode transitions require dedicated commands so clients cannot bypass
the community creation recipe or name registry. Structured role assignments and
chamber assignments cannot be self-granted, and expired roles do not authorize
moderation. Horizontal governance changes require a current facilitator and bounded,
ratified role terms. Deletes of old frozen ballots remain allowed.

M8 verifiers must use HTTPS or loopback HTTP. Requests contain the bound repository
DID and the claim, never an account JWT. Redirects are refused; `204` accepts,
`422` rejects, and missing configuration, timeout or another status refuses the
write with `ParaVerifierUnavailable`. AppView lookups use short-lived,
method-bound service JWTs and bounded response bodies. AppView indexing checks
remain necessary for records from foreign PDSes.

Community names are reserved with an object-store conditional create under
`<prefix>/para/community-names/<sha256(normalized-name)>`, after record validation.
This avoids races between local nodes while AppView ingestion is catching up.
An interrupted creator can retry using the reserved board key. A reservation is
kept after an ambiguous commit failure: operators must verify the repository and
name ownership before removing an abandoned reservation. Reservations belong in
bucket recovery and must not be removed by lifecycle policies. This registry is
local to this PDS; foreign name availability remains an AppView check and cannot
provide atomic uniqueness across independent PDSes. Names use Spanish accent and
combining-mark normalization; extending it to every Unicode script is separate
work. Command collection scans stop at 1,000 records or 8 MiB of stored bytes and fail
explicitly.

## Migration and client compatibility

Unreviewed `importRepo` requests remain refused with `ParaImportReviewRequired`.
The operator calls `vlpds.para.reviewImport` with `did`, `sha256`, `sourceCommit`,
`sourceSigningKey` (original `did:key`), `expectedHead`, `historicalExceptions`
(exact record CIDs), `requireIm8Enrollment` and a review `reason`. Pin the original
source key from the reviewed DID history before moving identity; this endpoint
does not establish the key's historical ownership for the operator.

The review expires after 15 minutes. The owner, with account repo-manage
permission, uploads that exact CAR using `para-import-review: <reviewToken>` to
`com.atproto.repo.importRepo` while the destination is deactivated. The source DID,
commit, signature, CAR hash and record schemas are checked. Exceptions identify
specific historical records; they do not reopen frozen write methods. Review the
privacy implications of retaining old public ballots. Names are seeded into the
bucket registry and the worker checks the reviewed destination head when beginning
the staged import. The approval is single use; an ambiguous failure requires head
inspection and another review. The reviewed path currently caps CARs at 64 MiB;
larger snapshots need a reviewed streaming extension before migration.

`requireIm8Enrollment: true` records a private activation gate for source accounts
that used iM8. They can enroll while inactive, but cannot activate before an
independent wallet factor exists. A raw CAR does not migrate passwords, email
verification/factors, app passwords, identity rotation keys or Matrix state. Do not
enable migrated accounts until those separate obligations are reviewed and met.

Alpha data has its own private migration. Before configuring any rollout or
issuing new codes, export the stopped/quiesced source with
`python3 para/scripts/export-alpha-state.py --db <account.sqlite> --output
<private-snapshot.json>`. It reads a coherent SQLite transaction without modifying
the source and creates a mode-0600 file. The operator uploads it to
`vlpds.para.importAlphaState`. The destination must be empty; quota counts, used
invites and approved requests must agree. Contradictory source data is rejected,
including invitation races from the old implementation; repair and review it
explicitly. The import commits all private rows together and recreates stable
waitlist positions. It is bounded to 8 MiB, 10,000 requests and 20,000 invites.
The state remains part of normal bucket/log recovery.

The founder client must authorize invitations and offer creator finalization;
`requiresCreatorFinalization` is returned when quorum is ready. Password clients
must support wallet approval tokens rather than M8 session tokens. PDS Security
and sign-in pages include the independent flow, and iM8 can approve while signed
out of M8. Test the paired PARA UI and physical wallet before production. See
[IM8-WALLET-FACTOR.md](IM8-WALLET-FACTOR.md) for protocol and recovery details.

The native suite uses real VLPDS workers, peer mTLS, bucket-log handoff/restart and
the actual JavaScript wallet signature format. AppView and civic-proof verifier
services in that suite are fixtures; it does not certify live service behavior,
production object-store failure modes, mixed-version safety or a production cutover.

The current local verification passed 18 native PARA tests, the existing CRUD,
app-password, OAuth, passkey, import, format, console and statistics regressions,
and Clippy across all Rust targets. The iM8 suite passed 203 tests plus typechecking
and lint. The generated WatZappa API and the PDS UI built successfully. The Linux
image `para-vlpds:para-native-local` was rebuilt; its JavaScript-wallet smoke
verified enrollment/login, replay refusal and private alpha authorization. Run the
smoke from the sibling iM8 checkout with `pnpm exec tsx
../vlpds/para/scripts/native-wallet-image-smoke.mts`. It creates and removes its
own isolated, memory-only container and uses synthetic credentials.

## Build and enable

The default upstream image does not contain these handlers. The PARA Compose
profiles now build the local fork as `para-vlpds:para-native-local` and set
`VLPDS_PARA=true`; `.env` from an older deployment must be updated explicitly.
Preflight refuses the known upstream image when native PARA is enabled.
Production should use a CI-built immutable fork tag/digest after release review.

```bash
# from the VLPDS repository
cargo check --locked --no-default-features
cargo test --locked --test all para::
python3 para/scripts/sync-native-lexicons.py --check

# from para/: builds the fork and the existing development services
make dev
make dev-smoke
```

Configure the operator-controlled URLs in `para/env/vlpds.env` (omit unset entries):

```dotenv
VLPDS_PARA_VOTE_VERIFIER_URL=https://m8.example.org/civic/votes/verify
VLPDS_PARA_DELEGATION_VERIFIER_URL=https://m8.example.org/civic/delegations/verify
```

Use the actual verification endpoints exposed by your M8 deployment. Development
loads `para/env/vlpds-dev.env` instead, so production credentials are not inherited
by the development PDS. The native
flag defaults to false outside the PARA profiles, preserving upstream behavior.

## Why a relay remains

The patched Rust rsky-relay is the relay implementation used here. VLPDS assigns cursors as
`unix_micros * 256 + writer`, roughly `4.6e17`, beyond JavaScript's exact integer
range (`2^53 - 1`). The current TypeScript sync consumer rejects these cursors.
The relay consumes the 64-bit VLPDS stream and republishes it with its own sequence
numbers that these consumers accept. Its cursor and retention state are separate
from VLPDS's cursor; consumers must persist the relay cursor and never substitute
or truncate a VLPDS cursor.

The TypeScript PDS has a central sequencer producing small consecutive values,
so the same consumers can connect directly. Rust itself does not require Indigo,
and Indigo is not part of the custom-write path. A future end-to-end 64-bit cursor
migration across decoding, validation, persistence and reconnection in every
consumer could remove the local relay. Keeping a compatible relay is the smaller
change while the PARA write port is being validated.

The pinned source, PARA patch, deployment defaults and cursor migration procedure
are described in [RSKY-RELAY.md](RSKY-RELAY.md).
