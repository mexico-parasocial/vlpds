//! An operator reviews a specific public snapshot. The account owner imports
//! that snapshot into their inactive repository; the worker checks its head.
//! Private account/alpha state and relay checkpoints are separate migrations.
use super::*;
use crate::xrpc::cas::{Cond, Op};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

const LIMIT: usize = 64 << 20;
const TTL: u64 = 900;

#[derive(Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Review {
    did: String,
    sha256: String,
    source_commit: String,
    source_signing_key: String,
    expected_head: String,
    historical_exceptions: Vec<String>,
    reason: String,
    require_im8_enrollment: bool,
    #[serde(default)]
    expires_at: u64,
    #[serde(default)]
    reviewed_by: String,
    #[serde(default)]
    used: bool,
}

fn row(token: &str) -> String {
    format!("para/import-review/{}", hex::encode(Sha256::digest(token.as_bytes())))
}
fn required() -> XrpcError {
    XrpcError::bad("ParaImportReviewRequired", "An exact snapshot reviewed by the operator is required")
}

pub(super) async fn review(State(app): AppState, Auth(creds): Auth, Json(mut input): Json<Review>) -> XResult<Json<J>> {
    enabled(&app)?;
    super::super::admin::require_admin(&creds)?;
    if !vlatproto::syntax::valid_did(&input.did)
        || input.sha256.len() != 64
        || input.sha256.bytes().any(|b| !b.is_ascii_digit() && !(b'a'..=b'f').contains(&b))
        || Cid::parse(&input.source_commit).is_err()
        || Cid::parse(&input.expected_head).is_err()
        || !crate::plc::valid_did_key(&input.source_signing_key)
        || input.historical_exceptions.len() > 1000
        || input.historical_exceptions.iter().any(|c| Cid::parse(c).is_err())
        || input.reason.trim().is_empty()
        || input.reason.len() > 2000
    {
        return Err(XrpcError::bad("InvalidRequest", "Invalid PARA import review manifest"));
    }
    input.expires_at = super::super::server::now_secs() + TTL;
    input.reviewed_by = creds.operator().unwrap_or("admin").into();
    input.used = false;
    let token = hex::encode(rand::random::<[u8; 32]>());
    let name = row(&token);
    let value = Bytes::from(serde_json::to_vec(&input).map_err(XrpcError::from_err)?);
    if !app.private_cas(&input.did, vec![Cond::eq(&name, None)], vec![Op::put(name, Some(value))]).await?.applied {
        return Err(XrpcError::bad("InvalidSwap", "Import review already exists; retry"));
    }
    Ok(Json(json!({"reviewToken":token,"expiresAt":input.expires_at,"sha256":input.sha256})))
}

pub(crate) async fn import_reviewed(
    app: Arc<App>,
    creds: Credentials,
    headers: HeaderMap,
    body: Body,
) -> XResult<StatusCode> {
    let token = headers
        .get("para-import-review")
        .and_then(|v| v.to_str().ok())
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(required)?;
    let did = creds.user_did()?.to_owned();
    creds.need_account("repo", "manage")?;
    let account = app.account(&did).await?;
    if account.status.as_deref() != Some("deactivated") {
        return Err(XrpcError::bad(
            "ParaImportInactiveRequired",
            "Deactivate the destination before importing a reviewed snapshot",
        ));
    }
    let name = row(token);
    let raw = app.get_private(&did, &name).await?.ok_or_else(required)?;
    let mut review: Review = serde_json::from_slice(&raw).map_err(XrpcError::from_err)?;
    if review.did != did || review.used || review.expires_at <= super::super::server::now_secs() {
        return Err(required());
    }
    let expected = Cid::parse(&review.expected_head).map_err(|_| required())?;
    if app.head(&did).await?.commit != expected {
        return Err(XrpcError::bad("InvalidSwap", "Destination changed since review"));
    }
    let max = app.config.max_import_bytes.min(LIMIT);
    let declared = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(max);
    if declared > max {
        return Err(super::super::import_stream::too_large(max));
    }
    // Covers the CAR, its full validation parse and the staging parse together.
    // The same reservation reaches staging, so max_imports=1 cannot deadlock.
    let res = app
        .imports
        .admit_working_set(super::super::import_budget::buffered_working_set(declared as u64) + declared as u64)
        .await?;
    let bytes = tokio::time::timeout(std::time::Duration::from_secs(60), axum::body::to_bytes(body, declared))
        .await
        .map_err(|_| XrpcError::bad("InvalidRequest", "Reviewed CAR upload timed out"))?
        .map_err(|_| super::super::import_stream::too_large(max))?;
    if hex::encode(Sha256::digest(&bytes)) != review.sha256 {
        return Err(XrpcError::bad("ParaImportDigestMismatch", "CAR does not match the reviewed snapshot"));
    }
    let validation = bytes.clone();
    let source_did = did.clone();
    let source_cid = review.source_commit.clone();
    let source_key = review.source_signing_key.clone();
    let records = tokio::task::spawn_blocking(move || {
        verify_source(&validation, &source_did, &source_cid, &source_key)?;
        super::super::repo::parse_import(&validation).map(|(records, _)| records)
    })
    .await
    .map_err(XrpcError::from_err)??;
    let exceptions: HashSet<&str> = review.historical_exceptions.iter().map(String::as_str).collect();
    let mut found = HashSet::new();
    let mut boards = Vec::new();
    for (path, cid, record, _) in &records {
        let (coll, key) = path.split_once('/').ok_or_else(required)?;
        let value = Value::decode(record).map_err(XrpcError::from_err)?.to_json();
        if coll.starts_with("com.para.") {
            let raw = serde_json::to_vec(&value).map_err(XrpcError::from_err)?;
            let mut tree = JsonValue::parse(&raw).map_err(XrpcError::from_err)?;
            super::super::repo::encode_record(&mut tree, coll, key, Some(true), None)?;
        }
        if (coll == AUTHORITY && value["issuer"] != did) || (coll == SHARE && value["sharedBy"] != did) {
            return Err(XrpcError::bad("ParaImportActorMismatch", "Historical record claims another repository owner"));
        }
        let cid = cid.to_string();
        if exceptions.contains(cid.as_str()) {
            found.insert(cid);
        } else {
            check_record(&app, &did, coll, key, &value, true).await?;
        }
        if coll == BOARD {
            boards.push((key.to_owned(), text(&value, "name")?.to_owned(), text(&value, "quadrant")?.to_owned()));
        }
    }
    if found.len() != exceptions.len() {
        return Err(XrpcError::bad("InvalidRequest", "Every historical exception must identify a record in this CAR"));
    }
    drop(records);
    // Consume before staging: an ambiguous failure cannot reuse the approval.
    // Failed/ambiguous imports require another review after inspecting the head.
    review.used = true;
    if !app
        .private_cas(
            &did,
            vec![Cond::eq(&name, Some(raw))],
            vec![
                Op::put(&name, Some(Bytes::from(serde_json::to_vec(&review).map_err(XrpcError::from_err)?))),
                Op::put(
                    super::account::MIGRATION_REQUIRED,
                    review.require_im8_enrollment.then(|| Bytes::from_static(b"true")),
                ),
            ],
        )
        .await?
        .applied
    {
        return Err(required());
    }
    for (key, name, quadrant) in boards {
        super::community::reserve_name(&app, &did, &name, &quadrant, &key).await?;
    }
    super::super::staged_import::import_admitted(&app, &did, Body::from(bytes), res, Some(expected)).await?;
    Ok(StatusCode::OK)
}

fn verify_source(bytes: &[u8], did: &str, cid: &str, key: &str) -> XResult<()> {
    let invalid =
        || XrpcError::bad("ParaImportSourceInvalid", "Source DID, commit or signature does not match the review");
    let (roots, blocks) = car::read_car(bytes).map_err(|_| invalid())?;
    if roots.len() != 1 || roots[0].to_string() != cid {
        return Err(invalid());
    }
    let raw = blocks.iter().find_map(|(c, b)| (*c == roots[0]).then_some(*b)).ok_or_else(invalid)?;
    let commit = Value::decode(raw).map_err(|_| invalid())?;
    if !matches!(commit.get("did"),Some(Value::Text(s)) if s==did) {
        return Err(invalid());
    }
    let sig = match commit.get("sig") {
        Some(Value::Bytes(sig)) => sig.clone(),
        _ => return Err(invalid()),
    };
    let Value::Map(mut fields) = commit else { return Err(invalid()) };
    fields.retain(|(k, _)| k != "sig");
    let mut unsigned = Vec::new();
    Value::Map(fields).encode(&mut unsigned);
    if !crypto::verify_multikey(key.strip_prefix("did:key:").ok_or_else(invalid)?, &unsigned, &sig, false)
        .map_err(|_| invalid())?
    {
        return Err(invalid());
    }
    Ok(())
}

pub(crate) fn check_row(routing: &str, name: &str, value: &[u8]) -> Option<anyhow::Result<&'static str>> {
    (routing.starts_with("did:") && name.starts_with("para/import-review/"))
        .then(|| super::super::private_rows::typed_row::<Review>("PARA import review", value))
}
