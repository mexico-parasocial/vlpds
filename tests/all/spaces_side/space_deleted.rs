//! The reference's tests/space/notifications.test.ts "syncer notices": a
//! space's deletion notice, and a lapsed registration's expiry notice,
//! that fail are kept (`sE`) and retried by the authority's shard owner.
//! Retries run here through `vlpds::space::notices::retry_due` with a clock
//! ahead of the backoff (its schedule is `notices`' own unit test).

use super::ref_net::*;
use crate::common::spaces::SpaceClient;
use crate::common::*;
use std::time::Duration;

const UPDATE: &str = "com.atproto.space.notifySpaceUpdate";
const DELETED: &str = "com.atproto.space.notifySpaceUpdate#spaceDeleted";
const EXPIRED: &str = "com.atproto.space.notifySpaceUpdate#registrationExpired";
const HOUR: u64 = 3_600_000_000;

/// A space with `reader` a member, and a syncer registered through the
/// reader's authorization.
async fn registered(net: &Net, owner: &SpaceClient, reader: &SpaceClient, skey: &str) -> (String, MockService) {
    let syncer = MockService::syncer().await;
    let space = net.create_space(owner, SpaceOpts { skey: Some(skey), members: &[reader], ..Default::default() }).await;
    reader.authorize_notify(&net.pds[0].url, &space, syncer.notify_registration("syncer-token")).await.ok();
    (space, syncer)
}

async fn delete(owner: &SpaceClient, space: &str) {
    owner.post("com.atproto.space.deleteSpace", json!({"space": space})).await.ok();
}

async fn retry(net: &Net, ahead: u64) -> usize {
    vlpds::space::notices::retry_due(&net.pds[0].app, vlatproto::tid::now_micros() + ahead).await.unwrap()
}

fn kinds(svc: &MockService) -> Vec<String> {
    svc.space_updates().iter().map(|u| u["$type"].as_str().unwrap_or_default().to_string()).collect()
}

/// Waits for `n` updates of `kind` at `svc`.
async fn notices(svc: &MockService, kind: &str, n: usize) {
    for _ in 0..200 {
        if kinds(svc).iter().filter(|k| *k == kind).count() >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{n} {kind} notices: got {:?}", kinds(svc));
}

/// Lapses the registration of `svc` in `space`, as its credential's expiry
/// would.
async fn lapse(net: &Net, space: &str, svc: &MockService) {
    let app = &net.pds[0].app;
    let at = vlatproto::tid::now_micros() - 60_000_000;
    vlpds::xrpc::space::set_registration_expiry(app, space, &svc.url, "syncer-token", at).await.unwrap();
}

async fn registration(
    net: &Net,
    owner: &SpaceClient,
    space: &str,
    svc: &MockService,
) -> Option<vlpds::space::rows::RegRow> {
    let p = net.pds[0].app.partition(&owner.did).ok().unwrap();
    let id = vlpds::state::registration_id(&svc.url, "syncer-token");
    let key = vlpds::state::space_registration_key(&owner.did, &vlpds::state::space_id(space), &id);
    p.db.get(key).await.unwrap().map(|v| vlpds::space::rows::RegRow::decode(&v).unwrap())
}

/// "sends immediately without queueing a successful notification", "retries
/// a transient failure until delivered", "does not retry a permanent
/// failure" and "notifies registered syncers, including lapsed
/// registrations".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deliveries_and_retries() {
    let net = Net::new(1).await;
    let (alice, bob) = (net.actor("alice", 0).await, net.actor("bob", 1).await);

    let (space, ok) = registered(&net, &alice, &bob, "sent").await;
    delete(&alice, &space).await;
    notices(&ok, DELETED, 1).await;
    let call = &ok.calls_to(UPDATE)[0];
    assert_eq!(call.body, json!({"space": space, "token": "syncer-token", "update": {"$type": DELETED}}));
    assert_eq!(call.auth, None, "the token is the registration's proof, not service auth");
    assert_eq!(retry(&net, HOUR).await, 0, "nothing queued");

    let (space, flaky) = registered(&net, &alice, &bob, "flaky").await;
    flaky.respond(503, json!({"error": "Unavailable"}));
    delete(&alice, &space).await;
    notices(&flaky, DELETED, 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(retry(&net, 0).await, 0, "not due yet");
    assert_eq!(retry(&net, 2 * 60_000_000).await, 0, "a second try, failing");
    assert_eq!(kinds(&flaky).len(), 2);
    flaky.respond(200, json!({}));
    assert_eq!(retry(&net, 2 * HOUR).await, 1, "delivered on retry");
    assert_eq!(kinds(&flaky).len(), 3);
    assert_eq!(retry(&net, 30 * HOUR).await, 0, "and cleared");

    let (space, gone) = registered(&net, &alice, &bob, "gone").await;
    gone.respond(400, json!({"error": "InvalidRequest"}));
    delete(&alice, &space).await;
    notices(&gone, DELETED, 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(retry(&net, 2 * HOUR).await, 0, "a permanent failure isn't kept");
    assert_eq!(kinds(&gone).len(), 1);

    // a lapsed registration is told of a deletion too
    let (space, lapsed) = registered(&net, &alice, &bob, "lapsed").await;
    lapse(&net, &space, &lapsed).await;
    delete(&alice, &space).await;
    notices(&lapsed, DELETED, 1).await;
}

/// "stops retrying after a final retry past 72 hours" and "drops a
/// deletion notice once the space is recreated".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retries_end_with_the_window_or_a_new_space() {
    let net = Net::new(1).await;
    let (alice, bob) = (net.actor("alice", 0).await, net.actor("bob", 1).await);

    let (space, down) = registered(&net, &alice, &bob, "window").await;
    down.respond(503, json!({}));
    delete(&alice, &space).await;
    notices(&down, DELETED, 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(retry(&net, 80 * HOUR).await, 1, "the last retry, past the window, settles it");
    assert_eq!(kinds(&down).len(), 2);
    assert_eq!(retry(&net, 90 * HOUR).await, 0);

    let (space, again) = registered(&net, &alice, &bob, "again").await;
    again.respond(503, json!({}));
    delete(&alice, &space).await;
    notices(&again, DELETED, 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    net.create_space(&alice, SpaceOpts { skey: Some("again"), ..Default::default() }).await;
    again.respond(200, json!({}));
    assert_eq!(retry(&net, 2 * HOUR).await, 1, "dropped for the new space");
    assert_eq!(kinds(&again).len(), 1, "the new space's syncer isn't told");
}

/// "sends a lapsed registration an expiry notice in place of the next
/// update, then nothing more", "drops a lapsed registration that rejects
/// its expiry notice", "drops a lapsed registration once its expiry notice
/// stops being retried" and "drops an expiry notice once the registration
/// is renewed".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn expiry_notices() {
    let net = Net::new(1).await;
    let (alice, bob) = (net.actor("alice", 0).await, net.actor("bob", 1).await);

    let (space, svc) = registered(&net, &alice, &bob, "once").await;
    lapse(&net, &space, &svc).await;
    write(&alice, &space, W::new().rkey("a")).await.ok();
    notices(&svc, EXPIRED, 1).await;
    write(&alice, &space, W::new().rkey("b")).await.ok();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(kinds(&svc), [EXPIRED], "one notice in place of the updates");
    assert!(svc.space_updates()[0]["spaceRev"].is_string());
    assert!(registration(&net, &alice, &space, &svc).await.is_some_and(|r| r.expiry_notified));
    // authorizing again renews it: updates flow again
    bob.authorize_notify(&net.pds[0].url, &space, svc.notify_registration("syncer-token")).await.ok();
    write(&alice, &space, W::new().rkey("c")).await.ok();
    notices(&svc, "com.atproto.space.defs#repoWrite", 1).await;

    let (space, rejects) = registered(&net, &alice, &bob, "rejects").await;
    lapse(&net, &space, &rejects).await;
    rejects.respond(400, json!({"error": "InvalidRequest"}));
    write(&alice, &space, W::new()).await.ok();
    notices(&rejects, EXPIRED, 1).await;
    for _ in 0..100 {
        if registration(&net, &alice, &space, &rejects).await.is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(registration(&net, &alice, &space, &rejects).await.is_none(), "unregistered");

    let (space, down) = registered(&net, &alice, &bob, "down").await;
    lapse(&net, &space, &down).await;
    down.respond(503, json!({}));
    write(&alice, &space, W::new()).await.ok();
    notices(&down, EXPIRED, 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(registration(&net, &alice, &space, &down).await.is_some());
    assert_eq!(retry(&net, 80 * HOUR).await, 1);
    assert!(registration(&net, &alice, &space, &down).await.is_none(), "dropped once the notice gave up");

    let (space, renews) = registered(&net, &alice, &bob, "renews").await;
    lapse(&net, &space, &renews).await;
    renews.respond(503, json!({}));
    write(&alice, &space, W::new()).await.ok();
    notices(&renews, EXPIRED, 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    renews.respond(200, json!({}));
    bob.authorize_notify(&net.pds[0].url, &space, renews.notify_registration("syncer-token")).await.ok();
    assert_eq!(retry(&net, 2 * HOUR).await, 1, "no longer owed");
    assert_eq!(kinds(&renews), [EXPIRED], "and not sent again");
    assert!(registration(&net, &alice, &space, &renews).await.is_some());
}

/// "lets a deletion notice supersede a pending expiry notice, but not the
/// reverse".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deletion_notice_supersedes_a_pending_expiry_notice() {
    let net = Net::new(1).await;
    let (alice, bob) = (net.actor("alice", 0).await, net.actor("bob", 1).await);
    let (space, svc) = registered(&net, &alice, &bob, "supersede").await;
    lapse(&net, &space, &svc).await;
    svc.respond(503, json!({}));
    write(&alice, &space, W::new()).await.ok();
    notices(&svc, EXPIRED, 1).await;
    delete(&alice, &space).await;
    notices(&svc, DELETED, 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let p = net.pds[0].app.partition(&alice.did).ok().unwrap();
    let id = vlpds::state::registration_id(&svc.url, "syncer-token");
    let key = vlpds::state::space_notice_key(&alice.did, &vlpds::state::space_id(&space), &id);
    let row: vlpds::space::notices::NoticeRow = serde_json::from_slice(&p.db.get(key).await.unwrap().unwrap()).unwrap();
    assert_eq!(row.notice, vlpds::space::notices::Notice::SpaceDeleted);
}

/// An `sE` row that doesn't parse is dropped and counted, and the pass
/// still retries the space's other notices.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_undecodable_notice_is_dropped_without_stalling_the_rest() {
    let net = Net::new(1).await;
    let (alice, bob) = (net.actor("alice", 0).await, net.actor("bob", 1).await);
    let (space, flaky) = registered(&net, &alice, &bob, "bad-row").await;
    flaky.respond(503, json!({}));
    delete(&alice, &space).await;
    notices(&flaky, DELETED, 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let p = net.pds[0].app.partition(&alice.did).ok().unwrap();
    let bad = vlpds::state::space_notice_key(&alice.did, &vlpds::state::space_id(&space), &[0; 32]);
    p.db.put(&bad, b"not a notice".to_vec()).await.unwrap();
    flaky.respond(200, json!({}));
    assert_eq!(retry(&net, 2 * HOUR).await, 2, "the bad row dropped, the good one delivered");
    assert!(p.db.get(&bad).await.unwrap().is_none());
    assert_eq!(kinds(&flaky).len(), 2);
}
