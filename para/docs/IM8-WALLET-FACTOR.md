# Independent iM8 wallet authentication

M8 obtains sessions through PDS OAuth. Requiring an M8 session token to enter
that same OAuth flow would create a login loop. The native factor uses wallet
signatures directly, with no broker-session dependency.

The PDS stores an sr25519 authentication public key in private account state.
iM8 derives a dedicated key from its backed-up seed, the account DID and the PDS
service DID. It does not reuse civic, anonymous, public-persona or INE holder keys.
The client uses its existing `@scure/sr25519`; Rust uses pinned `schnorrkel 0.11.5`.
The checked-in JavaScript vector is verified by Rust.

## Enrollment, sign-in and removal

Open the PDS `/account` Security page. Copy its account DID and server identity
into iM8's account sign-in approval panel, and copy the authentication public key
back to the PDS. The wallet requires a confirmed phrase backup for enrollment.

- Enrollment starts with `vlpds.para.startIm8Enrollment`, authenticated by a full
  account session, with `{ "publicKey": "<64 lowercase hex characters>" }`.
- Login starts with public `vlpds.para.getIm8Challenge?identifier=<handle-or-DID>`.
  The PDS OAuth and account sign-in pages also display a fresh challenge.
- Removal starts with authenticated `vlpds.para.startIm8Removal`.

Each challenge is an HMAC-authenticated PDS JWT with a 90-second lifetime. It binds
the PDS, account, purpose (`enroll`, `login`, `disable`), credential epoch and key.
The wallet displays the account/server/action and signs the exact UTF-8 bytes
`para-pds/auth/v1\n<challenge JWT>` with the sr25519 substrate transcript context.
The result is `im8:` followed by unpadded base64url JSON containing `challenge`,
`publicKey` and `signature` (64-byte lowercase hex). The proof has a 4,096-character
cap and its nonce is consumed durably using private CAS, with the credential epoch
and factor row as conditions. A proof cannot be reused after failover or restart.

`com.para.account.setAuthFactor` takes `authFactorType: "im8"` for enrollment,
or omits it for removal, plus `password` and `authFactorToken`. OAuth/app-password
credentials cannot manage the factor. Password clients send a login approval as
`com.atproto.server.createSession.authFactorToken`. OAuth pages accept it at the
second-factor step; passkey session calls accept the same `authFactorToken`.

Factor changes replace the credential epoch and revoke password/OAuth sessions
and trusted devices in the same private transaction. Statistics use a separate
account index flag; authentication reads the actual private factor row. If the
index update fails after the factor commits, sign in again and inspect the factor
rather than assuming the operation changed nothing. Trusted browsers and app
passwords do not waive an enrolled wallet factor. Existing local email/TOTP/passkey
factors are retained; turning off iM8 restores the normal local-factor policy.

## Recovery and release obligations

Restoring the same wallet phrase recovers the key when the account and service DIDs
are unchanged. The signed-out iM8 screen supports phrase restoration and approval,
so recovery does not require a working M8 session. Email password resets preserve
the wallet factor. The existing operator second-factor reset clears iM8 and always
revokes sessions when an iM8 factor existed; use the audited recovery procedure.

Existing session-token iM8 flags cannot be copied as native wallet factors. The
reviewed import manifest must state `requireIm8Enrollment` from the source private
account state; migrated accounts remain inactive until required reenrollment.

This new authentication protocol requires review and device testing before release.
Signatures are single use and scoped; this document does not claim resistance to
all approval phishing. Confirm the displayed account, server and requested action.
Deploy the compatible native fork on every node before enrollment. A rollback to
an upstream binary that cannot enforce these private rows is unsupported. Disabling
the PARA flag in the native binary continues to enforce already-enrolled factors.
