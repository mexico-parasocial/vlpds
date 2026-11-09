//! An independent wallet factor: never require an M8 session obtained through
//! this PDS's own OAuth flow. Challenges bind account, PDS, key, purpose and
//! credential epoch. Verification and replay consumption happen in Rust.
use super::*;
use crate::xrpc::cas::{Cond, Op};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use sha2::{Digest, Sha256};

pub(crate) const ROW: &str = "ctl/para-im8";
pub(crate) const MIGRATION_REQUIRED: &str = "ctl/para-im8-migration";
const PREFIX: &str = "im8:";
const DOMAIN: &str = "para-pds/auth/v1";
const TTL: u64 = 90;

#[derive(Deserialize, serde::Serialize)]
pub(super) struct Factor {
    pub public_key: String,
    pub created_at: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Proof {
    challenge: String,
    public_key: String,
    signature: String,
}

async fn load(app: &App, did: &str) -> XResult<(Option<Factor>, Option<Bytes>)> {
    let raw = app.get_private(did, ROW).await?;
    let factor = raw.as_ref().map(|v| serde_json::from_slice(v).map_err(XrpcError::from_err)).transpose()?;
    Ok((factor, raw))
}

async fn setup_account(app: &App, did: &str) -> XResult<()> {
    let a = app.account(did).await?;
    if a.status.as_deref().is_some_and(|s| s != "deactivated") {
        return Err(XrpcError::auth("Account is unavailable for factor setup"));
    }
    Ok(())
}

pub(crate) async fn check_activation(app: &App, did: &str) -> XResult<()> {
    if app.get_private(did, MIGRATION_REQUIRED).await?.is_some() && !enabled(app, did).await? {
        return Err(XrpcError::bad(
            "ParaIm8EnrollmentRequired",
            "Enroll an independent iM8 wallet factor before activating this migrated account",
        ));
    }
    Ok(())
}

pub(crate) async fn enabled(app: &App, did: &str) -> XResult<bool> {
    // Enrolled factors remain enforced even if PARA is disabled later.
    Ok(load(app, did).await?.0.is_some())
}

fn bound(epoch: &str, key: &str) -> String {
    hex::encode(Sha256::digest(format!("{epoch}\0{key}").as_bytes()))
}

fn public_key(value: &str) -> XResult<schnorrkel::PublicKey> {
    if value.len() != 64
        || value.bytes().all(|c| c == b'0')
        || value.bytes().any(|c| !c.is_ascii_digit() && !(b'a'..=b'f').contains(&c))
    {
        return Err(required());
    }
    let bytes = hex::decode(value).map_err(|_| required())?;
    schnorrkel::PublicKey::from_bytes(&bytes).map_err(|_| required())
}

pub(crate) fn required() -> XrpcError {
    XrpcError {
        status: StatusCode::UNAUTHORIZED,
        error: "AuthFactorTokenRequired".into(),
        message: "Approve this sign-in with your enrolled iM8 wallet".into(),
    }
}

fn challenge(app: &App, did: &str, purpose: &str, epoch: &str, key: &str) -> Json<J> {
    let nonce = hex::encode(rand::random::<[u8; 32]>());
    let token = app.jwt.issue_scoped(
        did,
        &format!("para.im8.{purpose}"),
        Some(&bound(epoch, key)),
        TTL,
        "im8+jwt",
        Some(&nonce),
    );
    Json(
        json!({"challenge":token,"message":format!("{DOMAIN}\n{token}"),"did":did,"audience":app.jwt.service_did,"publicKey":key,"purpose":purpose,"expiresIn":TTL}),
    )
}

pub(super) async fn start_enrollment(
    State(app): AppState,
    Auth(creds): Auth,
    Json(input): Json<J>,
) -> XResult<Json<J>> {
    super::enabled(&app)?;
    let did = super::super::server::full_access(&creds)?;
    setup_account(&app, &did).await?;
    let key = text(&input, "publicKey")?;
    public_key(key)?;
    if enabled(&app, &did).await? {
        return Err(XrpcError::bad("Im8AlreadyEnrolled", "Remove the current iM8 factor before enrolling another"));
    }
    let epoch = super::super::server::auth_epoch(&app, &did).await?;
    Ok(challenge(&app, &did, "enroll", &epoch, key))
}

pub(super) async fn start_removal(State(app): AppState, Auth(creds): Auth) -> XResult<Json<J>> {
    super::enabled(&app)?;
    let did = super::super::server::full_access(&creds)?;
    setup_account(&app, &did).await?;
    let factor = load(&app, &did).await?.0.ok_or_else(required)?;
    let epoch = super::super::server::auth_epoch(&app, &did).await?;
    Ok(challenge(&app, &did, "disable", &epoch, &factor.public_key))
}

#[derive(Deserialize)]
pub(super) struct ChallengeQuery {
    identifier: String,
}

pub(super) async fn get_challenge(State(app): AppState, Query(q): Query<ChallengeQuery>) -> XResult<Json<J>> {
    crate::ratelimit::check_with_ip(&[&crate::ratelimit::CREATE_SESSION_5MIN], q.identifier.trim(), 1)?;
    let acct = super::super::server::login_account(&app, &q.identifier).await?.ok_or_else(required)?;
    setup_account(&app, &acct.did).await?;
    Ok(Json(login_challenge(&app, &acct.did).await?))
}

pub(crate) async fn login_challenge(app: &App, did: &str) -> XResult<J> {
    let factor = load(app, did).await?.0.ok_or_else(required)?;
    let epoch = super::super::server::auth_epoch(app, did).await?;
    Ok(challenge(app, did, "login", &epoch, &factor.public_key).0)
}

struct Checked {
    nonce_row: String,
    until: u64,
    public_key: String,
}

fn verify(app: &App, did: &str, purpose: &str, epoch: &str, key: Option<&str>, token: &str) -> XResult<Checked> {
    if token.len() > 4096 {
        return Err(required());
    }
    let bytes = B64.decode(token.strip_prefix(PREFIX).ok_or_else(required)?).map_err(|_| required())?;
    let proof: Proof = serde_json::from_slice(&bytes).map_err(|_| required())?;
    let claims = app.jwt.verify_signature(&proof.challenge).ok_or_else(required)?;
    let now = super::super::server::now_secs();
    if claims.sub != did
        || claims.aud != app.jwt.service_did
        || claims.scope != format!("para.im8.{purpose}")
        || claims.exp <= now
        || claims.exp > now + TTL
        || claims.iat > now
        || claims.iat + TTL != claims.exp
        || claims.app_pass_scope.as_deref() != Some(bound(epoch, &proof.public_key).as_str())
        || key.is_some_and(|key| key != proof.public_key)
    {
        return Err(required());
    }
    let nonce = claims
        .jti
        .as_deref()
        .filter(|s| s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit()))
        .ok_or_else(required)?;
    let public = public_key(&proof.public_key)?;
    let signature = hex::decode(&proof.signature)
        .ok()
        .filter(|b| b.len() == 64)
        .and_then(|b| schnorrkel::Signature::from_bytes(&b).ok())
        .ok_or_else(required)?;
    public
        .verify_simple(b"substrate", format!("{DOMAIN}\n{}", proof.challenge).as_bytes(), &signature)
        .map_err(|_| required())?;
    // Reuse the expiring, durable OAuth replay-row format and its GC.
    let hash = crate::oauth::util::sha256_b64u(format!("para-im8:{nonce}"));
    Ok(Checked {
        nonce_row: format!("{}{hash}", crate::oauth::util::REPLAY_ROW),
        until: claims.exp,
        public_key: proof.public_key,
    })
}

pub(crate) async fn check_login(app: &App, did: &str, token: Option<&str>) -> XResult<()> {
    let epoch = super::super::server::auth_epoch(app, did).await?;
    let (factor, raw) = load(app, did).await?;
    let Some(factor) = factor else {
        return Ok(());
    };
    let checked = verify(app, did, "login", &epoch, Some(&factor.public_key), token.ok_or_else(required)?)?;
    let out = app
        .private_cas(
            did,
            vec![super::super::server::auth_epoch_cond(&epoch), Cond::eq(ROW, raw), Cond::eq(&checked.nonce_row, None)],
            vec![Op::put(
                checked.nonce_row,
                Some(Bytes::from(serde_json::to_vec(&checked.until).map_err(XrpcError::from_err)?)),
            )],
        )
        .await?;
    if !out.applied {
        return Err(required());
    }
    Ok(())
}

pub(super) async fn get_factor(State(app): AppState, Auth(creds): Auth) -> XResult<Json<J>> {
    super::enabled(&app)?;
    let did = creds.user_did()?.to_owned();
    setup_account(&app, &did).await?;
    creds.need_account("repo", "read")?;
    Ok(Json(if enabled(&app, &did).await? {
        json!({"authFactorType":"im8","authFactorMode":"wallet-v1"})
    } else {
        json!({"authFactorMode":"wallet-v1"})
    }))
}

pub(super) async fn set_factor(State(app): AppState, Auth(creds): Auth, Json(input): Json<J>) -> XResult<Json<J>> {
    super::enabled(&app)?;
    let did = super::super::server::full_access(&creds)?;
    setup_account(&app, &did).await?;
    let want = match input.get("authFactorType") {
        None | Some(J::Null) => false,
        Some(J::String(s)) if s == "im8" => true,
        _ => return Err(XrpcError::bad("InvalidRequest", "Only the im8 auth factor is supported")),
    };
    let acct = super::super::passkeys::check_password(&app, &did, text(&input, "password")?).await?;
    let epoch = super::super::server::epoch_for_login(&app, &acct).await?.ok_or_else(required)?;
    let (old, raw) = load(&app, &did).await?;
    if want == old.is_some() {
        note_flag(&app, &did, want).await?;
        return Ok(Json(if want {
            json!({"authFactorType":"im8","authFactorMode":"wallet-v1"})
        } else {
            json!({"authFactorMode":"wallet-v1"})
        }));
    }
    let checked = verify(
        &app,
        &did,
        if want { "enroll" } else { "disable" },
        &epoch,
        old.as_ref().map(|f| f.public_key.as_str()),
        text(&input, "authFactorToken")?,
    )?;
    let next = want.then(|| Factor { public_key: checked.public_key, created_at: now() });
    let mut ops = super::super::server::revoke_all_session_ops();
    ops.push(Op::DeletePrefix { prefix: "oauth/ses/".into() });
    ops.push(Op::put(
        ROW,
        next.as_ref().map(serde_json::to_vec).transpose().map_err(XrpcError::from_err)?.map(Bytes::from),
    ));
    ops.push(Op::put(
        &checked.nonce_row,
        Some(Bytes::from(serde_json::to_vec(&checked.until).map_err(XrpcError::from_err)?)),
    ));
    if !app
        .private_cas(
            &did,
            vec![super::super::server::auth_epoch_cond(&epoch), Cond::eq(ROW, raw), Cond::eq(&checked.nonce_row, None)],
            ops,
        )
        .await?
        .applied
    {
        return Err(required());
    }
    app.changes.account(&did);
    // Account statistics use an index flag; authentication always reads ROW.
    // A reported failure here leaves the factor enforced and sessions revoked.
    note_flag(&app, &did, want).await?;
    Ok(Json(if want {
        json!({"authFactorType":"im8","authFactorMode":"wallet-v1"})
    } else {
        json!({"authFactorMode":"wallet-v1"})
    }))
}

pub(crate) async fn note_flag(app: &App, did: &str, enabled: bool) -> XResult<()> {
    super::super::server::update_account(app, did, false, false, move |a| {
        super::super::server::set_extra(a, "im8Enabled", json!(enabled));
        Ok(())
    })
    .await?;
    Ok(())
}

pub(crate) fn check_row(routing: &str, name: &str, value: &[u8]) -> Option<anyhow::Result<&'static str>> {
    if routing.starts_with("did:") && name == MIGRATION_REQUIRED {
        return Some(super::super::private_rows::typed_row::<bool>("PARA iM8 migration gate", value));
    }
    (routing.starts_with("did:") && name == ROW)
        .then(|| super::super::private_rows::typed_row::<Factor>("PARA iM8 factor", value))
}
