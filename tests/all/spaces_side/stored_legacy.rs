//! Rows the previous build wrote that this one can't carry over: its
//! registerNotify registrations (`sN`) name a service, where a registration
//! now is an endpoint and a token the syncer chose, and its pending
//! notifySpaceDeleted retries (`sD`) are for a method the alpha dropped.
//! Both stay inert, nothing sent to them, and are dropped at their expiry,
//! as the previous build dropped them. The `sN` rows are the L2 fixture's
//! encoding (`testdata/formats/L2/state/space_notify.json`), the `sD` rows
//! that build's JSON.

use super::ref_net::*;
use crate::common::*;
use std::time::Duration;
use vlpds::space::rows::LegacyNotifyRow;
use vlpds::state::{space_deleted_key, space_id, space_notify_key};

const HOUR: u64 = 3_600_000_000;

fn fixture_row() -> Vec<u8> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/formats/L2/state/space_notify.json");
    let j: J = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    hex::decode(j["row"].as_str().unwrap()).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_previous_builds_registrations_stay_inert_until_they_expire() {
    let net = Net::new(1).await;
    let (alice, bob) = (net.actor("alice", 0).await, net.actor("bob", 1).await);
    let svc = MockService::syncer().await;
    let space =
        net.create_space(&alice, SpaceOpts { skey: Some("legacy"), members: &[&bob], ..Default::default() }).await;
    let sid = space_id(&space);
    let p = net.pds[0].app.partition(&alice.did).ok().unwrap();
    let now = vlatproto::tid::now_micros();

    let expired_row = fixture_row();
    assert!(LegacyNotifyRow::decode(&expired_row).unwrap().expires < now, "the fixture's registration has expired");
    let expired = space_notify_key(&alice.did, &sid, "did:web:old.example#atproto_space_syncer");
    let live = space_notify_key(&alice.did, &sid, &svc.service_ref());
    let soon = now + 4_000_000;
    p.db.put(&expired, expired_row).await.unwrap();
    p.db.put(&live, LegacyNotifyRow { endpoint: svc.url.clone(), expires: soon }.encode().to_vec()).await.unwrap();
    let deleted = |expires: u64| {
        let row = json!({"uri": space, "endpoint": svc.url, "expires": expires, "attempts": 1, "retryAt": now});
        serde_json::to_vec(&row).unwrap()
    };
    let d_expired = space_deleted_key(&alice.did, &sid, "did:web:old.example#atproto_space_syncer");
    let d_live = space_deleted_key(&alice.did, &sid, &svc.service_ref());
    p.db.put(&d_expired, deleted(now - 1)).await.unwrap();
    p.db.put(&d_live, deleted(now + HOUR)).await.unwrap();

    // a write runs the space's fan-out: the expired registration goes, the
    // live one stays, and neither is sent anything
    write(&alice, &space, W::new()).await.ok();
    write(&bob, &space, W::new()).await.ok();
    let gone =
        eventually(Duration::from_secs(10), async || p.db.get(&expired).await.unwrap().is_none().then_some(())).await;
    assert!(gone.is_some(), "the expired registerNotify row was dropped");
    assert!(p.db.get(&live).await.unwrap().is_some(), "a live one waits for its expiry");
    let regs = vlpds::space::host::registrations(&net.pds[0].app, &alice.did, &sid).await.unwrap();
    assert_eq!(regs.all().count(), 0, "neither counts as a registration");

    let retry = |at: u64| vlpds::space::notices::retry_due(&net.pds[0].app, at);
    assert_eq!(retry(now).await.unwrap(), 1, "the expired sD row is dropped");
    assert!(p.db.get(&d_expired).await.unwrap().is_none());
    assert!(p.db.get(&d_live).await.unwrap().is_some(), "a live one isn't retried, nor dropped yet");
    assert_eq!(retry(now + 2 * HOUR).await.unwrap(), 1, "and goes at its expiry");
    assert!(p.db.get(&d_live).await.unwrap().is_none());

    tokio::time::sleep(Duration::from_micros(soon.saturating_sub(vlatproto::tid::now_micros()) + 500_000)).await;
    write(&alice, &space, W::new()).await.ok();
    let gone =
        eventually(Duration::from_secs(10), async || p.db.get(&live).await.unwrap().is_none().then_some(())).await;
    assert!(gone.is_some(), "the live registerNotify row is dropped once it expires");

    let sent: Vec<String> =
        svc.calls().into_iter().map(|c| c.lxm).filter(|l| l.starts_with("com.atproto.space.")).collect();
    assert!(sent.is_empty(), "nothing goes to the previous build's registrations: {sent:?}");
}

/// deleteSpace takes the previous build's rows with the space's own.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deleted_space_takes_the_previous_builds_rows() {
    let net = Net::new(1).await;
    let alice = net.actor("alice", 0).await;
    let space = net.create_space(&alice, SpaceOpts { skey: Some("legacy-del"), ..Default::default() }).await;
    let sid = space_id(&space);
    let p = net.pds[0].app.partition(&alice.did).ok().unwrap();
    let key = space_notify_key(&alice.did, &sid, "did:web:old.example#atproto_space_syncer");
    let row = LegacyNotifyRow { endpoint: "https://old.example".into(), expires: vlatproto::tid::now_micros() + HOUR };
    p.db.put(&key, row.encode().to_vec()).await.unwrap();
    alice.post("com.atproto.space.deleteSpace", json!({"space": space})).await.ok();
    assert!(p.db.get(&key).await.unwrap().is_none());
}
