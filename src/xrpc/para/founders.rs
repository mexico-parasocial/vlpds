//! The creator signs the invitation (a founder-list item) and activation.
//! Acceptance signs only the invitee's membership. Public records provide the
//! durable workflow, so no outbox can acquire authority to sign a foreign repo.
use super::*;
use std::collections::BTreeSet;

const ITEM: &str = "app.bsky.graph.listitem";
const STARTER: &str = "app.bsky.graph.starterpack";
const QUORUM: usize = 9;

async fn exists(app: &App, did: &str) -> XResult<bool> {
    match super::super::internal::account_anywhere(app, did).await {
        Ok(_) => Ok(true),
        Err(e) if e.error == "AccountNotFound" => Ok(false),
        Err(e) => Err(e),
    }
}

async fn local_board(app: &App, community: &str) -> XResult<(String, String, Cid, J, String)> {
    let (owner, key) = at_uri(community, BOARD)?;
    if !exists(app, owner).await? {
        return Err(XrpcError::bad(
            "FounderPdsMismatch",
            "The creator and founders must use the same PDS for this workflow",
        ));
    }
    let (cid, record) = read(app, owner, BOARD, key)
        .await?
        .ok_or_else(|| XrpcError::bad("CommunityNotFound", "Community not found"))?;
    let starter = text(&record, "founderStarterPackUri")?;
    let (starter_owner, starter_key) = at_uri(starter, STARTER)?;
    if starter_owner != owner {
        return Err(XrpcError::bad("InvalidRequest", "Founder starter pack must belong to the creator"));
    }
    let (_, pack) = read(app, owner, STARTER, starter_key)
        .await?
        .ok_or_else(|| XrpcError::bad("InvalidRequest", "Founder starter pack is missing"))?;
    let list = text(&pack, "list")?.to_owned();
    let (list_owner, _) = at_uri(&list, "app.bsky.graph.list")?;
    if list_owner != owner {
        return Err(XrpcError::bad("InvalidRequest", "Founder list must belong to the creator"));
    }
    Ok((owner.into(), key.into(), cid, record, list))
}

async fn invited(app: &App, owner: &str, list_uri: &str) -> XResult<BTreeSet<String>> {
    Ok(list(app, owner, ITEM)
        .await?
        .into_iter()
        .filter_map(|(_, _, r)| (r["list"] == list_uri).then(|| r["subject"].as_str().map(str::to_owned)).flatten())
        .collect())
}

pub(super) async fn authorized(app: &App, community: &str, did: &str) -> XResult<bool> {
    let (owner, _, _, board, list_uri) = local_board(app, community).await?;
    Ok(board["status"] == "draft" && invited(app, &owner, &list_uri).await?.contains(did))
}

async fn accepted_count(app: &App, owner: &str, key: &str, community: &str, list_uri: &str) -> XResult<usize> {
    let mut count = 1; // the creator's atomic founding membership
    for subject in invited(app, owner, list_uri).await? {
        if subject == owner || !exists(app, &subject).await? {
            continue;
        }
        let membership = read(app, &subject, MEMBER, key).await?.map(|(_, r)| r);
        // Leaving later does not erase an accepted founding commitment.
        if membership.is_some_and(|r| {
            r["community"] == community
                && r["source"] == "acceptDraftInvite"
                && r["roles"].as_array().is_some_and(|a| a.iter().any(|v| v == "founder"))
        }) {
            count += 1;
        }
    }
    Ok(count)
}

pub(super) async fn invite(State(app): AppState, Auth(creds): Auth, Json(input): Json<J>) -> XResult<Json<J>> {
    let did = caller(&app, &creds).await?;
    creds.need_repo(ITEM, "create")?;
    let head = app.head(&did).await?.commit;
    let community = text(&input, "communityUri")?;
    let founder = text(&input, "founderDid")?;
    let (owner, _, _, record, list_uri) = local_board(&app, community).await?;
    if owner != did.as_ref() {
        return Err(XrpcError::auth("Only the community creator may invite a founder"));
    }
    if !vlatproto::syntax::valid_did(founder) || founder == did.as_ref() {
        return Err(XrpcError::bad("InvalidRequest", "A different founder DID is required"));
    }
    if record["status"] != "draft" {
        return Err(XrpcError::bad("CommunityNotDraft", "Community is no longer a draft"));
    }
    if !exists(&app, founder).await? {
        return Err(XrpcError::bad("FounderPdsMismatch", "Founder must have an account on this PDS"));
    }
    if invited(&app, &owner, &list_uri).await?.contains(founder) {
        return Ok(Json(json!({"communityUri":community,"founderDid":founder,"invited":true})));
    }
    let key = app.tids.next().to_string();
    commit(
        &app,
        &creds,
        did,
        head,
        vec![(ITEM, key, json!({"$type":ITEM,"list":list_uri,"subject":founder,"createdAt":now()}), Some(None))],
    )
    .await?;
    Ok(Json(json!({"communityUri":community,"founderDid":founder,"invited":true})))
}

pub(super) async fn accept(State(app): AppState, Auth(creds): Auth, Json(input): Json<J>) -> XResult<Json<J>> {
    let did = caller(&app, &creds).await?;
    creds.need_repo(MEMBER, "create")?;
    creds.need_repo(MEMBER, "update")?;
    let head = app.head(&did).await?.commit;
    let community = text(&input, "communityUri")?;
    let (owner, key, _, record, list_uri) = local_board(&app, community).await?;
    if owner == did.as_ref() {
        return Err(XrpcError::bad("InvalidRequest", "The creator is already a founder"));
    }
    let current = read(&app, &did, MEMBER, &key).await?;
    if current.as_ref().is_some_and(|(_, r)| r["community"] != community) {
        return Err(XrpcError::bad("CommunityMembershipConflict", "Membership key belongs to another community"));
    }
    let already =
        current.as_ref().is_some_and(|(_, r)| r["source"] == "acceptDraftInvite" && r["roles"] == json!(["founder"]));
    if !already {
        if record["status"] != "draft" {
            return Err(XrpcError::bad("CommunityNotDraft", "Community is no longer a draft"));
        }
        if !invited(&app, &owner, &list_uri).await?.contains(did.as_ref()) {
            return Err(XrpcError::bad(
                "FounderInviteRequired",
                "The creator must authorize this founder before acceptance",
            ));
        }
        let joined = current.as_ref().and_then(|(_, r)| r["joinedAt"].as_str()).map(str::to_owned).unwrap_or_else(now);
        let membership = json!({"$type":MEMBER,"community":community,"membershipState":"active","roles":["founder"],"source":"acceptDraftInvite","joinedAt":joined});
        commit(&app, &creds, did, head, vec![(MEMBER, key.clone(), membership, Some(current.map(|(cid, _)| cid)))])
            .await?;
    }
    let count = accepted_count(&app, &owner, &key, community, &list_uri).await?;
    // The creator must sign finalizeFounders. No automatic cross-repo write.
    Ok(Json(
        json!({"status":record["status"],"memberCount":count,"requiresCreatorFinalization":record["status"]=="draft" && count>=QUORUM}),
    ))
}

pub(super) async fn finalize(State(app): AppState, Auth(creds): Auth, Json(input): Json<J>) -> XResult<Json<J>> {
    let did = caller(&app, &creds).await?;
    creds.need_repo(BOARD, "update")?;
    let head = app.head(&did).await?.commit;
    let community = text(&input, "communityUri")?;
    let (owner, key, cid, mut record, list_uri) = local_board(&app, community).await?;
    if owner != did.as_ref() {
        return Err(XrpcError::auth("Only the creator may activate the community"));
    }
    let count = accepted_count(&app, &owner, &key, community, &list_uri).await?;
    if record["status"] != "active" {
        if count < QUORUM {
            return Err(XrpcError::bad(
                "FounderQuorumRequired",
                "Nine accepted founders, including the creator, are required",
            ));
        }
        record["status"] = json!("active");
        commit(&app, &creds, did, head, vec![(BOARD, key, record, Some(Some(cid)))]).await?;
    }
    Ok(Json(json!({"status":"active","memberCount":count})))
}
