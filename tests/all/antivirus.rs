//! uploadBlob's malware scan (`--av-socket`; src/antivirus.rs,
//! docs/_antivirus-scanning.md) against a fake clamd speaking the INSTREAM
//! protocol: clean, cached, infected (refused, with a case and a takedown),
//! the multipart path, a repo moving in (kept in quarantine), and both fail
//! modes.
use crate::common::*;
use object_store::ObjectStoreExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Bytes containing this are "infected". Not EICAR, so nothing on the
/// machine running the tests flags this file.
const MARKER: &[u8] = b"VLPDS-AV-TEST-MARKER";
const SIGNATURE: &str = "Vlpds.Test.Marker-1";

#[derive(Default)]
struct Fake {
    /// Every stream is infected, marker or not.
    infect_all: AtomicBool,
    /// Every stream gets clamd's StreamMaxLength error.
    error: AtomicBool,
    /// VERSION goes unanswered (no db version: nothing is cached).
    no_version: AtomicBool,
    /// Streams clamd received in full and answered (abandoned ones don't count).
    scans: AtomicUsize,
}

struct Clamd {
    fake: Arc<Fake>,
    path: PathBuf,
}

impl Drop for Clamd {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

async fn fake_clamd() -> Clamd {
    let path = std::env::temp_dir().join(format!("{}.sock", unique_name("av")));
    let _ = std::fs::remove_file(&path);
    let l = tokio::net::UnixListener::bind(&path).unwrap();
    let fake = Arc::new(Fake::default());
    let f = fake.clone();
    tokio::spawn(async move {
        while let Ok((c, _)) = l.accept().await {
            tokio::spawn(serve(f.clone(), c));
        }
    });
    Clamd { fake, path }
}

async fn serve(f: Arc<Fake>, mut c: tokio::net::UnixStream) -> std::io::Result<()> {
    let mut cmd = Vec::new();
    loop {
        match c.read_u8().await? {
            0 => break,
            b => cmd.push(b),
        }
    }
    match cmd.as_slice() {
        b"zVERSION" if !f.no_version.load(Ordering::SeqCst) => {
            c.write_all(b"ClamAV 1.4.1/27001/Tue Nov  3 00:00:00 2026\0").await?;
        }
        b"zINSTREAM" => {
            let mut data = Vec::new();
            loop {
                // a dropped connection before the terminator: abandoned, unscanned
                let n = c.read_u32().await? as usize;
                if n == 0 {
                    break;
                }
                let at = data.len();
                data.resize(at + n, 0);
                c.read_exact(&mut data[at..]).await?;
            }
            f.scans.fetch_add(1, Ordering::SeqCst);
            let reply = if f.error.load(Ordering::SeqCst) {
                "INSTREAM size limit exceeded. ERROR".to_string()
            } else if f.infect_all.load(Ordering::SeqCst) || data.windows(MARKER.len()).any(|w| w == MARKER) {
                format!("stream: {SIGNATURE} FOUND")
            } else {
                "stream: OK".to_string()
            };
            c.write_all(reply.as_bytes()).await?;
            c.write_all(&[0]).await?;
        }
        _ => {}
    }
    c.shutdown().await
}

fn av(path: &std::path::Path, fail_open: bool) -> vlpds::antivirus::Config {
    vlpds::antivirus::Config {
        addr: vlpds::antivirus::Addr::Unix(path.to_path_buf()),
        fail_open,
        timeout: Duration::from_secs(5),
        max_concurrency: 4,
    }
}

async fn db_known(s: &TestServer) {
    let scanner = s.app.av.clone().expect("scanner");
    for _ in 0..200 {
        if scanner.db_version().is_some() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the scanner never learned clamd's db version");
}

fn infected(n: usize, seed: u8) -> Vec<u8> {
    let mut b: Vec<u8> = (0..n).map(|i| (i as u8).wrapping_mul(17).wrapping_add(seed)).collect();
    let at = n / 2;
    b[at..at + MARKER.len()].copy_from_slice(MARKER);
    b
}

fn cid_of(j: &J) -> String {
    j["blob"]["ref"]["$link"].as_str().unwrap().to_string()
}

async fn object_exists(s: &TestServer, root: &str, did: &str, cid: &str) -> bool {
    let p = object_store::path::Path::from(format!("{}/{root}/{did}/{cid}", s.app.store.prefix));
    s.app.store.raw.head(&p).await.is_ok()
}

async fn cases(s: &TestServer, did: &str) -> Vec<J> {
    s.xrpc.get("vlpds.admin.listCases", &[("did", did)], &Auth::Admin).await.ok()["cases"].as_array().unwrap().clone()
}

/// sha256 → CID, as uploadBlob computes it.
fn raw_cid(b: &[u8]) -> String {
    use sha2::Digest;
    let digest: [u8; 32] = sha2::Sha256::digest(b).into();
    vlatproto::cid::Cid { codec: vlatproto::cid::CODEC_RAW, digest }.to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn clean_cached_and_infected_uploads() {
    let clamd = fake_clamd().await;
    let path = clamd.path.clone();
    let s = TestServer::spawn_with(|c| {
        c.max_blob_size = 20 << 20;
        c.antivirus = Some(av(&path, false));
    })
    .await;
    db_known(&s).await;
    let a = s.create_account("avclean").await;
    let up = |b: Vec<u8>| {
        let (x, auth) = (s.xrpc.clone(), a.auth());
        async move { x.post_bytes("com.atproto.repo.uploadBlob", b, "image/png", &auth).await }
    };

    // clean: scanned once, stored and served
    let png = random_png(201);
    let r = up(png.clone()).await.ok();
    assert_eq!(clamd.fake.scans.load(Ordering::SeqCst), 1);
    let g = s.get_blob(&a.did, &cid_of(&r)).await;
    assert_eq!(g.status, 200);
    assert_eq!(&g.body[..], &png[..]);

    // the same bytes again: clean by the cache, the stream abandoned unscanned
    up(png.clone()).await.ok();
    assert_eq!(clamd.fake.scans.load(Ordering::SeqCst), 1, "a cached blob is not rescanned");

    // infected: refused, never stored, a case opened and the blob taken down
    let bad = infected(4000, 3);
    let cid = raw_cid(&bad);
    up(bad.clone()).await.err(400, "BlobRejected");
    assert_eq!(clamd.fake.scans.load(Ordering::SeqCst), 2);
    assert!(!object_exists(&s, "blob", &a.did, &cid).await);
    assert!(!object_exists(&s, "blob-quarantine", &a.did, &cid).await, "a refused upload keeps no bytes");
    let cs = cases(&s, &a.did).await;
    assert_eq!(cs.len(), 1, "{cs:?}");
    assert_eq!(cs[0]["status"], "actioned", "the takedown is already applied");
    assert!(cs[0]["source"].as_str().unwrap().contains(SIGNATURE), "{}", cs[0]);
    assert_eq!(cs[0]["subjects"][0]["cid"], json!(cid));
    assert_eq!(cs[0]["actions"][0]["action"], "takedown", "{}", cs[0]);
    let st =
        s.xrpc.get("com.atproto.admin.getSubjectStatus", &[("did", &a.did), ("blob", &cid)], &Auth::Admin).await.ok();
    assert_eq!(st["takedown"]["applied"], json!(true), "{st}");
    assert_eq!(st["takedown"]["ref"], json!(format!("av:{SIGNATURE}")), "{st}");

    // a retry is refused by the takedown before any scan, with no second case
    let r = up(bad.clone()).await;
    assert!(!r.is_ok() && r.text().contains("takendown"), "{}", r.text());
    assert_eq!(clamd.fake.scans.load(Ordering::SeqCst), 2, "a taken-down blob is not rescanned");
    assert_eq!(cases(&s, &a.did).await.len(), 1);

    // over one 8 MiB part: the multipart path, scanned as it streams
    let big = infected(12_000_000, 4);
    let big_cid = raw_cid(&big);
    up(big).await.err(400, "BlobRejected");
    assert!(!object_exists(&s, "blob", &a.did, &big_cid).await);
    assert_eq!(cases(&s, &a.did).await.len(), 2);
    let big_clean: Vec<u8> = (0..12_000_000).map(|i| (i as u8).wrapping_mul(13)).collect();
    let r = up(big_clean.clone()).await.ok();
    let g = s.get_blob(&a.did, &cid_of(&r)).await;
    assert_eq!(g.body.len(), big_clean.len());

    // results are counted
    let m = prometheus::gather();
    let total = |result: &str| {
        m.iter()
            .find(|f| f.name() == "vlpds_blob_av_total")
            .and_then(|f| f.get_metric().iter().find(|x| x.get_label().iter().any(|l| l.value() == result)))
            .map(|x| x.get_counter().get_value())
            .unwrap_or(0.0)
    };
    assert!(total("clean") >= 2.0 && total("cached") >= 1.0 && total("infected") >= 2.0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_verdict_fails_closed_or_open() {
    let clamd = fake_clamd().await;
    let path = clamd.path.clone();
    let s = TestServer::spawn_with(|c| c.antivirus = Some(av(&path, false))).await;
    let a = s.create_account("avclosed").await;
    clamd.fake.error.store(true, Ordering::SeqCst);
    let png = random_png(202);
    let r = s.xrpc.post_bytes("com.atproto.repo.uploadBlob", png.clone(), "image/png", &a.auth()).await;
    r.err(503, "ScanUnavailable");
    assert!(!object_exists(&s, "blob", &a.did, &raw_cid(&png)).await);
    // clamd back: the retry goes through
    clamd.fake.error.store(false, Ordering::SeqCst);
    s.upload_blob(&a, &png, "image/png").await;

    // open: no clamd at all, stored unscanned
    let gone = std::env::temp_dir().join(format!("{}.sock", unique_name("avgone")));
    let o = TestServer::spawn_with(|c| c.antivirus = Some(av(&gone, true))).await;
    let b = o.create_account("avopen").await;
    let blob = o.upload_blob(&b, &png, "image/png").await;
    assert_eq!(o.get_blob(&b.did, blob["ref"]["$link"].as_str().unwrap()).await.status, 200);
}

/// A repo moving in uploads the blobs it references. An infected one is not
/// refused (the import would never complete) but kept in quarantine under a
/// case: never served, purged with the quarantine.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn moving_in_quarantines_infected_blobs() {
    let clamd = fake_clamd().await;
    // no db version: the first, clean scan isn't cached
    clamd.fake.no_version.store(true, Ordering::SeqCst);
    let path = clamd.path.clone();
    let s = TestServer::spawn_with(|c| c.antivirus = Some(av(&path, false))).await;
    let a = s.create_account("avmig").await;
    let png = random_png(203);
    let blob = s.upload_blob(&a, &png, "image/png").await;
    let cid = blob["ref"]["$link"].as_str().unwrap().to_string();
    s.create_record(&a, "app.bsky.feed.post", image_post("pic", &blob)).await;
    let car = s.get_repo_car(&a.did).await;
    // the account "arrives" without its blob
    let p = object_store::path::Path::from(format!("{}/blob/{}/{}", s.app.store.prefix, a.did, cid));
    s.app.store.raw.delete(&p).await.unwrap();
    vlpds::xrpc::blobs::sweep_blobs_settle(&s.app, Duration::ZERO, Duration::ZERO).await.unwrap();
    s.xrpc.post("com.atproto.server.deactivateAccount", &json!({}), &a.auth()).await.ok();
    s.import_repo(&a.auth(), car).await.ok();

    // the signatures now flag it
    clamd.fake.infect_all.store(true, Ordering::SeqCst);
    let r = s.xrpc.post_bytes("com.atproto.repo.uploadBlob", png.clone(), "image/png", &a.auth()).await.ok();
    assert_eq!(cid_of(&r), cid);
    assert!(!object_exists(&s, "blob", &a.did, &cid).await);
    assert!(object_exists(&s, "blob-quarantine", &a.did, &cid).await);
    s.get_blob(&a.did, &cid).await.client_err();
    let admin = s.xrpc.get("com.atproto.sync.getBlob", &[("did", &a.did), ("cid", &cid)], &Auth::Admin).await;
    assert_eq!(admin.status, 200, "operators can still fetch it: {}", admin.text());
    assert_eq!(&admin.body[..], &png[..]);
    let cs = cases(&s, &a.did).await;
    assert_eq!(cs.len(), 1, "{cs:?}");
    assert!(cs[0]["notes"][0]["text"].as_str().unwrap().contains("quarantine"), "{}", cs[0]);
    // nothing missing: the import can finish
    let missing = s.xrpc.get("com.atproto.repo.listMissingBlobs", &[], &a.auth()).await.ok();
    assert!(missing["blobs"].as_array().unwrap().is_empty(), "{missing}");

    // an unrelated new upload while moving in is held to the scan as usual
    s.xrpc
        .post_bytes("com.atproto.repo.uploadBlob", random_png(204), "image/png", &a.auth())
        .await
        .err(400, "BlobRejected");
}

/// Against a real clamd: `VLPDS_TEST_CLAMD=tcp://127.0.0.1:3310 cargo test
/// --test all antivirus::real_clamd -- --ignored` (e.g. the clamav-debian
/// image, configured as deploy/ansible's vlpds role does). The EICAR test file
/// is refused, a clean one and one near --max-blob-mb (clamd's
/// StreamMaxLength must allow it) are stored.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a clamd: VLPDS_TEST_CLAMD"]
async fn real_clamd() {
    let addr = std::env::var("VLPDS_TEST_CLAMD").expect("VLPDS_TEST_CLAMD=tcp://host:3310 or a socket path");
    let cfg = vlpds::antivirus::Config {
        addr: vlpds::antivirus::Addr::parse(&addr).unwrap(),
        fail_open: false,
        timeout: Duration::from_secs(60),
        max_concurrency: 4,
    };
    let s = TestServer::spawn_with(|c| c.antivirus = Some(cfg)).await;
    db_known(&s).await;
    let a = s.create_account("avreal").await;
    let up = |b: Vec<u8>, mime: &'static str| {
        let (x, auth) = (s.xrpc.clone(), a.auth());
        async move { x.post_bytes("com.atproto.repo.uploadBlob", b, mime, &auth).await }
    };

    // EICAR, put together at run time so this source file isn't itself flagged
    let eicar = [r"X5O!P%@AP[4\PZX54(P^)7CC)7}$EICAR-STANDARD-", "ANTIVIRUS-TEST-FILE!$H+H*"].concat().into_bytes();
    let cid = raw_cid(&eicar);
    up(eicar, "text/plain").await.err(400, "BlobRejected");
    assert!(!object_exists(&s, "blob", &a.did, &cid).await);
    let cs = cases(&s, &a.did).await;
    assert_eq!(cs.len(), 1, "{cs:?}");
    let source = cs[0]["source"].as_str().unwrap();
    assert!(source.to_ascii_lowercase().contains("eicar"), "clamd's signature name in the case: {source}");

    let png = random_png(205);
    let r = up(png.clone(), "image/png").await.ok();
    assert_eq!(&s.get_blob(&a.did, &cid_of(&r)).await.body[..], &png[..]);

    // 99 MiB, under the default 100 MiB --max-blob-mb: streamed to clamd as it uploads
    let big: Vec<u8> = (0..99usize << 20).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
    let t = std::time::Instant::now();
    let r = up(big.clone(), "video/mp4").await.ok();
    eprintln!("99 MiB upload + scan: {:?}", t.elapsed());
    assert_eq!(r["blob"]["size"], json!(big.len()));
}
