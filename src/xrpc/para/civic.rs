use super::*;

pub(super) async fn cast_vote(State(app): AppState, Auth(creds): Auth, Json(input): Json<J>) -> XResult<Json<J>> {
    let did = caller(&app, &creds).await?;
    let coll = "com.para.civic.vote";
    creds.need_repo(coll, "create")?;
    let head = app.head(&did).await?.commit;
    let subject = text(&input, "cabildeo")?;
    let result = appview(&app, &did, "com.para.civic.getCabildeo", &[("cabildeo", subject)]).await?;
    let c = &result["cabildeo"];
    if c["uri"] != subject {
        return Err(XrpcError::bad("NotFound", "Cabildeo not found"));
    }
    if c["phase"] != "voting" {
        return Err(XrpcError::bad("InvalidPhase", "Cabildeo is not in voting phase"));
    }
    let now = chrono::Utc::now();
    if let Some(deadline) = c["phaseDeadline"].as_str() {
        let deadline = chrono::DateTime::parse_from_rfc3339(deadline)
            .map_err(|_| XrpcError::bad("InvalidPhase", "Invalid voting deadline"))?;
        if deadline <= now {
            return Err(XrpcError::bad("DeadlineExpired", "Voting deadline passed"));
        }
    }
    let option =
        input["selectedOption"].as_u64().ok_or_else(|| XrpcError::bad("InvalidOption", "Invalid vote option"))?;
    if !c["options"].as_array().is_some_and(|a| option < (a.len() as u64)) {
        return Err(XrpcError::bad("InvalidOption", "Invalid vote option"));
    }
    if c["viewerContext"]["currentVoteIsDirect"] == true {
        if let Some(voted) = c["viewerContext"]["currentVoteCreatedAt"].as_str() {
            let voted = chrono::DateTime::parse_from_rfc3339(voted)
                .map_err(|_| XrpcError::bad("InvalidVoteProof", "Invalid current vote timestamp"))?;
            if now > voted + chrono::Duration::seconds(310) {
                return Err(XrpcError::bad("VoteEditWindowExpired", "Vote edit window expired"));
            }
        }
    }
    // Also consult local votes: AppView ingestion is asynchronous.
    for (_, _, v) in list(&app, &did, coll).await? {
        if v["subject"] == subject && v["isDirect"] == true {
            let voted = chrono::DateTime::parse_from_rfc3339(text(&v, "createdAt")?).map_err(XrpcError::from_err)?;
            if now > voted + chrono::Duration::seconds(310) {
                return Err(XrpcError::bad("VoteEditWindowExpired", "Vote edit window expired"));
            }
        }
    }
    let community = text(c, "community")?;
    let membership = if community.starts_with("at://") {
        appview(&app, &did, "com.para.community.getBoard", &[("uri", community)]).await?
    } else {
        let slug = community.trim_start_matches("p/").to_lowercase();
        appview(&app, &did, "com.para.community.getBoard", &[("communityId", &slug)]).await?
    };
    if membership["board"]["viewerMembershipState"] != "active" {
        return Err(XrpcError::bad("CommunityMembershipRequired", "Active community membership required"));
    }
    let record = json!({"$type":coll,"subject":subject,"subjectType":"cabildeo","cabildeo":subject,"selectedOption":option,"isDirect":true,"voteNullifier":text(&input,"voteNullifier")?.trim(),"eligibilityProofRef":text(&input,"eligibilityProofRef")?.trim(),"createdAt":super::now()});
    let key = app.tids.next().to_string();
    let (ack, cids) = commit(&app, &creds, did.clone(), head, vec![(coll, key.clone(), record, Some(None))]).await?;
    Ok(Json(
        json!({"uri":uri(&did,coll,&key),"cid":cids[0].to_string(),"commit":super::super::repo::commit_json(&ack)}),
    ))
}
