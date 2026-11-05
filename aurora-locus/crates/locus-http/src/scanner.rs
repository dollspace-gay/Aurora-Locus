//! The blob scanning hook: every upload is scanned in temp storage before
//! it can be used, when a scanner is configured. A match is refused,
//! quarantined (kept, never collected) and queued for review; matches can be
//! false positives, so an operator decides what happens next.
//!
//! The scanner is a generic HTTP webhook, reached with the
//! operator-configured client (a scanner on a private network works): it's
//! POSTed the bytes, with `content-type` and `x-locus-did`/`x-locus-cid`, and
//! answers `{"result": "allow" | "reject" | "quarantine", "reason"?}`.

use std::time::Duration;

use serde_json::Value;

use crate::state::AppState;

/// What a scan came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scan {
    /// No scanner configured.
    NotConfigured,
    Allowed,
    /// `reject` or `quarantine`.
    Matched {
        result: String,
        reason: Option<String>,
    },
    /// The scanner couldn't be reached or gave no usable answer.
    Unavailable(String),
}

/// How long one scan may take.
const SCAN_TIMEOUT: Duration = Duration::from_secs(60);

/// Scans the bytes at `storage_key`.
pub async fn scan_key(s: &AppState, did: &str, cid: &str, mime: &str, storage_key: &str) -> Scan {
    let snap = s.snapshot();
    if snap.text("blobs.scanner") == "none" {
        return Scan::NotConfigured;
    }
    let url = snap.text("blobs.scanner_url").to_owned();
    if url.is_empty() {
        return Scan::Unavailable("no scanner URL".into());
    }
    let file = match s.blobs.open(storage_key).await {
        Ok(Some(f)) => f,
        Ok(None) => return Scan::Unavailable("the bytes are missing".into()),
        Err(e) => return Scan::Unavailable(e.to_string()),
    };
    let Ok(client) = crate::pipethrough::client(s, false) else {
        return Scan::Unavailable("no client".into());
    };
    let body = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(file));
    let res = client
        .post(&url)
        .header(reqwest::header::CONTENT_TYPE, mime)
        .header("x-locus-did", did)
        .header("x-locus-cid", cid)
        .body(body)
        .timeout(SCAN_TIMEOUT)
        .send()
        .await;
    let res = match res {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => return Scan::Unavailable(format!("the scanner answered {}", r.status())),
        Err(e) => return Scan::Unavailable(e.to_string()),
    };
    let v: Value = match res.json().await {
        Ok(v) => v,
        Err(_) => return Scan::Unavailable("the scanner's answer isn't JSON".into()),
    };
    match v["result"].as_str() {
        Some("allow") => Scan::Allowed,
        Some(r @ ("reject" | "quarantine")) => Scan::Matched {
            result: r.to_owned(),
            reason: v["reason"].as_str().map(str::to_owned),
        },
        _ => Scan::Unavailable("the scanner's answer has no result".into()),
    }
}

/// A match: into the review queue, the audit log, and an operator alert.
pub async fn record_match(
    s: &AppState,
    did: &str,
    cid: &str,
    source: &str,
    result: &str,
    reason: Option<&str>,
) -> locus_core::CoreResult<()> {
    let id = s.store.add_review(did, cid, source, result, reason).await?;
    s.kr.audit.record_locus(
        "scan.match",
        format!("review={id} did={did} cid={cid} result={result}"),
    );
    tracing::error!(
        review = id,
        did,
        cid,
        result,
        "the scanner matched a blob; it's quarantined for review"
    );
    Ok(())
}

/// Scans blobs stored unscanned while the scanner was down. Stops at the
/// first scan the scanner can't answer. Returns how many it scanned.
pub async fn rescan_pass(s: &AppState) -> locus_core::CoreResult<u64> {
    let mut done = 0;
    for b in s.store.unscanned_blobs(100).await? {
        match scan_key(s, &b.did, &b.cid, &b.mime_type, &b.storage_key).await {
            Scan::Allowed => s.store.set_scan_status(&b.did, &b.cid, "allowed").await?,
            Scan::Matched { result, reason } => {
                s.store.quarantine_match(&b.did, &b.cid).await?;
                record_match(s, &b.did, &b.cid, "rescan", &result, reason.as_deref()).await?;
            }
            Scan::NotConfigured | Scan::Unavailable(_) => break,
        }
        done += 1;
    }
    Ok(done)
}
