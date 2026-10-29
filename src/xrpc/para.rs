//! Native PARA commands. All records use the ordinary repo worker and bucket log.
//! A head compare-and-swap covers every read used to authorize a command.
use super::authn::Credentials;
use super::*;
use vlatproto::cbor::JsonValue;
pub(super) mod account;
pub(super) mod alpha;
mod civic;
mod community;
mod founders;
pub(super) mod migration;

pub(super) const BOARD: &str = "com.para.community.board";
pub(super) const MEMBER: &str = "com.para.community.membership";
pub(super) const GOVERNANCE: &str = "com.para.community.governance";
pub(super) const AUTHORITY: &str = "com.para.community.authorityEvent";
pub(super) const PACK: &str = "com.para.community.briefingPack";
pub(super) const SHARE: &str = "com.para.community.sharedContent";
pub(super) const SHARE_ACTION: &str = "com.para.community.sharedContentAction";

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/xrpc/com.para.community.createBoard", post(community::create_board))
        .route("/xrpc/com.para.community.join", post(community::join))
        .route("/xrpc/com.para.community.leave", post(community::leave))
        .route("/xrpc/com.para.community.createBriefingPack", post(community::create_pack))
        .route("/xrpc/com.para.community.updateBriefingPack", post(community::update_pack))
        .route("/xrpc/com.para.community.shareContent", post(community::share))
        .route("/xrpc/com.para.community.removeSharedContent", post(community::remove_share))
        .route("/xrpc/com.para.community.restoreSharedContent", post(community::restore_share))
        .route("/xrpc/com.para.civic.castVote", post(civic::cast_vote))
        .route("/xrpc/com.para.community.acceptDraftInvite", post(founders::accept))
        .route("/xrpc/vlpds.para.inviteFounder", post(founders::invite))
        .route("/xrpc/vlpds.para.finalizeFounders", post(founders::finalize))
        .route("/xrpc/com.para.account.getAuthFactor", get(account::get_factor))
        .route("/xrpc/com.para.account.setAuthFactor", post(account::set_factor))
        .route("/xrpc/vlpds.para.startIm8Enrollment", post(account::start_enrollment))
        .route("/xrpc/vlpds.para.startIm8Removal", post(account::start_removal))
        .route("/xrpc/vlpds.para.getIm8Challenge", get(account::get_challenge))
        .route("/xrpc/com.para.alpha.requestAccess", post(alpha::request_access))
        .route("/xrpc/com.para.alpha.createInvite", post(alpha::create_invite))
        .route("/xrpc/com.para.alpha.getAccess", get(alpha::get_access))
        .route("/xrpc/com.para.alpha.getRolloutStatus", get(alpha::get_rollout))
        .route("/xrpc/vlpds.para.setAlphaRollout", post(alpha::set_rollout))
        .route(
            "/xrpc/vlpds.para.importAlphaState",
            post(alpha::import_state).layer(axum::extract::DefaultBodyLimit::max(8 << 20)),
        )
        .route("/xrpc/vlpds.para.reviewImport", post(migration::review))
}

fn enabled(app: &App) -> XResult<()> {
    if !app.config.para.enabled {
        return Err(XrpcError {
            status: StatusCode::NOT_IMPLEMENTED,
            error: "MethodNotImplemented".into(),
            message: "Native PARA support is disabled".into(),
        });
    }
    Ok(())
}

async fn caller(app: &App, creds: &Credentials) -> XResult<Arc<str>> {
    enabled(app)?;
    let did = creds.user_did()?;
    app.ensure_active(did).await?;
    Ok(did.into())
}

pub(super) async fn head_guard(app: &App, did: &str, needed: bool) -> XResult<Option<Cid>> {
    if app.config.para.enabled && needed {
        Ok(Some(app.head(did).await?.commit))
    } else {
        Ok(None)
    }
}

pub(super) fn check_action(app: &App, collection: &str, action: &str) -> XResult<()> {
    if app.config.para.enabled && collection == MEMBER && action == "delete" {
        return Err(XrpcError::bad("InvalidRequest", "Memberships retain authorization history; use community.leave"));
    }
    if app.config.para.enabled && collection == AUTHORITY && action != "create" {
        return Err(XrpcError::bad(
            "InvalidRequest",
            "Authority events are append-only; publish a later event to reverse one",
        ));
    }
    Ok(())
}

fn text<'a>(v: &'a J, k: &str) -> XResult<&'a str> {
    v[k].as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| XrpcError::bad("InvalidRequest", format!("{k} is required")))
}

fn at_uri<'a>(s: &'a str, collection: &str) -> XResult<(&'a str, &'a str)> {
    let invalid = || XrpcError::bad("InvalidRequest", format!("Expected a {collection} AT URI"));
    let mut p = s.strip_prefix("at://").ok_or_else(invalid)?.split('/');
    let did = p.next().ok_or_else(invalid)?;
    let coll = p.next().ok_or_else(invalid)?;
    let key = p.next().ok_or_else(invalid)?;
    if !vlatproto::syntax::valid_did(did)
        || coll != collection
        || !vlatproto::syntax::valid_rkey(key)
        || p.next().is_some()
    {
        return Err(invalid());
    }
    Ok((did, key))
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
fn uri(did: &str, coll: &str, key: &str) -> String {
    format!("at://{did}/{coll}/{key}")
}

async fn read(app: &App, did: &str, coll: &str, key: &str) -> XResult<Option<(Cid, J)>> {
    if let Some(owner) = app.remote_owner(did) {
        return Ok(peer_records(app, &owner, did, coll, Some(key)).await?.into_iter().next().map(|(_, c, r)| (c, r)));
    }
    local_read(app, did, coll, key).await
}

async fn local_read(app: &App, did: &str, coll: &str, key: &str) -> XResult<Option<(Cid, J)>> {
    let Some(raw) = app.record_value(did, None, &format!("{coll}/{key}")).await? else { return Ok(None) };
    let (cid, bytes) = state::decode_record_value(&raw).map_err(XrpcError::from_err)?;
    let v = Value::decode(&bytes).map_err(XrpcError::from_err)?;
    Ok(Some((cid, v.to_json())))
}

async fn list(app: &App, did: &str, coll: &str) -> XResult<Vec<(String, Cid, J)>> {
    if let Some(owner) = app.remote_owner(did) {
        return peer_records(app, &owner, did, coll, None).await;
    }
    local_list(app, did, coll).await
}

async fn local_list(app: &App, did: &str, coll: &str) -> XResult<Vec<(String, Cid, J)>> {
    let p = app.partition(did)?;
    let gen = app.repo_gen(did).await?;
    let prefix = state::record_key(did, gen, &format!("{coll}/"));
    let mut scan =
        p.db.scan(prefix.clone()..vlsync_store::keys::prefix_end(&prefix)).await.map_err(XrpcError::from_err)?;
    let mut out = Vec::new();
    let mut scanned_bytes = 0usize;
    while let Some(kv) = scan.next().await.map_err(XrpcError::from_err)? {
        scanned_bytes = scanned_bytes.saturating_add(kv.value.len());
        if out.len() == 1000 || scanned_bytes > 8 << 20 {
            return Err(XrpcError::bad("ParaLookupLimit", "Collection exceeds the command lookup record/byte limit"));
        }
        let key = String::from_utf8(kv.key[prefix.len()..].to_vec()).map_err(XrpcError::from_err)?;
        let (cid, bytes) = state::decode_record_value(&kv.value).map_err(XrpcError::from_err)?;
        out.push((key, cid, Value::decode(&bytes).map_err(XrpcError::from_err)?.to_json()));
    }
    Ok(out)
}

pub(super) fn internal_routes() -> Router<Arc<App>> {
    Router::new().route("/internal/v1/para/records", get(internal_records))
}

#[derive(Deserialize)]
struct Lookup {
    did: String,
    collection: String,
    rkey: Option<String>,
}

async fn internal_records(State(app): AppState, headers: HeaderMap, Query(q): Query<Lookup>) -> XResult<Json<J>> {
    super::internal::check(&app, &headers)?;
    if !vlatproto::syntax::valid_did(&q.did)
        || ![BOARD, MEMBER, "app.bsky.graph.starterpack", "app.bsky.graph.listitem"].contains(&q.collection.as_str())
    {
        return Err(XrpcError::bad("InvalidRequest", "Unsupported PARA peer lookup"));
    }
    let records = match q.rkey {
        Some(key) => {
            super::repo::check_path(&q.collection, Some(&key))?;
            local_read(&app, &q.did, &q.collection, &key).await?.map(|(c, r)| vec![(key, c, r)]).unwrap_or_default()
        }
        None => local_list(&app, &q.did, &q.collection).await?,
    };
    Ok(Json(json!(records.into_iter().map(|(k, c, r)| (k, c.to_string(), r)).collect::<Vec<_>>())))
}

async fn peer_records(
    app: &App,
    owner: &str,
    did: &str,
    coll: &str,
    key: Option<&str>,
) -> XResult<Vec<(String, Cid, J)>> {
    let mut query = vec![("did", did), ("collection", coll)];
    if let Some(key) = key {
        query.push(("rkey", key));
    }
    let unavailable =
        || XrpcError::unavailable("ParaPeerUnavailable", "PARA repository owner could not complete the lookup; retry");
    let mut response = app
        .http
        .get(format!("{owner}/internal/v1/para/records"))
        .header(super::internal::HDR, &app.config.internal_token)
        .header(header::ACCEPT_ENCODING, "identity")
        .query(&query)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .map_err(|_| unavailable())?;
    let status = response.status();
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
        if bytes.len().saturating_add(chunk.len()) > 32 << 20 {
            return Err(unavailable());
        }
        bytes.extend_from_slice(&chunk);
    }
    if !status.is_success() {
        let body: J = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
        return Err(XrpcError {
            status,
            error: body["error"].as_str().unwrap_or("ParaPeerUnavailable").into(),
            message: body["message"].as_str().unwrap_or("PARA peer lookup failed").into(),
        });
    }
    let rows: Vec<(String, String, J)> = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
    if rows.len() > 1000 {
        return Err(unavailable());
    }
    rows.into_iter().map(|(k, c, r)| Ok((k, Cid::parse(&c).map_err(|_| unavailable())?, r))).collect()
}

async fn appview(app: &App, did: &str, method: &str, params: &[(&str, &str)]) -> XResult<J> {
    let (url, aud) =
        app.config.appview.as_ref().ok_or_else(|| {
            XrpcError::unavailable("ParaAppViewUnavailable", "An AppView is required for this operation")
        })?;
    let acct = app.ensure_active(did).await?;
    let key = app.secrets.signing_key(did, &acct.wrapped_signing_key, &acct.signing_pubkey).await?;
    let jwt = crate::auth::service_auth_jwt(&key, did, aud, Some(method), 60)?;
    let mut response = crate::para::HTTP
        .get(format!("{}/xrpc/{method}", url.trim_end_matches('/')))
        .query(params)
        .bearer_auth(jwt)
        .header("accept-encoding", "identity")
        .send()
        .await
        .map_err(|_| XrpcError::unavailable("ParaAppViewUnavailable", "AppView lookup failed"))?;
    if !response.status().is_success() {
        return Err(XrpcError::unavailable("ParaAppViewUnavailable", "AppView lookup was refused"));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(XrpcError::from_err)? {
        if body.len() + chunk.len() > 2 << 20 {
            return Err(XrpcError::unavailable("ParaAppViewUnavailable", "AppView response too large"));
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(XrpcError::from_err)
}

async fn board(app: &App, did: &str, board_uri: &str) -> XResult<J> {
    let (owner, key) = at_uri(board_uri, BOARD)?;
    if owner == did {
        return read(app, owner, BOARD, key)
            .await?
            .map(|(_, v)| v)
            .ok_or_else(|| XrpcError::bad("NotFound", "Community not found"));
    }
    let view = appview(app, did, "com.para.community.getBoard", &[("uri", board_uri)]).await?;
    if view["board"]["uri"] != board_uri {
        return Err(XrpcError::bad("NotFound", "Community not found"));
    }
    Ok(view["board"].clone())
}

type PlannedRecord<'a> = (&'a str, String, J, Option<Option<Cid>>);

async fn commit(
    app: &Arc<App>,
    creds: &Credentials,
    did: Arc<str>,
    head: Cid,
    records: Vec<PlannedRecord<'_>>,
) -> XResult<(CommitAck, Vec<Cid>)> {
    let mut writes = Vec::new();
    let mut cids = Vec::new();
    let mut decls = Vec::new();
    crate::ratelimit::check_repo_write(Some(&did), records.len() as u32 * crate::ratelimit::CREATE_POINTS)?;
    for (coll, key, record, swap) in records {
        let action = if swap == Some(None) { "create" } else { "update" };
        creds.need_repo(coll, action)?;
        check_action(app, coll, action)?;
        check_record(app, &did, coll, &key, &record, true).await?;
        super::repo::check_path(coll, Some(&key))?;
        super::repo::check_rkey_slur(Some(&key))?;
        let raw = serde_json::to_vec(&record).map_err(XrpcError::from_err)?;
        let mut tree = JsonValue::parse(&raw).map_err(XrpcError::from_err)?;
        let (cid, bytes, blobs, _, d) = super::repo::encode_record(&mut tree, coll, &key, Some(true), None)?;
        decls.extend(d);
        cids.push(cid);
        writes.push(if action == "create" {
            Write::Create { collection: coll.into(), rkey: key, cid, bytes, blobs, prune_backlinks: false }
        } else {
            Write::Update { collection: coll.into(), rkey: key, cid, bytes, blobs, swap, must_exist: true }
        });
    }
    let _held = super::repo::check_blobs(app, &did, &decls).await?;
    let ack = super::repo::submit(app, did, writes, Some(head)).await?;
    Ok((ack, cids))
}

pub(super) async fn check_write(app: &App, did: &str, coll: &str, key: &str, record: &J) -> XResult<()> {
    check_record(app, did, coll, key, record, false).await
}

async fn check_record(app: &App, did: &str, coll: &str, key: &str, record: &J, command: bool) -> XResult<()> {
    crate::para::check(app, did, coll, record).await?;
    if !app.config.para.enabled {
        return Ok(());
    }
    // PARA contracts are security relevant even when the caller opts out of schema validation.
    if coll.starts_with("com.para.") {
        let raw = serde_json::to_vec(record).map_err(XrpcError::from_err)?;
        let mut tree = JsonValue::parse(&raw).map_err(XrpcError::from_err)?;
        super::repo::encode_record(&mut tree, coll, key, Some(true), None)?;
    }
    if coll == AUTHORITY {
        if record["issuer"] != did || record["basis"] == "migration" {
            return Err(XrpcError::bad(
                "InvalidRequest",
                "Authority event issuer must match the repository owner; migration events are service generated",
            ));
        }
        if matches!(record["basis"].as_str(), Some("openAdmission" | "resignation")) && record["subject"] != did {
            return Err(XrpcError::bad("InvalidRequest", "Authority event must be authored by its subject"));
        }
        if matches!(record["basis"].as_str(), Some("assemblyDecision" | "ownerAppointment" | "ownerTransfer"))
            && record.get("evidence").is_none()
        {
            return Err(XrpcError::bad("InvalidRequest", "Authority event requires evidence"));
        }
        if record["basis"] == "resignation" && record["action"] != "moderator.resign" {
            return Err(XrpcError::bad("InvalidRequest", "Resignation can only resign the moderator role"));
        }
    }
    if coll == AUTHORITY && record["action"] == "moderator.grant" && record.get("expiresAt").is_some() {
        let expiry = chrono::DateTime::parse_from_rfc3339(text(record, "expiresAt")?).map_err(XrpcError::from_err)?;
        let effective =
            chrono::DateTime::parse_from_rfc3339(text(record, "effectiveAt")?).map_err(XrpcError::from_err)?;
        if expiry <= effective {
            return Err(XrpcError::bad("InvalidRequest", "Moderator expiry must follow its effective time"));
        }
    }
    if coll == BOARD {
        community::check_board(app, did, key, record, command).await?;
    }
    if coll == MEMBER {
        community::check_membership(app, did, key, record).await?;
    }
    if coll == SHARE || coll == SHARE_ACTION {
        community::check_content(app, did, coll, record).await?;
    }
    if coll == GOVERNANCE {
        community::check_governance(app, did, record).await?;
    }
    Ok(())
}
