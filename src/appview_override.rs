//! The AppView override (docs/proxying.md "AppView override"): chosen
//! accounts' calls for chosen `app.bsky.*` methods go to a second AppView.
//!
//! The flags (`--appview-override*`) are the boot default. Once the config
//! object `{prefix}/config/appview_override.json` exists, it wins on every
//! node, whatever the flags say. Every node keeps in step with it the way it
//! does with the rate-limit config (DESIGN.md "Rate limits: observability
//! and runtime config"): saves are compare-and-swap on a version, the saving
//! node nudges its peers, and each node also re-reads the object
//! conditionally every [`REFRESH_EVERY`]. An object a node can't accept
//! never takes it down: it keeps its last good config and reports why.
//!
//! ```json
//! {
//!   "version": 3,
//!   "url": "http://appview.internal:2584",
//!   "did": "did:web:appview.example",
//!   "accounts": ["did:plc:..."],
//!   "methods": ["app.bsky.feed.getTimeline"],
//!   "fallback": true,
//!   "updatedAt": "...", "updatedBy": "admin", "note": "...", "history": [...]
//! }
//! ```
//!
//! `enabled: false` keeps the lists but moves nothing. Unknown fields are
//! refused on save so a typo never silently does nothing, and dropped (with
//! a warning) when reading the stored object, which a newer build may have
//! written.

use object_store::{GetOptions, PutMode, PutOptions, PutPayload};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use vlsync_store::store::Store;

/// Peers are nudged on every save, so this only bounds staleness after a
/// lost nudge (or a hand edit of the object). One conditional GET per node
/// per minute.
pub const REFRESH_EVERY: Duration = Duration::from_secs(60);
const CALL_DEADLINE: Duration = Duration::from_secs(5);
pub const MAX_ACCOUNTS: usize = 1000;
pub const MAX_METHODS: usize = 256;
pub const HISTORY: usize = 50;
const MAX_NOTE: usize = 280;

/// Served by this PDS, never proxied, so an override can't name them.
const LOCAL_APPVIEW_METHODS: &[&str] = &[
    "app.bsky.actor.getPreferences",
    "app.bsky.actor.putPreferences",
    "app.bsky.notification.registerPush",
    "app.bsky.notification.unregisterPush",
];

/// An override in force. It applies only to calls that would go to the
/// default AppView, so a node without `--appview` moves nothing.
#[derive(Clone, Debug, PartialEq)]
pub struct AppviewOverride {
    /// Operator-configured, so trusted like `--appview`'s: it may be private.
    pub url: String,
    /// The service-auth audience, never resolved.
    pub did: String,
    pub accounts: HashSet<String>,
    /// Exact `app.bsky.*` NSIDs: an allowlist, so a method the override
    /// hasn't been told about (a new one, suggestions, search) stays on the
    /// default.
    pub methods: HashSet<String>,
    /// A GET it answers `MethodNotImplemented` (501, or 400 with that error)
    /// is sent to the default AppView.
    pub fallback: bool,
}

fn check_url(url: &str) -> Result<(), String> {
    let ok = reqwest::Url::parse(url).is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some());
    if ok {
        Ok(())
    } else {
        Err(format!("url must be http(s)://host[:port]: {url}"))
    }
}

fn check_did(did: &str) -> Result<(), String> {
    if vlatproto::syntax::valid_did(did) {
        Ok(())
    } else {
        Err(format!("invalid DID {did}"))
    }
}

fn check_method(m: &str) -> Result<(), String> {
    if !(m.starts_with("app.bsky.") && vlatproto::syntax::valid_nsid(m)) {
        return Err(format!("{m} is not an app.bsky.* NSID"));
    }
    if LOCAL_APPVIEW_METHODS.contains(&m) {
        return Err(format!("{m} is served by this PDS, never proxied"));
    }
    Ok(())
}

impl AppviewOverride {
    /// From the flags. None when `url` is unset or `accounts` is empty (off).
    pub fn from_flags(
        url_did: Option<(String, String)>,
        accounts: &[String],
        methods: &[String],
        fallback: bool,
        appview: Option<&(String, String)>,
    ) -> anyhow::Result<Option<AppviewOverride>> {
        let set = |v: &[String]| -> HashSet<String> {
            v.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).map(String::from).collect()
        };
        let (accounts, methods) = (set(accounts), set(methods));
        let Some((url, did)) = url_did else {
            anyhow::ensure!(
                accounts.is_empty() && methods.is_empty(),
                "--appview-override-accounts/-methods need --appview-override"
            );
            return Ok(None);
        };
        if accounts.is_empty() {
            return Ok(None);
        }
        anyhow::ensure!(appview.is_some(), "--appview-override needs --appview (the default AppView)");
        check_url(&url).map_err(|e| anyhow::anyhow!("--appview-override: {e}"))?;
        check_did(&did).map_err(|e| anyhow::anyhow!("--appview-override: {e}"))?;
        for a in &accounts {
            check_did(a).map_err(|e| anyhow::anyhow!("--appview-override-accounts: {e}"))?;
        }
        anyhow::ensure!(!methods.is_empty(), "--appview-override needs --appview-override-methods");
        for m in &methods {
            check_method(m).map_err(|e| anyhow::anyhow!("--appview-override-methods: {e}"))?;
        }
        Ok(Some(AppviewOverride { url, did, accounts, methods, fallback }))
    }

    pub fn applies(&self, did: &str, lxm: &str) -> bool {
        self.accounts.contains(did) && self.methods.contains(lxm)
    }

    /// As an editable document (lists sorted), for the console.
    pub fn to_doc(&self) -> Doc {
        let sorted = |s: &HashSet<String>| {
            let mut v: Vec<String> = s.iter().cloned().collect();
            v.sort();
            v
        };
        Doc {
            url: self.url.clone(),
            did: self.did.clone(),
            accounts: sorted(&self.accounts),
            methods: sorted(&self.methods),
            fallback: self.fallback,
            ..Doc::default()
        }
    }
}

fn yes() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Doc {
    #[serde(default)]
    pub version: u64,
    /// False keeps the lists but moves nothing.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub did: String,
    #[serde(default)]
    pub accounts: Vec<String>,
    #[serde(default)]
    pub methods: Vec<String>,
    #[serde(default = "yes")]
    pub fallback: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<Audit>,
}

impl Default for Doc {
    fn default() -> Self {
        Doc {
            version: 0,
            enabled: true,
            url: String::new(),
            did: String::new(),
            accounts: Vec::new(),
            methods: Vec::new(),
            fallback: true,
            updated_at: None,
            updated_by: None,
            note: None,
            history: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Audit {
    pub version: u64,
    pub at: String,
    pub by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    pub node: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub changes: Vec<String>,
}

impl Doc {
    /// Without version or audit metadata, values trimmed.
    pub fn editable(&self) -> Doc {
        let trim = |v: &[String]| v.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        Doc {
            enabled: self.enabled,
            url: self.url.trim().to_string(),
            did: self.did.trim().to_string(),
            accounts: trim(&self.accounts),
            methods: trim(&self.methods),
            fallback: self.fallback,
            ..Doc::default()
        }
    }
}

/// Validates `doc`, returning every problem with its JSON path. Ok(None):
/// valid, and moves nothing (off, or no accounts).
pub fn compile(doc: &Doc) -> Result<Option<AppviewOverride>, Vec<String>> {
    let mut errs = Vec::new();
    if !doc.url.is_empty() {
        if let Err(e) = check_url(&doc.url) {
            errs.push(format!("url: {e}"));
        }
    }
    if !doc.did.is_empty() {
        if let Err(e) = check_did(&doc.did) {
            errs.push(format!("did: {e}"));
        }
    }
    let mut seen = HashSet::new();
    for (i, a) in doc.accounts.iter().enumerate() {
        if let Err(e) = check_did(a) {
            errs.push(format!("accounts[{i}]: {e}"));
        } else if !seen.insert(a.as_str()) {
            errs.push(format!("accounts[{i}]: {a} is listed twice"));
        }
    }
    if doc.accounts.len() > MAX_ACCOUNTS {
        errs.push(format!("accounts: more than {MAX_ACCOUNTS}"));
    }
    let mut seen = HashSet::new();
    for (i, m) in doc.methods.iter().enumerate() {
        if let Err(e) = check_method(m) {
            errs.push(format!("methods[{i}]: {e}"));
        } else if !seen.insert(m.as_str()) {
            errs.push(format!("methods[{i}]: {m} is listed twice"));
        }
    }
    if doc.methods.len() > MAX_METHODS {
        errs.push(format!("methods: more than {MAX_METHODS}"));
    }
    if doc.note.as_ref().is_some_and(|n| n.chars().count() > MAX_NOTE) {
        errs.push(format!("note: longer than {MAX_NOTE} characters"));
    }
    let active = doc.enabled && !doc.accounts.is_empty();
    if active {
        if doc.url.is_empty() {
            errs.push("url: required while accounts are listed".into());
        }
        if doc.did.is_empty() {
            errs.push("did: required while accounts are listed".into());
        }
        if doc.methods.is_empty() {
            errs.push("methods: at least one is required while accounts are listed".into());
        }
    }
    if !errs.is_empty() {
        return Err(errs);
    }
    Ok(active.then(|| AppviewOverride {
        url: doc.url.clone(),
        did: doc.did.clone(),
        accounts: doc.accounts.iter().cloned().collect(),
        methods: doc.methods.iter().cloned().collect(),
        fallback: doc.fallback,
    }))
}

/// Refuses unknown fields (a save).
pub fn parse(v: serde_json::Value) -> Result<Doc, String> {
    serde_json::from_value(v).map_err(|e| format!("invalid config: {e}"))
}

const DOC_FIELDS: &[&str] = &[
    "version",
    "enabled",
    "url",
    "did",
    "accounts",
    "methods",
    "fallback",
    "updatedAt",
    "updatedBy",
    "note",
    "history",
];

/// Drops (and returns) top-level fields this build doesn't know; otherwise
/// as strict as [`parse`].
pub fn parse_stored(bytes: &[u8]) -> Result<(Doc, Vec<String>), String> {
    let mut v: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| format!("invalid config JSON: {e}"))?;
    let mut dropped = Vec::new();
    if let Some(m) = v.as_object_mut() {
        m.retain(|k, _| {
            let keep = DOC_FIELDS.contains(&k.as_str());
            if !keep {
                dropped.push(k.clone());
            }
            keep
        });
    }
    let doc = serde_json::from_value(v).map_err(|e| format!("invalid config JSON: {e}"))?;
    Ok((doc, dropped))
}

/// One readable line per change, for the audit log and the history.
pub fn changes(old: Option<&Doc>, new: &Doc) -> Vec<String> {
    let def = Doc::default();
    let old = old.unwrap_or(&def);
    let mut out = Vec::new();
    let onoff = |b: bool| if b { "on" } else { "off" };
    if old.enabled != new.enabled {
        out.push(format!("override {}", if new.enabled { "enabled" } else { "disabled" }));
    }
    if old.url != new.url {
        out.push(format!("url {}→{}", or_dash(&old.url), or_dash(&new.url)));
    }
    if old.did != new.did {
        out.push(format!("did {}→{}", or_dash(&old.did), or_dash(&new.did)));
    }
    if old.fallback != new.fallback {
        out.push(format!("fallback {}→{}", onoff(old.fallback), onoff(new.fallback)));
    }
    for (what, a, b) in [("account", &old.accounts, &new.accounts), ("method", &old.methods, &new.methods)] {
        for x in b.iter().filter(|x| !a.contains(x)) {
            out.push(format!("+{what} {x}"));
        }
        for x in a.iter().filter(|x| !b.contains(x)) {
            out.push(format!("-{what} {x}"));
        }
    }
    out
}

fn or_dash(s: &str) -> &str {
    if s.is_empty() {
        "—"
    } else {
        s
    }
}

pub fn config_path(store: &Store) -> object_store::path::Path {
    object_store::path::Path::from(format!("{}/config/appview_override.json", store.prefix))
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConfigError {
    /// If that much could be read.
    pub version: Option<u64>,
    pub message: String,
    pub at_ms: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Status {
    /// The stored object, once one was read. None: the flags are in force.
    pub doc: Option<Doc>,
    /// Of the last object fetched, good or not.
    seen_etag: Option<String>,
    pub loaded_at_ms: Option<u64>,
    pub checked_at_ms: Option<u64>,
    /// Set while the newest object is rejected.
    pub error: Option<ConfigError>,
}

/// Where the override in force on a node came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    /// No object yet: the node's flags.
    Flags,
    /// The bucket's config object.
    Object,
}

/// A node's override: the flags' until an object is read, then the
/// object's. The hot path reads [`Runtime::active`].
pub struct Runtime {
    boot: Option<Arc<AppviewOverride>>,
    /// Whether `current` is Some: proxied calls skip the lock while off.
    any: AtomicBool,
    current: RwLock<Option<Arc<AppviewOverride>>>,
    status: Mutex<Status>,
    io: tokio::sync::Mutex<()>,
    wake: tokio::sync::Notify,
    started: AtomicBool,
}

#[derive(Debug)]
pub enum SaveError {
    Invalid(Vec<String>),
    Conflict { expected: u64, current: Option<u64> },
    Store(String),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::Invalid(e) => write!(f, "invalid config: {}", e.join("; ")),
            SaveError::Conflict { expected, current: Some(c) } => {
                write!(f, "the config changed since version {expected} (now version {c}); reload and reapply your edit")
            }
            SaveError::Conflict { expected, current: None } => {
                write!(f, "the config changed since version {expected}; reload and reapply your edit")
            }
            SaveError::Store(e) => write!(f, "config store error: {e}"),
        }
    }
}

pub struct SaveReq {
    /// Metadata fields are ignored.
    pub doc: Doc,
    /// 0: none stored.
    pub if_version: u64,
    pub actor: String,
    pub ip: Option<String>,
    pub node: String,
    pub note: Option<String>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

async fn bounded<T>(f: impl std::future::Future<Output = object_store::Result<T>>) -> object_store::Result<T> {
    match tokio::time::timeout(CALL_DEADLINE, f).await {
        Ok(r) => r,
        Err(_) => {
            Err(object_store::Error::Generic { store: "appview_override", source: "config call timed out".into() })
        }
    }
}

/// None when absent.
async fn fetch(store: &Store, etag: Option<String>) -> object_store::Result<Option<(bytes::Bytes, Option<String>)>> {
    let path = config_path(store);
    let got = bounded(async {
        let r = store.raw.get_opts(&path, GetOptions { if_none_match: etag, ..Default::default() }).await?;
        let e = r.meta.e_tag.clone();
        Ok((r.bytes().await?, e))
    })
    .await;
    match got {
        Ok(x) => Ok(Some(x)),
        Err(object_store::Error::NotFound { .. }) => Ok(None),
        Err(e) => Err(e),
    }
}

fn is_conflict(e: &object_store::Error) -> bool {
    matches!(e, object_store::Error::Precondition { .. } | object_store::Error::AlreadyExists { .. })
}

impl Runtime {
    /// `boot`: the flags' override, in force until an object is read.
    pub fn new(boot: Option<AppviewOverride>) -> Runtime {
        let boot = boot.map(Arc::new);
        Runtime {
            any: AtomicBool::new(boot.is_some()),
            current: RwLock::new(boot.clone()),
            boot,
            status: Mutex::new(Status::default()),
            io: tokio::sync::Mutex::new(()),
            wake: tokio::sync::Notify::new(),
            started: AtomicBool::new(false),
        }
    }

    /// The override in force, if any.
    pub fn active(&self) -> Option<Arc<AppviewOverride>> {
        if !self.any.load(Ordering::Acquire) {
            return None;
        }
        self.current.read().clone()
    }

    /// The flags' override.
    pub fn boot(&self) -> Option<&AppviewOverride> {
        self.boot.as_deref()
    }

    pub fn status(&self) -> Status {
        self.status.lock().clone()
    }

    pub fn source(&self) -> Source {
        if self.status.lock().doc.is_some() {
            Source::Object
        } else {
            Source::Flags
        }
    }

    /// The stored version in force (0: none).
    pub fn version(&self) -> u64 {
        self.status.lock().doc.as_ref().map_or(0, |d| d.version)
    }

    fn install(&self, o: Option<AppviewOverride>) {
        let mut cur = self.current.write();
        *cur = o.map(Arc::new);
        self.any.store(cur.is_some(), Ordering::Release);
    }

    /// Validates and installs `doc` (None: back to the flags). On failure
    /// the override in force stays and the error is recorded.
    fn apply(&self, doc: Option<Doc>, etag: Option<String>) -> Result<(), Vec<String>> {
        let now = now_ms();
        let compiled = match &doc {
            None => Ok(self.boot.as_deref().cloned()),
            Some(d) => compile(d),
        };
        let mut st = self.status.lock();
        st.seen_etag = etag;
        st.checked_at_ms = Some(now);
        match compiled {
            Ok(o) => {
                let changed = st.doc != doc;
                self.install(o);
                st.doc = doc;
                st.error = None;
                if changed {
                    st.loaded_at_ms = Some(now);
                }
                Ok(())
            }
            Err(errs) => {
                crate::metrics::APPVIEW_OVERRIDE_CONFIG_ERRORS.inc();
                st.error = Some(ConfigError { version: doc.map(|d| d.version), message: errs.join("; "), at_ms: now });
                Err(errs)
            }
        }
    }

    fn reject(&self, version: Option<u64>, message: String, etag: Option<String>) {
        crate::metrics::APPVIEW_OVERRIDE_CONFIG_ERRORS.inc();
        let now = now_ms();
        let mut st = self.status.lock();
        st.seen_etag = etag;
        st.checked_at_ms = Some(now);
        st.error = Some(ConfigError { version, message, at_ms: now });
    }

    /// Asks the refresher to re-read the object now.
    pub fn wake(&self) {
        self.wake.notify_one();
    }

    /// Re-reads the object (conditional on the last ETag) and installs it if
    /// it changed and validates. Returns whether it installed a new config.
    pub async fn refresh(&self, store: &Store) -> anyhow::Result<bool> {
        let _io = self.io.lock().await;
        let (seen, had_doc) = {
            let st = self.status.lock();
            (st.seen_etag.clone(), st.doc.is_some() || st.error.is_some())
        };
        let got = match fetch(store, seen).await {
            Ok(g) => g,
            Err(object_store::Error::NotModified { .. }) => {
                self.status.lock().checked_at_ms = Some(now_ms());
                return Ok(false);
            }
            Err(e) => return Err(e.into()),
        };
        let Some((bytes, etag)) = got else {
            if !had_doc {
                self.status.lock().checked_at_ms = Some(now_ms());
                return Ok(false);
            }
            // deleted by hand: back to the flags
            let _ = self.apply(None, None);
            return Ok(true);
        };
        match parse_stored(&bytes) {
            Err(msg) => {
                let version =
                    serde_json::from_slice::<serde_json::Value>(&bytes).ok().and_then(|v| v["version"].as_u64());
                tracing::warn!(?version, "AppView override config rejected (keeping the last good config): {msg}");
                self.reject(version, msg, etag);
                Ok(false)
            }
            Ok((doc, dropped)) => {
                if !dropped.is_empty() {
                    tracing::warn!(?dropped, "AppView override config has fields this build doesn't know: ignored");
                }
                let version = doc.version;
                {
                    let mut st = self.status.lock();
                    if st.doc.as_ref() == Some(&doc) {
                        st.seen_etag = etag;
                        st.error = None;
                        st.checked_at_ms = Some(now_ms());
                        return Ok(false);
                    }
                }
                match self.apply(Some(doc), etag) {
                    Ok(()) => {
                        tracing::info!(version, "AppView override config applied");
                        Ok(true)
                    }
                    Err(errs) => {
                        tracing::warn!(
                            version,
                            "AppView override config rejected (keeping the last good config): {}",
                            errs.join("; ")
                        );
                        Ok(false)
                    }
                }
            }
        }
    }

    /// Validates and stores `req.doc` as the next version (CAS), installs it
    /// on this node and returns the stored object.
    pub async fn save(&self, store: &Store, req: SaveReq) -> Result<Doc, SaveError> {
        let _io = self.io.lock().await;
        let cur = fetch(store, None).await.map_err(|e| SaveError::Store(e.to_string()))?;
        let (cur_doc, cur_version, etag) = match &cur {
            None => (None, 0, None),
            Some((b, e)) => {
                // an unreadable object can still be replaced: its version (if
                // any) is what the caller must have seen
                let raw: Option<serde_json::Value> = serde_json::from_slice(b).ok();
                let v = raw.as_ref().and_then(|v| v["version"].as_u64()).unwrap_or(0);
                (parse_stored(b).ok().map(|(d, _)| d), v, e.clone())
            }
        };
        if cur_version != req.if_version {
            return Err(SaveError::Conflict { expected: req.if_version, current: Some(cur_version) });
        }
        let note = req.note.filter(|n| !n.trim().is_empty());
        let mut doc = req.doc.editable();
        doc.version = cur_version + 1;
        doc.note = note.clone();
        compile(&doc).map_err(SaveError::Invalid)?;
        // the first object starts from the flags this node booted with
        let before = cur_doc.clone().or_else(|| self.boot.as_ref().map(|b| b.to_doc()));
        let changes = changes(before.as_ref(), &doc);
        let at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        doc.updated_at = Some(at.clone());
        doc.updated_by = Some(req.actor.clone());
        let mut history = cur_doc.map(|d| d.history).unwrap_or_default();
        history.push(Audit {
            version: doc.version,
            at,
            by: req.actor.clone(),
            ip: req.ip.clone(),
            node: req.node.clone(),
            note,
            changes: if changes.is_empty() { vec!["no changes".into()] } else { changes.clone() },
        });
        let drop = history.len().saturating_sub(HISTORY);
        history.drain(..drop);
        doc.history = history;
        let body = serde_json::to_vec_pretty(&doc).map_err(|e| SaveError::Store(e.to_string()))?;
        let mode = match etag {
            Some(e) => crate::cluster::if_match(Some(e)),
            None => PutMode::Create,
        };
        let put = bounded(store.raw.put_opts(
            &config_path(store),
            PutPayload::from(body),
            PutOptions { mode, ..Default::default() },
        ))
        .await;
        let new_etag = match put {
            Ok(r) => r.e_tag,
            Err(e) if is_conflict(&e) => {
                return Err(SaveError::Conflict { expected: req.if_version, current: None });
            }
            Err(e) => return Err(SaveError::Store(e.to_string())),
        };
        self.apply(Some(doc.clone()), new_etag).map_err(SaveError::Invalid)?;
        tracing::info!(
            version = doc.version,
            by = %req.actor,
            ip = req.ip.as_deref().unwrap_or("-"),
            node = %req.node,
            changes = %changes.join("; "),
            "AppView override config updated"
        );
        Ok(doc)
    }
}

/// Starts this node's refresher (once): a load now, then every
/// [`REFRESH_EVERY`] or when woken. Stops when the runtime is dropped.
pub fn spawn_refresher(rt: &Arc<Runtime>, store: Store) {
    if rt.started.swap(true, Ordering::AcqRel) {
        return;
    }
    let weak = Arc::downgrade(rt);
    tokio::spawn(async move {
        loop {
            let Some(r) = weak.upgrade() else { return };
            if let Err(e) = r.refresh(&store).await {
                tracing::warn!("AppView override config refresh failed (retrying): {e:#}");
            }
            tokio::select! {
                _ = tokio::time::sleep(REFRESH_EVERY) => {}
                _ = r.wake.notified() => {}
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use object_store::ObjectStoreExt;
    use serde_json::json;

    const ME: &str = "did:plc:ewvi7nxzyoun6zhxrhs64oiz";

    #[test]
    fn flags() {
        let av = ("https://api.bsky.app".to_string(), "did:web:api.bsky.app".to_string());
        let ov = || Some(("http://appview.internal:2584".to_string(), "did:web:appview2.example".to_string()));
        let me = vec![ME.to_string()];
        let methods = vec!["app.bsky.feed.getTimeline".to_string(), " app.bsky.actor.getProfile ".to_string()];
        let none: Vec<String> = Vec::new();
        let blank = vec![String::new()];
        fn parse(
            ov: Option<(String, String)>,
            accounts: &[String],
            methods: &[String],
            appview: Option<&(String, String)>,
        ) -> anyhow::Result<Option<AppviewOverride>> {
            AppviewOverride::from_flags(ov, accounts, methods, true, appview)
        }

        // off: the default config, an unset URL, or no accounts
        assert!(crate::server::Config::default().appview_override.is_none());
        assert!(parse(None, &none, &none, Some(&av)).unwrap().is_none());
        assert!(parse(None, &blank, &blank, Some(&av)).unwrap().is_none());
        assert!(parse(ov(), &none, &methods, Some(&av)).unwrap().is_none());

        let o = parse(ov(), &me, &methods, Some(&av)).unwrap().unwrap();
        assert!(o.applies(ME, "app.bsky.actor.getProfile"));
        assert!(!o.applies(ME, "app.bsky.actor.getSuggestions"));
        assert!(!o.applies("did:plc:aaaaaaaaaaaaaaaaaaaaaaaa", "app.bsky.feed.getTimeline"));

        fn refused(
            ov: Option<(String, String)>,
            accounts: &[String],
            methods: &[&str],
            appview: Option<&(String, String)>,
        ) -> String {
            let methods: Vec<String> = methods.iter().map(|m| m.to_string()).collect();
            parse(ov, accounts, &methods, appview).unwrap_err().to_string()
        }
        assert!(refused(None, &me, &[], Some(&av)).contains("need --appview-override"));
        assert!(refused(ov(), &me, &["app.bsky.feed.getTimeline"], None).contains("needs --appview"));
        assert!(refused(ov(), &me, &[], Some(&av)).contains("--appview-override-methods"));
        for m in ["chat.bsky.convo.listConvos", "tools.ozone.moderation.getRepo", "app.bsky.feed.*"] {
            assert!(refused(ov(), &me, &[m], Some(&av)).contains("not an app.bsky.* NSID"), "{m}");
        }
        assert!(refused(ov(), &me, &["app.bsky.actor.getPreferences"], Some(&av)).contains("served by this PDS"));
        assert!(refused(ov(), &["alice".into()], &["app.bsky.feed.getTimeline"], Some(&av)).contains("invalid DID"));
        let bad_url = Some(("appview2:2584".to_string(), "did:web:appview2.example".to_string()));
        assert!(refused(bad_url, &me, &["app.bsky.feed.getTimeline"], Some(&av)).contains("http(s)"));
    }

    fn doc(v: serde_json::Value) -> Doc {
        parse(v).unwrap()
    }

    fn good() -> serde_json::Value {
        json!({
            "url": "http://appview.internal:2584",
            "did": "did:web:appview2.example",
            "accounts": [ME],
            "methods": ["app.bsky.feed.getTimeline"],
        })
    }

    #[test]
    fn validation() {
        let o = compile(&doc(good())).unwrap().unwrap();
        assert!(o.applies(ME, "app.bsky.feed.getTimeline") && o.fallback);
        // valid but moving nothing: off, or no accounts yet
        assert!(compile(&doc(json!({}))).unwrap().is_none());
        let mut off = good();
        off["enabled"] = json!(false);
        assert!(compile(&doc(off)).unwrap().is_none());
        assert!(compile(&doc(json!({"url": "http://a.example", "methods": ["app.bsky.feed.getTimeline"]})))
            .unwrap()
            .is_none());

        let errs = |v: serde_json::Value| compile(&doc(v)).unwrap_err().join("\n");
        let e = errs(json!({
            "url": "ftp://x", "did": "nope", "accounts": [ME, ME, "alice"],
            "methods": ["app.bsky.feed.*", "chat.bsky.convo.listConvos", "app.bsky.actor.putPreferences",
                        "app.bsky.feed.getTimeline", "app.bsky.feed.getTimeline"],
        }));
        for want in [
            "url: url must be http(s)",
            "did: invalid DID nope",
            "accounts[1]: did:plc:ewvi7nxzyoun6zhxrhs64oiz is listed twice",
            "accounts[2]: invalid DID alice",
            "methods[0]: app.bsky.feed.* is not an app.bsky.* NSID",
            "methods[1]: chat.bsky.convo.listConvos is not an app.bsky.* NSID",
            "methods[2]: app.bsky.actor.putPreferences is served by this PDS",
            "methods[4]: app.bsky.feed.getTimeline is listed twice",
        ] {
            assert!(e.contains(want), "{want} in {e}");
        }
        let e = errs(json!({"accounts": [ME]}));
        assert!(
            e.contains("url: required") && e.contains("did: required") && e.contains("methods: at least one"),
            "{e}"
        );
        // unknown fields: refused on save, dropped when stored
        assert!(parse(json!({"methods": [], "wildcards": true})).unwrap_err().contains("wildcards"));
        let (d, dropped) = parse_stored(br#"{"version": 4, "methods": [], "laterFeature": 1}"#).unwrap();
        assert_eq!((d.version, dropped), (4, vec!["laterFeature".to_string()]));
    }

    fn req(v: serde_json::Value, if_version: u64) -> SaveReq {
        SaveReq {
            doc: doc(v),
            if_version,
            actor: "tester".into(),
            ip: Some("127.0.0.1".into()),
            node: "n1".into(),
            note: Some("why".into()),
        }
    }

    fn boot() -> AppviewOverride {
        AppviewOverride {
            url: "http://flags.example".into(),
            did: "did:web:flags.example".into(),
            accounts: [ME.to_string()].into(),
            methods: ["app.bsky.actor.getProfile".to_string()].into(),
            fallback: false,
        }
    }

    #[tokio::test]
    async fn round_trip_cas_and_peers_pick_it_up() {
        let store = Store::memory(None);
        let (a, b) = (Arc::new(Runtime::new(Some(boot()))), Arc::new(Runtime::new(None)));
        // no object: the flags (b has none)
        assert!(!b.refresh(&store).await.unwrap());
        assert_eq!((a.active().unwrap().url.as_str(), a.source()), ("http://flags.example", Source::Flags));
        assert!(b.active().is_none());

        let d = a.save(&store, req(good(), 0)).await.unwrap();
        assert_eq!((d.version, d.history.len()), (1, 1));
        // the first object's changes are against the flags it replaces
        let c = &d.history[0].changes;
        assert!(c.contains(&"url http://flags.example→http://appview.internal:2584".to_string()), "{c:?}");
        assert!(c.contains(&"-method app.bsky.actor.getProfile".to_string()), "{c:?}");
        assert!(c.contains(&"fallback off→on".to_string()), "{c:?}");
        // the stored object reads back as saved
        let raw = store.raw.get(&config_path(&store)).await.unwrap().bytes().await.unwrap();
        assert_eq!(parse_stored(&raw).unwrap().0, d);
        // installed on the writer at once, the object winning over its flags
        let o = a.active().unwrap();
        assert_eq!(
            (o.url.as_str(), o.fallback, a.source(), a.version()),
            ("http://appview.internal:2584", true, Source::Object, 1)
        );
        // a peer on its next refresh; then 304s
        assert!(b.refresh(&store).await.unwrap());
        assert!(b.active().unwrap().applies(ME, "app.bsky.feed.getTimeline"));
        assert!(!b.refresh(&store).await.unwrap());

        // a stale edit is refused, with the version it missed
        let stale = b.save(&store, req(json!({}), 0)).await.unwrap_err();
        assert!(matches!(stale, SaveError::Conflict { expected: 0, current: Some(1) }), "{stale}");
        // turning it off keeps the lists; the flags don't come back
        let mut off = good();
        off["enabled"] = json!(false);
        let d = b.save(&store, req(off, 1)).await.unwrap();
        assert_eq!(
            (d.version, d.history.len(), d.history[1].changes.clone()),
            (2, 2, vec!["override disabled".to_string()])
        );
        assert!(b.active().is_none());
        assert!(a.refresh(&store).await.unwrap());
        assert!(a.active().is_none(), "the object wins over the flags");
        assert_eq!(a.version(), 2);
    }

    #[tokio::test]
    async fn lost_races_and_invalid_objects() {
        let store = Store::memory(None);
        let a = Arc::new(Runtime::new(None));
        // invalid saves are refused and write nothing
        assert!(matches!(a.save(&store, req(json!({"accounts": [ME]}), 0)).await, Err(SaveError::Invalid(_))));
        assert!(store.raw.get(&config_path(&store)).await.is_err());
        a.save(&store, req(good(), 0)).await.unwrap();
        // someone writes garbage by hand: a keeps its last good config, b
        // (never had one) stays off, and both say why
        store
            .raw
            .put(&config_path(&store), PutPayload::from_static(br#"{"version": 5, "accounts": ["alice"]}"#))
            .await
            .unwrap();
        let b = Arc::new(Runtime::new(None));
        assert!(!a.refresh(&store).await.unwrap());
        assert!(!b.refresh(&store).await.unwrap());
        for r in [&a, &b] {
            let e = r.status().error.expect("error surfaced");
            assert_eq!(e.version, Some(5));
            assert!(e.message.contains("invalid DID alice"), "{}", e.message);
        }
        assert!(a.active().is_some() && b.active().is_none());
        // the broken object can be replaced (version 5 seen)
        assert_eq!(a.save(&store, req(good(), 5)).await.unwrap().version, 6);
        assert!(b.refresh(&store).await.unwrap());
        assert!(b.status().error.is_none() && b.active().is_some());
        // deleted by hand: back to the flags
        store.raw.delete(&config_path(&store)).await.unwrap();
        assert!(b.refresh(&store).await.unwrap());
        assert!(b.active().is_none() && b.source() == Source::Flags);
    }
}
