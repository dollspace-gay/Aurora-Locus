//! atproto-OAuth Pushed Authorization Request (PAR) endpoint (Arc 2 Phase β.3,
//! chainlink #420 / LOCKED design §3.2 PAR / RFC 9126).
//!
//! `POST /oauth/atproto/par` lets a client push its authorization parameters
//! to the AS over a back channel and receive an opaque `request_uri`, which it
//! then hands to the authorize endpoint in lieu of the individual parameters.
//! atproto OAuth makes PAR mandatory (the AS metadata advertises
//! `require_pushed_authorization_requests: true`).
//!
//! The endpoint is **client-authenticated by DPoP**: the proof demonstrates
//! the client controls its key. The pushed request is persisted with NO holder
//! DID — the DID is bound later, at the authorize step, once the resource
//! owner authenticates their browser session. A short (60s) TTL bounds the
//! window between push and use.

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::Response;
use axum::Form;
use chrono::{Duration, Utc};

use super::params::{self, RawAuthParams};
use super::request_store::{self, AtprotoAuthorizationRequest};
use super::{oauth_error_json, opaque_token, verify_dpop_required};
use crate::context::AppContext;

/// Lifetime of a pushed authorization request before it must be consumed by
/// the authorize endpoint.
const PAR_TTL_SECS: i64 = 60;

/// `POST /oauth/atproto/par`
pub async fn par(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Form(raw): Form<RawAuthParams>,
) -> Response {
    match par_inner(&ctx, &headers, raw).await {
        Ok(resp) => resp,
        Err(e) => e,
    }
}

async fn par_inner(
    ctx: &AppContext,
    headers: &HeaderMap,
    raw: RawAuthParams,
) -> Result<Response, Response> {
    // 1. Client authentication via DPoP. Absent/invalid proof → 401.
    let htu = format!("{}/oauth/atproto/par", ctx.service_url());
    verify_dpop_required(ctx, headers, &htu)
        .await
        .map_err(|e| oauth_error_json(StatusCode::UNAUTHORIZED, "invalid_client", &e.to_string()))?;

    // 2. Validate the pushed parameters against the atproto-OAuth profile.
    let validated = params::validate(&raw).map_err(|e| {
        oauth_error_json(StatusCode::BAD_REQUEST, e.oauth_code(), &e.description())
    })?;

    // 3. Resolve + trust the client by its metadata document.
    let metadata = ctx
        .client_metadata_fetcher
        .fetch(&validated.client_id)
        .await
        .map_err(|e| {
            oauth_error_json(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                &format!("client metadata resolution failed: {e}"),
            )
        })?;

    // 4. The pushed redirect_uri must be registered for this client.
    if !metadata.allows_redirect_uri(&validated.redirect_uri) {
        return Err(oauth_error_json(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "redirect_uri not registered for this client",
        ));
    }

    // 5. Persist the request with a fresh opaque request_uri and NO did yet
    //    (bound at the authorize step once the holder authenticates).
    let now = Utc::now();
    let request_uri = format!("urn:ietf:params:oauth:request_uri:{}", opaque_token());
    let req = AtprotoAuthorizationRequest {
        request_id: opaque_token(),
        request_uri: Some(request_uri.clone()),
        client_id: validated.client_id,
        redirect_uri: validated.redirect_uri,
        scope: validated.scope.to_canonical_string(),
        state: validated.state,
        code_challenge: validated.code_challenge,
        code_challenge_method: "S256".to_string(),
        did: None,
        code_hash: None,
        code_used_at: None,
        denied_at: None,
        created_at: now.to_rfc3339(),
        expires_at: (now + Duration::seconds(PAR_TTL_SECS)).to_rfc3339(),
    };
    request_store::insert(&ctx.account_db, &req)
        .await
        .map_err(|e| {
            oauth_error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                &e.to_string(),
            )
        })?;

    // 6. Return the request_uri + its lifetime (RFC 9126 §2.2).
    let body = serde_json::json!({
        "request_uri": request_uri,
        "expires_in": PAR_TTL_SECS,
    });
    let bytes = serde_json::to_vec(&body).map_err(|e| {
        oauth_error_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            "server_error",
            &e.to_string(),
        )
    })?;
    Ok(Response::builder()
        .status(StatusCode::CREATED)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store")
        .body(bytes.into())
        .expect("static header set builds a valid response"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    async fn ctx() -> AppContext {
        crate::api::federation_peers::test_support::create_test_context_with(|_| {}).await
    }

    fn form(client_id: &str, redirect_uri: &str) -> RawAuthParams {
        RawAuthParams {
            client_id: Some(client_id.to_string()),
            response_type: Some("code".to_string()),
            scope: Some("atproto".to_string()),
            redirect_uri: Some(redirect_uri.to_string()),
            state: Some("st".to_string()),
            code_challenge: Some("chal".to_string()),
            code_challenge_method: Some("S256".to_string()),
            request_uri: None,
        }
    }

    #[tokio::test]
    async fn par_without_dpop_is_401() {
        let ctx = ctx().await;
        let resp = par(
            State(ctx.clone()),
            HeaderMap::new(),
            Form(form(
                "https://app.example.com/client-metadata.json",
                "https://app.example.com/cb",
            )),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let bytes = to_bytes(resp.into_body(), 8192).await.unwrap();
        let doc: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(doc["error"], "invalid_client");
    }
}
