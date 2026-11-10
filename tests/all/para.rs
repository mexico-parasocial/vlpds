use crate::common::*;
use axum::{routing::get, Router};
use std::sync::{Arc, Mutex};

async fn fixture() -> (TestServer, Arc<Mutex<J>>, tokio::task::JoinHandle<()>) {
    let state = Arc::new(Mutex::new(json!({"board":{},"cabildeo":{},"verifier":204})));
    let st = state.clone();
    let router = Router::new()
        .fallback(get(move |uri: axum::http::Uri| {
            let st = st.clone();
            async move {
                let s = st.lock().unwrap().clone();
                let body = match uri.path() {
                    "/xrpc/com.para.community.listBoards" => json!({"boards":[]}),
                    "/xrpc/com.para.community.getBoard" => json!({"board":s["board"]}),
                    "/xrpc/com.para.civic.getCabildeo" => json!({"cabildeo":s["cabildeo"]}),
                    _ => json!({}),
                };
                axum::Json(body)
            }
        }))
        .route(
            "/verify",
            axum::routing::post({
                let st = state.clone();
                move || {
                    let st = st.clone();
                    async move {
                        axum::http::StatusCode::from_u16(st.lock().unwrap()["verifier"].as_u64().unwrap() as u16)
                            .unwrap()
                    }
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let server = TestServer::spawn_with(|c| {
        c.para = vlpds::para::Config::new(true, Some(format!("{url}/verify")), Some(format!("{url}/verify"))).unwrap();
        c.appview = Some((url, "did:web:appview.test".into()));
    })
    .await;
    (server, state, task)
}

async fn count(s: &TestServer, did: &str, coll: &str) -> usize {
    s.list_records(did, coll, &[]).await.ok()["records"].as_array().unwrap().len()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn alpha_private_snapshot_preserves_access_and_cannot_overwrite_live_state() {
    let (s, _, task) = fixture().await;
    let a = s.create_account("alphamigrated").await;
    let waiting = s.create_account("alphawaiting").await;
    let code = "FRIEND-ABCD-EFGH";
    let mut snapshot = json!({"version":1,"rollouts":[{"state":"NL","totalSlots":0,"usedSlots":0,"isOpen":true},{"state":"FRIEND","totalSlots":1,"usedSlots":1,"isOpen":false}],"requests":[{"did":a.did,"state":"NL","status":"approved","createdAt":now_iso(),"approvedAt":now_iso(),"inviteCode":code},{"did":waiting.did,"state":"NL","status":"pending","createdAt":now_iso(),"approvedAt":null,"inviteCode":null}],"invites":[{"code":code,"state":"FRIEND","did":a.did,"createdBy":"admin","createdAt":now_iso(),"usedAt":now_iso()}]});
    s.xrpc.post("vlpds.para.importAlphaState", &snapshot, &a.auth()).await.err_status(401);
    snapshot["rollouts"][1]["usedSlots"] = json!(0);
    s.xrpc.post("vlpds.para.importAlphaState", &snapshot, &Auth::Admin).await.err_status(400);
    snapshot["rollouts"][1]["usedSlots"] = json!(1);
    s.xrpc.post("vlpds.para.importAlphaState", &snapshot, &Auth::Admin).await.ok();
    assert_eq!(s.xrpc.get("com.para.alpha.getAccess", &[], &a.auth()).await.ok()["hasAccess"], true);
    assert_eq!(s.xrpc.get("com.para.alpha.getAccess", &[], &waiting.auth()).await.ok()["waitlistPosition"], 1);
    s.xrpc.post("vlpds.para.importAlphaState", &snapshot, &Auth::Admin).await.err(400, "AlphaAlreadyInitialized");
    s.xrpc
        .post("com.para.alpha.requestAccess", &json!({"state":"NL","inviteCode":code}), &waiting.auth())
        .await
        .err(400, "InvalidInviteCode");
    task.abort();
}

fn wallet() -> schnorrkel::Keypair {
    schnorrkel::MiniSecretKey::from_bytes(&rand::random::<[u8; 32]>())
        .unwrap()
        .expand_to_keypair(schnorrkel::ExpansionMode::Ed25519)
}

#[test]
fn native_verifier_accepts_the_im8_javascript_wallet_vector() {
    let vector: J = serde_json::from_str(include_str!("../fixtures/para-im8-wallet.json")).unwrap();
    let public =
        schnorrkel::PublicKey::from_bytes(&hex::decode(vector["publicKey"].as_str().unwrap()).unwrap()).unwrap();
    let signature =
        schnorrkel::Signature::from_bytes(&hex::decode(vector["signature"].as_str().unwrap()).unwrap()).unwrap();
    let message = vector["message"].as_str().unwrap();
    assert!(public.verify_simple(b"substrate", message.as_bytes(), &signature).is_ok());
    assert!(public
        .verify_simple(b"substrate", message.replace("para-pds/auth/v1", "para-id/sig/v1").as_bytes(), &signature)
        .is_err());
}

fn wallet_proof(key: &schnorrkel::Keypair, challenge: &J) -> String {
    use base64::Engine;
    let token = challenge["challenge"].as_str().unwrap();
    let sig = key.sign_simple(b"substrate", format!("para-pds/auth/v1\n{token}").as_bytes());
    let value = json!({"challenge":token,"publicKey":hex::encode(key.public.to_bytes()),"signature":hex::encode(sig.to_bytes())});
    format!("im8:{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(&value).unwrap()))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn historical_import_exceptions_are_exact_and_do_not_reopen_writes() {
    use sha2::{Digest, Sha256};
    let (s, _, task) = fixture().await;
    let a = s.create_account("historical").await;
    let path = "com.para.community.vote/3kaaaaaaaaaaa";
    let record = json!({"$type":"com.para.community.vote","voter":a.did,"signal":0,"community":format!("at://{}/com.para.community.board/3kaaaaaaaaaaa",a.did),"proposal":format!("at://{}/com.para.community.proposal/3kaaaaaaaaaaa",a.did),"createdAt":now_iso()});
    let record = Value::from_json(&record).unwrap().to_cbor();
    let cid = Cid::dag_cbor(&record);
    let mut tree = vlatproto::mst::Tree::new();
    tree.insert_no_proof(path.as_bytes(), cid).unwrap();
    let mut blocks = vec![(cid, record)];
    let data = tree.write_diff_blocks(&mut blocks).unwrap();
    let key = vlatproto::crypto::Keypair::generate();
    let mut fields: Vec<(String, Value)> = vec![
        ("did".into(), Value::Text(a.did.clone())),
        ("data".into(), Value::Link(data)),
        ("version".into(), Value::Int(3)),
        ("rev".into(), Value::Text("3kaaaaaaaaaaa".into())),
        ("prev".into(), Value::Null),
    ];
    fields.sort_by(|a, b| vlatproto::cbor::key_cmp(&a.0, &b.0));
    let signature = key.sign(&Value::Map(fields.clone()).to_cbor());
    fields.push(("sig".into(), Value::Bytes(signature.to_vec())));
    fields.sort_by(|a, b| vlatproto::cbor::key_cmp(&a.0, &b.0));
    let commit = Value::Map(fields).to_cbor();
    let root = Cid::dag_cbor(&commit);
    let mut car = Vec::new();
    vlatproto::car::write_header(&mut car, &root);
    vlatproto::car::write_block(&mut car, &root, &commit);
    for (cid, bytes) in blocks {
        vlatproto::car::write_block(&mut car, &cid, &bytes);
    }
    s.xrpc.post("com.atproto.server.deactivateAccount", &json!({}), &a.auth()).await.ok();
    let head = s.app.head(&a.did).await.ok().unwrap().commit;
    let mut manifest = json!({"did":a.did,"sha256":hex::encode(Sha256::digest(&car)),"sourceCommit":root.to_string(),"sourceSigningKey":key.did_key(),"expectedHead":head.to_string(),"historicalExceptions":[],"requireIm8Enrollment":false,"reason":"Reviewed historical public ballot retention"});
    let import = |token: String| {
        s.xrpc
            .http
            .post(format!("{}/xrpc/com.atproto.repo.importRepo", s.url))
            .bearer_auth(&a.access)
            .header("content-type", "application/vnd.ipld.car")
            .header("para-import-review", token)
            .body(car.clone())
            .send()
    };
    let review = s.xrpc.post("vlpds.para.reviewImport", &manifest, &Auth::Admin).await.ok();
    assert_eq!(
        import(review["reviewToken"].as_str().unwrap().into()).await.unwrap().json::<J>().await.unwrap()["error"],
        "BallotRefused"
    );
    manifest["historicalExceptions"] = json!([cid.to_string()]);
    manifest["sourceSigningKey"] = json!(vlatproto::crypto::Keypair::generate().did_key());
    let review = s.xrpc.post("vlpds.para.reviewImport", &manifest, &Auth::Admin).await.ok();
    assert_eq!(
        import(review["reviewToken"].as_str().unwrap().into()).await.unwrap().json::<J>().await.unwrap()["error"],
        "ParaImportSourceInvalid"
    );
    manifest["sourceSigningKey"] = json!(key.did_key());
    let review = s.xrpc.post("vlpds.para.reviewImport", &manifest, &Auth::Admin).await.ok();
    let response = import(review["reviewToken"].as_str().unwrap().into()).await.unwrap();
    assert_eq!(response.status().as_u16(), 200, "{}", response.text().await.unwrap_or_default());
    assert!(s.app.record_value(&a.did, None, path).await.ok().unwrap().is_some());
    let error = vlpds::para::check(&s.app, &a.did, "com.para.community.vote", &json!({})).await.err().unwrap();
    assert_eq!(error.error, "BallotRefused");
    task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn private_alpha_founders_and_im8_survive_peer_handoff_and_restart() {
    let (fixtures, _, task) = fixture().await;
    let store = Arc::new(object_store::memory::InMemory::new());
    let appview = fixtures.app.config.appview.clone();
    let config = |c: &mut vlpds::server::Config| {
        c.para.enabled = true;
        c.service_did = "did:web:para.cluster.test".into();
        c.jwt_secret = "para-test-shared-jwt-secret".into();
        c.appview = appview.clone();
    };
    let a = cluster_node("para-a", store.clone(), 8, config).await;
    let b = cluster_node("para-b", store.clone(), 8, config).await;
    balanced(&[&a, &b]).await;
    let owner = a.create_account("clusterowner").await;
    let founder = b.create_account("clusterfounder").await;
    let created = board(&a, &owner).await;
    let community = created["uri"].clone();
    a.xrpc
        .post("vlpds.para.inviteFounder", &json!({"communityUri":community,"founderDid":founder.did}), &owner.auth())
        .await
        .ok();
    b.xrpc.post("com.para.community.acceptDraftInvite", &json!({"communityUri":community}), &founder.auth()).await.ok();
    a.xrpc
        .post("vlpds.para.setAlphaRollout", &json!({"state":"NL","totalSlots":1,"isOpen":true}), &Auth::Admin)
        .await
        .ok();
    let aa = owner.auth();
    let ba = founder.auth();
    let request = json!({"state":"NL"});
    let (one, two) = tokio::join!(
        a.xrpc.post("com.para.alpha.requestAccess", &request, &aa),
        b.xrpc.post("com.para.alpha.requestAccess", &request, &ba)
    );
    assert_ne!(one.ok()["status"], two.ok()["status"]);
    let key = wallet();
    let enrollment = a
        .xrpc
        .post("vlpds.para.startIm8Enrollment", &json!({"publicKey":hex::encode(key.public.to_bytes())}), &owner.auth())
        .await
        .ok();
    a.xrpc
        .post(
            "com.para.account.setAuthFactor",
            &json!({"authFactorType":"im8","password":PASSWORD,"authFactorToken":wallet_proof(&key,&enrollment)}),
            &owner.auth(),
        )
        .await
        .ok();
    let proof = wallet_proof(
        &key,
        &b.xrpc.get("vlpds.para.getIm8Challenge", &[("identifier", owner.handle.as_str())], &Auth::None).await.ok(),
    );
    let login = json!({"identifier":owner.handle,"password":PASSWORD,"authFactorToken":proof});
    b.xrpc.post("com.atproto.server.createSession", &login, &Auth::None).await.ok();
    vlpds::server::shutdown(&a.app).await;
    balanced(&[&b]).await;
    b.xrpc.post("com.atproto.server.createSession", &login, &Auth::None).await.err(401, "AuthFactorTokenRequired");
    vlpds::server::shutdown(&b.app).await;
    let recovered = cluster_node("para-recovered", store, 8, |c| {
        config(c);
        c.para.enabled = false;
    })
    .await;
    balanced(&[&recovered]).await;
    // Disabling PARA cannot turn off an enrolled authentication factor.
    recovered.create_session(&owner.handle, PASSWORD).await.err(401, "AuthFactorTokenRequired");
    recovered
        .xrpc
        .post("com.atproto.server.createSession", &login, &Auth::None)
        .await
        .err(401, "AuthFactorTokenRequired");
    let proof = wallet_proof(
        &key,
        &recovered
            .xrpc
            .get("vlpds.para.getIm8Challenge", &[("identifier", owner.handle.as_str())], &Auth::None)
            .await
            .ok(),
    );
    recovered
        .xrpc
        .post(
            "com.atproto.server.createSession",
            &json!({"identifier":owner.handle,"password":PASSWORD,"authFactorToken":proof}),
            &Auth::None,
        )
        .await
        .ok();
    let raw = recovered.app.get_private("_para_alpha", "rollout/NL").await.ok().unwrap().unwrap();
    assert_eq!(serde_json::from_slice::<J>(&raw).unwrap()["usedSlots"], 1);
    vlpds::server::shutdown(&recovered.app).await;
    task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reviewed_import_requires_owner_exact_snapshot_signature_and_head() {
    use sha2::{Digest, Sha256};
    let (s, _, task) = fixture().await;
    let a = s.create_account("migrating").await;
    let original = board(&s, &a).await;
    let car = s.get_repo_car(&a.did).await;
    let (roots, _) = vlatproto::car::read_car(&car).unwrap();
    let account = s.app.account(&a.did).await.ok().unwrap();
    let key =
        s.app.secrets.signing_key(&a.did, &account.wrapped_signing_key, &account.signing_pubkey).await.ok().unwrap();
    let source_key = key.did_key();
    s.xrpc.post("com.atproto.server.deactivateAccount", &json!({}), &a.auth()).await.ok();
    let head = s.app.head(&a.did).await.ok().unwrap().commit;
    let manifest = json!({"did":a.did,"sha256":hex::encode(Sha256::digest(&car)),"sourceCommit":roots[0].to_string(),"sourceSigningKey":source_key,"expectedHead":head.to_string(),"historicalExceptions":[],"requireIm8Enrollment":true,"reason":"Reviewed the exact owned snapshot; wallet reenrollment is required"});
    s.xrpc.post("vlpds.para.reviewImport", &manifest, &a.auth()).await.err_status(401);
    let review = s.xrpc.post("vlpds.para.reviewImport", &manifest, &Auth::Admin).await.ok();
    let token = review["reviewToken"].as_str().unwrap();
    let import = |body: Vec<u8>| {
        s.xrpc
            .http
            .post(format!("{}/xrpc/com.atproto.repo.importRepo", s.url))
            .bearer_auth(&a.access)
            .header("content-type", "application/vnd.ipld.car")
            .header("para-import-review", token)
            .body(body)
            .send()
    };
    let mut changed = car.clone();
    changed.push(0);
    let response = import(changed).await.unwrap();
    assert_eq!(response.status().as_u16(), 400);
    assert_eq!(response.json::<J>().await.unwrap()["error"], "ParaImportDigestMismatch");
    let response = import(car.clone()).await.unwrap();
    assert_eq!(response.status().as_u16(), 200, "{}", response.text().await.unwrap_or_default());
    let response = import(car).await.unwrap();
    assert_eq!(response.status().as_u16(), 400);
    assert_eq!(response.json::<J>().await.unwrap()["error"], "ParaImportReviewRequired");
    assert_eq!(
        s.get_record(&a.did, "com.para.community.board", original["uri"].as_str().unwrap().rsplit('/').next().unwrap())
            .await
            .status,
        400
    ); // still inactive
    assert_ne!(s.app.head(&a.did).await.ok().unwrap().commit, head);
    s.xrpc
        .post("com.atproto.server.activateAccount", &json!({}), &a.auth())
        .await
        .err(400, "ParaIm8EnrollmentRequired");
    let wallet = wallet();
    let enrollment = s
        .xrpc
        .post("vlpds.para.startIm8Enrollment", &json!({"publicKey":hex::encode(wallet.public.to_bytes())}), &a.auth())
        .await
        .ok();
    s.xrpc
        .post(
            "com.para.account.setAuthFactor",
            &json!({"authFactorType":"im8","password":PASSWORD,"authFactorToken":wallet_proof(&wallet,&enrollment)}),
            &a.auth(),
        )
        .await
        .ok();
    let challenge =
        s.xrpc.get("vlpds.para.getIm8Challenge", &[("identifier", a.handle.as_str())], &Auth::None).await.ok();
    s.xrpc
        .post(
            "com.atproto.server.createSession",
            &json!({"identifier":a.handle,"password":PASSWORD,"authFactorToken":wallet_proof(&wallet,&challenge)}),
            &Auth::None,
        )
        .await
        .ok();
    task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn founders_require_creator_invitation_and_creator_signed_activation() {
    let (s, _, task) = fixture().await;
    let creator = s.create_account("founderowner").await;
    let board = board(&s, &creator).await;
    let community = board["uri"].as_str().unwrap();
    let input = json!({"communityUri":community});
    let unauthorized = s.create_account("notinvited").await;
    s.xrpc
        .post("com.para.community.acceptDraftInvite", &input, &unauthorized.auth())
        .await
        .err(400, "FounderInviteRequired");
    s.xrpc.post("vlpds.para.finalizeFounders", &input, &creator.auth()).await.err(400, "FounderQuorumRequired");
    for i in 0..8 {
        let member = s.create_account(&format!("founder{i}")).await;
        let invite = json!({"communityUri":community,"founderDid":member.did});
        s.xrpc.post("vlpds.para.inviteFounder", &invite, &member.auth()).await.err_status(401);
        s.xrpc.post("vlpds.para.inviteFounder", &invite, &creator.auth()).await.ok();
        let before = s.app.head(&creator.did).await.ok().unwrap().commit;
        let accepted = s.xrpc.post("com.para.community.acceptDraftInvite", &input, &member.auth()).await.ok();
        assert_eq!(accepted["status"], "draft");
        assert_eq!(accepted["memberCount"], i + 2);
        assert_eq!(s.app.head(&creator.did).await.ok().unwrap().commit, before);
        let member_head = s.app.head(&member.did).await.ok().unwrap().commit;
        assert_eq!(
            s.xrpc.post("com.para.community.acceptDraftInvite", &input, &member.auth()).await.ok()["memberCount"],
            i + 2
        );
        assert_eq!(s.app.head(&member.did).await.ok().unwrap().commit, member_head);
        s.xrpc.post("vlpds.para.finalizeFounders", &input, &member.auth()).await.err_status(401);
    }
    let key = community.rsplit('/').next().unwrap();
    let mut record = s.get_record(&creator.did, "com.para.community.board", key).await.ok()["value"].clone();
    record["status"] = json!("active");
    s.xrpc.post("com.atproto.repo.putRecord",&json!({"repo":creator.did,"collection":"com.para.community.board","rkey":key,"validate":false,"record":record}),&creator.auth()).await.err(400,"ParaCommandRequired");
    assert_eq!(s.xrpc.post("vlpds.para.finalizeFounders", &input, &creator.auth()).await.ok()["status"], "active");
    let head = s.app.head(&creator.did).await.ok().unwrap().commit;
    s.xrpc.post("vlpds.para.finalizeFounders", &input, &creator.auth()).await.ok();
    assert_eq!(s.app.head(&creator.did).await.ok().unwrap().commit, head);
    task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn im8_wallet_factor_binds_account_purpose_epoch_and_replay() {
    let (s, _, task) = fixture().await;
    let a = s.create_account("walletuser").await;
    let b = s.create_account("walletother").await;
    let key = wallet();
    let public = hex::encode(key.public.to_bytes());
    let enrollment = s.xrpc.post("vlpds.para.startIm8Enrollment", &json!({"publicKey":public}), &a.auth()).await.ok();
    let proof = wallet_proof(&key, &enrollment);
    let set = json!({"authFactorType":"im8","password":PASSWORD,"authFactorToken":proof});
    s.xrpc.post("com.para.account.setAuthFactor", &set, &b.auth()).await.err(401, "AuthFactorTokenRequired");
    s.xrpc.post("com.para.account.setAuthFactor", &set, &a.auth()).await.ok();
    s.xrpc.get("com.para.account.getAuthFactor", &[], &a.auth()).await.err(400, "ExpiredToken");
    s.create_session(&a.handle, PASSWORD).await.err(401, "AuthFactorTokenRequired");
    let params = [("identifier", a.handle.as_str())];
    let anon = Auth::None;
    let get = || s.xrpc.get("vlpds.para.getIm8Challenge", &params, &anon);
    let login = wallet_proof(&key, &get().await.ok());
    let input = json!({"identifier":a.handle,"password":PASSWORD,"authFactorToken":login});
    let session = s.xrpc.post("com.atproto.server.createSession", &input, &Auth::None).await.ok();
    s.xrpc.post("com.atproto.server.createSession", &input, &Auth::None).await.err(401, "AuthFactorTokenRequired");
    let auth = Auth::Bearer(session["accessJwt"].as_str().unwrap().into());
    assert_eq!(s.xrpc.get("com.para.account.getAuthFactor", &[], &auth).await.ok()["authFactorType"], "im8");
    let app_password =
        s.xrpc.post("com.atproto.server.createAppPassword", &json!({"name":"walletguard"}), &auth).await.ok()
            ["password"]
            .as_str()
            .unwrap()
            .to_owned();
    s.create_session(&a.handle, &app_password).await.err(401, "AuthFactorTokenRequired");
    let wrong_purpose = wallet_proof(&key, &get().await.ok());
    s.xrpc
        .post("com.para.account.setAuthFactor", &json!({"password":PASSWORD,"authFactorToken":wrong_purpose}), &auth)
        .await
        .err(401, "AuthFactorTokenRequired");
    let removal = s.xrpc.post("vlpds.para.startIm8Removal", &json!({}), &auth).await.ok();
    let remove_proof = wallet_proof(&key, &removal);
    s.xrpc
        .post("com.para.account.setAuthFactor", &json!({"password":"wrong","authFactorToken":remove_proof}), &auth)
        .await
        .err_status(401);
    s.xrpc
        .post("com.para.account.setAuthFactor", &json!({"password":PASSWORD,"authFactorToken":remove_proof}), &auth)
        .await
        .ok();
    s.xrpc.get("com.para.account.getAuthFactor", &[], &auth).await.err(400, "ExpiredToken");
    s.create_session(&a.handle, PASSWORD).await.ok();
    task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn im8_is_enforced_during_oauth_browser_login() {
    use super::oauth::{self, Browser, DpopKey, Flow};
    let s = oauth::spawn_with(|c| c.para.enabled = true).await;
    let a = oauth::create_account(&s, "im8oauth").await;
    let key = wallet();
    let (_, enroll) = s
        .bearer(
            &a.jwt,
            "vlpds.para.startIm8Enrollment",
            true,
            Some(json!({"publicKey":hex::encode(key.public.to_bytes())})),
        )
        .await;
    let proof = wallet_proof(&key, &enroll);
    assert_eq!(
        s.bearer(
            &a.jwt,
            "com.para.account.setAuthFactor",
            true,
            Some(json!({"authFactorType":"im8","password":oauth::PASSWORD,"authFactorToken":proof}))
        )
        .await
        .0,
        200
    );
    let dpop = DpopKey::new();
    let flow = Flow::loopback("atproto transition:generic", &dpop);
    let ru = flow.request_uri(&s, &oauth::pkce(), "im8-test").await;
    let mut browser = Browser::default();
    let (_, _, html) = browser.authorize(&s, &flow, &ru).await;
    let (_, _, html) = browser.sign_in(&s, &ru, &oauth::csrf_of(&html), &a.handle, oauth::PASSWORD).await;
    assert!(html.contains("iM8 approval token"), "{html}");
    let challenge = s.get_json(&format!("/xrpc/vlpds.para.getIm8Challenge?identifier={}", oauth::enc(&a.handle))).await;
    let proof = wallet_proof(&key, &challenge);
    let (status, html) = browser.second_factor(&s, &ru, &oauth::csrf_of(&html), &proof).await;
    assert_eq!(status, 200, "{html}");
    assert!(html.contains("name=\"action\" value=\"allow\""), "{html}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn alpha_allocation_invites_and_waitlist_are_atomic() {
    let (s, _, task) = fixture().await;
    let a = s.create_account("alphaone").await;
    let b = s.create_account("alphatwo").await;
    let c = s.create_account("alphathree").await;
    let body = json!({"state":"NL","totalSlots":1,"isOpen":true});
    s.xrpc.post("vlpds.para.setAlphaRollout", &body, &a.auth()).await.err_status(401);
    s.xrpc.post("com.para.alpha.createInvite", &json!({"state":"NL"}), &a.auth()).await.err_status(401);
    s.xrpc.get("com.para.alpha.getRolloutStatus", &[], &a.auth()).await.err_status(401);
    s.xrpc.post("vlpds.para.setAlphaRollout", &body, &Auth::Admin).await.ok();
    let input = json!({"state":"nl"});
    let aa = a.auth();
    let ba = b.auth();
    let (left, right) = tokio::join!(
        s.xrpc.post("com.para.alpha.requestAccess", &input, &aa),
        s.xrpc.post("com.para.alpha.requestAccess", &input, &ba)
    );
    let left = left.ok();
    let right = right.ok();
    assert_ne!(left["status"], right["status"]);
    let (winner, waiting) = if left["status"] == "approved" { (&a, &b) } else { (&b, &a) };
    assert_eq!(
        s.xrpc.post("com.para.alpha.requestAccess", &input, &winner.auth()).await.ok()["status"],
        "already_has_access"
    );
    assert_eq!(s.xrpc.get("com.para.alpha.getAccess", &[], &waiting.auth()).await.ok()["waitlistPosition"], 1);
    assert_eq!(s.xrpc.post("com.para.alpha.requestAccess", &input, &waiting.auth()).await.ok()["position"], 1);
    s.xrpc
        .post("vlpds.para.setAlphaRollout", &json!({"state":"NL","totalSlots":0,"isOpen":true}), &Auth::Admin)
        .await
        .err_status(400);
    s.xrpc
        .post("vlpds.para.setAlphaRollout", &json!({"state":"FRIEND","totalSlots":1,"isOpen":false}), &Auth::Admin)
        .await
        .ok();
    let code = s.xrpc.post("com.para.alpha.createInvite", &json!({"state":"FRIEND"}), &Auth::Admin).await.ok()["codes"]
        [0]
    .clone();
    let with_invite = json!({"state":"NL","inviteCode":code});
    let wa = waiting.auth();
    let ca = c.auth();
    let (one, two) = tokio::join!(
        s.xrpc.post("com.para.alpha.requestAccess", &with_invite, &wa),
        s.xrpc.post("com.para.alpha.requestAccess", &with_invite, &ca)
    );
    assert_ne!(one.is_ok(), two.is_ok(), "{one:?} {two:?}");
    let states = s.xrpc.get("com.para.alpha.getRolloutStatus", &[], &Auth::Admin).await.ok()["states"].clone();
    for state in ["NL", "FRIEND"] {
        assert_eq!(states.as_array().unwrap().iter().find(|s| s["state"] == state).unwrap()["usedSlots"], 1);
    }
    let new_code = s.xrpc.post("com.para.alpha.createInvite", &json!({"state":"FRIEND"}), &Auth::Admin).await.ok()
        ["codes"][0]
        .clone();
    let loser = if one.is_ok() { &c } else { waiting };
    s.xrpc
        .post("com.para.alpha.requestAccess", &json!({"state":"NL","inviteCode":new_code}), &loser.auth())
        .await
        .err(400, "AlphaCapacityReached");
    task.abort();
}
async fn board(s: &TestServer, a: &TestAccount) -> J {
    s.xrpc
        .post("com.para.community.createBoard", &json!({"name":"Native PARA","quadrant":"MX-NL"}), &a.auth())
        .await
        .ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn board_region_is_persisted_and_invalid_region_does_not_reserve_a_name() {
    let (s, _, task) = fixture().await;
    let alice = s.create_account("alice").await;
    let bob = s.create_account("bob").await;
    let input = json!({"name":"Agua Limpia","quadrant":"norte","region":"  Jalisco  "});
    let created = s.xrpc.post("com.para.community.createBoard", &input, &alice.auth()).await.ok();
    let key = created["uri"].as_str().unwrap().rsplit('/').next().unwrap();
    let record = s.get_record(&alice.did, "com.para.community.board", key).await.ok();
    assert_eq!(record["value"]["region"], "Jalisco");

    let replay = s
        .xrpc
        .post("com.para.community.createBoard", &json!({"name":"Agua Limpia","quadrant":"norte"}), &alice.auth())
        .await
        .ok();
    assert_eq!(replay["uri"], created["uri"]);
    assert_eq!(s.get_record(&alice.did, "com.para.community.board", key).await.ok()["value"]["region"], "Jalisco");

    for (name, region) in [("Invalid Type", json!(42)), ("Invalid Length", json!("x".repeat(65)))] {
        s.xrpc
            .post(
                "com.para.community.createBoard",
                &json!({"name":name,"quadrant":"norte","region":region}),
                &alice.auth(),
            )
            .await
            .err(400, "InvalidRequest");
        s.xrpc.post("com.para.community.createBoard", &json!({"name":name,"quadrant":"norte"}), &bob.auth()).await.ok();
    }

    let blank = s
        .xrpc
        .post(
            "com.para.community.createBoard",
            &json!({"name":"Blank region","quadrant":"norte","region":"  "}),
            &alice.auth(),
        )
        .await
        .ok();
    let blank_key = blank["uri"].as_str().unwrap().rsplit('/').next().unwrap();
    assert!(s.get_record(&alice.did, "com.para.community.board", blank_key).await.ok()["value"]
        .get("region")
        .is_none());
    task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn commands_use_owner_repo_atomic_commits_and_handle_proxy_headers() {
    let (s, state, task) = fixture().await;
    let a = s.create_account("alice").await;
    let b = s.create_account("bob").await;
    let mut subscription = s.subscribe_from_now().await;
    let response = s
        .xrpc
        .http
        .post(format!("{}/xrpc/com.para.community.createBoard", s.url))
        .bearer_auth(&a.access)
        .header("atproto-proxy", "did:web:appview.test#bsky_appview")
        .json(&json!({"name":"Native PARA","quadrant":"MX-NL"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200, "{}", response.text().await.unwrap_or_default());
    let frames = subscription
        .until(std::time::Duration::from_secs(5), |fs| fs.iter().any(|f| f.commit().is_some_and(|c| c.ops.len() == 8)))
        .await;
    assert!(frames.iter().filter_map(|f| f.commit()).any(|c| c.ops.len() == 8));
    let created = board(&s, &a).await;
    for (coll, n) in [
        ("com.para.community.board", 1),
        ("com.para.community.membership", 1),
        ("com.para.community.governance", 1),
        ("com.para.community.authorityEvent", 2),
        ("app.bsky.graph.list", 1),
        ("app.bsky.graph.listitem", 1),
        ("app.bsky.graph.starterpack", 1),
    ] {
        assert_eq!(count(&s, &a.did, coll).await, n)
    }
    state.lock().unwrap()["board"] =
        json!({"uri":created["uri"],"admissionMode":"open","viewerMembershipState":"active"});
    let before = s.app.head(&a.did).await.ok().unwrap().commit;
    let joined = s.xrpc.post("com.para.community.join", &json!({"communityUri":created["uri"]}), &b.auth()).await.ok();
    assert_eq!(joined["membershipState"], "active");
    assert_eq!(before, s.app.head(&a.did).await.ok().unwrap().commit);
    let left = s.xrpc.post("com.para.community.leave", &json!({"communityUri":created["uri"]}), &b.auth()).await.ok();
    assert_eq!(left["membershipState"], "left");
    s.xrpc
        .post("com.para.community.leave", &json!({"communityUri":created["uri"]}), &a.auth())
        .await
        .err(400, "CommunityOwnerCannotLeave");
    let rkey = joined["uri"].as_str().unwrap().rsplit('/').next().unwrap();
    s.xrpc.post("com.atproto.repo.putRecord",&json!({"repo":b.did,"collection":"com.para.community.membership","rkey":rkey,"validate":false,"record":{"community":created["uri"],"membershipState":"active","roles":["owner"],"joinedAt":now_iso()}}),&b.auth()).await.err(400,"CommunityStewardRequired");
    task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn packs_enforce_cid_and_repository_ownership() {
    let (s, _, task) = fixture().await;
    let a = s.create_account("alice").await;
    let b = s.create_account("bob").await;
    let board = board(&s, &a).await;
    let pack=s.xrpc.post("com.para.community.createBriefingPack",&json!({"packType":"party_lobbying","communityUri":board["uri"],"party":"test","title":"First","summary":"Evidence"}),&a.auth()).await.ok()["pack"].clone();
    let update = json!({"uri":pack["uri"],"cid":pack["cid"],"pack":{"title":"Second"}});
    assert_eq!(
        s.xrpc.post("com.para.community.updateBriefingPack", &update, &a.auth()).await.ok()["pack"]["title"],
        "Second"
    );
    s.xrpc.post("com.para.community.updateBriefingPack", &update, &a.auth()).await.err(400, "InvalidSwap");
    s.xrpc.post("com.para.community.updateBriefingPack", &update, &b.auth()).await.err_status(401);
    task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn policy_cannot_be_bypassed_by_validation_or_batches() {
    let (s, _, task) = fixture().await;
    let a = s.create_account("alice").await;
    for coll in [
        "com.para.community.vote",
        "com.para.community.intensity",
        "com.para.raq.proposalAnswer",
        "com.para.community.civicTreeVote",
        "app.bsky.feed.repost",
        "com.para.raq.axisVote",
    ] {
        let record = json!({"voteNullifier":"forbidden"});
        let error = if coll == "app.bsky.feed.repost" { "UnsupportedRecord" } else { "BallotRefused" };
        for method in ["createRecord", "putRecord"] {
            s.xrpc
                .post(
                    &format!("com.atproto.repo.{method}"),
                    &json!({"repo":a.did,"collection":coll,"rkey":"3kaaaaaaaaaaa","record":record,"validate":false}),
                    &a.auth(),
                )
                .await
                .err(400, error);
        }
    }
    s.xrpc.post("com.atproto.repo.applyWrites",&json!({"repo":a.did,"validate":false,"writes":[{"$type":"com.atproto.repo.applyWrites#create","collection":"app.bsky.actor.profile","rkey":"self","value":{"displayName":"Must not commit"}},{"$type":"com.atproto.repo.applyWrites#create","collection":"com.para.community.vote","value":{}}]}),&a.auth()).await.err(400,"BallotRefused");
    assert_eq!(count(&s, &a.did, "app.bsky.actor.profile").await, 0);
    s.xrpc
        .post(
            "com.atproto.repo.deleteRecord",
            &json!({"repo":a.did,"collection":"com.para.community.vote","rkey":"3kaaaaaaaaaaa"}),
            &a.auth(),
        )
        .await
        .ok();
    task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn proof_verification_fails_closed_and_vote_command_writes_normally() {
    let (s, state, task) = fixture().await;
    let a = s.create_account("alice").await;
    let subject = format!("at://{}/com.para.civic.cabildeo/3kaaaaaaaaaaa", a.did);
    let proof = format!("m8:cabildeo:v1:{}", "A".repeat(43));
    let input =
        json!({"cabildeo":subject,"selectedOption":0,"voteNullifier":"a".repeat(64),"eligibilityProofRef":proof});
    state.lock().unwrap()["cabildeo"] =
        json!({"uri":subject,"phase":"voting","options":["yes","no"],"community":"p/native"});
    state.lock().unwrap()["board"] = json!({"viewerMembershipState":"active"});
    state.lock().unwrap()["verifier"] = json!(503);
    s.xrpc.post("com.para.civic.castVote", &input, &a.auth()).await.err(503, "ParaVerifierUnavailable");
    assert_eq!(count(&s, &a.did, "com.para.civic.vote").await, 0);
    state.lock().unwrap()["verifier"] = json!(422);
    s.xrpc.post("com.para.civic.castVote", &input, &a.auth()).await.err(400, "BallotRefused");
    state.lock().unwrap()["verifier"] = json!(204);
    let vote = s.xrpc.post("com.para.civic.castVote", &input, &a.auth()).await.ok();
    assert!(vote["commit"]["cid"].is_string());
    assert_eq!(count(&s, &a.did, "com.para.civic.vote").await, 1);
    task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authority_events_are_immutable_and_disabled_para_is_explicit() {
    let (s, _, task) = fixture().await;
    let a = s.create_account("alice").await;
    let b = board(&s, &a).await;
    let key = b["uri"].as_str().unwrap().rsplit('/').next().unwrap();
    s.xrpc
        .post(
            "com.atproto.repo.deleteRecord",
            &json!({"repo":a.did,"collection":"com.para.community.authorityEvent","rkey":key}),
            &a.auth(),
        )
        .await
        .err(400, "InvalidRequest");
    s.xrpc
        .post("com.para.community.acceptDraftInvite", &json!({"communityUri":b["uri"]}), &a.auth())
        .await
        .err(400, "InvalidRequest");
    let disabled = TestServer::spawn().await;
    let user = disabled.create_account("off").await;
    disabled
        .xrpc
        .post("com.para.community.join", &json!({"communityUri":b["uri"]}), &user.auth())
        .await
        .err(501, "MethodNotImplemented");
    task.abort();
}

#[test]
fn verifier_configuration_and_claim_contracts_are_strict() {
    for url in [
        "http://remote.example/verify",
        "https://user:pass@example.test/verify",
        "https://example.test/verify?token=x",
        "https://example.test/verify#fragment",
    ] {
        assert!(vlpds::para::Config::new(true, Some(url.into()), None).is_err())
    }
    assert!(vlpds::para::Config::new(true, Some("http://127.0.0.1:1234/verify".into()), None).is_ok());
    let mut claim = json!({"subjectType":"policy","subject":"at://did:plc:abc/com.para.civic.policy/test","signal":3,"isDirect":true,"voteNullifier":"a".repeat(64),"eligibilityProofRef":format!("m8:policy:v1:{}","A".repeat(43))});
    assert_eq!(vlpds::para::ballot_claim(&claim).unwrap()["signal"], 3);
    claim["signal"] = json!(4);
    assert!(vlpds::para::ballot_claim(&claim).is_none());
    // No neutral ballot (OD-10 §7.5): a signal of 0 is not a ballot.
    claim["signal"] = json!(0);
    assert!(vlpds::para::ballot_claim(&claim).is_none());
    claim["signal"] = json!(-3);
    assert_eq!(vlpds::para::ballot_claim(&claim).unwrap()["signal"], -3);
    claim["signal"] = json!(1);
    claim["delegatedFrom"] = json!(["did:plc:abc"]);
    assert!(vlpds::para::ballot_claim(&claim).is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_scopes_and_shared_content_contracts_are_enforced() {
    let (s, _, task) = fixture().await;
    let a = s.create_account("owner").await;
    let created = board(&s, &a).await;
    let post = s
        .xrpc
        .post(
            "com.atproto.repo.createRecord",
            &json!({"repo":a.did,"collection":"app.bsky.feed.post","record":post_record("Shared evidence")}),
            &a.auth(),
        )
        .await
        .ok();
    let input =
        json!({"communityUri":created["uri"],"contentType":"post","subject":{"uri":post["uri"],"cid":post["cid"]}});
    let shared = s.xrpc.post("com.para.community.shareContent", &input, &a.auth()).await.ok();
    assert_eq!(shared["sharedContent"]["removed"], false);
    s.xrpc.post("com.para.community.shareContent", &input, &a.auth()).await.err(400, "DuplicateSharedContent");
    let moderation = json!({"communityUri":created["uri"],"sharedContent":{"uri":shared["uri"],"cid":shared["cid"]}});
    for (method, action) in [("removeSharedContent", "remove"), ("restoreSharedContent", "restore")] {
        assert_eq!(
            s.xrpc.post(&format!("com.para.community.{method}"), &moderation, &a.auth()).await.ok()["action"]["action"],
            action
        );
    }
    let password = s
        .xrpc
        .post(
            "com.atproto.server.createAppPassword",
            &json!({"name":"post-only","scopes":"atproto repo?collection=app.bsky.feed.post&action=create"}),
            &a.auth(),
        )
        .await
        .ok();
    let session = s.create_session(&a.handle, password["password"].as_str().unwrap()).await.ok();
    let auth = Auth::Bearer(session["accessJwt"].as_str().unwrap().into());
    s.xrpc
        .post("com.para.community.createBoard", &json!({"name":"Unauthorized","quadrant":"MX-NL"}), &auth)
        .await
        .err(403, "ScopeMissingError");
    s.xrpc.post("com.para.community.shareContent", &input, &auth).await.err(403, "ScopeMissingError");
    assert_eq!(count(&s, &a.did, "com.para.community.board").await, 1);
    task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn name_reservations_are_atomic_and_invalid_commands_do_not_claim_names() {
    let (s, _, task) = fixture().await;
    let a = s.create_account("alice").await;
    let b = s.create_account("bob").await;
    let body = json!({"name":"Same name","quadrant":"MX-NL"});
    let auth_a = a.auth();
    let auth_b = b.auth();
    let (left, right) = tokio::join!(
        s.xrpc.post("com.para.community.createBoard", &body, &auth_a),
        s.xrpc.post("com.para.community.createBoard", &body, &auth_b)
    );
    assert_ne!(left.is_ok(), right.is_ok(), "{left:?} {right:?}");
    if left.is_ok() {
        right.err(400, "CommunityNameTaken");
    } else {
        left.err(400, "CommunityNameTaken");
    }
    let bad = json!({"name":"Retry after rejection","quadrant":"MX-NL","founderStarterPackName":"x".repeat(1000)});
    s.xrpc.post("com.para.community.createBoard", &bad, &a.auth()).await.err(400, "InvalidRequest");
    s.xrpc
        .post("com.para.community.createBoard", &json!({"name":"Retry after rejection","quadrant":"MX-NL"}), &b.auth())
        .await
        .ok();
    task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn large_collection_lookup_refuses_without_writing() {
    let (s, _, task) = fixture().await;
    let a = s.create_account("owner").await;
    let created = board(&s, &a).await;
    let subject = json!({"uri":format!("at://{}/app.bsky.feed.post/3kaaaaaaaaaaa",a.did),"cid":Cid::dag_cbor(b"evidence").to_string()});
    let padding = "x".repeat(50_000);
    for _ in 0..17 {
        let writes:Vec<J>=(0..10).map(|_|json!({"$type":"com.atproto.repo.applyWrites#create","collection":"com.para.community.sharedContent","rkey":s.app.tids.next().to_string(),"value":{"$type":"com.para.community.sharedContent","communityUri":created["uri"],"subject":subject,"contentType":"post","sharedBy":a.did,"createdAt":now_iso(),"padding":padding}})).collect();
        s.xrpc
            .post("com.atproto.repo.applyWrites", &json!({"repo":a.did,"validate":false,"writes":writes}), &a.auth())
            .await
            .ok();
    }
    let before = s.app.head(&a.did).await.ok().unwrap().commit;
    s.xrpc
        .post(
            "com.para.community.shareContent",
            &json!({"communityUri":created["uri"],"subject":subject,"contentType":"post"}),
            &a.auth(),
        )
        .await
        .err(400, "ParaLookupLimit");
    assert_eq!(before, s.app.head(&a.did).await.ok().unwrap().commit);
    task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn horizontal_governance_is_retired_and_boards_are_always_hierarchical() {
    let (s, _, task) = fixture().await;
    let a = s.create_account("hier").await;
    let horizontal = json!({"name":"Horizontal","quadrant":"MX-NL","governanceMode":"horizontal"});
    s.xrpc.post("com.para.community.createBoard", &horizontal, &a.auth()).await.err(400, "InvalidRequest");
    // A client that still sends the old default is served the only model left.
    let input = json!({"name":"Hierarchical","quadrant":"MX-NL","governanceMode":"hierarchical"});
    let created = s.xrpc.post("com.para.community.createBoard", &input, &a.auth()).await.ok();
    let key = created["uri"].as_str().unwrap().rsplit('/').next().unwrap();
    let board = s.get_record(&a.did, "com.para.community.board", key).await.ok();
    assert!(board["value"].get("governanceMode").is_none());
    let events = s.list_records(&a.did, "com.para.community.authorityEvent", &[]).await.ok();
    let actions: Vec<_> = events["records"].as_array().unwrap().iter().map(|r| r["value"]["action"].clone()).collect();
    assert!(actions.contains(&json!("owner.grant")) && !actions.contains(&json!("moderator.grant")));
    let owner = events["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["value"]["action"] == "owner.grant")
        .unwrap();
    assert!(owner["value"].get("expiresAt").is_none());
    // No write may introduce horizontal governance on a board.
    let mut record = board["value"].clone();
    record["governanceMode"] = json!("horizontal");
    s.xrpc
        .post(
            "com.atproto.repo.putRecord",
            &json!({"repo":a.did,"collection":"com.para.community.board","rkey":key,"record":record}),
            &a.auth(),
        )
        .await
        .err(400, "ParaCommandRequired");
    task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn space_writes_run_the_ballot_policy() {
    use crate::common::spaces::SpaceClient;
    const SCOPE: &str = "space:com.example.group?collection=com.para.community.vote&collection=com.para.civic.vote&action=read&action=create&action=update&action=delete&manage=create&manage=update&manage=delete";
    let s = TestServer::spawn_with(|c| {
        c.spaces = true;
        c.para = vlpds::para::Config::new(true, None, None).unwrap();
    })
    .await;
    let owner = SpaceClient::new(&s, "spballot", SCOPE).await;
    let space = owner.create_space("com.example.group", "main").await;
    let ballot = json!({"$type":"com.para.community.vote","voter":owner.did,"signal":3,"createdAt":now_iso()});
    // A frozen ballot collection is refused in a space as in the repo, and `validate: false` does not waive it.
    owner.create_record(&space, "com.para.community.vote", Some("a"), ballot.clone()).await.err(400, "BallotRefused");
    owner
        .post(
            "com.atproto.space.applyWrites",
            json!({"space":space,"repo":owner.did,"validate":false,"writes":[{"$type":"com.atproto.space.applyWrites#create","collection":"com.para.community.vote","rkey":"b","value":ballot}]}),
        )
        .await
        .err(400, "BallotRefused");
    // A public cabildeo ballot needs its proof shape and verifier wherever it is written.
    let bare = json!({"$type":"com.para.civic.vote","subject":"at://did:plc:abc/com.para.civic.cabildeo/x","subjectType":"cabildeo","createdAt":now_iso()});
    owner.put_record(&space, "com.para.civic.vote", "c", bare).await.err(400, "BallotRefused");
}
