//! Third-party sign-in over OpenID Connect (docs/federated-sign-in.html):
//! operator-configured providers ([`config`]) and the relying-party side of
//! the code flow ([`oidc`]). The HTTP surface, rows and sign-in glue are in
//! `crate::xrpc::federated`.

pub mod config;
pub mod oidc;

use sha2::{Digest, Sha256};
use std::sync::Arc;

/// Everything a node keeps for it: the providers and the OIDC caches.
#[derive(Default)]
pub struct Federated {
    pub providers: Arc<config::Providers>,
    pub oidc: oidc::Oidc,
}

/// The identity's key: SHA-256 over the issuer, a zero byte and the subject.
pub fn identity_hash(iss: &str, sub: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(iss.as_bytes());
    h.update([0u8]);
    h.update(sub.as_bytes());
    h.finalize().into()
}

/// What a session records as the identity that signed it in, the size of a
/// passkey's `pk:` reference.
pub fn auth_ref(iss: &str, sub: &str) -> String {
    format!("fed:{}", hex::encode(&identity_hash(iss, sub)[..12]))
}

/// The credential references in a session's `auth_cred`: one, or a
/// provider's and the passkey that was its second factor.
pub fn cred_refs(auth_cred: Option<&str>) -> impl Iterator<Item = &str> {
    auth_cred.into_iter().flat_map(|c| c.split(' ')).filter(|r| !r.is_empty())
}

/// Whether a session with `auth_cred` was signed in with `r`.
pub fn signed_in_with(auth_cred: Option<&str>, r: &str) -> bool {
    cred_refs(auth_cred).any(|x| x == r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs() {
        let r = auth_ref("https://idp.example", "123");
        assert_eq!(r.len(), 4 + 24);
        assert_ne!(r, auth_ref("https://idp.example1", "23"), "the zero byte separates issuer and subject");
        let both = format!("{r} pk:00");
        assert!(signed_in_with(Some(&both), &r) && signed_in_with(Some(&both), "pk:00"));
        assert!(!signed_in_with(Some(&both), "pk:0"));
        assert!(!signed_in_with(None, &r));
    }
}
