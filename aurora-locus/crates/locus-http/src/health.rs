//! Health, and the proxy's certificate question.
//!
//! - `GET /xrpc/_health`: `{version}` (as the reference), 503 when the
//!   database doesn't answer.
//! - `GET /healthz`: the process is up.
//! - `GET /readyz`: ready for traffic: the database answers, the blob store
//!   takes a write, and setup is done (setup mode answers 503 itself).
//! - `GET /tls-check?domain=`: 200 when the proxy may get a certificate for
//!   the name: the PDS hostname, a hosted did:web host, or an existing
//!   handle; 404 otherwise, so a TLS handshake for any other name fails.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use crate::state::AppState;

type St = State<Arc<AppState>>;

pub async fn health(State(s): St) -> Response {
    match s.store.now_ms().await {
        Ok(_) => Json(json!({ "version": env!("CARGO_PKG_VERSION") })).into_response(),
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "version": env!("CARGO_PKG_VERSION"), "error": format!("the database doesn't answer: {e}") })),
        )
            .into_response(),
    }
}

pub async fn healthz() -> &'static str {
    "ok"
}

pub async fn readyz(State(s): St) -> Response {
    if let Err(e) = s.store.now_ms().await {
        return (StatusCode::SERVICE_UNAVAILABLE, format!("database: {e}")).into_response();
    }
    let key = format!("tmp/readyz-{}", s.node_id);
    let stream = futures::stream::iter([Ok::<_, std::io::Error>(axum::body::Bytes::from_static(
        b"ok",
    ))]);
    if let Err(e) = s.blobs.write_stream(&key, stream, 16).await {
        return (StatusCode::SERVICE_UNAVAILABLE, format!("blob store: {e}")).into_response();
    }
    let _ = s.blobs.delete(&key).await;
    "ready".into_response()
}

#[derive(Deserialize)]
pub struct TlsQuery {
    #[serde(default)]
    domain: String,
}

/// Whether the proxy may get a certificate for `name`.
pub async fn approved(s: &AppState, name: &str) -> bool {
    let name = locus_core::setup::normalize_host(name);
    if name.is_empty() {
        return false;
    }
    if name == s.hostname {
        return true;
    }
    if matches!(s.store.hosted_did_web_by_host(&name).await, Ok(Some(_))) {
        return true;
    }
    matches!(
        s.store.account_by_handle(&name).await,
        Ok(Some(a)) if a.status != locus_store::accounts::Status::Deleted
    )
}

pub async fn tls_check(State(s): St, Query(q): Query<TlsQuery>) -> StatusCode {
    if approved(&s, &q.domain).await {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}
