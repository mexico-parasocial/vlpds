//! PARA's public-record policy. Schema validation never disables this gate.
//! Keep the claim contracts aligned with WatZappa/packages/common/src/para-*.
use crate::xrpc::{App, XResult};
use serde_json::{json, Value as J};
use std::sync::LazyLock;
use std::time::Duration;
use vlatproto::xrpc::XrpcError;

#[derive(Clone, Default)]
pub struct Config {
    pub enabled: bool,
    pub vote_verifier_url: Option<String>,
    pub delegation_verifier_url: Option<String>,
}

impl Config {
    pub fn new(enabled: bool, vote: Option<String>, delegation: Option<String>) -> anyhow::Result<Self> {
        for url in vote.iter().chain(delegation.iter()) {
            verifier_url(url)?;
        }
        Ok(Self { enabled, vote_verifier_url: vote, delegation_verifier_url: delegation })
    }
}

fn verifier_url(s: &str) -> anyhow::Result<()> {
    let u = reqwest::Url::parse(s)?;
    anyhow::ensure!(
        u.username().is_empty() && u.password().is_none() && u.query().is_none() && u.fragment().is_none(),
        "PARA verifier URL cannot contain credentials, query or fragment"
    );
    anyhow::ensure!(
        u.scheme() == "https"
            || (u.scheme() == "http" && matches!(u.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))),
        "PARA verifier requires HTTPS or loopback HTTP"
    );
    Ok(())
}

pub(crate) static HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("PARA HTTP client")
});

fn refused(message: impl Into<String>) -> XrpcError {
    XrpcError::bad("BallotRefused", message)
}

pub fn frozen(collection: &str) -> bool {
    matches!(
        collection,
        "com.para.community.vote"
            | "com.para.community.intensity"
            | "com.para.raq.proposalAnswer"
            | "com.para.community.civicTreeVote"
    )
}

fn proof_format(v: &J, prefix: &str, uuid: bool) -> bool {
    let Some(s) = v.as_str().and_then(|s| s.strip_prefix(prefix)) else { return false };
    let mac = if uuid {
        let Some((id, mac)) = s.split_once(':') else { return false };
        if id.len() != 36
            || !id.bytes().enumerate().all(|(i, b)| {
                if [8, 13, 18, 23].contains(&i) {
                    b == b'-'
                } else {
                    b.is_ascii_hexdigit() && !b.is_ascii_uppercase()
                }
            })
        {
            return false;
        }
        mac
    } else {
        s
    };
    mac.len() == 43 && mac.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

pub fn ballot_claim(record: &J) -> Option<J> {
    let subject = record["subject"].as_str()?;
    let nullifier = record["voteNullifier"].as_str()?;
    if record["isDirect"] != true
        || !subject.starts_with("at://")
        || subject.len() > 1024
        || nullifier.len() != 64
        || !nullifier.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    let mut claim =
        json!({"subjectUri":subject, "voteNullifier":nullifier, "eligibilityProofRef":record["eligibilityProofRef"]});
    match record["subjectType"].as_str()? {
        "cabildeo"
            if record["cabildeo"] == subject
                && record.get("signal").is_none()
                && proof_format(&record["eligibilityProofRef"], "m8:cabildeo:v1:", false) =>
        {
            let option = record["selectedOption"].as_u64()?;
            if option > 9_007_199_254_740_991 {
                return None;
            }
            claim["selectedOption"] = json!(option);
        }
        "policy"
            if record.get("cabildeo").is_none()
                && record.get("selectedOption").is_none()
                && proof_format(&record["eligibilityProofRef"], "m8:policy:v1:", false) =>
        {
            let signal = record["signal"].as_i64()?;
            // No neutral ballot (OD-10 §7.5): whoever takes no stance casts none.
            if signal == 0 || !(-3..=3).contains(&signal) {
                return None;
            }
            claim["signal"] = json!(signal);
        }
        _ => return None,
    }
    if record["delegatedFrom"].as_array().is_some_and(|a| !a.is_empty()) {
        return None;
    }
    Some(claim)
}

pub fn delegation_claim(record: &J) -> Option<J> {
    if !record["delegateTo"].as_str()?.starts_with("did:")
        || record.get("signal").is_some()
        || !proof_format(&record["eligibilityProofRef"], "m8:delegation:v1:", true)
    {
        return None;
    }
    let keys: &[&str] = match record["mode"].as_str()? {
        "active" if record["cabildeo"].as_str()?.starts_with("at://") => {
            &["mode", "delegateTo", "cabildeo", "eligibilityProofRef"]
        }
        "passive"
            if record.get("cabildeo").is_none()
                && !record["party"].as_str()?.trim().is_empty()
                && !record["community"].as_str()?.trim().is_empty()
                && record["scopeFlairs"].as_array().is_some_and(|a| {
                    !a.is_empty() && a.iter().all(|f| f.as_str().is_some_and(|f| !f.trim().is_empty()))
                }) =>
        {
            &["mode", "delegateTo", "party", "community", "scopeFlairs", "eligibilityProofRef"]
        }
        _ => return None,
    };
    Some(J::Object(keys.iter().map(|k| ((*k).into(), record[*k].clone())).collect()))
}

pub async fn check(app: &App, did: &str, collection: &str, record: &J) -> XResult<()> {
    if !app.config.para.enabled {
        return Ok(());
    }
    if frozen(collection) {
        return Err(refused(format!(
            "{collection} is frozen pending private ballots; existing records may be deleted"
        )));
    }
    if collection == "app.bsky.feed.repost" {
        return Err(XrpcError::bad(
            "UnsupportedRecord",
            "PARA shares posts by quoting or highlighting; reposts are disabled",
        ));
    }
    if matches!(collection, "com.para.civic.openQuestionVote" | "com.para.raq.proposalVote" | "com.para.raq.axisVote")
        && (record.get("voteNullifier").is_some() || record.get("eligibilityProofRef").is_some())
    {
        return Err(refused("Public reactions must not carry identity proofs"));
    }
    let (url, claim) = match collection {
        "com.para.civic.vote" => (app.config.para.vote_verifier_url.as_deref(), ballot_claim(record)),
        "com.para.civic.delegation" => (app.config.para.delegation_verifier_url.as_deref(), delegation_claim(record)),
        _ => return Ok(()),
    };
    let mut claim = claim.ok_or_else(|| refused("A valid PARA authorization proof and ballot shape are required"))?;
    let unavailable = || {
        XrpcError::unavailable(
            "ParaVerifierUnavailable",
            "PARA proof verification is unavailable; no record was written",
        )
    };
    let url = url.ok_or_else(unavailable)?;
    verifier_url(url).map_err(|_| unavailable())?;
    claim["actorDid"] = json!(did);
    let response = HTTP.post(url).json(&claim).send().await.map_err(|_| unavailable())?;
    // Status-only protocol: no verifier body or user/session credentials.
    match response.status().as_u16() {
        204 => Ok(()),
        422 => Err(refused("PARA authorization proof was rejected")),
        _ => Err(unavailable()),
    }
}
