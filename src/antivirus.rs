//! Malware scanning of uploaded blobs with ClamAV's daemon (`--av-socket`,
//! docs/_antivirus-scanning.md).
//!
//! uploadBlob opens a [`Session`] before it reads the body and feeds it each
//! chunk as it hashes it, so a large upload is never read back. clamd's
//! `INSTREAM` spools what it gets and scans only on the zero-length
//! terminator, so a session dropped before [`Session::finish`] costs clamd no
//! scan: that is how a cached clean verdict (keyed by the blob's sha256 and
//! the signature database version) skips one.
//!
//! The scanner never decides what happens to a blob; uploadBlob does, from
//! the [`Verdict`] and `--av-fail-mode`.

use bytes::Bytes;
use parking_lot::Mutex;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, Semaphore};

/// clamd's reply is one short line; anything longer is not clamd.
const MAX_REPLY: usize = 4096;
/// INSTREAM frames are split to at most this (clamd's StreamMaxLength caps
/// the whole stream, not a frame, but a frame over it is refused outright).
const MAX_FRAME: usize = 1 << 20;
/// Chunks queued between the upload and the clamd connection.
const QUEUE: usize = 64;
const CACHE_ENTRIES: usize = 100_000;
const VERSION_EVERY: Duration = Duration::from_secs(60);
/// Signature names are shown to operators and stored in cases; clamd's are
/// short ASCII, so anything else is cut.
const MAX_SIGNATURE: usize = 200;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Addr {
    Unix(PathBuf),
    Tcp(String),
}

impl Addr {
    /// `tcp://host:port`, or a unix socket path (optionally `unix://`).
    pub fn parse(s: &str) -> anyhow::Result<Addr> {
        let s = s.trim();
        if let Some(hp) = s.strip_prefix("tcp://") {
            anyhow::ensure!(
                hp.rsplit_once(':').is_some_and(|(h, p)| !h.is_empty() && p.parse::<u16>().is_ok()),
                "--av-socket: tcp://host:port, not {s}"
            );
            return Ok(Addr::Tcp(hp.to_string()));
        }
        let p = s.strip_prefix("unix://").unwrap_or(s);
        anyhow::ensure!(!p.is_empty(), "--av-socket: empty path");
        Ok(Addr::Unix(PathBuf::from(p)))
    }
}

impl std::fmt::Display for Addr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Addr::Unix(p) => write!(f, "unix://{}", p.display()),
            Addr::Tcp(hp) => write!(f, "tcp://{hp}"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub addr: Addr,
    /// `--av-fail-mode open`: store a blob the scanner could not judge.
    /// Default (closed): refuse it with a retryable 503.
    pub fail_open: bool,
    /// One scan, from the first byte queued to clamd's answer.
    pub timeout: Duration,
    /// Scans in flight to clamd from this node (keep under its MaxThreads).
    pub max_concurrency: usize,
}

impl Config {
    pub fn new(addr: Addr) -> Config {
        Config { addr, fail_open: false, timeout: Duration::from_secs(30), max_concurrency: 8 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Clean,
    /// Clean by the cache: these bytes passed under the current signatures.
    Cached,
    /// The signature clamd named.
    Infected(String),
    /// No verdict: clamd unreachable, slow, or refusing (e.g. the stream
    /// over its StreamMaxLength).
    Error(String),
}

impl Verdict {
    pub fn label(&self) -> &'static str {
        match self {
            Verdict::Clean => "clean",
            Verdict::Cached => "cached",
            Verdict::Infected(_) => "infected",
            Verdict::Error(_) => "error",
        }
    }
}

pub struct Scanner {
    pub cfg: Config,
    permits: Semaphore,
    /// sha256 → the signature database version it was found clean under.
    cache: Mutex<lru::LruCache<[u8; 32], u64>>,
    /// The database version clamd last reported; None until it answers.
    db_version: Mutex<Option<u64>>,
}

impl Scanner {
    /// Starts the background `VERSION` poll (metrics, cache validity) for
    /// as long as the scanner lives.
    pub fn spawn(cfg: Config) -> Arc<Scanner> {
        let s = Arc::new(Scanner {
            permits: Semaphore::new(cfg.max_concurrency.max(1)),
            cache: Mutex::new(lru::LruCache::new(NonZeroUsize::new(CACHE_ENTRIES).unwrap())),
            db_version: Mutex::new(None),
            cfg,
        });
        tracing::info!(addr = %s.cfg.addr, fail_open = s.cfg.fail_open, "antivirus: scanning uploaded blobs with clamd");
        let weak = Arc::downgrade(&s);
        tokio::spawn(poll_version(weak));
        s
    }

    /// A scan of bytes still to come. Connecting waits for a permit; the
    /// chunks queue meanwhile (then the upload waits: backpressure).
    pub fn start(self: &Arc<Self>) -> Session {
        let (tx, rx) = mpsc::channel(QUEUE);
        let me = self.clone();
        let task = tokio::spawn(async move { me.run(rx).await });
        Session { tx: Some(tx), task: Some(task), started: std::time::Instant::now() }
    }

    /// Every wait here is bounded by the timeout on its own (the permit, the
    /// connect, each write), so a stalled clamd can't hold the upload; the
    /// reply is bounded by [`Session::finish`]. The upload itself may take as
    /// long as the client needs.
    async fn run(&self, mut rx: mpsc::Receiver<Bytes>) -> Verdict {
        let t = self.cfg.timeout;
        let _permit = match tokio::time::timeout(t, self.permits.acquire()).await {
            Ok(Ok(p)) => p,
            Ok(Err(_)) => return discard(&mut rx, "scanner closed".into()).await,
            Err(_) => return discard(&mut rx, format!("{} scans in flight for {t:?}", self.cfg.max_concurrency)).await,
        };
        let mut conn = match tokio::time::timeout(t, connect(&self.cfg.addr)).await {
            Ok(Ok(c)) => c,
            Ok(Err(e)) => return discard(&mut rx, format!("connect {}: {e}", self.cfg.addr)).await,
            Err(_) => return discard(&mut rx, format!("connect {}: timed out", self.cfg.addr)).await,
        };
        instream(&mut conn, &mut rx, t).await
    }

    /// Ends `s` with a verdict. A cached clean result for `digest` drops the
    /// stream unscanned; a fresh clean one is cached.
    pub async fn conclude(&self, s: Session, digest: &[u8; 32]) -> Verdict {
        let current = *self.db_version.lock();
        if let Some(v) = current {
            if self.cache.lock().get(digest) == Some(&v) {
                s.cancel();
                crate::metrics::BLOB_AV.with_label_values(&["cached"]).inc();
                return Verdict::Cached;
            }
        }
        let started = s.started;
        let verdict = s.finish(self.cfg.timeout).await;
        crate::metrics::BLOB_AV_SECONDS.observe(started.elapsed().as_secs_f64());
        crate::metrics::BLOB_AV.with_label_values(&[verdict.label()]).inc();
        match &verdict {
            Verdict::Clean => {
                if let Some(v) = current {
                    self.cache.lock().put(*digest, v);
                }
            }
            Verdict::Infected(sig) => tracing::warn!(signature = %sig, "antivirus: infected upload"),
            Verdict::Error(e) => tracing::warn!("antivirus: no verdict: {e}"),
            Verdict::Cached => {}
        }
        verdict
    }

    /// Asks clamd for its version once (startup check, tests).
    pub async fn version(&self) -> anyhow::Result<String> {
        let mut c = connect(&self.cfg.addr).await?;
        c.write_all(b"zVERSION\0").await?;
        c.flush().await?;
        read_reply(&mut c).await
    }

    pub fn db_version(&self) -> Option<u64> {
        *self.db_version.lock()
    }
}

/// One upload's scan. Dropping it (or [`Session::cancel`]) abandons the
/// stream before clamd scans it.
pub struct Session {
    tx: Option<mpsc::Sender<Bytes>>,
    task: Option<tokio::task::JoinHandle<Verdict>>,
    started: std::time::Instant,
}

impl Session {
    /// Queues a chunk (cheap: `Bytes` is shared with the hasher and store).
    /// A broken scan keeps taking chunks and discarding them, so the upload
    /// is never stuck on it; it answers [`Verdict::Error`] at the end.
    pub async fn feed(&mut self, b: Bytes) {
        if let Some(tx) = &self.tx {
            if tx.send(b).await.is_err() {
                self.tx = None;
            }
        }
    }

    /// Ends the stream and waits up to `timeout` for clamd's answer.
    pub async fn finish(mut self, timeout: Duration) -> Verdict {
        self.tx = None; // end of stream: the task sends the terminator
        let Some(mut t) = self.task.take() else { return Verdict::Error("scan already ended".into()) };
        match tokio::time::timeout(timeout, &mut t).await {
            Ok(r) => r.unwrap_or_else(|e| Verdict::Error(format!("scan task: {e}"))),
            Err(_) => {
                t.abort();
                Verdict::Error(format!("no answer within {timeout:?}"))
            }
        }
    }

    pub fn cancel(mut self) {
        if let Some(t) = self.task.take() {
            t.abort();
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(t) = self.task.take() {
            t.abort();
        }
    }
}

trait Conn: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Conn for T {}

async fn connect(addr: &Addr) -> std::io::Result<Box<dyn Conn>> {
    Ok(match addr {
        Addr::Tcp(hp) => Box::new(tokio::net::TcpStream::connect(hp).await?),
        #[cfg(unix)]
        Addr::Unix(p) => Box::new(tokio::net::UnixStream::connect(p).await?),
        #[cfg(not(unix))]
        Addr::Unix(_) => return Err(std::io::Error::other("unix sockets need a unix build")),
    })
}

/// Takes the rest of the upload without scanning it, then answers `e`.
async fn discard(rx: &mut mpsc::Receiver<Bytes>, e: String) -> Verdict {
    while rx.recv().await.is_some() {}
    Verdict::Error(e)
}

async fn write_frame(conn: &mut Box<dyn Conn>, b: &[u8], t: Duration) -> std::io::Result<()> {
    let w = async {
        conn.write_all(&(b.len() as u32).to_be_bytes()).await?;
        conn.write_all(b).await
    };
    tokio::time::timeout(t, w).await.unwrap_or_else(|_| Err(std::io::Error::other("write timed out")))
}

/// `zINSTREAM`: `<u32 BE length><bytes>` frames, a zero-length one to end,
/// then one NUL-terminated reply. clamd may answer early (stream too long)
/// and close: the write fails, the rest is discarded, its reply still read.
async fn instream(conn: &mut Box<dyn Conn>, rx: &mut mpsc::Receiver<Bytes>, t: Duration) -> Verdict {
    let mut broken: Option<std::io::Error> = None;
    if let Err(e) = conn.write_all(b"zINSTREAM\0").await {
        broken = Some(e);
    }
    while let Some(chunk) = rx.recv().await {
        if broken.is_some() {
            continue;
        }
        for frame in chunk.chunks(MAX_FRAME) {
            if let Err(e) = write_frame(conn, frame, t).await {
                broken = Some(e);
                break;
            }
        }
    }
    if broken.is_none() {
        if let Err(e) = async {
            conn.write_all(&[0, 0, 0, 0]).await?;
            conn.flush().await
        }
        .await
        {
            broken = Some(e);
        }
    }
    match read_reply(conn).await {
        Ok(r) => parse_reply(&r),
        Err(e) => Verdict::Error(match broken {
            Some(w) => format!("stream: {w}; reply: {e}"),
            None => format!("reply: {e}"),
        }),
    }
}

async fn read_reply<R: AsyncRead + Unpin + ?Sized>(r: &mut R) -> anyhow::Result<String> {
    let mut out = Vec::new();
    let mut buf = [0u8; 512];
    loop {
        let n = r.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
        if let Some(end) = out.iter().position(|&b| b == 0 || b == b'\n') {
            out.truncate(end);
            break;
        }
        anyhow::ensure!(out.len() <= MAX_REPLY, "reply over {MAX_REPLY} bytes");
    }
    anyhow::ensure!(!out.is_empty(), "connection closed without a reply");
    Ok(String::from_utf8_lossy(&out).trim().to_string())
}

/// `stream: OK`, `stream: <signature> FOUND`, or an error line (`... ERROR`).
pub fn parse_reply(r: &str) -> Verdict {
    let body = r.strip_prefix("stream:").map(str::trim);
    match body {
        Some("OK") => Verdict::Clean,
        Some(b) if b.ends_with(" FOUND") => {
            let sig = b.trim_end_matches(" FOUND").trim();
            let sig: String = sig.chars().filter(|c| c.is_ascii_graphic() || *c == ' ').take(MAX_SIGNATURE).collect();
            Verdict::Infected(if sig.is_empty() { "unknown".into() } else { sig })
        }
        _ => Verdict::Error(format!("clamd: {}", r.chars().take(200).collect::<String>())),
    }
}

/// `ClamAV 1.4.1/27448/Tue Nov  4 08:31:12 2026`: the database version.
pub fn parse_db_version(r: &str) -> Option<u64> {
    r.split('/').nth(1)?.trim().parse().ok()
}

async fn poll_version(weak: Weak<Scanner>) {
    let mut tick = tokio::time::interval(VERSION_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut warned = false;
    loop {
        tick.tick().await;
        let Some(s) = weak.upgrade() else { return };
        let r = tokio::time::timeout(Duration::from_secs(10), s.version()).await;
        match r {
            Ok(Ok(v)) => {
                crate::metrics::BLOB_AV_UP.set(1);
                if let Some(db) = parse_db_version(&v) {
                    crate::metrics::BLOB_AV_DB_VERSION.set(db as i64);
                    let prev = s.db_version.lock().replace(db);
                    if prev != Some(db) {
                        tracing::info!(clamd = %v, "antivirus: signature database version {db}");
                    }
                }
                warned = false;
            }
            Ok(Err(e)) => {
                crate::metrics::BLOB_AV_UP.set(0);
                if !warned {
                    tracing::warn!(addr = %s.cfg.addr, "antivirus: clamd VERSION failed: {e}");
                    warned = true;
                }
            }
            Err(_) => {
                crate::metrics::BLOB_AV_UP.set(0);
                if !warned {
                    tracing::warn!(addr = %s.cfg.addr, "antivirus: clamd VERSION timed out");
                    warned = true;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies() {
        assert_eq!(parse_reply("stream: OK"), Verdict::Clean);
        assert_eq!(parse_reply("stream: Win.Test.EICAR_HDB-1 FOUND"), Verdict::Infected("Win.Test.EICAR_HDB-1".into()));
        assert!(matches!(parse_reply("INSTREAM size limit exceeded. ERROR"), Verdict::Error(_)));
        assert!(matches!(parse_reply(""), Verdict::Error(_)));
        assert_eq!(parse_db_version("ClamAV 1.4.1/27448/Tue Nov  4 08:31:12 2026"), Some(27448));
        assert_eq!(parse_db_version("ClamAV 1.4.1"), None);
    }

    #[test]
    fn addrs() {
        assert_eq!(Addr::parse("/run/clamav/clamd.ctl").unwrap(), Addr::Unix("/run/clamav/clamd.ctl".into()));
        assert_eq!(Addr::parse("unix:///tmp/c.sock").unwrap(), Addr::Unix("/tmp/c.sock".into()));
        assert_eq!(Addr::parse("tcp://127.0.0.1:3310").unwrap(), Addr::Tcp("127.0.0.1:3310".into()));
        assert!(Addr::parse("tcp://nope").is_err());
        assert!(Addr::parse("").is_err());
    }
}
