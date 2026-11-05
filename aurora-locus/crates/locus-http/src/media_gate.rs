//! The private media gate: while it's on, a private-only blob (referenced
//! only by private-tier records of its account) is served only to the
//! owner, to audience members of at least one referencing private post, and
//! to operator-trusted services; everyone else gets the same answer as for
//! a missing blob. The gate reads the blob row (`gated`), never the storage
//! location, so it applies from the commit that makes a blob private-only.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::Response;

use crate::error::XrpcResult;
use crate::state::AppState;

/// The gate is on only with a scanner configured: no private media
/// without one.
#[must_use]
pub fn gate_on(s: &AppState) -> bool {
    let snap = s.snapshot();
    snap.bool("kryphocron.private_media_gated") && snap.text("blobs.scanner") != "none"
}

/// Who's asking for gated media.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reader {
    /// A trusted service (service auth from a listed DID).
    Trusted(String),
    /// An account: a local session or OAuth token, or service auth from a
    /// reader on another PDS.
    Account(String),
    Anonymous,
}

/// The reader behind a request's credentials.
pub async fn reader(s: &AppState, headers: &HeaderMap, lxm: &str) -> Reader {
    if headers.get(header::AUTHORIZATION).is_none() {
        return Reader::Anonymous;
    }
    if let Ok(a) = crate::auth::authorization(
        s,
        headers,
        crate::auth::STANDARD,
        &[],
        crate::auth::any_scope,
    )
    .await
    {
        return Reader::Account(a.did);
    }
    match crate::auth::user_service_auth(s, headers, lxm).await {
        Ok(did) => {
            let trusted = s
                .snapshot()
                .list("kryphocron.media_trusted_services")
                .contains(&did.as_str());
            if trusted {
                Reader::Trusted(did)
            } else {
                Reader::Account(did)
            }
        }
        Err(_) => Reader::Anonymous,
    }
}

/// Whether `reader` may have the private-only blob `cid` of `owner`.
pub async fn may_read(
    s: &Arc<AppState>,
    reader: &Reader,
    owner: &str,
    cid: &str,
) -> XrpcResult<bool> {
    let subject = match reader {
        Reader::Trusted(did) => {
            s.kr.audit.record_locus(
                "media.trusted_fetch",
                format!("service={did} did={owner} cid={cid}"),
            );
            return Ok(true);
        }
        Reader::Account(did) if did == owner => return Ok(true),
        Reader::Account(did) => did,
        Reader::Anonymous => return Ok(false),
    };
    // The referencing posts, by audience; any audience that admits the
    // reader (posts batched into one probe, at most 25) admits them.
    let mut by_audience: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for p in s.store.blob_private_posts(owner, cid).await? {
        if let (Some(ad), Some(ar)) = (p.audience_did, p.audience_rkey) {
            by_audience
                .entry(crate::audience::uri(&ad, &ar))
                .or_default()
                .push(format!(
                    "at://{owner}/{}/{}",
                    locus_kryphocron::nsid::POST_PRIVATE,
                    p.rkey
                ));
        }
    }
    for (audience, mut posts) in by_audience {
        posts.truncate(25);
        if crate::membership::is_member(s, subject, &audience, &posts).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// While the gate is on, gated responses and every refusal (missing blobs
/// included, so the two stay alike) are private and never cached.
pub fn no_store(res: &mut Response) {
    let h = res.headers_mut();
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    h.insert(header::VARY, HeaderValue::from_static("Authorization"));
}
