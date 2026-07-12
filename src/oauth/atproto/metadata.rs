//! atproto-OAuth authorization-server metadata (Arc 2 Phase β.3, chainlink
//! #420 / LOCKED design §3.2).
//!
//! Serves the RFC 8414 authorization-server metadata document (with the
//! atproto-OAuth extensions) at `/.well-known/oauth-authorization-server`.
//! A client discovers the provider's endpoints + capabilities by fetching
//! this document. All fields are derived from the service URL + the provider's
//! fixed capabilities, so the document is generated per-request with no stored
//! state. This is the AS counterpart to the existing
//! `/.well-known/oauth-protected-resource` document (`api/well_known.rs`).

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::Response;

use super::scope::AtprotoScope;
use crate::context::AppContext;
use crate::error::PdsError;

/// `GET /.well-known/oauth-authorization-server`
///
/// Publicly fetchable (CORS-permissive, `no-store`), per the spec's
/// discovery-document semantics.
pub async fn authorization_server_metadata(
    State(ctx): State<AppContext>,
) -> Result<Response, PdsError> {
    let issuer = ctx.service_url();
    let scopes_supported: Vec<&str> = AtprotoScope::all().iter().map(|s| s.as_str()).collect();

    let body = serde_json::json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/oauth/atproto/authorize"),
        "token_endpoint": format!("{issuer}/oauth/atproto/token"),
        "pushed_authorization_request_endpoint": format!("{issuer}/oauth/atproto/par"),
        "require_pushed_authorization_requests": true,
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
        "scopes_supported": scopes_supported,
        "dpop_signing_alg_values_supported": ["ES256"],
        "client_id_metadata_document_supported": true,
    });

    let bytes = serde_json::to_vec(&body).map_err(|e| {
        PdsError::Internal(format!(
            "Failed to serialise oauth-authorization-server body: {e}"
        ))
    })?;

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::CACHE_CONTROL, "no-store")
        .body(bytes.into())
        .map_err(|e| {
            PdsError::Internal(format!(
                "Failed to build oauth-authorization-server response: {e}"
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    async fn ctx() -> AppContext {
        crate::api::federation_peers::test_support::create_test_context_with(|_| {}).await
    }

    #[tokio::test]
    async fn metadata_is_well_formed_rfc8414_with_atproto_extensions() {
        let ctx = ctx().await;
        let issuer = ctx.service_url();
        let resp = authorization_server_metadata(State(ctx.clone()))
            .await
            .expect("metadata builds");
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );

        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let doc: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(doc["issuer"], issuer);
        assert_eq!(
            doc["authorization_endpoint"],
            format!("{issuer}/oauth/atproto/authorize")
        );
        assert_eq!(
            doc["pushed_authorization_request_endpoint"],
            format!("{issuer}/oauth/atproto/par")
        );
        assert_eq!(doc["require_pushed_authorization_requests"], true);
        assert_eq!(doc["code_challenge_methods_supported"][0], "S256");
        assert_eq!(doc["token_endpoint_auth_methods_supported"][0], "none");
        assert_eq!(doc["dpop_signing_alg_values_supported"][0], "ES256");
        assert_eq!(doc["client_id_metadata_document_supported"], true);
        // The atproto base scope is advertised.
        let scopes = doc["scopes_supported"].as_array().unwrap();
        assert!(scopes.iter().any(|s| s == "atproto"));
    }
}
