use super::*;
use sha2::{Digest, Sha256};

fn slug(value: &str) -> String {
    let mut out = String::new();
    for c in value.trim().to_lowercase().chars() {
        if (0x300..=0x36f).contains(&(c as u32)) {
            continue;
        }
        let c = match c {
            'á' | 'à' | 'ä' | 'â' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            c => c,
        };
        if c.is_ascii_alphanumeric() {
            out.push(c)
        } else if !out.ends_with('-') {
            out.push('-')
        }
    }
    out.trim_matches('-').into()
}

fn has_role(record: &J, wanted: &[&str]) -> bool {
    if let Some(assignments) = record["roleAssignments"].as_array().filter(|a| !a.is_empty()) {
        let now = chrono::Utc::now();
        return assignments.iter().any(|a| {
            let valid = |field: &str, before: bool| match a.get(field) {
                None => true,
                Some(v) => v.as_str().and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()).is_some_and(|t| {
                    if before {
                        t <= now
                    } else {
                        t > now
                    }
                }),
            };
            a["role"].as_str().is_some_and(|r| wanted.contains(&r))
                && valid("validFrom", true)
                && valid("validUntil", false)
        });
    }
    record["roles"]
        .as_array()
        .is_some_and(|roles| roles.iter().any(|r| r.as_str().is_some_and(|r| wanted.contains(&r))))
}

fn capabilities(record: &J) -> J {
    let mut out = vec!["create_community"];
    if record["membershipState"] == "active" {
        out.push("leave_community");
        if has_role(record, &["owner", "moderator"]) {
            out.extend(["manage_governance", "review_members", "remove_members", "edit_community"]);
        }
    } else if record["membershipState"] == "left" || record["membershipState"].is_null() {
        out.push("join_community")
    }
    json!(out)
}

pub(super) async fn join(State(app): AppState, Auth(creds): Auth, Json(input): Json<J>) -> XResult<Json<J>> {
    membership(app, creds, input, false).await
}
pub(super) async fn leave(State(app): AppState, Auth(creds): Auth, Json(input): Json<J>) -> XResult<Json<J>> {
    membership(app, creds, input, true).await
}

async fn membership(app: Arc<App>, creds: Credentials, input: J, leaving: bool) -> XResult<Json<J>> {
    let did = caller(&app, &creds).await?;
    creds.need_repo(MEMBER, "update")?;
    if !leaving {
        creds.need_repo(MEMBER, "create")?;
    }
    let head = app.head(&did).await?.commit;
    let community = text(&input, "communityUri")?;
    let (_, key) = at_uri(community, BOARD)?;
    let current = read(&app, &did, MEMBER, key).await?;
    let previous = current.as_ref().map(|(_, r)| r).cloned().unwrap_or(json!({}));
    // Same rkey from another repo cannot alias an existing membership.
    if previous.get("community").is_some_and(|v| v != community) {
        return Err(XrpcError::bad("CommunityMembershipConflict", "Membership key belongs to another community"));
    }
    let state = previous["membershipState"].as_str().unwrap_or("none");
    if state == "blocked" || (!leaving && state == "removed") {
        return Err(XrpcError::bad("CommunityMembershipLocked", "This membership cannot be changed by the viewer"));
    }
    if leaving && current.is_none() {
        return Err(XrpcError::bad("CommunityMembershipNotFound", "No membership exists for this community"));
    }
    if leaving && previous["roles"].as_array().is_some_and(|r| r.iter().any(|r| r == "owner")) {
        return Err(XrpcError::bad("CommunityOwnerCannotLeave", "Community owners cannot leave their own community"));
    }
    let target_state = if leaving {
        "left"
    } else {
        let b = board(&app, &did, community).await?;
        if b["admissionMode"] == "assembly_approval" && state != "active" {
            "pending"
        } else {
            "active"
        }
    };
    let now = now();
    let mut record = json!({"$type":MEMBER,"community":community,"membershipState":target_state,"roles":previous.get("roles").cloned().unwrap_or(json!([])),"source":previous.get("source").cloned().unwrap_or(json!("join")),"joinedAt":previous.get("joinedAt").cloned().unwrap_or(json!(now))});
    for field in ["roleAssignments", "chamberAssignment"] {
        if let Some(v) = previous.get(field) {
            record[field] = v.clone();
        }
    }
    if leaving {
        record["leftAt"] = json!(now);
    } else if let Some(source) = input["source"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
        record["source"] = json!(source);
    }
    let swap = Some(current.map(|(cid, _)| cid));
    let (_, cids) = commit(&app, &creds, did.clone(), head, vec![(MEMBER, key.into(), record.clone(), swap)]).await?;
    Ok(Json(
        json!({"uri":uri(&did,MEMBER,key),"cid":cids[0].to_string(),"communityUri":community,"membershipState":target_state,"viewerCapabilities":capabilities(&record)}),
    ))
}

fn name_path(app: &App, name: &str) -> object_store::path::Path {
    object_store::path::Path::from(format!(
        "{}/para/community-names/{}",
        app.store.prefix,
        hex::encode(Sha256::digest(slug(name).as_bytes()))
    ))
}

async fn reserved_name(app: &App, did: &str, name: &str, quadrant: &str) -> XResult<Option<String>> {
    let r = match app.store.raw.get(&name_path(app, name)).await {
        Ok(r) => r,
        Err(object_store::Error::NotFound { .. }) => return Ok(None),
        Err(e) => return Err(XrpcError::from_err(e)),
    };
    if r.meta.size > 4096 {
        return Err(XrpcError::internal("Invalid community name reservation"));
    }
    let old: J = serde_json::from_slice(&r.bytes().await.map_err(XrpcError::from_err)?).map_err(XrpcError::from_err)?;
    if old["did"] != did || old["quadrant"] != slug(quadrant) {
        return Err(XrpcError::bad("CommunityNameTaken", "Community name is already reserved"));
    }
    let key = text(&old, "rkey")?;
    if !crate::lexicon::valid_tid(key) {
        return Err(XrpcError::internal("Invalid reserved board key"));
    }
    Ok(Some(key.into()))
}

pub(super) async fn reserve_name(app: &App, did: &str, name: &str, quadrant: &str, key: &str) -> XResult<()> {
    let value = json!({"did":did,"rkey":key,"quadrant":slug(quadrant)});
    match app
        .store
        .raw
        .put_opts(
            &name_path(app, name),
            Bytes::from(serde_json::to_vec(&value).map_err(XrpcError::from_err)?).into(),
            PutOptions { mode: PutMode::Create, ..Default::default() },
        )
        .await
    {
        Ok(_) => Ok(()),
        Err(object_store::Error::AlreadyExists { .. }) | Err(object_store::Error::Precondition { .. }) => {
            if reserved_name(app, did, name, quadrant).await?.as_deref() != Some(key) {
                return Err(XrpcError::bad("InvalidSwap", "Concurrent community creation; retry"));
            }
            Ok(())
        }
        Err(e) => Err(XrpcError::from_err(e)),
    }
}

pub(super) async fn create_board(State(app): AppState, Auth(creds): Auth, Json(input): Json<J>) -> XResult<Json<J>> {
    let did = caller(&app, &creds).await?;
    for coll in [
        BOARD,
        MEMBER,
        GOVERNANCE,
        AUTHORITY,
        "app.bsky.graph.list",
        "app.bsky.graph.listitem",
        "app.bsky.graph.starterpack",
    ] {
        creds.need_repo(coll, "create")?;
    }
    let head = app.head(&did).await?.commit;
    let name = text(&input, "name")?.trim();
    let quadrant = text(&input, "quadrant")?.trim();
    let region = match input.get("region") {
        None => None,
        Some(J::String(region)) => Some(region.trim()).filter(|region| !region.is_empty()),
        Some(_) => return Err(XrpcError::bad("InvalidRequest", "region must be a string")),
    };
    // Horizontal governance is retired in WatZappa: every community is hierarchical. `governanceMode`
    // is no longer part of the contract; a client still sending "hierarchical" is harmless, "horizontal"
    // is refused rather than silently turned into a hierarchical community.
    if input["governanceMode"].as_str().is_some_and(|mode| mode != "hierarchical") {
        return Err(XrpcError::bad("InvalidRequest", "Horizontal governance is retired; every community is hierarchical"));
    }
    let admission = input["admissionMode"].as_str().unwrap_or("open");
    if !["open", "assembly_approval"].contains(&admission) {
        return Err(XrpcError::bad("InvalidRequest", "Invalid admission mode"));
    }
    for (key, cid, record) in list(&app, &did, BOARD).await? {
        if slug(record["name"].as_str().unwrap_or("")) == slug(name)
            && slug(record["quadrant"].as_str().unwrap_or("")) == slug(quadrant)
        {
            if app.head(&did).await?.commit != head {
                return Err(XrpcError::bad("InvalidSwap", "Repository changed; retry"));
            }
            return Ok(Json(
                json!({"uri":uri(&did,BOARD,&key),"cid":cid.to_string(),"delegatesChatId":record["delegatesChatId"],"subdelegatesChatId":record["subdelegatesChatId"],"founderStarterPackUri":record["founderStarterPackUri"]}),
            ));
        }
    }
    // Foreign communities live at other PDSes: check the index before reserving locally.
    let mut cursor: Option<String> = None;
    for page in 0..20 {
        let mut params = vec![("query", name), ("limit", "100")];
        if let Some(c) = &cursor {
            params.push(("cursor", c));
        }
        let result = appview(&app, &did, "com.para.community.listBoards", &params).await?;
        let boards = result["boards"]
            .as_array()
            .ok_or_else(|| XrpcError::unavailable("ParaAppViewUnavailable", "Invalid community name lookup"))?;
        if boards.iter().any(|b| slug(b["name"].as_str().unwrap_or("")) == slug(name)) {
            return Err(XrpcError::bad("CommunityNameTaken", "Community name already exists"));
        }
        cursor = result["cursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
        if page == 19 {
            return Err(XrpcError::unavailable("ParaAppViewUnavailable", "Community name lookup limit reached"));
        }
    }
    if slug(name).is_empty() {
        return Err(XrpcError::bad("InvalidRequest", "Community name must contain letters or digits"));
    }
    let key = reserved_name(&app, &did, name, quadrant).await?.unwrap_or_else(|| app.tids.next().to_string());
    let now = now();
    let slug = format!("{}-{key}", slug(name));
    if slug.len() > 128 {
        return Err(XrpcError::bad(
            "InvalidRequest",
            "Community name produces a governance slug longer than 128 bytes",
        ));
    }
    let list_key = app.tids.next().to_string();
    let pack_key = app.tids.next().to_string();
    let item_key = app.tids.next().to_string();
    let office_key = app.tids.next().to_string();
    let board_uri = uri(&did, BOARD, &key);
    let list_uri = uri(&did, "app.bsky.graph.list", &list_key);
    let pack_uri = uri(&did, "app.bsky.graph.starterpack", &pack_key);
    let delegates = format!("para://community/{key}/delegates");
    let subdelegates = format!("para://community/{key}/subdelegates");
    let starter_name =
        input["founderStarterPackName"].as_str().map(str::to_owned).unwrap_or(format!("Founders: {name}"));
    let mut b = json!({"$type":BOARD,"name":name,"quadrant":quadrant,"delegatesChatId":delegates,"subdelegatesChatId":subdelegates,"status":"draft","founderStarterPackUri":pack_uri,"admissionMode":admission,"createdAt":now});
    if let Some(region) = region {
        b["region"] = json!(region);
    }
    if let Some(d) = input.get("description") {
        b["description"] = d.clone();
    }
    let moderator = json!({"did":did,"role":"Founding moderator","badge":"Community Creator","capabilities":["appoint_deputies","edit_role_descriptions","review_applicants","publish_governance_updates","set_official_representatives"],"validFrom":now});
    let mut governance = json!({"$type":GOVERNANCE,"community":format!("p/{name}"),"communityId":slug,"slug":slug,"createdAt":now,"updatedAt":now,"moderators":[],"officials":[],"deputies":[],"metadata":{"state":"seeded","reviewCadence":"Initial setup","lastPublishedAt":now},"editHistory":[{"id":"seed-governance","action":"publish_governance_updates","actorDid":did,"createdAt":now,"summary":"Initial governance charter seeded during community creation."}]});
    governance["moderators"] = json!([moderator]);
    let membership = json!({"$type":MEMBER,"community":board_uri,"membershipState":"active","roles":["owner","moderator"],"source":"createBoard","joinedAt":now});
    let authority = json!({"$type":AUTHORITY,"community":board_uri,"subject":did,"action":"member.activate","issuer":did,"effectiveAt":now,"version":1,"basis":"foundingTransition","createdAt":now});
    let office = json!({"$type":AUTHORITY,"community":board_uri,"subject":did,"action":"owner.grant","issuer":did,"effectiveAt":now,"predecessor":uri(&did,AUTHORITY,&key),"version":2,"basis":"foundingTransition","createdAt":now});
    let records = vec![
        (BOARD, key.clone(), b, Some(None)),
        (MEMBER, key.clone(), membership, Some(None)),
        (GOVERNANCE, slug, governance, Some(None)),
        (AUTHORITY, key.clone(), authority, Some(None)),
        (AUTHORITY, office_key, office, Some(None)),
        (
            "app.bsky.graph.list",
            list_key,
            json!({"$type":"app.bsky.graph.list","purpose":"app.bsky.graph.defs#referencelist","name":starter_name,"createdAt":now}),
            Some(None),
        ),
        (
            "app.bsky.graph.starterpack",
            pack_key,
            json!({"$type":"app.bsky.graph.starterpack","list":list_uri,"name":starter_name,"createdAt":now}),
            Some(None),
        ),
        (
            "app.bsky.graph.listitem",
            item_key,
            json!({"$type":"app.bsky.graph.listitem","list":list_uri,"subject":did,"createdAt":now}),
            Some(None),
        ),
    ];
    // Validate the entire command before acquiring a durable name reservation.
    for (coll, rkey, record, _) in &records {
        let raw = serde_json::to_vec(record).map_err(XrpcError::from_err)?;
        let mut tree = JsonValue::parse(&raw).map_err(XrpcError::from_err)?;
        super::super::repo::encode_record(&mut tree, coll, rkey, Some(true), None)?;
    }
    crate::ratelimit::check_repo_write(Some(&did), crate::ratelimit::CREATE_POINTS)?;
    reserve_name(&app, &did, name, quadrant, &key).await?;
    let (_, cids) = commit(&app, &creds, did.clone(), head, records).await?;
    Ok(Json(
        json!({"uri":board_uri,"cid":cids[0].to_string(),"delegatesChatId":delegates,"subdelegatesChatId":subdelegates,"founderStarterPackUri":pack_uri}),
    ))
}

fn pack_view(did: &str, key: &str, cid: Cid, mut record: J) -> J {
    record["$type"] = json!("com.para.community.defs#briefingPackView");
    record["uri"] = json!(uri(did, PACK, key));
    record["cid"] = json!(cid.to_string());
    record
}

pub(super) async fn create_pack(State(app): AppState, Auth(creds): Auth, Json(input): Json<J>) -> XResult<Json<J>> {
    let did = caller(&app, &creds).await?;
    creds.need_repo(PACK, "create")?;
    let head = app.head(&did).await?.commit;
    at_uri(text(&input, "communityUri")?, BOARD)?;
    let key = app.tids.next().to_string();
    let now = now();
    let mut record = input;
    record["$type"] = json!(PACK);
    record["createdBy"] = json!(did);
    record["status"] = json!("draft");
    record["createdAt"] = json!(now);
    record["updatedAt"] = json!(now);
    for k in ["cabildeoUris", "civicTreeCardIds", "evidenceUris"] {
        if record.get(k).is_none() {
            record[k] = json!([]);
        }
    }
    let (_, cids) =
        commit(&app, &creds, did.clone(), head, vec![(PACK, key.clone(), record.clone(), Some(None))]).await?;
    Ok(Json(json!({"pack":pack_view(&did,&key,cids[0],record)})))
}

pub(super) async fn update_pack(State(app): AppState, Auth(creds): Auth, Json(input): Json<J>) -> XResult<Json<J>> {
    let did = caller(&app, &creds).await?;
    creds.need_repo(PACK, "update")?;
    let head = app.head(&did).await?.commit;
    let (owner, key) = at_uri(text(&input, "uri")?, PACK)?;
    if owner != &*did {
        return Err(XrpcError::auth("Cannot update another user's briefing pack"));
    }
    let (old, mut record) =
        read(&app, &did, PACK, key).await?.ok_or_else(|| XrpcError::bad("NotFound", "Briefing pack not found"))?;
    let swap = match input["cid"].as_str() {
        Some(c) => Cid::parse(c).map_err(|_| XrpcError::bad("InvalidRequest", "Invalid CID"))?,
        None => old,
    };
    for k in [
        "party",
        "title",
        "summary",
        "cabildeoUris",
        "civicTreeCardIds",
        "evidenceUris",
        "sembleCollectionUri",
        "marginCollectionUri",
        "obsidianExportUri",
        "status",
    ] {
        if let Some(v) = input["pack"].get(k) {
            record[k] = v.clone();
        }
    }
    record["updatedAt"] = json!(now());
    if !["draft", "published", "archived"].contains(&record["status"].as_str().unwrap_or("")) {
        return Err(XrpcError::bad("InvalidRequest", "Invalid briefing pack status"));
    }
    let (_, cids) =
        commit(&app, &creds, did.clone(), head, vec![(PACK, key.into(), record.clone(), Some(Some(swap)))]).await?;
    Ok(Json(json!({"pack":pack_view(&did,key,cids[0],record)})))
}

async fn active_member(app: &App, did: &str, community: &str, steward: bool) -> XResult<()> {
    let (_, key) = at_uri(community, BOARD)?;
    let member = read(app, did, MEMBER, key).await?.map(|(_, r)| r).unwrap_or(json!({}));

    if member["community"] != community
        || member["membershipState"] != "active"
        || (steward && !has_role(&member, &["owner", "moderator", "steward", "official"]))
    {
        return Err(XrpcError::bad(
            if steward { "CommunityStewardRequired" } else { "CommunityMembershipRequired" },
            "Active community membership with the required role is required",
        ));
    }
    Ok(())
}

pub(super) async fn share(State(app): AppState, Auth(creds): Auth, Json(input): Json<J>) -> XResult<Json<J>> {
    let did = caller(&app, &creds).await?;
    creds.need_repo(SHARE, "create")?;
    let head = app.head(&did).await?.commit;
    let community = text(&input, "communityUri")?;
    active_member(&app, &did, community, false).await?;
    for (_, _, r) in list(&app, &did, SHARE).await? {
        if r["communityUri"] == community && r["subject"]["uri"] == input["subject"]["uri"] {
            return Err(XrpcError::bad("DuplicateSharedContent", "Content already shared into this community"));
        }
    }
    let key = app.tids.next().to_string();
    let mut record = input;
    record["$type"] = json!(SHARE);
    record["sharedBy"] = json!(did);
    record["createdAt"] = json!(now());
    if record.get("visibility").is_none() {
        record["visibility"] = json!("community");
    }
    let (_, cids) =
        commit(&app, &creds, did.clone(), head, vec![(SHARE, key.clone(), record.clone(), Some(None))]).await?;
    record["$type"] = json!("com.para.community.defs#sharedContentView");
    record["removed"] = json!(false);
    record["uri"] = json!(uri(&did, SHARE, &key));
    record["cid"] = json!(cids[0].to_string());
    Ok(Json(json!({"uri":record["uri"],"cid":record["cid"],"sharedContent":record})))
}
pub(super) async fn remove_share(State(app): AppState, Auth(creds): Auth, Json(input): Json<J>) -> XResult<Json<J>> {
    share_action(app, creds, input, "remove").await
}
pub(super) async fn restore_share(State(app): AppState, Auth(creds): Auth, Json(input): Json<J>) -> XResult<Json<J>> {
    share_action(app, creds, input, "restore").await
}
async fn share_action(app: Arc<App>, creds: Credentials, input: J, action: &str) -> XResult<Json<J>> {
    let did = caller(&app, &creds).await?;
    creds.need_repo(SHARE_ACTION, "create")?;
    let head = app.head(&did).await?.commit;
    active_member(&app, &did, text(&input, "communityUri")?, true).await?;
    at_uri(text(&input["sharedContent"], "uri")?, SHARE)?;
    let key = app.tids.next().to_string();
    let mut record = input;
    record["$type"] = json!(SHARE_ACTION);
    record["action"] = json!(action);
    record["createdAt"] = json!(now());
    let (_, cids) =
        commit(&app, &creds, did.clone(), head, vec![(SHARE_ACTION, key.clone(), record.clone(), Some(None))]).await?;
    record["uri"] = json!(uri(&did, SHARE_ACTION, &key));
    record["cid"] = json!(cids[0].to_string());
    record["createdBy"] = json!(did);
    record.as_object_mut().unwrap().remove("$type");
    Ok(Json(json!({"uri":record["uri"],"cid":record["cid"],"action":record})))
}

pub(super) async fn check_membership(app: &App, did: &str, key: &str, record: &J) -> XResult<()> {
    let community = text(record, "community")?;
    let (owner, board_key) = at_uri(community, BOARD)?;
    if key != board_key {
        return Err(XrpcError::bad("InvalidRequest", "Membership key must match its board"));
    }
    let old = read(app, did, MEMBER, key).await?.map(|(_, r)| r);
    if old.as_ref().is_some_and(|r| r["community"] != community) {
        return Err(XrpcError::bad("CommunityMembershipConflict", "Membership key belongs to another community"));
    }
    if owner != did {
        let roles = record.get("roles").cloned().unwrap_or(json!([]));
        let old_roles = old.as_ref().and_then(|r| r.get("roles")).cloned().unwrap_or(json!([]));
        let founder_grant = roles == json!(["founder"])
            && record["source"] == "acceptDraftInvite"
            && record.get("roleAssignments").is_none()
            && record.get("chamberAssignment").is_none()
            && super::founders::authorized(app, community, did).await?;
        if (roles != old_roles && !founder_grant)
            || record.get("roleAssignments") != old.as_ref().and_then(|r| r.get("roleAssignments"))
            || record.get("chamberAssignment") != old.as_ref().and_then(|r| r.get("chamberAssignment"))
        {
            return Err(XrpcError::bad("CommunityStewardRequired", "Members cannot grant themselves community roles"));
        }
        if old.as_ref().is_some_and(|r| r["membershipState"] == "blocked" || r["membershipState"] == "removed")
            && old.as_ref().unwrap()["membershipState"] != record["membershipState"]
        {
            return Err(XrpcError::bad("CommunityMembershipLocked", "Locked membership cannot be reactivated"));
        }
        if record["membershipState"] == "active" && !founder_grant {
            let b = board(app, did, community).await?;
            if b["admissionMode"] == "assembly_approval"
                && !founder_grant
                && !old.as_ref().is_some_and(|r| r["membershipState"] == "active")
            {
                return Err(XrpcError::bad("CommunityMembershipRequired", "Assembly admission requires approval"));
            }
        }
    }
    Ok(())
}

pub(super) async fn check_content(app: &App, did: &str, coll: &str, record: &J) -> XResult<()> {
    let community = text(record, "communityUri")?;
    active_member(app, did, community, coll == SHARE_ACTION).await?;
    if coll == SHARE {
        if !["post", "cabildeo", "collection", "mapInitiative", "external"]
            .contains(&record["contentType"].as_str().unwrap_or(""))
        {
            return Err(XrpcError::bad("InvalidRequest", "Unsupported shared content type"));
        }
        if record.get("visibility").is_some()
            && !["community", "public", "stewards"].contains(&record["visibility"].as_str().unwrap_or(""))
        {
            return Err(XrpcError::bad("InvalidRequest", "Unsupported shared content visibility"));
        }
        if record["pinned"] == true {
            active_member(app, did, community, true).await?;
        }
    }
    if coll == SHARE && record["sharedBy"] != did {
        return Err(XrpcError::bad("InvalidRequest", "sharedBy must match the repository owner"));
    }
    if coll == SHARE_ACTION {
        at_uri(text(&record["sharedContent"], "uri")?, SHARE)?;
        if !["remove", "restore", "pin", "unpin"].contains(&record["action"].as_str().unwrap_or("")) {
            return Err(XrpcError::bad("InvalidRequest", "Invalid shared content action"));
        }
    }
    Ok(())
}

pub(super) async fn check_board(app: &App, did: &str, key: &str, record: &J, command: bool) -> XResult<()> {
    // Horizontal governance is retired. A legacy board that still carries it may be edited as it is, but
    // no write may introduce it.
    let mode = record["governanceMode"].as_str().unwrap_or("hierarchical");
    let admission = record["admissionMode"].as_str().unwrap_or("open");
    if !["open", "assembly_approval"].contains(&admission)
        || !["draft", "active"].contains(&record["status"].as_str().unwrap_or(""))
    {
        return Err(XrpcError::bad("InvalidRequest", "Unsupported community policy"));
    }
    let old = read(app, did, BOARD, key).await?;
    if let Some((_, old)) = old {
        if old["status"] != record["status"] && !command {
            return Err(XrpcError::bad(
                "ParaCommandRequired",
                "Activate a draft through the creator's finalizeFounders command",
            ));
        }
        if slug(text(&old, "name")?) != slug(text(record, "name")?)
            || slug(text(&old, "quadrant")?) != slug(text(record, "quadrant")?)
            || old["governanceMode"].as_str().unwrap_or("hierarchical") != mode
        {
            return Err(XrpcError::bad(
                "ParaCommandRequired",
                "Community identity and governance transitions require a dedicated command",
            ));
        }
    } else if mode != "hierarchical" {
        return Err(XrpcError::bad("InvalidRequest", "Horizontal governance is retired; every community is hierarchical"));
    } else if !command {
        return Err(XrpcError::bad(
            "ParaCommandRequired",
            "Create communities through community.createBoard so their records and name reservation are atomic",
        ));
    }
    Ok(())
}
