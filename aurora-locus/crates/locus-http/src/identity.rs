//! `com.atproto.identity.resolveHandle` and the documents locus serves:
//! `/.well-known/atproto-did` and `/.well-known/did.json`.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use locus_identity::did_doc::{DidDocument, ServiceEntry, VerificationMethod};
use locus_identity::handle;
use locus_store::accounts::Status;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{XrpcError, XrpcResult};
use crate::state::AppState;

type St = State<Arc<AppState>>;

/// The request's host, lowercased, without a port.
pub fn request_host(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::HOST)?.to_str().ok()?;
    let host = if raw.starts_with('[') {
        raw.split_once(']')
            .map_or(raw, |(h, _)| h)
            .trim_start_matches('[')
    } else {
        raw.split(':').next().unwrap_or(raw)
    };
    Some(host.trim_end_matches('.').to_ascii_lowercase())
}

#[derive(Deserialize)]
pub struct ResolveHandleParams {
    handle: String,
}

pub async fn resolve_handle(
    State(s): St,
    Query(p): Query<ResolveHandleParams>,
) -> XrpcResult<Json<Value>> {
    let h = handle::normalize_and_validate(&p.handle)
        .map_err(|e| XrpcError::named("InvalidHandle", e.to_string()))?;
    if let Some(account) = s.store.account_by_handle(&h).await?
        && account.status == Status::Active
    {
        return Ok(Json(json!({ "did": account.did })));
    }
    // A name on our own handle domains with no local account doesn't exist.
    // External resolution (DNS and HTTPS) arrives with the resolver work.
    Err(XrpcError::named(
        "HandleNotFound",
        "Unable to resolve handle",
    ))
}

/// `/.well-known/atproto-did`: the DID of the active account whose handle is
/// the request's host, on one of this PDS's handle domains.
pub async fn well_known_atproto_did(State(s): St, headers: HeaderMap) -> Response {
    let not_found = || (StatusCode::NOT_FOUND, "User not found").into_response();
    let Some(host) = request_host(&headers) else {
        return not_found();
    };
    if handle::service_domain(&host, &s.handle_domains()).is_none() {
        return not_found();
    }
    match s.store.account_by_handle(&host).await {
        Ok(Some(a)) if a.status == Status::Active => {
            ([(header::CONTENT_TYPE, "text/plain")], a.did).into_response()
        }
        Ok(_) => not_found(),
        Err(e) => XrpcError::from(e).into_response(),
    }
}

/// `/.well-known/did.json`: the service DID document on the PDS hostname.
pub async fn well_known_did_json(State(s): St, headers: HeaderMap) -> Response {
    if request_host(&headers).as_deref() != Some(s.hostname.as_str()) {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    }
    let doc = DidDocument {
        id: s.service_did.clone(),
        also_known_as: Vec::new(),
        verification_methods: vec![VerificationMethod {
            fragment: "atproto".into(),
            did_key: s.service_did_key(),
        }],
        services: vec![ServiceEntry {
            fragment: "atproto_pds".into(),
            kind: "AtprotoPersonalDataServer".into(),
            endpoint: s.public_url.clone(),
        }],
    };
    Json(doc.to_json()).into_response()
}

pub async fn health() -> Json<Value> {
    Json(json!({ "version": env!("CARGO_PKG_VERSION") }))
}
