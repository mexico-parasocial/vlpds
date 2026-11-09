//! The operator's sign-in providers, stored for the cluster in
//! `{prefix}/config/signin-providers.json` and kept in memory on every node
//! the way `crate::handle_domains` keeps its domains: a conditional GET at
//! startup, every [`REFRESH_EVERY`] and on a peer's nudge, and an unreadable
//! object keeps the last good set. Secrets are only ever stored wrapped
//! under the KEK ([`crate::secrets::Purpose::SignInProviderSecret`], subject
//! the provider id). There is no file form: an admin API that names files
//! would let an operator login read any file on the node and send it to a
//! token endpoint of their choosing.

use object_store::{GetOptions, PutMode, PutOptions, PutPayload};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use vlsync_store::store::Store;

pub const REFRESH_EVERY: Duration = Duration::from_secs(10);
const CALL_DEADLINE: Duration = Duration::from_secs(5);
const CAS_RETRIES: usize = 5;
pub const MAX_PROVIDERS: usize = 20;
pub const MAX_ICON_BYTES: usize = 16 * 1024;
const MAX_NAME: usize = 64;
const MAX_ID: usize = 32;
const MAX_SCOPES: usize = 16;
const MAX_REQUIRED_CLAIMS: usize = 8;
/// Some providers cap a signed client secret at about six months.
pub const MAX_SIGNED_SECRET_SECS: u64 = 180 * 86_400;
pub const DEFAULT_SIGNED_SECRET_SECS: u64 = 3600;
pub const ALGS: [&str; 4] = ["RS256", "PS256", "ES256", "EdDSA"];

pub fn config_path(store: &Store) -> object_store::path::Path {
    object_store::path::Path::from(format!("{}/config/signin-providers.json", store.prefix))
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Doc {
    #[serde(default)]
    pub providers: Vec<Provider>,
    /// Bumped by every save: `ifVersion` on the admin API.
    #[serde(default)]
    pub version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    pub id: String,
    pub name: String,
    /// A `data:image/…` URI, which the sign-in pages' CSP allows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub issuer: String,
    pub client_id: String,
    pub client_auth: ClientAuth,
    #[serde(default = "default_scopes")]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub response_mode: ResponseMode,
    #[serde(default)]
    pub signup: Signup,
    /// Accounts it may create a UTC day, cluster-wide; 0: no cap.
    #[serde(default = "default_signups_per_day")]
    pub signups_per_day: u32,
    #[serde(default)]
    pub trust_email: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub required_claims: BTreeMap<String, String>,
    /// Empty: [`ALGS`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub id_token_algs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<String>,
}

pub const DEFAULT_SIGNUPS_PER_DAY: u32 = 100;

fn default_signups_per_day() -> u32 {
    DEFAULT_SIGNUPS_PER_DAY
}

fn default_scopes() -> Vec<String> {
    vec!["openid".into(), "email".into()]
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResponseMode {
    #[default]
    Query,
    FormPost,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Signup {
    #[default]
    Off,
    Invite,
    Open,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    ClientSecretBasic,
    ClientSecretPost,
    PrivateKeyJwt,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ClientAuth {
    pub method: AuthMethod,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<SecretRef>,
    /// A P-256 key (PKCS#8, PEM or DER): `private_key_jwt`, or `signed_secret`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<SecretRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_id: Option<String>,
    /// The client secret is an ES256 JWT this PDS signs with `key`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signed_secret: Option<SignedSecret>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SecretRef {
    Wrapped(String),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SignedSecret {
    /// The JWT's `iss` (a team or account id at the provider).
    pub issuer: String,
    /// Default: the provider's issuer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audience: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifetime_secs: Option<u64>,
}

impl Provider {
    pub fn algs(&self) -> Vec<&str> {
        match self.id_token_algs.is_empty() {
            true => ALGS.to_vec(),
            false => self.id_token_algs.iter().map(String::as_str).collect(),
        }
    }

    /// `openid` first, whatever the config says.
    pub fn scope(&self) -> String {
        let mut out = vec!["openid"];
        out.extend(self.scopes.iter().map(String::as_str).filter(|s| *s != "openid"));
        out.join(" ")
    }
}

fn is_loopback_http(u: &reqwest::Url) -> bool {
    let host = u.host_str().unwrap_or("").trim_start_matches('[').trim_end_matches(']');
    u.scheme() == "http" && (host == "localhost" || host.parse::<std::net::IpAddr>().is_ok_and(|a| a.is_loopback()))
}

/// https, or plain http to loopback or in dev mode.
pub fn url_ok(s: &str, dev_mode: bool) -> Result<reqwest::Url, String> {
    let u = reqwest::Url::parse(s).map_err(|e| format!("{s}: {e}"))?;
    let ok = u.scheme() == "https" || (u.scheme() == "http" && (dev_mode || is_loopback_http(&u)));
    if !ok || u.host().is_none() || !u.username().is_empty() || u.password().is_some() || u.fragment().is_some() {
        return Err(format!("{s}: must be an https URL without credentials or fragment"));
    }
    Ok(u)
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !id.starts_with('-')
}

/// Why `p` can't be saved, if it can't.
pub fn invalid(p: &Provider, dev_mode: bool) -> Option<String> {
    let id = &p.id;
    if !valid_id(id) {
        return Some(format!("id {id:?}: 1 to {MAX_ID} lowercase letters, digits or hyphens"));
    }
    let name = p.name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME || name.chars().any(char::is_control) {
        return Some(format!("{id}: name must be 1 to {MAX_NAME} characters"));
    }
    if let Some(icon) = &p.icon {
        let ok = icon.len() <= MAX_ICON_BYTES
            && [
                "data:image/png;base64,",
                "data:image/svg+xml;base64,",
                "data:image/webp;base64,",
                "data:image/jpeg;base64,",
            ]
            .iter()
            .any(|pre| icon.starts_with(pre))
            && icon.bytes().all(|b| b.is_ascii_graphic());
        if !ok {
            return Some(format!(
                "{id}: icon must be a base64 data:image/(png|svg+xml|webp|jpeg) URI of at most {MAX_ICON_BYTES} bytes"
            ));
        }
    }
    if let Err(e) = url_ok(&p.issuer, dev_mode) {
        return Some(format!("{id}: issuer {e}"));
    }
    if p.client_id.trim().is_empty() || p.client_id.len() > 512 || p.client_id.chars().any(char::is_control) {
        return Some(format!("{id}: clientId is required"));
    }
    if p.scopes.len() > MAX_SCOPES
        || p.scopes.iter().any(|s| {
            s.is_empty() || s.len() > 128 || !s.bytes().all(|b| b.is_ascii_graphic() && b != b'"' && b != b'\\')
        })
    {
        return Some(format!("{id}: scopes must be at most {MAX_SCOPES} plain scope tokens"));
    }
    if p.required_claims.len() > MAX_REQUIRED_CLAIMS {
        return Some(format!("{id}: at most {MAX_REQUIRED_CLAIMS} required claims"));
    }
    if let Some(a) = p.id_token_algs.iter().find(|a| !ALGS.contains(&a.as_str())) {
        return Some(format!("{id}: idTokenAlgs: {a} is not one of {}", ALGS.join(", ")));
    }
    let a = &p.client_auth;
    let has_key = a.key.is_some() && a.key_id.as_deref().is_some_and(|k| !k.is_empty() && k.len() <= 256);
    match a.method {
        AuthMethod::ClientSecretBasic | AuthMethod::ClientSecretPost => match (&a.secret, &a.signed_secret) {
            (Some(_), None) => {}
            (None, Some(s)) => {
                if !has_key {
                    return Some(format!("{id}: signedSecret needs key and keyId"));
                }
                if s.issuer.trim().is_empty() {
                    return Some(format!("{id}: signedSecret.issuer is required"));
                }
                if s.lifetime_secs.is_some_and(|l| !(60..=MAX_SIGNED_SECRET_SECS).contains(&l)) {
                    return Some(format!("{id}: signedSecret.lifetimeSecs must be 60 to {MAX_SIGNED_SECRET_SECS}"));
                }
            }
            _ => return Some(format!("{id}: {:?} needs exactly one of secret or signedSecret", a.method)),
        },
        AuthMethod::PrivateKeyJwt => {
            if !has_key || a.secret.is_some() || a.signed_secret.is_some() {
                return Some(format!("{id}: private_key_jwt needs key and keyId, and no secret"));
            }
        }
    }
    None
}

pub struct Providers {
    stored: RwLock<Arc<Doc>>,
    seen_etag: Mutex<Option<String>>,
    io: tokio::sync::Mutex<()>,
    wake: tokio::sync::Notify,
    started: AtomicBool,
}

impl Default for Providers {
    fn default() -> Self {
        Providers {
            stored: Default::default(),
            seen_etag: Default::default(),
            io: Default::default(),
            wake: Default::default(),
            started: AtomicBool::new(false),
        }
    }
}

impl Providers {
    pub fn doc(&self) -> Arc<Doc> {
        self.stored.read().clone()
    }

    pub fn get(&self, id: &str) -> Option<Provider> {
        self.stored.read().providers.iter().find(|p| p.id == id).cloned()
    }

    pub fn by_issuer(&self, iss: &str) -> Option<Provider> {
        self.stored.read().providers.iter().find(|p| p.issuer == iss).cloned()
    }

    pub fn is_empty(&self) -> bool {
        self.stored.read().providers.is_empty()
    }

    fn install(&self, doc: Doc, etag: Option<String>) {
        *self.stored.write() = Arc::new(doc);
        *self.seen_etag.lock() = etag;
    }

    pub fn wake(&self) {
        self.wake.notify_one();
    }
}

#[derive(Debug)]
pub enum SaveError {
    Invalid(String),
    NotFound(String),
    Conflict(u64),
    TooMany,
    Store(String),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::Invalid(m) => write!(f, "{m}"),
            SaveError::NotFound(id) => write!(f, "no sign-in provider {id}"),
            SaveError::Conflict(v) => write!(f, "the providers changed (now version {v}): reload and retry"),
            SaveError::TooMany => write!(f, "at most {MAX_PROVIDERS} sign-in providers"),
            SaveError::Store(e) => write!(f, "config store error: {e}"),
        }
    }
}

async fn bounded<T>(f: impl std::future::Future<Output = object_store::Result<T>>) -> object_store::Result<T> {
    match tokio::time::timeout(CALL_DEADLINE, f).await {
        Ok(r) => r,
        Err(_) => {
            Err(object_store::Error::Generic { store: "signin-providers", source: "config call timed out".into() })
        }
    }
}

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

/// Returns whether the set was replaced.
pub async fn refresh(d: &Providers, store: &Store) -> anyhow::Result<bool> {
    let _io = d.io.lock().await;
    let seen = d.seen_etag.lock().clone();
    let got = match fetch(store, seen.clone()).await {
        Ok(g) => g,
        Err(object_store::Error::NotModified { .. }) => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    let Some((bytes, etag)) = got else {
        if seen.is_none() && d.stored.read().providers.is_empty() {
            return Ok(false);
        }
        d.install(Doc::default(), None);
        return Ok(true);
    };
    match serde_json::from_slice::<Doc>(&bytes) {
        Ok(doc) => {
            let n = doc.providers.len();
            d.install(doc, etag);
            tracing::info!(providers = n, "sign-in providers loaded");
            Ok(true)
        }
        Err(e) => {
            *d.seen_etag.lock() = etag;
            tracing::warn!("signin-providers.json unreadable (keeping the last good set): {e}");
            Ok(false)
        }
    }
}

pub fn spawn_refresher(d: &Arc<Providers>, store: Store) {
    if d.started.swap(true, Ordering::AcqRel) {
        return;
    }
    let weak = Arc::downgrade(d);
    tokio::spawn(async move {
        loop {
            let Some(d) = weak.upgrade() else { return };
            if let Err(e) = refresh(&d, &store).await {
                tracing::warn!("sign-in provider refresh failed (retrying): {e:#}");
            }
            tokio::select! {
                _ = tokio::time::sleep(REFRESH_EVERY) => {}
                _ = d.wake.notified() => {}
            }
        }
    });
}

fn is_conflict(e: &object_store::Error) -> bool {
    matches!(e, object_store::Error::Precondition { .. } | object_store::Error::AlreadyExists { .. })
}

/// Read-modify-write under CAS; installs the result on this node. `f` may
/// be called more than once.
pub async fn update(
    d: &Providers,
    store: &Store,
    if_version: Option<u64>,
    f: impl Fn(&mut Doc) -> Result<(), SaveError>,
) -> Result<Doc, SaveError> {
    let _io = d.io.lock().await;
    let store_err = |e: object_store::Error| SaveError::Store(e.to_string());
    for _ in 0..CAS_RETRIES {
        let (mut doc, etag) = match fetch(store, None).await.map_err(store_err)? {
            None => (Doc::default(), None),
            Some((b, e)) => (serde_json::from_slice(&b).unwrap_or_else(|_| Doc::clone(&d.stored.read())), e),
        };
        if if_version.is_some_and(|v| v != doc.version) {
            return Err(SaveError::Conflict(doc.version));
        }
        f(&mut doc)?;
        if doc.providers.len() > MAX_PROVIDERS {
            return Err(SaveError::TooMany);
        }
        doc.version += 1;
        doc.updated_at = Some(vlatproto::events::now_rfc3339());
        let mode = match etag {
            Some(e) => crate::cluster::if_match(Some(e)),
            None => PutMode::Create,
        };
        let body = serde_json::to_vec_pretty(&doc).map_err(|e| SaveError::Store(e.to_string()))?;
        let put = bounded(store.raw.put_opts(
            &config_path(store),
            PutPayload::from(body),
            PutOptions { mode, ..Default::default() },
        ))
        .await;
        match put {
            Ok(r) => {
                d.install(doc.clone(), r.e_tag);
                return Ok(doc);
            }
            Err(e) if is_conflict(&e) => continue,
            Err(e) => return Err(store_err(e)),
        }
    }
    Err(SaveError::Store("signin-providers.json kept changing under the update".into()))
}

/// Replaces the provider with `p.id`, or adds it.
pub async fn put(d: &Providers, store: &Store, p: Provider, if_version: Option<u64>) -> Result<Doc, SaveError> {
    update(d, store, if_version, |doc| {
        if doc.providers.iter().any(|x| x.id != p.id && x.issuer == p.issuer) {
            return Err(SaveError::Invalid(format!("another provider already has the issuer {}", p.issuer)));
        }
        match doc.providers.iter_mut().find(|x| x.id == p.id) {
            Some(x) => *x = p.clone(),
            None => doc.providers.push(p.clone()),
        }
        Ok(())
    })
    .await
}

pub async fn remove(d: &Providers, store: &Store, id: &str, if_version: Option<u64>) -> Result<Doc, SaveError> {
    update(d, store, if_version, |doc| {
        let before = doc.providers.len();
        doc.providers.retain(|p| p.id != id);
        if doc.providers.len() == before {
            return Err(SaveError::NotFound(id.into()));
        }
        Ok(())
    })
    .await
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use object_store::ObjectStoreExt;

    pub(crate) fn provider(id: &str, issuer: &str) -> Provider {
        Provider {
            id: id.into(),
            name: "Example".into(),
            icon: None,
            issuer: issuer.into(),
            client_id: "client".into(),
            client_auth: ClientAuth {
                method: AuthMethod::ClientSecretBasic,
                secret: Some(SecretRef::Wrapped("vw1.x.c2VjcmV0".into())),
                key: None,
                key_id: None,
                signed_secret: None,
            },
            scopes: default_scopes(),
            response_mode: ResponseMode::Query,
            signup: Signup::Off,
            signups_per_day: DEFAULT_SIGNUPS_PER_DAY,
            trust_email: false,
            required_claims: Default::default(),
            id_token_algs: vec![],
            updated_at: None,
            updated_by: None,
        }
    }

    #[test]
    fn validation() {
        let ok = provider("google", "https://accounts.example.com");
        assert_eq!(invalid(&ok, false), None);
        let bad = |f: &dyn Fn(&mut Provider)| {
            let mut p = ok.clone();
            f(&mut p);
            invalid(&p, false).is_some()
        };
        assert!(bad(&|p| p.id = "Google".into()));
        assert!(bad(&|p| p.id = "".into()));
        assert!(bad(&|p| p.issuer = "http://idp.example.com".into()));
        assert!(!bad(&|p| p.issuer = "http://127.0.0.1:8080".into()), "loopback http is fine");
        assert!(bad(&|p| p.issuer = "https://user:pw@idp.example.com".into()));
        assert!(bad(&|p| p.icon = Some("https://evil.example/x.png".into())));
        assert!(bad(&|p| p.icon = Some("data:text/html;base64,PGI+".into())));
        assert!(!bad(&|p| p.icon = Some("data:image/png;base64,iVBORw0K".into())));
        assert!(bad(&|p| p.id_token_algs = vec!["HS256".into()]));
        assert!(bad(&|p| p.id_token_algs = vec!["none".into()]));
        assert!(bad(&|p| p.client_auth.secret = None));
        assert!(bad(&|p| {
            p.client_auth.method = AuthMethod::PrivateKeyJwt;
        }));
        assert!(!bad(&|p| {
            p.client_auth = ClientAuth {
                method: AuthMethod::PrivateKeyJwt,
                secret: None,
                key: Some(SecretRef::Wrapped("vw1.x.y".into())),
                key_id: Some("k1".into()),
                signed_secret: None,
            };
        }));
        assert!(!bad(&|p| {
            p.client_auth = ClientAuth {
                method: AuthMethod::ClientSecretPost,
                secret: None,
                key: Some(SecretRef::Wrapped("vw1.x.a2V5".into())),
                key_id: Some("ABC".into()),
                signed_secret: Some(SignedSecret { issuer: "TEAM".into(), audience: None, lifetime_secs: Some(3600) }),
            };
        }));
        assert_eq!(ok.scope(), "openid email");
    }

    #[tokio::test]
    async fn put_remove_and_peers_pick_it_up() {
        let store = Store::memory(None);
        let (a, b) = (Providers::default(), Providers::default());
        assert!(!refresh(&b, &store).await.unwrap());
        put(&a, &store, provider("one", "https://one.example"), None).await.unwrap();
        assert!(a.get("one").is_some());
        assert!(b.get("one").is_none());
        assert!(refresh(&b, &store).await.unwrap());
        assert_eq!(b.by_issuer("https://one.example").map(|p| p.id), Some("one".into()));
        assert!(matches!(
            put(&b, &store, provider("two", "https://one.example"), None).await,
            Err(SaveError::Invalid(_))
        ));
        assert!(matches!(
            put(&b, &store, provider("two", "https://two.example"), Some(0)).await,
            Err(SaveError::Conflict(1))
        ));
        let doc = put(&b, &store, provider("two", "https://two.example"), Some(1)).await.unwrap();
        assert_eq!((doc.version, doc.providers.len()), (2, 2));
        assert!(matches!(remove(&b, &store, "nope", None).await, Err(SaveError::NotFound(_))));
        remove(&b, &store, "one", None).await.unwrap();
        assert!(refresh(&a, &store).await.unwrap());
        assert!(a.get("one").is_none() && a.get("two").is_some());
        store.raw.put(&config_path(&store), PutPayload::from_static(b"{{")).await.unwrap();
        assert!(!refresh(&a, &store).await.unwrap());
        assert!(a.get("two").is_some(), "garbage keeps the last good set");
    }
}
