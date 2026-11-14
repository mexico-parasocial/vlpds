//! Space repo commits (@atproto/space `repo-commit.ts`). A commit names the
//! repo, space and rev, and carries the LtHash digest as `data`. It has no
//! signature: the repo host authenticates it to one syncer at a time with a
//! MAC keyed by that request's shared secret ([`super::mac`]), sent in
//! response headers. A leaked response proves nothing about what the author
//! wrote to a third party.

use super::lthash::LtHash;
use serde_json::{json, Value as J};

pub const COMMIT_VERSION: i64 = 1;

/// The reference's `SpaceCommit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub ver: i64,
    pub repo: String,
    pub space: String,
    pub rev: String,
    pub data: [u8; 32],
}

impl Commit {
    pub fn new(set: &LtHash, space: &str, repo: &str, rev: &str) -> Commit {
        Commit { ver: COMMIT_VERSION, repo: repo.into(), space: space.into(), rev: rev.into(), data: set.digest() }
    }

    /// `com.atproto.space.defs#commit` as JSON.
    pub fn to_json(&self) -> J {
        use base64::Engine;
        json!({
            "ver": self.ver,
            "repo": self.repo,
            "space": self.space,
            "rev": self.rev,
            "data": {"$bytes": base64::engine::general_purpose::STANDARD_NO_PAD.encode(self.data)},
        })
    }

    pub fn from_json(v: &J) -> Option<Commit> {
        use base64::Engine;
        let data = v["data"]["$bytes"].as_str()?;
        let data = base64::engine::general_purpose::STANDARD_NO_PAD.decode(data.trim_end_matches('=')).ok()?;
        Some(Commit {
            ver: v["ver"].as_i64()?,
            repo: v["repo"].as_str()?.into(),
            space: v["space"].as_str()?.into(),
            rev: v["rev"].as_str()?.into(),
            data: data.try_into().ok()?,
        })
    }
}

/// The set hash element of a record. Injective without length prefixes:
/// the collection (an NSID) and the CID (base32) contain no `/`, so the
/// first and last slashes delimit the rkey whatever it holds.
pub fn element(collection: &str, rkey: &str, cid: &str) -> String {
    debug_assert!(!collection.contains('/') && !cid.contains('/'));
    format!("{collection}/{rkey}/{cid}")
}

/// The reference's `verifyCommit`: the commit is for the repo and space
/// asked for, at a version this reads, and its MAC verifies. Only then is
/// `data` the author's claim.
pub fn verify(
    c: &Commit,
    space: &str,
    repo: &str,
    key_share: &str,
    secret: &[u8; 32],
    header: &dyn Fn(&str) -> Option<String>,
) -> Result<(), String> {
    if c.ver != COMMIT_VERSION {
        return Err(format!("unsupported commit version: {}", c.ver));
    }
    if c.repo != repo {
        return Err(format!("commit is for another repo: {}", c.repo));
    }
    if c.space != space {
        return Err(format!("commit is for another space: {}", c.space));
    }
    super::mac::verify_commit(c, key_share, secret, header).map_err(|e| e.0.to_string())
}

/// Whether `set` is the repo `commit` describes. Verify the commit first:
/// on its own this says nothing about authenticity.
pub fn matches(set: &LtHash, commit: &Commit) -> bool {
    set.digest() == commit.data
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::space::vectors::VECTORS;

    fn h(v: &J) -> Vec<u8> {
        hex::decode(v.as_str().unwrap()).unwrap()
    }

    fn h32(v: &J) -> [u8; 32] {
        h(v).try_into().unwrap()
    }

    fn set_of(c: &J) -> LtHash {
        let mut set = LtHash::default();
        for r in c["records"].as_array().unwrap() {
            let el =
                element(r["collection"].as_str().unwrap(), r["rkey"].as_str().unwrap(), r["cid"].as_str().unwrap());
            assert_eq!(el, r["element"].as_str().unwrap());
            set.add(&el);
        }
        set
    }

    /// testdata/spaces-alpha/vectors.json `commitMacs`: the digest the
    /// reference computes over the records, and its verifyCommit.
    #[test]
    fn generated_vectors() {
        let cases = VECTORS["commitMacs"].as_array().unwrap();
        assert!(!cases.is_empty());
        for c in cases {
            let (space, repo) = (c["space"].as_str().unwrap(), c["repo"].as_str().unwrap());
            let commit = Commit::new(&set_of(c), space, repo, c["rev"].as_str().unwrap());
            assert_eq!(commit.data[..], h(&c["data"])[..]);
            assert_eq!(Commit::from_json(&commit.to_json()), Some(commit.clone()));
            let headers: std::collections::HashMap<String, String> = c["headers"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
                .collect();
            let lookup = |k: &str| headers.get(k).cloned();
            let secret = h32(&c["sharedSecret"]);
            let share = c["keyShare"].as_str().unwrap();
            verify(&commit, space, repo, share, &secret, &lookup).unwrap();
            assert!(verify(&commit, space, "did:example:other", share, &secret, &lookup)
                .unwrap_err()
                .contains("another repo"));
            assert!(verify(&commit, "at://x", repo, share, &secret, &lookup).unwrap_err().contains("another space"));
            let v2 = Commit { ver: 2, ..commit.clone() };
            assert!(verify(&v2, space, repo, share, &secret, &lookup).unwrap_err().contains("version"));
        }
    }

    #[test]
    fn elements() {
        let cid = "bafyreidefdycgbfy3oglcb6ism3eqhyp5llsrpzxjsuac2gsy4mtrtx244";
        assert_eq!(element("n.c.a", "1", cid), format!("n.c.a/1/{cid}"));
        assert_ne!(element("n.c.a", "b/1", cid), element("n.c.a", "b/2", cid));
        let (mut a, mut b) = (LtHash::default(), LtHash::default());
        a.add(&element("n.c.a", "1", cid));
        b.add(&element("n.c.a", "2", cid));
        assert_ne!(a, b);
        assert!(matches(&a, &Commit::new(&a, "s", "r", "v")));
        assert!(!matches(&b, &Commit::new(&a, "s", "r", "v")));
    }

    /// The empty repo's digest, which clearRecords leaves.
    #[test]
    fn empty_digest() {
        assert_eq!(
            hex::encode(LtHash::default().digest()),
            "e5a00aa9991ac8a5ee3109844d84a55583bd20572ad3ffcd42792f3c36b183ad"
        );
    }

    pub(crate) fn sub_be(a: &[u8], b: &[u8]) -> Vec<u8> {
        let mut out = vec![0u8; a.len()];
        let mut borrow = 0i16;
        for i in (0..a.len()).rev() {
            let d = a[i] as i16 - b[i] as i16 - borrow;
            borrow = (d < 0) as i16;
            out[i] = d.rem_euclid(256) as u8;
        }
        out
    }
}
