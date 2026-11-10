# Antivirus scanning for uploadBlob

Status: implemented (`src/antivirus.rs`, `src/xrpc/blobs.rs`, tests in
`tests/all/antivirus.rs`), off unless `--av-socket` is set. Inspired by
[decidim-module-antivirus](https://github.com/mainio/decidim-module-antivirus)
(ClamAV via clamd, run as an upload validation). We copy the pattern, not the
code (it is AGPL Ruby), and fix its main weakness: when clamd is missing it
silently skips the scan.

## Goal

Every blob a user uploads through `com.atproto.repo.uploadBlob` gets scanned by
ClamAV **before** it is stored under `blob/{did}/{cid}`. Infected uploads are
refused, and a moderation case is opened with the blob taken down under it. This covers every client upload path
in PARA (avatars, banners, link icons, Live Now images, onboarding images). It
also covers any future document or PDF attachments on the Documents screen,
because they will go through the same call.

Out of scope: Matrix chat attachments. They go to the Matrix media repo, not
vlpds. matrix-content-scanner is the separate fix there.

## Where it goes

`src/xrpc/blobs.rs`, `upload_blob`:

```
receive(body)  ──►  checks { takedown?, imported?, ratelimit }  ──►  blob_quota::commit(.., up.commit(cid))
     │                         ▲
     └── tee chunks to clamd ──┘  verdict awaited here, before commit
```

- **Stream while receiving.** `Upload::receive` already sees every chunk to
  hash it. A new `Scan` handle opens a clamd `zINSTREAM` connection and is fed
  each chunk alongside the hasher. That way an upload of 8 MB or more (`PART_SIZE`,
  which goes to `blob-tmp/` multipart) never has to be read back. clamd only
  scans after it receives the zero-length terminator, so the added latency is
  roughly the scan time, not the upload time.
- **The verdict is awaited inside the `checks` block**, next to the takedown check.
  On a refusal the existing `up.abort()` path cleans up `blob-tmp/`, and nothing
  ever lands under `blob/`.
- **Verdict cache keyed by sha256.** Blobs are content-addressed, so a clean
  verdict is cached (in memory, 100k entries per node) as `digest → clamd DB
  version`. A re-upload of the same bytes (common during migration or retries)
  drops the stream before its terminator, so clamd never scans it. A new
  signature database invalidates the cache. Until the node has learned the DB
  version (its `VERSION` poll, every 60 s), nothing is cached. Infected
  verdicts are recorded as a blob takedown, so the existing
  `is_blob_takendown` check refuses the same bytes cheaply from then on,
  before any scan.

## Verdicts

| clamd reply | Normal upload | Account moving in (`imported` blob) |
|---|---|---|
| `stream: OK` | store | store |
| `stream: <sig> FOUND` | 400 `BlobRejected`, bytes not kept. A case (actor `antivirus`, status `actioned`) + blob takedown under ref `av:<sig>`, both audited. | Store directly into `blob-quarantine/` (never served) + case. A refusal would break migration of a repo that references the blob. |
| error / timeout / `size limit exceeded` | depends on `--av-fail-mode` (below) | store + `vlpds_blob_av_total{result="skipped"}` |

`--av-fail-mode closed` (default) refuses with 503 `ScanUnavailable`, so the
client can retry. `open` stores the blob anyway and counts it as skipped. Either
way, a failure is counted in metrics and alerted on; it is never silent. If
`--av-socket` is unset, scanning is off (startup logs say when it is on, and
`vlpds.admin.getConfig` shows the flags).

## New config (`src/main.rs`)

- `--av-socket` / `VLPDS_AV_SOCKET`: clamd unix socket (e.g. `/run/clamav/clamd.ctl`)
  or `tcp://host:3310`. Unset means scanning is off.
- `--av-fail-mode` / `VLPDS_AV_FAIL_MODE`: `closed` | `open`.
- `--av-timeout-secs` / `VLPDS_AV_TIMEOUT_SECS`: default 30. It bounds each wait
  on clamd separately (permit, connect, every write, the final answer), not the
  upload, so a slow client is never cut off by it.
- `--av-max-concurrency` / `VLPDS_AV_MAX_CONCURRENCY`: default 8. A semaphore, so a
  burst of uploads can't overrun clamd's `MaxThreads`.

clamd has to be configured with `StreamMaxLength >= --max-blob-mb` (default
100M; clamd's own default is 25M). Otherwise large uploads get
`INSTREAM size limit exceeded`. The node polls `VERSION` every 60 s and logs
a warning when it fails.

## Deployment (`deploy/ansible`, roles/vlpds)

- `vlpds_antivirus: true` adds a `clamav` service (the official
  `clamav/clamav-debian` image, which is multi-arch; `clamav/clamav` is amd64
  only) to the node's compose project. It sits on the vlpds Docker network at
  `tcp://clamav:3310` (not published), with signatures in the `clamav-db`
  volume. Options are set through the image's `CLAMD_CONF_*` variables
  (`StreamMaxLength`/`MaxFileSize` from `vlpds_max_blob_mb`, `MaxThreads` from
  `vlpds_antivirus_max_threads`) on top of its own clamd.conf. Mounting a
  whole config breaks its logging and healthcheck socket.
- vlpds does not wait for it. While clamd loads (30-60 s), uploads get a retryable 503
  under fail-closed. A PDS shouldn't go down because its scanner is.
- Memory: clamd keeps signatures resident, about **1.2–1.6 GB RAM**, and about 2×
  during a reload (`ConcurrentDatabaseReload yes`). `vlpds_antivirus_mem_limit_mb`
  (default 3584) comes off `vlpds_mem_limit_mb`'s default, and an assert checks that
  both fit on the host. This is the main real cost.

## Observability

- `vlpds_blob_av_total{result=clean|cached|infected|error|refused|skipped}`
  (`refused`/`skipped`: how an `error` was handled)
- `vlpds_blob_av_scan_seconds` histogram
- `vlpds_blob_av_up`, `vlpds_blob_av_db_version` gauges (from the VERSION poll)
- Alerts in `ops/alerts.yml` (group `vlpds-antivirus`), each with a RUNBOOK
  entry: `VlpdsAntivirusDown` (page), `VlpdsAntivirusErrors`,
  `VlpdsAntivirusSignaturesStale` (no DB version change in 3 days),
  `VlpdsMalwareUploaded`.

## Tests

- `src/antivirus.rs` unit tests: reply and address parsing.
- `tests/all/antivirus.rs`, against a fake clamd (`tokio::net::UnixListener`
  speaking INSTREAM/VERSION). Clean → stored. Same bytes → cached (no second
  scan). Infected → 400, no bytes, case + takedown, and a retry is refused
  before any scan. Infected over `PART_SIZE` (multipart tee). clamd error →
  503 under closed; no clamd at all → stored under open. A repo moving in →
  the infected blob is quarantined, nothing is missing, and an operator can
  still fetch it.
- `real_clamd` (ignored unless `VLPDS_TEST_CLAMD` names a clamd): EICAR refused
  with clamd's signature name in the case, a clean PNG and a 99 MiB blob
  stored. It passed against `clamav/clamav-debian:1.4` run with the role's
  `CLAMD_CONF_*` settings (about 970 MB resident, 99 MiB upload and scan in
  about 1.4 s on an M-series Mac).
- Not covered: the Ansible role itself (no ansible run here); the compose
  file was checked by hand.

## Phase 2 (not in this change)

- Rescan existing blobs after signature updates (a sweep like `sweep_quarantine`,
  rate-limited, newest first).
- MIME allow-list for document uploads once the Documents screen accepts files.
  ClamAV mostly catches known malware, so a type allow-list is the other half of
  the defence for PDFs and Office files.

## Size (estimate before implementing)

About 300 lines for a new `src/xrpc/antivirus.rs` (clamd client, cache,
semaphore), about 40 lines changed in `blobs.rs`, plus flags, metrics, an Ansible
role, alerts and a runbook entry. Roughly 2–3 days including CI clamd.
