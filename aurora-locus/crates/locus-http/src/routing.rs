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
