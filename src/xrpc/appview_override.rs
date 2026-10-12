//! The AppView override's console API (docs/proxying.md "AppView
//! override"): read and change the cluster's config object, and each node's
//! view of it with its traffic to the override. Same auth, audit and peer
//! nudges as the rate-limit config (`super::ratelimits`).

use super::admin::require_admin;
use super::internal::HDR as INTERNAL_HDR;
use super::ratelimits::PeerIp;
use super::*;
use crate::appview_override::{self as ov, SaveError, SaveReq};
use std::time::Duration;

const RELOAD_TIMEOUT: Duration = Duration::from_secs(2);
const RESULTS: [&str; 4] = ["ok", "client_error", "server_error", "unreachable"];

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/xrpc/vlpds.admin.getAppviewOverride", get(get_override))
        .route("/xrpc/vlpds.admin.updateAppviewOverride", post(update_override))
}

pub fn internal_routes() -> Router<Arc<App>> {
    Router::new()
        .route("/internal/v1/appview-override", get(internal_status))
        .route("/internal/v1/appview-override/reload", post(internal_reload))
}

/// Idempotent.
pub fn start(app: &Arc<App>) {
    ov::spawn_refresher(&app.appview_override, app.store.clone());
}

fn node_id(app: &App) -> String {
    app.cluster.as_ref().map(|c| c.cfg.node_id.clone()).unwrap_or_else(|| "single".into())
}

/// This node's traffic to the override since it started (the
/// `service="appview_override"` upstream series and the fallback counter).
fn stats() -> J {
    let m = &crate::metrics::UPSTREAM_REQUESTS;
    let n = |r: &str| m.with_label_values(&["appview_override", r]).get();
    let h = crate::metrics::UPSTREAM_DURATION.with_label_values(&["appview_override"]);
    let fb = |s: &str| crate::metrics::APPVIEW_OVERRIDE_FALLBACKS.with_label_values(&[s]).get();
    json!({
        "ok": n(RESULTS[0]),
        "clientError": n(RESULTS[1]),
        "serverError": n(RESULTS[2]),
        "unreachable": n(RESULTS[3]),
        // fallbacks are also counted in the override's 4xx/5xx above
        "fallbacks": fb("501") + fb("400"),
        "fallbackServerErrors": fb("501"),
        "fallbackClientErrors": fb("400"),
        "headSecondsSum": h.get_sample_sum(),
        "headCount": h.get_sample_count(),
    })
}

fn node_status(app: &App) -> J {
    let rt = &app.appview_override;
    let st = rt.status();
    let active = rt.active();
    json!({
        "node": node_id(app),
        "source": rt.source(),
        "configVersion": rt.version(),
        "configError": st.error,
        "loadedAtMs": st.loaded_at_ms,
        "checkedAtMs": st.checked_at_ms,
        "active": active.is_some(),
        "accounts": active.as_ref().map_or(0, |o| o.accounts.len()),
        "appviewConfigured": app.config.appview.is_some(),
        "stats": stats(),
    })
}

#[derive(Deserialize)]
struct GetQ {
    #[serde(default)]
    local: bool,
}

/// Summed over nodes.
fn sum_stats(nodes: &[J]) -> J {
    let mut out = serde_json::Map::new();
    for k in [
        "ok",
        "clientError",
        "serverError",
        "unreachable",
        "fallbacks",
        "fallbackServerErrors",
        "fallbackClientErrors",
        "headCount",
    ] {
        out.insert(k.into(), json!(nodes.iter().filter_map(|n| n["stats"][k].as_u64()).sum::<u64>()));
    }
    out.insert(
        "headSecondsSum".into(),
        json!(nodes.iter().filter_map(|n| n["stats"]["headSecondsSum"].as_f64()).sum::<f64>()),
    );
    J::Object(out)
}

async fn get_override(State(app): AppState, Auth(creds): Auth, Query(q): Query<GetQ>) -> XResult<Json<J>> {
    require_admin(&creds)?;
    let me = node_id(&app);
    let rt = &app.appview_override;
    let mut nodes = vec![{
        let mut s = node_status(&app);
        s["self"] = json!(true);
        s["reachable"] = json!(true);
        s
    }];
    let mut unreachable = Vec::new();
    if !q.local {
        let g = internal::gather(&app, "/internal/v1/appview-override", &[]).await;
        for r in g.replies {
            let mut s = r.body;
            s["self"] = json!(false);
            s["reachable"] = json!(true);
            nodes.push(s);
        }
        unreachable.extend(g.unreachable);
        for u in g.unsupported {
            nodes.push(json!({"node": u, "self": false, "reachable": true, "unsupported": true}));
        }
        for u in &unreachable {
            nodes.push(json!({"node": u, "self": false, "reachable": false}));
        }
    }
    nodes.sort_by(|a, b| a["node"].as_str().cmp(&b["node"].as_str()));
    let st = rt.status();
    let mut out = json!({
        "node": me,
        "source": rt.source(),
        "config": st.doc,
        "configVersion": rt.version(),
        "configError": st.error,
        "flags": rt.boot().map(|b| b.to_doc()),
        "effective": rt.active().map(|o| o.to_doc()),
        "defaultAppview": app.config.appview.as_ref().map(|(url, did)| json!({"url": url, "did": did})),
        "refreshSecs": ov::REFRESH_EVERY.as_secs(),
        "limits": {"accounts": ov::MAX_ACCOUNTS, "methods": ov::MAX_METHODS},
        "stats": sum_stats(&nodes),
        "nodes": nodes,
        "time": crate::ratelimit::now_ms(),
    });
    if !unreachable.is_empty() {
        out["unreachableNodes"] = json!(unreachable);
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateIn {
    config: J,
    if_version: u64,
    actor: Option<String>,
    note: Option<String>,
}

fn save_error(e: SaveError) -> XrpcError {
    let message = e.to_string();
    match e {
        SaveError::Invalid(_) => XrpcError::bad("InvalidConfig", message),
        SaveError::Conflict { .. } => {
            XrpcError { status: StatusCode::CONFLICT, error: "ConfigConflict".into(), message }
        }
        SaveError::Store(_) => XrpcError { status: StatusCode::BAD_GATEWAY, error: "UpstreamFailure".into(), message },
    }
}

async fn update_override(
    State(app): AppState,
    Auth(creds): Auth,
    PeerIp(peer): PeerIp,
    Json(inp): Json<UpdateIn>,
) -> XResult<Json<J>> {
    require_admin(&creds)?;
    let doc = ov::parse(inp.config).map_err(|e| XrpcError::bad("InvalidConfig", e))?;
    let who = super::moderation::Who::of(&creds, inp.actor.as_deref(), peer);
    let me = node_id(&app);
    let req = SaveReq {
        doc,
        if_version: inp.if_version,
        actor: who.actor.clone(),
        ip: who.ip.clone(),
        node: me.clone(),
        note: inp.note,
    };
    let saved = app.appview_override.save(&app.store, req).await.map_err(save_error)?;
    let changes = saved.history.last().map_or(0, |h| h.changes.iter().filter(|c| *c != "no changes").count());
    let detail = json!({"version": saved.version, "changes": changes});
    let subject = super::moderation::SubjectRef::other("config", "appview-override");
    super::moderation::audit(
        &app,
        &who,
        "appviewOverride.update",
        Some(&subject),
        saved.note.as_deref(),
        None,
        Some(detail),
    )
    .await?;
    // peers also re-read within REFRESH_EVERY; this makes it now
    let mut applied = vec![json!({"node": me, "configVersion": app.appview_override.version(), "ok": true})];
    if let Some(c) = &app.cluster {
        let peers: Vec<_> = c.peers().into_iter().filter(|l| l.node_id != me).collect();
        let sends = peers.into_iter().map(|l| {
            let app = app.clone();
            async move {
                let r = app
                    .http
                    .post(format!("{}/internal/v1/appview-override/reload", l.addr.trim_end_matches('/')))
                    .header(INTERNAL_HDR, &app.config.internal_token)
                    .timeout(RELOAD_TIMEOUT)
                    .send()
                    .await
                    .and_then(|r| r.error_for_status());
                match r {
                    Ok(r) => {
                        let v: J = r.json().await.unwrap_or(J::Null);
                        json!({"node": l.node_id, "configVersion": v["configVersion"], "configError": v["configError"], "ok": true})
                    }
                    Err(e) => {
                        tracing::warn!(peer = %l.node_id, "AppView override reload nudge failed (it re-reads on its own): {e}");
                        json!({"node": l.node_id, "ok": false, "error": e.to_string()})
                    }
                }
            }
        });
        applied.extend(futures::future::join_all(sends).await);
    }
    app.changes.emit("appview-override", me, saved.version);
    Ok(Json(json!({"version": saved.version, "config": saved, "nodes": applied})))
}

async fn internal_status(State(app): AppState, headers: HeaderMap) -> XResult<Json<J>> {
    internal::check(&app, &headers)?;
    Ok(Json(node_status(&app)))
}

async fn internal_reload(State(app): AppState, headers: HeaderMap) -> XResult<Json<J>> {
    internal::check(&app, &headers)?;
    let rt = &app.appview_override;
    if let Err(e) = rt.refresh(&app.store).await {
        return Err(XrpcError {
            status: StatusCode::BAD_GATEWAY,
            error: "UpstreamFailure".into(),
            message: format!("{e:#}"),
        });
    }
    app.changes.emit("appview-override", node_id(&app), rt.version());
    Ok(Json(json!({"node": node_id(&app), "configVersion": rt.version(), "configError": rt.status().error})))
}
