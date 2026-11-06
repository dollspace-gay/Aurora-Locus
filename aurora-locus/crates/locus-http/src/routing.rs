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
    if path == "/xrpc/com.atproto.repo.uploadBlob" || path == "/xrpc/com.atproto.repo.importRepo" {
        return next.run(req).await;
    }
    let snap = s.snapshot();
    let limit = usize::try_from(if REPO_WRITES.contains(&path) {
        snap.int("limits.repo_write_body")
    } else if path == crate::branding::UPLOAD_PATH {
        crate::branding::MAX_UPLOAD_BODY
    } else if path == crate::keys::IMPORT_PATH {
        crate::keys::MAX_IMPORT_BODY
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

/// The method and URI of the request being handled (what DPoP proofs are
/// checked against).
#[derive(Debug, Clone)]
pub struct RequestInfo {
    pub method: axum::http::Method,
    pub uri: axum::http::Uri,
}

tokio::task_local! {
    pub static REQUEST: RequestInfo;
}

/// Routes whose work is bounded by their own caps, not the request
/// deadline (streams and uploads).
fn streaming(path: &str) -> bool {
    matches!(
        path,
        "/xrpc/com.atproto.repo.uploadBlob"
            | "/xrpc/com.atproto.repo.importRepo"
            | "/xrpc/com.atproto.sync.getRepo"
            | "/xrpc/com.atproto.sync.subscribeRepos"
            | "/xrpc/com.atproto.sync.getBlob"
    ) || path.starts_with("/blob/")
}

/// CORS as the reference's `cors({ maxAge: 1 day })` (any origin;
/// preflights answered with the requested headers), `Cache-Control:
/// private` and `Vary: Authorization` on authenticated responses, and the
/// request deadline (`http.request_deadline_ms`; a request that outlasts
/// it gets a retryable 503).
pub async fn headers_and_deadline(
    State(s): State<Arc<AppState>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    use axum::http::{HeaderValue, Method, header};
    if req.method() == Method::OPTIONS && req.headers().contains_key(header::ORIGIN) {
        let mut res = StatusCode::NO_CONTENT.into_response();
        let h = res.headers_mut();
        h.insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static("*"),
        );
        h.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET,HEAD,PUT,PATCH,POST,DELETE"),
        );
        if let Some(asked) = req.headers().get(header::ACCESS_CONTROL_REQUEST_HEADERS) {
            h.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, asked.clone());
            h.append(
                header::VARY,
                HeaderValue::from_static("Access-Control-Request-Headers"),
            );
        }
        h.insert(
            header::ACCESS_CONTROL_MAX_AGE,
            HeaderValue::from_static("86400"),
        );
        return res;
    }
    let authed = req.headers().contains_key(header::AUTHORIZATION);
    let dpop = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.len() > 5 && v[..5].eq_ignore_ascii_case("dpop "));
    let path = req.uri().path().to_owned();
    let info = RequestInfo {
        method: req.method().clone(),
        uri: req.uri().clone(),
    };
    let mut res = if streaming(&path) {
        REQUEST.scope(info, next.run(req)).await
    } else {
        let ms = u64::try_from(s.snapshot().int("http.request_deadline_ms")).unwrap_or(60_000);
        match tokio::time::timeout(
            std::time::Duration::from_millis(ms),
            REQUEST.scope(info, next.run(req)),
        )
        .await
        {
            Ok(r) => r,
            Err(_) => {
                tracing::warn!(path, "request passed its deadline");
                let mut r = crate::error::XrpcError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "RequestTimeout",
                    "The request took too long; try again",
                )
                .into_response();
                r.headers_mut()
                    .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
                r
            }
        }
    };
    if dpop {
        // A fresh nonce on every OAuth-authenticated response (RFC 9449 §8.2).
        crate::oauth::add_nonce(&s, res.headers_mut());
    }
    let h = res.headers_mut();
    h.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    if authed {
        if !h.contains_key(header::CACHE_CONTROL) {
            h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private"));
        }
        h.append(header::VARY, HeaderValue::from_static("Authorization"));
    }
    res
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
