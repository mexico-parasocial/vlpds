//! All alpha rows share one routing key. Conditional log writes atomically
//! consume an invitation, allocate capacity and approve the account across nodes.
use super::*;
use crate::xrpc::cas::{Cond, Op};
use serde::Serialize;

const ROUTING: &str = "_para_alpha";
const ROUNDS: usize = 16;
const MAX_WAITERS: usize = 10_000;
const STATES: &[&str] = &[
    "AGU", "BCN", "BCS", "CAM", "CHH", "CHP", "CDMX", "COA", "COL", "DUR", "GRO", "GTO", "HGO", "JAL", "MEX", "MIC",
    "MOR", "NAY", "NL", "OAX", "PUE", "QUE", "QRO", "SIN", "SLP", "SON", "TAB", "TAM", "TLA", "VER", "YUC", "ZAC",
    "FRIEND",
];

pub(crate) fn check_row(routing: &str, name: &str, val: &[u8]) -> Option<anyhow::Result<&'static str>> {
    if routing != ROUTING {
        return None;
    }
    let check = if name == "initialized" {
        super::super::private_rows::typed_row::<bool>("PARA alpha initialization", val)
    } else if name.starts_with("rollout/") {
        super::super::private_rows::typed_row::<Rollout>("PARA alpha rollout", val)
    } else if name.starts_with("request/") {
        super::super::private_rows::typed_row::<Request>("PARA alpha request", val)
    } else if name.starts_with("invite/") {
        super::super::private_rows::typed_row::<Invite>("PARA alpha invite", val)
    } else {
        return None;
    };
    Some(check)
}

async fn mutate(app: &App, conditions: Vec<Cond>, mut writes: Vec<Op>) -> XResult<super::super::cas::Outcome> {
    writes.push(Op::put("initialized", Some(Bytes::from_static(b"true"))));
    app.private_cas(ROUTING, conditions, writes).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Snapshot {
    version: u32,
    rollouts: Vec<J>,
    requests: Vec<J>,
    invites: Vec<J>,
}

pub(super) async fn import_state(
    State(app): AppState,
    Auth(creds): Auth,
    Json(input): Json<Snapshot>,
) -> XResult<Json<J>> {
    enabled(&app)?;
    super::super::admin::require_admin(&creds)?;
    if input.version != 1
        || input.rollouts.len() > STATES.len()
        || input.requests.len() > 10_000
        || input.invites.len() > 20_000
    {
        return Err(XrpcError::bad("InvalidRequest", "Unsupported or oversized alpha snapshot"));
    }
    let invalid = || {
        XrpcError::bad("InvalidRequest", "Inconsistent alpha snapshot; repair and review the source before importing")
    };
    for value in input.rollouts.iter().chain(&input.requests).chain(&input.invites) {
        for field in ["createdAt", "approvedAt", "usedAt", "openedAt"] {
            if let Some(v) = value.get(field).filter(|v| !v.is_null()) {
                if !v
                    .as_str()
                    .filter(|s| s.len() <= 64)
                    .is_some_and(|s| chrono::DateTime::parse_from_rfc3339(s).is_ok())
                {
                    return Err(invalid());
                }
            }
        }
    }
    let mut rollouts = std::collections::BTreeMap::new();
    for value in &input.rollouts {
        let state = normalize(text(value, "state")?, true)?;
        let r = Rollout {
            total_slots: value["totalSlots"].as_u64().ok_or_else(invalid)?,
            used_slots: value["usedSlots"].as_u64().ok_or_else(invalid)?,
            is_open: value["isOpen"].as_bool().ok_or_else(invalid)?,
            opened_at: value["openedAt"].as_str().map(str::to_owned),
            waiters: Vec::new(),
        };
        if r.total_slots > 1_000_000 || r.used_slots > r.total_slots || rollouts.insert(state, r).is_some() {
            return Err(invalid());
        }
    }
    for state in STATES {
        rollouts.entry(state.to_string()).or_default();
    }
    let mut invites = std::collections::BTreeMap::new();
    for value in &input.invites {
        let code = text(value, "code")?.to_ascii_uppercase();
        let state = normalize(text(value, "state")?, true)?;
        let did = value["did"].as_str().map(str::to_owned);
        let used = value["usedAt"].as_str().map(str::to_owned);
        if code.len() > 64
            || !code.starts_with(&format!("{state}-"))
            || code.bytes().any(|c| !c.is_ascii_uppercase() && !c.is_ascii_digit() && c != b'-')
            || did.as_ref().is_some_and(|d| !vlatproto::syntax::valid_did(d))
            || did.is_some() != used.is_some()
        {
            return Err(invalid());
        }
        let invite = Invite {
            state,
            created_by: text(value, "createdBy")?.into(),
            created_at: text(value, "createdAt")?.into(),
            did,
            used_at: used,
        };
        if invites.insert(code, invite).is_some() {
            return Err(invalid());
        }
    }
    let mut requests = std::collections::BTreeMap::new();
    let mut allocated = std::collections::BTreeMap::<String, u64>::new();
    let mut pending = Vec::new();
    for value in &input.requests {
        let did = text(value, "did")?;
        if !vlatproto::syntax::valid_did(did) {
            return Err(invalid());
        }
        let state = normalize(text(value, "state")?, true)?;
        let approved = match text(value, "status")? {
            "approved" => true,
            "pending" => false,
            _ => return Err(invalid()),
        };
        let code = value["inviteCode"].as_str().map(|s| s.to_ascii_uppercase());
        let request = Request {
            state: state.clone(),
            approved,
            created_at: text(value, "createdAt")?.into(),
            approved_at: value["approvedAt"].as_str().map(str::to_owned),
            invite_code: code.clone(),
        };
        if approved {
            let quota = match code {
                Some(code) => {
                    let invite = invites.get(&code).ok_or_else(invalid)?;
                    if invite.did.as_deref() != Some(did) || (invite.state != "FRIEND" && invite.state != state) {
                        return Err(invalid());
                    }
                    invite.state.clone()
                }
                None => state,
            };
            *allocated.entry(quota).or_default() += 1;
        } else {
            pending.push((request.created_at.clone(), did.to_owned(), state));
        }
        if requests.insert(did.to_owned(), request).is_some() {
            return Err(invalid());
        }
    }
    for (code, invite) in &invites {
        if let Some(did) = &invite.did {
            let r = requests.get(did).ok_or_else(invalid)?;
            if !r.approved || r.invite_code.as_deref() != Some(code) {
                return Err(invalid());
            }
        }
    }
    for (state, rollout) in &rollouts {
        if rollout.used_slots != allocated.get(state).copied().unwrap_or(0) {
            return Err(invalid());
        }
    }
    pending.sort();
    for (_, did, state) in pending {
        rollouts.get_mut(&state).ok_or_else(invalid)?.waiters.push(did);
    }
    let mut conditions = vec![Cond::eq("initialized", None)];
    let mut writes = Vec::new();
    for (state, r) in &rollouts {
        let name = format!("rollout/{state}");
        conditions.push(Cond::eq(&name, None));
        writes.push(put(name, r)?);
    }
    for (did, r) in &requests {
        let name = format!("request/{did}");
        conditions.push(Cond::eq(&name, None));
        writes.push(put(name, r)?);
    }
    for (code, r) in &invites {
        let name = format!("invite/{code}");
        conditions.push(Cond::eq(&name, None));
        writes.push(put(name, r)?);
    }
    if !mutate(&app, conditions, writes).await?.applied {
        return Err(XrpcError::bad(
            "AlphaAlreadyInitialized",
            "Alpha snapshots can only be imported into an empty alpha store",
        ));
    }
    Ok(Json(json!({"requests":requests.len(),"invites":invites.len(),"states":rollouts.len()})))
}

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Rollout {
    total_slots: u64,
    used_slots: u64,
    is_open: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    opened_at: Option<String>,
    #[serde(default)]
    waiters: Vec<String>,
}

#[derive(Deserialize, Serialize)]
struct Request {
    state: String,
    approved: bool,
    created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    approved_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    invite_code: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct Invite {
    state: String,
    created_by: String,
    created_at: String,
    did: Option<String>,
    used_at: Option<String>,
}

fn normalize(value: &str, friend: bool) -> XResult<String> {
    let state = value.trim().to_ascii_uppercase();
    if !STATES.contains(&state.as_str()) || (!friend && state == "FRIEND") {
        return Err(XrpcError::bad("InvalidRequest", "Expected a Mexican state abbreviation"));
    }
    Ok(state)
}

async fn load<T: serde::de::DeserializeOwned>(app: &App, name: &str) -> XResult<(Option<T>, Option<Bytes>)> {
    let raw = app.get_private(ROUTING, name).await?;
    let value = raw.as_ref().map(|r| serde_json::from_slice(r).map_err(XrpcError::from_err)).transpose()?;
    Ok((value, raw))
}

fn put(name: impl Into<String>, value: &impl Serialize) -> XResult<Op> {
    Ok(Op::put(name, Some(Bytes::from(serde_json::to_vec(value).map_err(XrpcError::from_err)?))))
}

fn conflict() -> XrpcError {
    XrpcError::bad("InvalidSwap", "Alpha state changed concurrently; retry")
}

fn permission(app: &App, creds: &Credentials, method: &str) -> XResult<()> {
    // Preserve the RPC permission used when these methods were proxied.
    let aud = app.config.appview.as_ref().map(|(_, did)| format!("{did}#bsky_appview"));
    creds.need_rpc(method, aud.as_deref().unwrap_or(&app.jwt.service_did))
}

pub(super) async fn set_rollout(State(app): AppState, Auth(creds): Auth, Json(input): Json<J>) -> XResult<Json<J>> {
    enabled(&app)?;
    super::super::admin::require_admin(&creds)?;
    let state = normalize(text(&input, "state")?, true)?;
    let total = input["totalSlots"]
        .as_u64()
        .filter(|n| *n <= 1_000_000)
        .ok_or_else(|| XrpcError::bad("InvalidRequest", "totalSlots must be between 0 and 1000000"))?;
    let open = input["isOpen"].as_bool().ok_or_else(|| XrpcError::bad("InvalidRequest", "isOpen is required"))?;
    let name = format!("rollout/{state}");
    for _ in 0..ROUNDS {
        let (value, raw) = load::<Rollout>(&app, &name).await?;
        let mut value = value.unwrap_or_default();
        if total < value.used_slots {
            return Err(XrpcError::bad("InvalidRequest", "Capacity cannot be reduced below allocated slots"));
        }
        value.total_slots = total;
        if open && !value.is_open {
            value.opened_at = Some(now());
        }
        value.is_open = open;
        if mutate(&app, vec![Cond::eq(&name, raw)], vec![put(&name, &value)?]).await?.applied {
            return Ok(Json(json!({"state":state,"totalSlots":total,"usedSlots":value.used_slots,"isOpen":open})));
        }
    }
    Err(conflict())
}

pub(super) async fn get_rollout(State(app): AppState, Auth(creds): Auth) -> XResult<Json<J>> {
    enabled(&app)?;
    super::super::admin::require_admin(&creds)?;
    let mut states = Vec::new();
    for state in STATES {
        let (value, _) = load::<Rollout>(&app, &format!("rollout/{state}")).await?;
        let value = value.unwrap_or_default();
        let mut row =
            json!({"state":state,"totalSlots":value.total_slots,"usedSlots":value.used_slots,"isOpen":value.is_open});
        if let Some(at) = value.opened_at {
            row["openedAt"] = json!(at);
        }
        states.push(row);
    }
    Ok(Json(json!({"states":states})))
}

pub(super) async fn create_invite(State(app): AppState, Auth(creds): Auth, Json(input): Json<J>) -> XResult<Json<J>> {
    enabled(&app)?;
    super::super::admin::require_admin(&creds)?;
    let state = normalize(text(&input, "state")?, true)?;
    let count = input
        .get("count")
        .map_or(Some(1), J::as_u64)
        .filter(|n| (1..=100).contains(n))
        .ok_or_else(|| XrpcError::bad("InvalidRequest", "count must be between 1 and 100"))?;
    let mut conditions = Vec::new();
    let mut writes = Vec::new();
    let mut codes = Vec::new();
    for _ in 0..count {
        // 128 random bits; no predictable counter or short invitation entropy.
        let token = hex::encode(rand::random::<[u8; 16]>()).to_ascii_uppercase();
        let code = format!("{state}-{}-{}", &token[..16], &token[16..]);
        let name = format!("invite/{code}");
        let invite = Invite {
            state: state.clone(),
            created_by: creds.operator().unwrap_or("admin").into(),
            created_at: now(),
            did: None,
            used_at: None,
        };
        conditions.push(Cond::eq(&name, None));
        writes.push(put(&name, &invite)?);
        codes.push(code);
    }
    if !mutate(&app, conditions, writes).await?.applied {
        return Err(conflict());
    }
    Ok(Json(json!({"codes":codes,"state":state})))
}

pub(super) async fn get_access(State(app): AppState, Auth(creds): Auth) -> XResult<Json<J>> {
    let did = caller(&app, &creds).await?;
    permission(&app, &creds, "com.para.alpha.getAccess")?;
    let (request, _) = load::<Request>(&app, &format!("request/{did}")).await?;
    let Some(request) = request else { return Ok(Json(json!({"hasAccess":false}))) };
    if request.approved {
        return Ok(Json(json!({"hasAccess":true,"state":request.state})));
    }
    let (rollout, _) = load::<Rollout>(&app, &format!("rollout/{}", request.state)).await?;
    let position = rollout.unwrap_or_default().waiters.iter().position(|v| v == did.as_ref()).map(|n| n + 1);
    let mut out = json!({"hasAccess":false,"state":request.state});
    if let Some(n) = position {
        out["waitlistPosition"] = json!(n);
    }
    Ok(Json(out))
}

pub(super) async fn request_access(State(app): AppState, Auth(creds): Auth, Json(input): Json<J>) -> XResult<Json<J>> {
    let did = caller(&app, &creds).await?;
    permission(&app, &creds, "com.para.alpha.requestAccess")?;
    let state = normalize(text(&input, "state")?, false)?;
    let code = match input.get("inviteCode") {
        None => None,
        Some(v) => Some(
            v.as_str()
                .filter(|s| !s.trim().is_empty() && s.len() <= 64)
                .ok_or_else(|| XrpcError::bad("InvalidRequest", "Invalid invite code"))?
                .trim()
                .to_ascii_uppercase(),
        ),
    };
    let request_name = format!("request/{did}");
    for _ in 0..ROUNDS {
        let (old, request_raw) = load::<Request>(&app, &request_name).await?;
        if let Some(old) = &old {
            if old.approved {
                return Ok(Json(json!({"status":"already_has_access","state":old.state})));
            }
            if old.state != state {
                return Err(XrpcError::bad("InvalidRequest", "An existing waitlist request is bound to its state"));
            }
        }
        let mut conditions = vec![Cond::eq(&request_name, request_raw)];
        let mut writes = Vec::new();
        let mut invite = None;
        let mut quota_state = state.clone();
        if let Some(code) = &code {
            let name = format!("invite/{code}");
            let (value, raw) = load::<Invite>(&app, &name).await?;
            let mut value = value.ok_or_else(|| XrpcError::bad("InvalidInviteCode", "Invalid alpha invite code"))?;
            if value.did.is_some() {
                return Err(XrpcError::bad("InvalidInviteCode", "Alpha invite has already been used"));
            }
            if value.state != "FRIEND" && value.state != state {
                return Err(XrpcError::bad("InvalidInviteCode", "Alpha invite is for another state"));
            }
            quota_state = value.state.clone();
            value.did = Some(did.to_string());
            value.used_at = Some(now());
            conditions.push(Cond::eq(&name, raw));
            invite = Some(put(name, &value)?);
        }
        let name = format!("rollout/{quota_state}");
        let (value, raw) = load::<Rollout>(&app, &name).await?;
        let mut rollout = value.unwrap_or_default();
        conditions.push(Cond::eq(&name, raw));
        if invite.is_none() && !rollout.is_open {
            return Err(XrpcError::bad("AlphaClosed", "Alpha access is closed for this state; an invite is required"));
        }
        let available = rollout.used_slots < rollout.total_slots;
        if invite.is_some() && !available {
            return Err(XrpcError::bad("AlphaCapacityReached", "The invitation quota is full"));
        }
        // New public applicants cannot jump ahead of a persisted waitlist.
        let approved = available && (invite.is_some() || rollout.waiters.first().is_none_or(|v| v == did.as_ref()));
        let timestamp = now();
        let request = Request {
            state: state.clone(),
            approved,
            created_at: old.map(|r| r.created_at).unwrap_or_else(|| timestamp.clone()),
            approved_at: approved.then_some(timestamp),
            invite_code: approved.then(|| code.clone()).flatten(),
        };
        if approved {
            rollout.used_slots += 1;
            rollout.waiters.retain(|v| v != did.as_ref());
            if let Some(op) = invite {
                writes.push(op);
            }
            if quota_state != state {
                let old_name = format!("rollout/{state}");
                let (value, raw) = load::<Rollout>(&app, &old_name).await?;
                let mut value = value.unwrap_or_default();
                value.waiters.retain(|v| v != did.as_ref());
                conditions.push(Cond::eq(&old_name, raw));
                writes.push(put(old_name, &value)?);
            }
        } else if !rollout.waiters.iter().any(|v| v == did.as_ref()) {
            if rollout.waiters.len() >= MAX_WAITERS {
                return Err(XrpcError::bad("AlphaWaitlistFull", "This state's waitlist is full"));
            }
            rollout.waiters.push(did.to_string());
        }
        let position = rollout.waiters.iter().position(|v| v == did.as_ref()).map(|n| n + 1);
        writes.push(put(&name, &rollout)?);
        writes.push(put(&request_name, &request)?);
        if mutate(&app, conditions, writes).await?.applied {
            let mut out = json!({"status":if approved {"approved"} else {"waitlisted"},"state":state});
            if let Some(n) = position {
                out["position"] = json!(n);
            }
            return Ok(Json(out));
        }
    }
    Err(conflict())
}
