//! Routing by `Host`:
//! - the PDS hostname, and internal names (`localhost`, IP literals,
//!   single-label names like a compose service name, `http.internal_hosts`),
//!   get every route; internal names act only under the PDS's identity and
//!   are not a trust boundary;
//! - handle hosts and hosted did:web hosts get only
//!   `/.well-known/atproto-did` and `/.well-known/did.json`;
//! - the health endpoints answer on every host;
//! - any other name gets 404.
//!
//! So the console, OAuth pages and account portal are never reachable on a
//! name a user chose.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use locus_identity::handle;

use crate::identity::request_host;
use crate::state::AppState;

/// Paths that answer on every host.
const ANY_HOST: &[&str] = &["/xrpc/_health", "/healthz", "/readyz", "/tls-check"];

/// Paths a handle host or hosted did:web host gets.
const IDENTITY_HOST: &[&str] = &["/.well-known/atproto-did", "/.well-known/did.json"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostClass {
    Full,
    Identity,
    Unknown,
}

/// Whether a name is internal: `localhost` (or `*.localhost`), an IP
/// literal, or a single label.
#[must_use]
pub fn is_internal_name(host: &str) -> bool {
    host == "localhost"
        || host.ends_with(".localhost")
        || host.parse::<std::net::IpAddr>().is_ok()
        || !host.contains('.')
}

pub async fn classify(s: &AppState, host: Option<&str>) -> HostClass {
    let Some(host) = host else {
        // No Host header (not HTTP/1.1): nothing to derive an identity from.
        return HostClass::Full;
    };
    if host == s.hostname || is_internal_name(host) {
        return HostClass::Full;
    }
    let snap = s.snapshot();
    if snap
        .list("http.internal_hosts")
        .iter()
        .any(|h| h.eq_ignore_ascii_case(host))
    {
        return HostClass::Full;
    }
    if handle::service_domain(host, &s.handle_domains()).is_some() {
        return HostClass::Identity;
    }
    match s.store.hosted_did_web_by_host(host).await {
        Ok(Some(_)) => HostClass::Identity,
        Ok(None) => HostClass::Unknown,
        Err(e) => {
            tracing::warn!("host lookup failed: {e}");
            HostClass::Unknown
        }
    }
}

pub async fn by_host(State(s): State<Arc<AppState>>, req: Request<Body>, next: Next) -> Response {
    let path = req.uri().path();
    if ANY_HOST.contains(&path) {
        return next.run(req).await;
    }
    let host = request_host(req.headers());
    match classify(&s, host.as_deref()).await {
        HostClass::Full => next.run(req).await,
        HostClass::Identity if IDENTITY_HOST.contains(&path) => next.run(req).await,
        HostClass::Identity | HostClass::Unknown => {
            (StatusCode::NOT_FOUND, "Not found").into_response()
        }
    }
}

/// Paths whose bodies are record writes (the larger limit).
const REPO_WRITES: &[&str] = &[
    "/xrpc/com.atproto.repo.createRecord",
    "/xrpc/com.atproto.repo.putRecord",
    "/xrpc/com.atproto.repo.deleteRecord",
    "/xrpc/com.atproto.repo.applyWrites",
];

/// Request body limits, live from settings: record writes get
/// `limits.repo_write_body`, uploads their own limit (checked while
/// streaming), everything else `limits.json_body`. A declared length over
/// the limit is refused before reading; a chunked body is cut off at it.
pub async fn limit_body(
    State(s): State<Arc<AppState>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let path = req.uri().path();
    if path == "/xrpc/com.atproto.repo.uploadBlob" {
        return next.run(req).await;
    }
    let snap = s.snapshot();
    let limit = usize::try_from(if REPO_WRITES.contains(&path) {
        snap.int("limits.repo_write_body")
    } else {
        snap.int("limits.json_body")
    })
    .unwrap_or(usize::MAX);
    let declared = req
        .headers()
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok());
    if declared.is_some_and(|n| n > limit) {
        return too_large();
    }
    let (parts, body) = req.into_parts();
    let req = Request::from_parts(parts, Body::new(http_body_util::Limited::new(body, limit)));
    let res = next.run(req).await;
    // A cut-off body surfaces as a 413 from the extractor; give it the
    // XRPC shape.
    if res.status() == StatusCode::PAYLOAD_TOO_LARGE {
        return too_large();
    }
    res
}

fn too_large() -> Response {
    crate::error::XrpcError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        "PayloadTooLarge",
        "request entity too large",
    )
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::is_internal_name;

    #[test]
    fn internal_names() {
        for h in ["localhost", "pds.localhost", "127.0.0.1", "::1", "locus"] {
            assert!(is_internal_name(h), "{h}");
        }
        for h in ["pds.example.com", "alice.example.com"] {
            assert!(!is_internal_name(h), "{h}");
        }
    }
}
