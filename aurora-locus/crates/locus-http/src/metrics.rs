//! `/metrics` (Prometheus) and the per-request XRPC metrics. Readable from
//! the allowed addresses (loopback by default; the real client IP, so
//! through a trusted proxy too) or with the bearer token.

use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Request, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use locus_core::metrics;
use subtle::ConstantTimeEq;

use crate::state::AppState;

/// Records each XRPC request's count and latency by NSID.
pub async fn record(req: Request<Body>, next: Next) -> Response {
    let Some(nsid) = req
        .uri()
        .path()
        .strip_prefix("/xrpc/")
        .filter(|n| *n != "_health")
        .map(|n| {
            if plausible_nsid(n) {
                n.to_owned()
            } else {
                "invalid".to_owned()
            }
        })
    else {
        return next.run(req).await;
    };
    let start = Instant::now();
    let res = next.run(req).await;
    let status = res.status().as_u16().to_string();
    metrics::inc(
        "locus_http_requests_total",
        &[("nsid", &nsid), ("status", &status)],
        1,
    );
    metrics::observe(
        "locus_http_request_seconds",
        &[("nsid", &nsid)],
        start.elapsed().as_secs_f64(),
    );
    res
}

fn plausible_nsid(n: &str) -> bool {
    n.len() <= 317
        && n.split('.').count() >= 3
        && n.split('.').all(|p| !p.is_empty())
        && n.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
}

fn allowed(s: &AppState, headers: &HeaderMap) -> bool {
    let snap = s.snapshot();
    if let Some(token) = &s.secrets.metrics_token
        && headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .is_some_and(|t| bool::from(t.trim().as_bytes().ct_eq(token.as_slice())))
    {
        return true;
    }
    crate::ratelimit::client_ip().is_some_and(|ip| {
        snap.list("metrics.allowed_ips")
            .iter()
            .any(|c| crate::ratelimit::cidr_contains(c, ip))
    })
}

/// GET `/metrics`.
pub async fn serve(State(s): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if !s.snapshot().bool("metrics.enabled") {
        return StatusCode::NOT_FOUND.into_response();
    }
    if !allowed(&s, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let (open, idle) = s.store.pool_stats();
    #[allow(clippy::cast_precision_loss)]
    {
        metrics::set(
            "locus_db_pool_connections",
            &[("state", "idle")],
            idle as f64,
        );
        metrics::set(
            "locus_db_pool_connections",
            &[("state", "busy")],
            f64::from(open) - idle as f64,
        );
    }
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        metrics::render(),
    )
        .into_response()
}
