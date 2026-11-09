//! The relying-party side of the OpenID Connect code flow: discovery and
//! JWKS (cached, through the client for operator-configured upstreams),
//! client authentication, the code exchange and the id_token checks. The
//! identity comes only from a validated id_token: there is no userinfo call.

use super::config::{AuthMethod, Provider, SecretRef};
use crate::oauth::jose::DecodedJwt;
use crate::oauth::util::{b64u, b64u_decode};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{json, Value as J};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

const CALL_DEADLINE: Duration = Duration::from_secs(10);
const MAX_DOC_BYTES: usize = 256 * 1024;
const MAX_TOKEN_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_ID_TOKEN_BYTES: usize = 16 * 1024;
const FRESH_FOR: Duration = Duration::from_secs(3600);
/// A failed refresh keeps the last good copy this long.
const STALE_FOR: Duration = Duration::from_secs(86_400);
/// Refetches of the JWKS for an unknown `kid`, per provider.
pub const KID_REFETCH_EVERY: Duration = Duration::from_secs(60);
pub const SKEW: i64 = 60;
/// An id_token comes straight from this exchange.
pub const MAX_ID_TOKEN_AGE: i64 = 600;
const MAX_SUB: usize = 255;

#[derive(Clone, Debug, Deserialize)]
pub struct Discovery {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
    #[serde(default)]
    pub authorization_response_iss_parameter_supported: bool,
    /// The issuer is on the operator's network ([`issuer_internal`]).
    #[serde(skip)]
    pub internal: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcError {
    /// For metrics and tests: discovery, jwks, token, id_token, config,
    /// secret.
    pub kind: &'static str,
    pub message: String,
}

impl std::fmt::Display for OidcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}

fn err(kind: &'static str, message: impl Into<String>) -> OidcError {
    OidcError { kind, message: message.into() }
}

struct Cached<T> {
    val: Arc<T>,
    at: Instant,
}

/// A URL whose fetch just failed isn't fetched again for this long, so a
/// provider that's down doesn't get one request per sign-in attempt.
const FAILED_FOR: Duration = Duration::from_secs(10);

/// Per-node caches. One fetch per URL at a time, so a burst of sign-ins
/// makes one request.
#[derive(Default)]
pub struct Oidc {
    discovery: Mutex<HashMap<String, Cached<Discovery>>>,
    jwks: Mutex<HashMap<String, Cached<Vec<J>>>>,
    kid_refetch: Mutex<HashMap<String, Instant>>,
    flights: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    failed: Mutex<HashMap<String, Instant>>,
    secrets: Mutex<HashMap<String, Arc<Zeroizing<Vec<u8>>>>>,
    signed: Mutex<HashMap<String, (String, i64)>>,
}

/// What the id_token says, once every check passed.
#[derive(Clone, Debug)]
pub struct Identity {
    pub iss: String,
    pub sub: String,
    pub email: Option<String>,
    pub email_verified: bool,
    /// For the user's list of sign-in methods only.
    pub display: Option<String>,
}

fn now_secs() -> i64 {
    crate::oauth::util::now_secs()
}

async fn read_capped(mut r: reqwest::Response, max: usize, kind: &'static str) -> Result<Vec<u8>, OidcError> {
    if r.content_length().is_some_and(|n| n as usize > max) {
        return Err(err(kind, format!("response over {max} bytes")));
    }
    let mut out = Vec::new();
    while let Some(c) = r.chunk().await.map_err(|e| err(kind, e.to_string()))? {
        out.extend_from_slice(&c);
        if out.len() > max {
            return Err(err(kind, format!("response over {max} bytes")));
        }
    }
    Ok(out)
}

/// Whether the issuer's own host is a non-public address (an adapter on
/// the operator's network): then the endpoints its discovery names are
/// fetched as they are. Otherwise every URL goes through the guarded
/// client, so a provider's discovery can't point this node at its own
/// network.
pub async fn issuer_internal(issuer: &str) -> bool {
    let Ok(u) = reqwest::Url::parse(issuer) else { return false };
    let Some(host) = u.host_str() else { return false };
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse::<std::net::IpAddr>() {
        return !vlatproto::did_resolver::is_public_ip(ip);
    }
    let name = bare.trim_end_matches('.').to_ascii_lowercase();
    if name == "localhost" || name.ends_with(".localhost") {
        return true;
    }
    let port = u.port_or_known_default().unwrap_or(443);
    let lookup = tokio::net::lookup_host(format!("{name}:{port}"));
    match tokio::time::timeout(Duration::from_secs(3), lookup).await {
        Ok(Ok(addrs)) => {
            let addrs: Vec<_> = addrs.collect();
            !addrs.is_empty() && addrs.iter().all(|a| !vlatproto::did_resolver::is_public_ip(a.ip()))
        }
        _ => false,
    }
}

/// The request for `url`: as configured for an internal issuer (https, or
/// http to loopback or in dev mode), else through the guarded client.
fn request(
    method: reqwest::Method,
    url: &str,
    internal: bool,
    dev_mode: bool,
    kind: &'static str,
) -> Result<reqwest::RequestBuilder, OidcError> {
    super::config::url_ok(url, dev_mode).map_err(|e| err(kind, e))?;
    match internal {
        true => Ok(vlatproto::http::public().request(method, url)),
        false => vlatproto::http::guarded(dev_mode).request(method, url).map_err(|e| err(kind, format!("{url}: {e}"))),
    }
}

async fn get_json(url: &str, internal: bool, dev_mode: bool, kind: &'static str) -> Result<J, OidcError> {
    let rb = request(reqwest::Method::GET, url, internal, dev_mode, kind)?;
    let fut = async {
        let r = rb
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|e| err(kind, e.to_string()))?;
        if !r.status().is_success() {
            return Err(err(kind, format!("{url}: HTTP {}", r.status().as_u16())));
        }
        let b = read_capped(r, MAX_DOC_BYTES, kind).await?;
        serde_json::from_slice::<J>(&b).map_err(|e| err(kind, format!("{url}: {e}")))
    };
    tokio::time::timeout(CALL_DEADLINE, fut).await.map_err(|_| err(kind, format!("{url}: timed out")))?
}

fn discovery_url(issuer: &str) -> String {
    format!("{}/.well-known/openid-configuration", issuer.trim_end_matches('/'))
}

impl Oidc {
    fn flight(&self, url: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut m = self.flights.lock();
        if m.len() > 256 {
            m.retain(|_, l| Arc::strong_count(l) > 1);
        }
        m.entry(url.to_string()).or_default().clone()
    }

    fn failed_recently(&self, url: &str) -> bool {
        self.failed.lock().get(url).is_some_and(|t| t.elapsed() < FAILED_FOR)
    }

    fn note(&self, url: &str, ok: bool) {
        let mut f = self.failed.lock();
        match ok {
            true => f.remove(url),
            false => f.insert(url.to_string(), Instant::now()),
        };
    }

    pub async fn discovery(&self, p: &Provider, dev_mode: bool) -> Result<Arc<Discovery>, OidcError> {
        let cached = |me: &Oidc, max: Duration| {
            me.discovery.lock().get(&p.issuer).filter(|c| c.at.elapsed() < max).map(|c| c.val.clone())
        };
        if let Some(d) = cached(self, FRESH_FOR) {
            return Ok(d);
        }
        let url = discovery_url(&p.issuer);
        let flight = self.flight(&url);
        let _g = flight.lock().await;
        if let Some(d) = cached(self, FRESH_FOR) {
            return Ok(d);
        }
        let fetched = match self.failed_recently(&url) {
            true => Err(err("discovery", format!("{url}: failed moments ago"))),
            false => {
                let r = self.fetch_discovery(p, dev_mode).await;
                self.note(&url, r.is_ok());
                r
            }
        };
        match fetched {
            Ok(d) => {
                let d = Arc::new(d);
                self.discovery.lock().insert(p.issuer.clone(), Cached { val: d.clone(), at: Instant::now() });
                Ok(d)
            }
            Err(e) => match cached(self, STALE_FOR) {
                Some(d) => {
                    tracing::warn!(provider = %p.id, "OIDC discovery refresh failed (using the last good copy): {e}");
                    Ok(d)
                }
                None => Err(e),
            },
        }
    }

    async fn fetch_discovery(&self, p: &Provider, dev_mode: bool) -> Result<Discovery, OidcError> {
        let internal = issuer_internal(&p.issuer).await;
        let j = get_json(&discovery_url(&p.issuer), internal, dev_mode, "discovery").await;
        crate::metrics::oidc_request(&p.id, "discovery", j.is_ok());
        let mut d: Discovery = serde_json::from_value(j?).map_err(|e| err("discovery", e.to_string()))?;
        if d.issuer != p.issuer {
            return Err(err("discovery", format!("issuer is {:?}, not the configured {:?}", d.issuer, p.issuer)));
        }
        d.internal = internal;
        for u in [&d.authorization_endpoint, &d.token_endpoint, &d.jwks_uri] {
            super::config::url_ok(u, dev_mode).map_err(|e| err("discovery", e))?;
            if !internal {
                let parsed = reqwest::Url::parse(u).map_err(|e| err("discovery", e.to_string()))?;
                vlatproto::did_resolver::check_outbound_url(&parsed, dev_mode)
                    .map_err(|e| err("discovery", format!("{u}: {e}")))?;
            }
        }
        Ok(d)
    }

    /// The key named `kid`: from the cache, or after one refetch if the
    /// cache is stale or doesn't have it (at most once per
    /// [`KID_REFETCH_EVERY`] per provider for an unknown kid). A failed
    /// refetch falls back to a copy younger than a day.
    async fn key(&self, p: &Provider, d: &Discovery, kid: &str, dev_mode: bool) -> Result<J, OidcError> {
        let uri = d.jwks_uri.as_str();
        let find = |keys: &[J]| keys.iter().find(|k| k.get("kid").and_then(|v| v.as_str()) == Some(kid)).cloned();
        let unknown = || err("id_token", format!("unknown signing key {kid:?}"));
        let cached = |me: &Oidc| me.jwks.lock().get(uri).map(|c| (c.val.clone(), c.at));
        let started = Instant::now();
        if let Some((keys, at)) = cached(self) {
            if at.elapsed() < FRESH_FOR {
                if let Some(k) = find(&keys) {
                    return Ok(k);
                }
                let mut last = self.kid_refetch.lock();
                if last.get(&p.id).is_some_and(|t| t.elapsed() < KID_REFETCH_EVERY) {
                    return Err(unknown());
                }
                last.insert(p.id.clone(), Instant::now());
            }
        }
        let flight = self.flight(uri);
        let _g = flight.lock().await;
        // another request refreshed it while this one waited
        if let Some((keys, at)) = cached(self).filter(|(_, at)| *at >= started) {
            let _ = at;
            return find(&keys).ok_or_else(unknown);
        }
        let fetched = match self.failed_recently(uri) {
            true => Err(err("jwks", format!("{uri}: failed moments ago"))),
            false => {
                let j = get_json(uri, d.internal, dev_mode, "jwks").await;
                crate::metrics::oidc_request(&p.id, "jwks", j.is_ok());
                let keys = j.and_then(|j| {
                    j.get("keys").and_then(|k| k.as_array()).cloned().ok_or_else(|| err("jwks", "no keys array"))
                });
                self.note(uri, keys.is_ok());
                keys
            }
        };
        let keys = match fetched {
            Ok(k) => {
                let k = Arc::new(k);
                self.jwks.lock().insert(uri.to_string(), Cached { val: k.clone(), at: Instant::now() });
                k
            }
            Err(e) => match cached(self).filter(|(_, at)| at.elapsed() < STALE_FOR) {
                Some((k, _)) => {
                    tracing::warn!(provider = %p.id, "JWKS refresh failed (using the last good copy): {e}");
                    k
                }
                None => return Err(e),
            },
        };
        find(&keys).ok_or_else(unknown)
    }

    /// Drops this node's discovery and JWKS copies for `issuer`.
    pub fn forget(&self, issuer: &str) {
        if let Some(d) = self.discovery.lock().remove(issuer) {
            self.jwks.lock().remove(&d.val.jwks_uri);
            self.failed.lock().remove(&d.val.jwks_uri);
        }
        self.failed.lock().remove(&discovery_url(issuer));
    }

    async fn secret_bytes(
        &self,
        secrets: &crate::secrets::Secrets,
        p: &Provider,
        r: &SecretRef,
    ) -> Result<Arc<Zeroizing<Vec<u8>>>, OidcError> {
        let SecretRef::Wrapped(blob) = r;
        let key = format!("{}\0{blob}", p.id);
        if let Some(v) = self.secrets.lock().get(&key) {
            return Ok(v.clone());
        }
        let u = secrets
            .unwrap(crate::secrets::Purpose::SignInProviderSecret, &p.id, blob)
            .await
            .map_err(|e| err("secret", e.to_string()))?;
        let v = Arc::new(u.plaintext);
        self.secrets.lock().insert(key, v.clone());
        Ok(v)
    }

    async fn signing_key(
        &self,
        secrets: &crate::secrets::Secrets,
        p: &Provider,
    ) -> Result<p256::ecdsa::SigningKey, OidcError> {
        let r = p.client_auth.key.as_ref().ok_or_else(|| err("config", "no key"))?;
        let b = self.secret_bytes(secrets, p, r).await?;
        parse_p256_key(&b).map_err(|e| err("secret", e))
    }

    /// The client secret: configured, or an ES256 JWT minted and cached
    /// for most of its lifetime.
    async fn client_secret(&self, secrets: &crate::secrets::Secrets, p: &Provider) -> Result<String, OidcError> {
        let a = &p.client_auth;
        if let Some(r) = &a.secret {
            let b = self.secret_bytes(secrets, p, r).await?;
            return String::from_utf8(b.to_vec()).map_err(|_| err("secret", "client secret isn't UTF-8"));
        }
        let s = a.signed_secret.as_ref().ok_or_else(|| err("config", "no client secret"))?;
        let cache_key = format!("{}\0{}", p.id, serde_json::to_string(a).unwrap_or_default());
        let now = now_secs();
        if let Some((jwt, exp)) = self.signed.lock().get(&cache_key) {
            let life = s.lifetime_secs.unwrap_or(super::config::DEFAULT_SIGNED_SECRET_SECS) as i64;
            if now < exp - life / 5 {
                return Ok(jwt.clone());
            }
        }
        let sk = self.signing_key(secrets, p).await?;
        let life = s.lifetime_secs.unwrap_or(super::config::DEFAULT_SIGNED_SECRET_SECS) as i64;
        let exp = now + life;
        let claims = json!({
            "iss": s.issuer,
            "iat": now,
            "exp": exp,
            "aud": s.audience.as_deref().unwrap_or(&p.issuer),
            "sub": p.client_id,
        });
        let jwt = sign_es256(&sk, a.key_id.as_deref().unwrap_or(""), &claims)?;
        self.signed.lock().insert(cache_key, (jwt.clone(), exp));
        Ok(jwt)
    }

    /// The token endpoint's id_token for `code`.
    #[allow(clippy::too_many_arguments)]
    pub async fn exchange(
        &self,
        secrets: &crate::secrets::Secrets,
        p: &Provider,
        d: &Discovery,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
        dev_mode: bool,
    ) -> Result<String, OidcError> {
        let mut form: Vec<(&str, String)> = vec![
            ("grant_type", "authorization_code".into()),
            ("code", code.into()),
            ("redirect_uri", redirect_uri.into()),
            ("code_verifier", verifier.into()),
        ];
        let mut basic = None;
        match p.client_auth.method {
            AuthMethod::ClientSecretBasic => {
                // RFC 6749 §2.3.1: each part form-encoded first
                let enc = crate::oauth::util::form_encode_component;
                basic = Some(base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    format!("{}:{}", enc(&p.client_id), enc(&self.client_secret(secrets, p).await?)),
                ));
            }
            AuthMethod::ClientSecretPost => {
                form.push(("client_id", p.client_id.clone()));
                form.push(("client_secret", self.client_secret(secrets, p).await?));
            }
            AuthMethod::PrivateKeyJwt => {
                let sk = self.signing_key(secrets, p).await?;
                let now = now_secs();
                let claims = json!({
                    "iss": p.client_id,
                    "sub": p.client_id,
                    "aud": d.token_endpoint,
                    "jti": crate::oauth::util::random_id("", 16),
                    "iat": now,
                    "exp": now + 60,
                });
                let jwt = sign_es256(&sk, p.client_auth.key_id.as_deref().unwrap_or(""), &claims)?;
                form.push(("client_id", p.client_id.clone()));
                form.push(("client_assertion_type", "urn:ietf:params:oauth:client-assertion-type:jwt-bearer".into()));
                form.push(("client_assertion", jwt));
            }
        }
        let rb = request(reqwest::Method::POST, &d.token_endpoint, d.internal, dev_mode, "token")?;
        let fut = async {
            let mut rb = rb.header(reqwest::header::ACCEPT, "application/json").form(&form);
            if let Some(b) = &basic {
                rb = rb.header(reqwest::header::AUTHORIZATION, format!("Basic {b}"));
            }
            let r = rb.send().await.map_err(|e| err("token", e.to_string()))?;
            let status = r.status();
            let b = read_capped(r, MAX_TOKEN_RESPONSE_BYTES, "token").await?;
            let j: J =
                serde_json::from_slice(&b).map_err(|_| err("token", format!("HTTP {}: not JSON", status.as_u16())))?;
            if !status.is_success() {
                let e = j.get("error").and_then(|v| v.as_str()).unwrap_or("error");
                return Err(err(
                    "token",
                    format!("HTTP {}: {}", status.as_u16(), e.chars().take(100).collect::<String>()),
                ));
            }
            j.get("id_token")
                .and_then(|v| v.as_str())
                .map(String::from)
                .ok_or_else(|| err("token", "no id_token in the token response"))
        };
        let r = tokio::time::timeout(CALL_DEADLINE, fut).await.map_err(|_| err("token", "timed out")).and_then(|r| r);
        crate::metrics::oidc_request(&p.id, "token", r.is_ok());
        r
    }

    /// Every check on the id_token (docs/federated-sign-in.html "id_token
    /// checks"), the signature against the provider's JWKS.
    pub async fn validate(
        &self,
        p: &Provider,
        d: &Discovery,
        id_token: &str,
        nonce: &str,
        dev_mode: bool,
    ) -> Result<Identity, OidcError> {
        if id_token.len() > MAX_ID_TOKEN_BYTES {
            return Err(err("id_token", "too large"));
        }
        let jwt = DecodedJwt::decode(id_token).map_err(|e| err("id_token", e))?;
        let alg = jwt.alg().to_string();
        if !p.algs().contains(&alg.as_str()) {
            return Err(err("id_token", format!("alg {alg:?} is not allowed")));
        }
        let kid = jwt
            .header
            .get("kid")
            .and_then(|v| v.as_str())
            .filter(|k| !k.is_empty())
            .ok_or_else(|| err("id_token", "no kid"))?
            .to_string();
        let key = self.key(p, d, &kid, dev_mode).await?;
        verify_signature(id_token, &alg, &key)?;
        check_claims(p, &jwt.payload, nonce, now_secs())
    }
}

pub(crate) fn parse_p256_key(b: &[u8]) -> Result<p256::ecdsa::SigningKey, String> {
    use p256::pkcs8::DecodePrivateKey;
    let text = std::str::from_utf8(b).ok().map(str::trim);
    let sk = match text {
        Some(t) if t.starts_with("-----BEGIN") => p256::SecretKey::from_pkcs8_pem(t).map_err(|e| e.to_string())?,
        _ => p256::SecretKey::from_pkcs8_der(b).map_err(|e| e.to_string())?,
    };
    Ok(p256::ecdsa::SigningKey::from(&sk))
}

/// Verified before it is returned.
fn sign_es256(sk: &p256::ecdsa::SigningKey, kid: &str, claims: &J) -> Result<String, OidcError> {
    use p256::ecdsa::signature::{RandomizedSigner, Verifier};
    use p256::elliptic_curve::{common::getrandom::SysRng, rand_core::UnwrapErr};
    let header = json!({"alg": "ES256", "kid": kid, "typ": "JWT"});
    let input = format!("{}.{}", b64u(serde_json::to_vec(&header).unwrap()), b64u(serde_json::to_vec(claims).unwrap()));
    for _ in 0..2 {
        let sig: p256::ecdsa::Signature = sk.sign_with_rng(&mut UnwrapErr(SysRng), input.as_bytes());
        if sk.verifying_key().verify(input.as_bytes(), &sig).is_ok() {
            return Ok(format!("{input}.{}", b64u(sig.to_bytes())));
        }
    }
    Err(err("secret", "signature fault"))
}

fn jwk_str<'a>(k: &'a J, f: &str) -> Option<&'a str> {
    k.get(f).and_then(|v| v.as_str())
}

fn jwk_bytes(k: &J, f: &str) -> Result<Vec<u8>, OidcError> {
    jwk_str(k, f).and_then(b64u_decode).ok_or_else(|| err("id_token", format!("signing key has no valid {f:?}")))
}

/// The signature over the token's first two parts, with `key` used only for
/// the algorithm it can serve.
pub fn verify_signature(token: &str, alg: &str, key: &J) -> Result<(), OidcError> {
    use ring::signature as rs;
    if jwk_str(key, "use").is_some_and(|u| u != "sig") {
        return Err(err("id_token", "signing key is not for signatures"));
    }
    if jwk_str(key, "alg").is_some_and(|a| a != alg) {
        return Err(err("id_token", format!("signing key is for {:?}, not {alg:?}", jwk_str(key, "alg"))));
    }
    let (input, sig) = token.rsplit_once('.').ok_or_else(|| err("id_token", "malformed"))?;
    let sig = b64u_decode(sig).ok_or_else(|| err("id_token", "malformed signature"))?;
    let kty = jwk_str(key, "kty").unwrap_or("");
    let ok = match (alg, kty) {
        ("RS256" | "PS256", "RSA") => {
            let n = jwk_bytes(key, "n")?;
            let e = jwk_bytes(key, "e")?;
            let n_trim = n.iter().skip_while(|b| **b == 0).count();
            if n_trim < 256 {
                return Err(err("id_token", "RSA signing key is under 2048 bits"));
            }
            let params = if alg == "RS256" { &rs::RSA_PKCS1_2048_8192_SHA256 } else { &rs::RSA_PSS_2048_8192_SHA256 };
            rs::RsaPublicKeyComponents { n: &n, e: &e }.verify(params, input.as_bytes(), &sig).is_ok()
        }
        ("ES256", "EC") => {
            if jwk_str(key, "crv") != Some("P-256") {
                return Err(err("id_token", "ES256 needs a P-256 key"));
            }
            let (x, y) = (jwk_bytes(key, "x")?, jwk_bytes(key, "y")?);
            if x.len() != 32 || y.len() != 32 || sig.len() != 64 {
                return Err(err("id_token", "malformed ES256 key or signature"));
            }
            let pt = [&[4u8][..], &x, &y].concat();
            rs::UnparsedPublicKey::new(&rs::ECDSA_P256_SHA256_FIXED, pt).verify(input.as_bytes(), &sig).is_ok()
        }
        ("EdDSA", "OKP") => {
            if jwk_str(key, "crv") != Some("Ed25519") {
                return Err(err("id_token", "EdDSA needs an Ed25519 key"));
            }
            let x = jwk_bytes(key, "x")?;
            rs::UnparsedPublicKey::new(&rs::ED25519, x).verify(input.as_bytes(), &sig).is_ok()
        }
        _ => return Err(err("id_token", format!("alg {alg:?} doesn't match a {kty:?} key"))),
    };
    if !ok {
        return Err(err("id_token", "bad signature"));
    }
    Ok(())
}

fn num(payload: &J, k: &str) -> Option<i64> {
    payload.get(k).and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
}

/// Some providers send booleans as strings.
fn truthy(v: Option<&J>) -> bool {
    match v {
        Some(J::Bool(b)) => *b,
        Some(J::String(s)) => s == "true",
        _ => false,
    }
}

fn claim_text(v: &J) -> Option<String> {
    match v {
        J::String(s) => Some(s.clone()),
        J::Bool(_) | J::Number(_) => Some(v.to_string()),
        _ => None,
    }
}

pub fn check_claims(p: &Provider, c: &J, nonce: &str, now: i64) -> Result<Identity, OidcError> {
    let bad = |m: String| Err(err("id_token", m));
    let iss = c.get("iss").and_then(|v| v.as_str()).unwrap_or("");
    if iss != p.issuer {
        return bad(format!("iss {iss:?} is not {:?}", p.issuer));
    }
    let aud: Vec<&str> = match c.get("aud") {
        Some(J::String(s)) => vec![s.as_str()],
        Some(J::Array(a)) => a.iter().filter_map(|v| v.as_str()).collect(),
        _ => vec![],
    };
    if !aud.contains(&p.client_id.as_str()) {
        return bad("aud doesn't name this client".into());
    }
    let azp = c.get("azp").and_then(|v| v.as_str());
    if (aud.len() > 1 && azp.is_none()) || azp.is_some_and(|a| a != p.client_id) {
        return bad("azp doesn't name this client".into());
    }
    let Some(exp) = num(c, "exp") else { return bad("no exp".into()) };
    if exp <= now - SKEW {
        return bad("expired".into());
    }
    let Some(iat) = num(c, "iat") else { return bad("no iat".into()) };
    if iat > now + SKEW || iat < now - MAX_ID_TOKEN_AGE {
        return bad("iat is not now".into());
    }
    if num(c, "nbf").is_some_and(|nbf| nbf > now + SKEW) {
        return bad("not valid yet".into());
    }
    let got = c.get("nonce").and_then(|v| v.as_str()).unwrap_or("");
    if nonce.is_empty() || !crate::auth::ct_eq(got.as_bytes(), nonce.as_bytes()) {
        return bad("nonce mismatch".into());
    }
    let sub = c.get("sub").and_then(|v| v.as_str()).unwrap_or("");
    if sub.is_empty() || sub.len() > MAX_SUB {
        return bad("no usable sub".into());
    }
    for (k, want) in &p.required_claims {
        if c.get(k).and_then(claim_text).as_deref() != Some(want.as_str()) {
            return bad(format!("required claim {k:?} doesn't match"));
        }
    }
    let s = |k: &str| c.get(k).and_then(|v| v.as_str()).map(|x| x.chars().take(256).collect::<String>());
    let email = s("email").filter(|e| e.contains('@'));
    Ok(Identity {
        iss: iss.to_string(),
        sub: sub.to_string(),
        email_verified: email.is_some() && truthy(c.get("email_verified")),
        display: email.clone().or_else(|| s("preferred_username")).or_else(|| s("name")),
        email,
    })
}

#[cfg(test)]
mod tests {
    use super::super::config::tests::provider;
    use super::*;

    /// A throwaway 2048-bit RSA key (PKCS#8 DER, base64) for RS256 and PS256.
    const RSA_TEST_KEY: &str = concat!(
        "MIIEvAIBADANBgkqhkiG9w0BAQEFAASCBKYwggSiAgEAAoIBAQDGSpORo6ZakgaxOBbCCuauTKnT4fRSTkwFg1OoTXawn50V",
        "DM0Yp9fMm4PTn0WHkfkymG12dvs6s+UdSEjx9wlRiXHKzrgQYpEKs/1HgvxlGldsgeVDIl/I+dVWqahtru3Y7u8MaM1mrnSL",
        "rdE1ccHgDYBdJSqR11mJClHIMT6bglez28rBBEGMiiJd5xya7sbOzp+tp2wDcnzDz0OiCCLpatuxkLrDlEC8LGmYn8WlKPUy",
        "ikIGnw7+aZ0sQgVG+VbZUztwejEK93AryqmbKxuc6TUvreYccxYhmKSpnKfhIBdivzj5/MOYrozRhxw/DPpMUIyXejQJfi3f",
        "Hj3a+BN1AgMBAAECggEAFEX2rXrCXNs2NPNLnF459JMg/+lHrgyUSDIDoR7+ZhODn23WIFG0xhYWVdIgbTp29H3SHz0N9ZQD",
        "9kj4MEj2GnCHglk9R2uUcbgySpfxTzLwwkbLlmAS0/z7G2ewMTqj1bykEyw55+gpQUitrZ2qWUp4F83feta+MD0uXF3wjTyJ",
        "wYJU80vPwrHbJEQashpRj2MUTh2agKNaH1V2oDHULUOSqE3ocCbeVGxTEt6vDlFsEBH3E9LFdNC1SkfdjKqxgTHBdGNHPs2R",
        "Hqa3PwtSihJ72LG3iWCOwFrguYCQxyowq4Z69eCluHudy0HZdlvnu1/E496knle3QKLoRGHfZwKBgQDixkBVpj1S4Tz6DCU+",
        "eHtDoCXiS+OZNlP08TEBzeIiS8YdxnIKJ6407jRHM6duN1CaWxCvsMggd8PFAbhv/9CPP2VUMcWBfZ3yrj95bMMEHUryszkZ",
        "03LS5ch8gQfPXCN5+DBIYGWT/O91PQaG1UBuScWuAHmVJHYyF5fwUrW6rwKBgQDf2JwR/jokAS8sBUwn+qPRXPqNAWmiJ9hf",
        "y3OGcNo+nun0ievKQxiOJxRBEQV8vL3YKHOf+6QH0Kp2m3f+5cM9jD3YCI8RFtdmv9tPcWrgrUEnkCsNJ9C/mQzYHw6ZEccJ",
        "gx5/9GcBG0jfMVU2GaXNbV3fKFs1MrEA8V+QyHWNGwJ/PgH3lXPfUMpF4hlwFaTcD6G6uiYGFd7FFjTdMaEaNEaD5aBdjmlL",
        "OS3pHY8COSDm6U39Qg0Uq8wTu8q8o+NPI9wx2NPDLaUrizAaKxmSOY3I8JQRCP7IZuJt751YhVqsWK2EhkXVgKGo2GXXYwbs",
        "lzuU0f9PP3i42PmakhwLlQKBgQC0WU8VaCDr1VBuRS5gn53RFvp/FP5IuZWpSMAhN1d/UIXqkhy5b2mfTnkRG2eyYlcKizzt",
        "Ts+PxF0zBnudkPzG3/EL14l9GUwe6sEFtrmHn8940rz88nFLjUI99nfKd/pG/a9bJEzBJXyG5rZiyjM92bniDB/AMsFNoyt9",
        "+SRMtQKBgQDEqroBUuoZSlbafvNAQkvLKjBb4H2vO4OORZ9i+eNbTp1xTw0F3PAl/O52fY3p9SijGE7gqQl2sOwIn1hccOA5",
        "6shcq2g9cQMXy30WHGQZVB1g2IGrtxtJFHDj18rxUfXEb+vYurlVIzJZxJsiXxWQAN4xBGzbKUUpBkNUnCxVwQ==",
    );

    fn rsa_jwk_and_sign(alg: &str) -> (J, String) {
        use base64::Engine;
        use ring::signature as rs;
        let der = base64::engine::general_purpose::STANDARD.decode(RSA_TEST_KEY).unwrap();
        let kp = rs::RsaKeyPair::from_pkcs8(&der).unwrap();
        let c: rs::RsaPublicKeyComponents<Vec<u8>> = kp.public().into();
        let (n, e) = (c.n, c.e);
        let jwk = json!({"kty": "RSA", "n": b64u(&n), "e": b64u(&e), "kid": "r"});
        let input = format!("{}.{}", b64u(format!(r#"{{"alg":"{alg}","kid":"r"}}"#)), b64u(b"{}"));
        let params: &dyn rs::RsaEncoding = if alg == "RS256" { &rs::RSA_PKCS1_SHA256 } else { &rs::RSA_PSS_SHA256 };
        let mut sig = vec![0; kp.public().modulus_len()];
        kp.sign(params, &ring::rand::SystemRandom::new(), input.as_bytes(), &mut sig).unwrap();
        (jwk, format!("{input}.{}", b64u(sig)))
    }

    #[test]
    fn rsa_and_eddsa_signatures() {
        for alg in ["RS256", "PS256"] {
            let (jwk, tok) = rsa_jwk_and_sign(alg);
            assert!(verify_signature(&tok, alg, &jwk).is_ok(), "{alg}");
            let other = if alg == "RS256" { "PS256" } else { "RS256" };
            assert!(verify_signature(&tok, other, &jwk).is_err(), "{alg} as {other}");
            assert!(verify_signature(&tok, "ES256", &jwk).is_err());
        }
        use ring::signature::KeyPair;
        let rng = ring::rand::SystemRandom::new();
        let doc = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        let kp = ring::signature::Ed25519KeyPair::from_pkcs8(doc.as_ref()).unwrap();
        let jwk = json!({"kty": "OKP", "crv": "Ed25519", "x": b64u(kp.public_key().as_ref())});
        let input = format!("{}.{}", b64u(br#"{"alg":"EdDSA","kid":"e"}"#), b64u(b"{}"));
        let tok = format!("{input}.{}", b64u(kp.sign(input.as_bytes()).as_ref()));
        assert!(verify_signature(&tok, "EdDSA", &jwk).is_ok());
        let mut x448 = jwk.clone();
        x448["crv"] = json!("Ed448");
        assert!(verify_signature(&tok, "EdDSA", &x448).is_err());
        // a short RSA key is refused whatever its signature
        let short = json!({"kty": "RSA", "n": b64u([0xffu8; 128]), "e": "AQAB"});
        assert!(verify_signature(&tok, "RS256", &short).unwrap_err().message.contains("2048"));
    }

    fn claims(now: i64) -> J {
        json!({"iss": "https://idp.example", "aud": "client", "sub": "u1", "exp": now + 300, "iat": now, "nonce": "n1",
               "email": "a@example.com", "email_verified": "true"})
    }

    #[tokio::test]
    async fn endpoints_of_a_public_issuer_go_through_the_guarded_client() {
        assert!(issuer_internal("http://127.0.0.1:8080").await);
        assert!(issuer_internal("https://10.1.2.3").await);
        assert!(issuer_internal("http://localhost:5556/dex").await);
        assert!(!issuer_internal("https://1.1.1.1").await);
        // outside dev mode, a public issuer's endpoints can't name this network
        for url in
            ["https://127.0.0.1/token", "https://10.0.0.1/token", "https://[::1]/token", "http://idp.example.com/token"]
        {
            assert!(request(reqwest::Method::POST, url, false, false, "token").is_err(), "{url}");
        }
        assert!(request(reqwest::Method::POST, "https://idp.example.com/token", false, false, "token").is_ok());
        // an internal issuer's own endpoints are fetched as configured
        assert!(request(reqwest::Method::POST, "http://127.0.0.1:8080/token", true, false, "token").is_ok());
        assert!(
            request(reqwest::Method::POST, "http://10.0.0.1/token", true, false, "token").is_err(),
            "plain http only to loopback"
        );
    }

    #[test]
    fn claim_rules() {
        let p = provider("x", "https://idp.example");
        let now = 1_800_000_000;
        let ok = check_claims(&p, &claims(now), "n1", now).unwrap();
        assert_eq!((ok.sub.as_str(), ok.email_verified, ok.display.as_deref()), ("u1", true, Some("a@example.com")));
        let with = |f: &dyn Fn(&mut J)| {
            let mut c = claims(now);
            f(&mut c);
            check_claims(&p, &c, "n1", now)
        };
        assert!(with(&|c| c["iss"] = json!("https://idp.example/")).is_err());
        assert!(with(&|c| c["aud"] = json!("other")).is_err());
        assert!(with(&|c| c["aud"] = json!(["client", "other"])).is_err(), "several audiences need azp");
        assert!(with(&|c| {
            c["aud"] = json!(["client", "other"]);
            c["azp"] = json!("client");
        })
        .is_ok());
        assert!(with(&|c| c["azp"] = json!("other")).is_err());
        assert!(with(&|c| c["exp"] = json!(now - 61)).is_err());
        assert!(with(&|c| c["exp"] = json!(now - 30)).is_ok(), "skew");
        assert!(with(&|c| c["iat"] = json!(now + 120)).is_err());
        assert!(with(&|c| c["iat"] = json!(now - 3600)).is_err());
        assert!(with(&|c| c["nbf"] = json!(now + 120)).is_err());
        assert!(with(&|c| c["nonce"] = json!("n2")).is_err());
        assert!(with(&|c| c.as_object_mut().unwrap().remove("nonce").map(|_| ()).unwrap_or(())).is_err());
        assert!(with(&|c| c["sub"] = json!("")).is_err());
        assert!(!with(&|c| c["email_verified"] = json!("false")).unwrap().email_verified);
        let mut hd = p.clone();
        hd.required_claims.insert("hd".into(), "example.com".into());
        assert!(check_claims(&hd, &claims(now), "n1", now).is_err());
        let mut c = claims(now);
        c["hd"] = json!("example.com");
        assert!(check_claims(&hd, &c, "n1", now).is_ok());
        assert!(check_claims(&p, &claims(now), "", now).is_err(), "an empty nonce never matches");
    }

    #[test]
    fn signatures_match_their_keys() {
        use p256::ecdsa::signature::Signer;
        use p256::elliptic_curve::Generate;
        let sk = p256::ecdsa::SigningKey::generate();
        let pt = sk.verifying_key().to_sec1_point(false);
        let jwk =
            json!({"kty": "EC", "crv": "P-256", "x": b64u(pt.x().unwrap()), "y": b64u(pt.y().unwrap()), "kid": "k"});
        let input = format!("{}.{}", b64u(br#"{"alg":"ES256","kid":"k"}"#), b64u(b"{}"));
        let sig: p256::ecdsa::Signature = sk.sign(input.as_bytes());
        let tok = format!("{input}.{}", b64u(sig.to_bytes()));
        assert!(verify_signature(&tok, "ES256", &jwk).is_ok());
        assert!(verify_signature(&tok, "RS256", &jwk).is_err(), "alg must fit the key type");
        let mut wrong_alg = jwk.clone();
        wrong_alg["alg"] = json!("ES384");
        assert!(verify_signature(&tok, "ES256", &wrong_alg).is_err());
        let mut enc = jwk.clone();
        enc["use"] = json!("enc");
        assert!(verify_signature(&tok, "ES256", &enc).is_err());
        let tampered =
            format!("{}.{}.{}", b64u(br#"{"alg":"ES256","kid":"k"}"#), b64u(b"{\"a\":1}"), b64u(sig.to_bytes()));
        assert!(verify_signature(&tampered, "ES256", &jwk).is_err());
    }

    #[test]
    fn p256_keys_parse_and_sign() {
        use p256::elliptic_curve::Generate;
        use p256::pkcs8::EncodePrivateKey;
        let sk = p256::SecretKey::generate();
        let pem = sk.to_pkcs8_pem(Default::default()).unwrap();
        let k = parse_p256_key(pem.as_bytes()).unwrap();
        let der = sk.to_pkcs8_der().unwrap();
        assert_eq!(parse_p256_key(der.as_bytes()).unwrap().verifying_key(), k.verifying_key());
        let jwt = sign_es256(&k, "kid1", &json!({"a": 1})).unwrap();
        let pt = k.verifying_key().to_sec1_point(false);
        let jwk = json!({"kty": "EC", "crv": "P-256", "x": b64u(pt.x().unwrap()), "y": b64u(pt.y().unwrap())});
        assert!(verify_signature(&jwt, "ES256", &jwk).is_ok());
    }
}
