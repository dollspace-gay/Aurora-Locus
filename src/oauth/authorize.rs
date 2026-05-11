// Allow dead_code - OAuth authorize features for future use
#![allow(dead_code)]

//! OAuth 2.1 Authorization Endpoint
//!
//! Implements the authorization code flow per OAuth 2.1 and ATProto spec:
//! - PKCE (Proof Key for Code Exchange) with S256 challenge method
//! - State parameter for CSRF protection
//! - Client validation and redirect URI checking
//! - Authorization request storage with expiration
//!
//! Flow:
//! 1. Client initiates: GET /oauth/authorize with parameters
//! 2. Server validates parameters and client
//! 3. If not authenticated, redirect to login
//! 4. Store authorization request (pending consent)
//! 5. Redirect to consent screen
//! 6. User grants/denies
//! 7. On grant, generate authorization code
//! 8. Redirect back to client with code + state
//!
//! References:
//! - https://datatracker.ietf.org/doc/html/draft-ietf-oauth-v2-1-09
//! - https://atproto.com/specs/oauth

use crate::distributed::{DistributedError, Lease};
use crate::error::{PdsError, PdsResult};
use crate::oauth::flow_state_adapter::{decode_request, encode_request_data};
use crate::oauth::models::{AuthorizationRequest, AuthorizationRequestData, AuthorizeQuery};
use crate::AppContext;
use axum::{
    extract::{Query, State},
    response::{IntoResponse, Redirect},
};
use chrono::Duration;
use tracing::{debug, warn};
use uuid::Uuid;


/// Map `DistributedError` to `PdsError` for the OAuth-handler
/// surface. The trait's KeyExists / UnsupportedTable variants
/// shouldn't surface on the happy path through these handlers
/// (the adapter routes only on `oauth_flow_state`, and OAuth
/// request_ids are fresh UUIDs so KeyExists is structurally
/// impossible), so we map them to Internal errors with an
/// explicit "bug" prefix for log triage.
fn map_distributed_err(err: DistributedError) -> PdsError {
    match err {
        DistributedError::Database(e) => PdsError::Database(e),
        other => PdsError::Internal(format!("distributed-store bug: {}", other)),
    }
}

/// Pull the distributed-store handle off AppContext. Panics
/// (via the calling `?`) if the store is missing — that's a
/// startup-config bug, not a request-time condition; the
/// Step-1 context wiring guarantees the registry is always
/// constructed (Some) even in SingleInstanceInmemory mode.
fn require_store(
    ctx: &AppContext,
) -> PdsResult<&std::sync::Arc<dyn crate::distributed::DistributedStore>> {
    ctx.distributed_store
        .as_ref()
        .ok_or_else(|| {
            PdsError::Internal(
                "distributed_store not constructed; AppContext::new \
                 contract violation"
                    .to_string(),
            )
        })
}

/// Authorization endpoint handler
///
/// GET /oauth/authorize
///
/// # Query Parameters
/// - response_type: Must be 'code' (authorization code flow)
/// - client_id: OAuth client identifier
/// - redirect_uri: Where to redirect after authorization
/// - scope: Requested permissions (space-separated)
/// - code_challenge: PKCE challenge (SHA-256 hash)
/// - code_challenge_method: PKCE method (must be 'S256')
/// - state: CSRF protection token (optional but recommended)
///
/// # Returns
/// - 302 Redirect to login (if not authenticated)
/// - 302 Redirect to consent screen (if authenticated)
/// - 400 Bad Request (invalid parameters)
pub async fn authorize(
    State(ctx): State<AppContext>,
    Query(query): Query<AuthorizeQuery>,
) -> PdsResult<impl IntoResponse> {
    let start_time = std::time::Instant::now();
    let client_id = query.client_id.clone();

    debug!(
        "OAuth authorization request: client_id={}, scope={}",
        query.client_id, query.scope
    );

    // Step 1: Validate response_type (must be 'code')
    if query.response_type != "code" {
        warn!("Invalid response_type: {}", query.response_type);
        crate::metrics::record_oauth_authorization(
            &client_id,
            "invalid_response_type",
            start_time.elapsed().as_secs_f64(),
        );
        return Err(PdsError::Validation(format!(
            "Unsupported response_type: {} (expected 'code')",
            query.response_type
        )));
    }

    // Step 2: Validate PKCE challenge method (must be 'S256' per OAuth 2.1)
    if query.code_challenge_method != "S256" {
        warn!(
            "Invalid code_challenge_method: {}",
            query.code_challenge_method
        );
        crate::metrics::record_oauth_authorization(
            &client_id,
            "invalid_challenge_method",
            start_time.elapsed().as_secs_f64(),
        );
        crate::metrics::record_oauth_pkce_failure("invalid_method");
        return Err(PdsError::Validation(format!(
            "Unsupported code_challenge_method: {} (expected 'S256')",
            query.code_challenge_method
        )));
    }

    // Step 3: Validate code_challenge (must not be empty)
    if query.code_challenge.is_empty() {
        crate::metrics::record_oauth_authorization(
            &client_id,
            "missing_challenge",
            start_time.elapsed().as_secs_f64(),
        );
        crate::metrics::record_oauth_pkce_failure("missing_challenge");
        return Err(PdsError::Validation(
            "code_challenge is required".to_string(),
        ));
    }

    // Step 4: Validate client_id (must not be empty)
    if query.client_id.is_empty() {
        crate::metrics::record_oauth_authorization(
            "unknown",
            "missing_client_id",
            start_time.elapsed().as_secs_f64(),
        );
        return Err(PdsError::Validation("client_id is required".to_string()));
    }

    // Step 5: Validate redirect_uri (must not be empty and must be valid URL)
    if query.redirect_uri.is_empty() {
        crate::metrics::record_oauth_authorization(
            &client_id,
            "missing_redirect_uri",
            start_time.elapsed().as_secs_f64(),
        );
        return Err(PdsError::Validation("redirect_uri is required".to_string()));
    }

    // Validate redirect_uri is a valid URL
    if let Err(e) = url::Url::parse(&query.redirect_uri) {
        warn!(
            "Invalid redirect_uri: {} - error: {}",
            query.redirect_uri, e
        );
        crate::metrics::record_oauth_authorization(
            &client_id,
            "invalid_redirect_uri",
            start_time.elapsed().as_secs_f64(),
        );
        return Err(PdsError::Validation(format!(
            "Invalid redirect_uri: {}",
            query.redirect_uri
        )));
    }

    // Step 6: Validate scope (must not be empty)
    if query.scope.is_empty() {
        crate::metrics::record_oauth_authorization(
            &client_id,
            "missing_scope",
            start_time.elapsed().as_secs_f64(),
        );
        return Err(PdsError::Validation("scope is required".to_string()));
    }

    // Step 7: Validate client (check if client_id is registered)
    // TODO: Implement client registry validation in Phase 2 Task 4
    // For now, we accept all clients (will be restricted later)

    // Step 8: Validate redirect_uri matches client registration
    // TODO: Implement redirect_uri whitelist checking in Phase 2 Task 4
    // For now, we accept any redirect_uri (will be restricted later)

    // Step 9: Check if user is authenticated
    // TODO: Extract user DID from session/auth context
    // For now, we'll need to redirect to login page

    // TEMPORARY: For development, we'll use a test DID
    // In production, this should come from the authenticated session
    let user_did = "did:plc:test123"; // TODO: Get from auth context

    // Step 10: Create authorization request
    let request_data = AuthorizationRequestData {
        did: user_did.to_string(),
        client_id: query.client_id.clone(),
        code_challenge: query.code_challenge.clone(),
        code_challenge_method: query.code_challenge_method.clone(),
        scope: query.scope.clone(),
        redirect_uri: query.redirect_uri.clone(),
        state: query.state.clone(),
    };

    // Step 11: Store authorization request in database
    let request_id = create_authorization_request(&ctx, request_data).await?;

    debug!(
        "Created authorization request: request_id={}, did={}",
        request_id, user_did
    );

    // Record successful authorization metrics
    crate::metrics::record_oauth_authorization(
        &query.client_id,
        "success",
        start_time.elapsed().as_secs_f64(),
    );

    // Record scope grants
    for scope in query.scope.split_whitespace() {
        crate::metrics::record_oauth_scope_grant(scope, true);
    }

    // Step 12: Redirect to consent screen
    // Pass request_id so consent screen can retrieve the request details
    let consent_url = format!("/oauth/consent?request_id={}", request_id);

    Ok(Redirect::to(&consent_url))
}

/// Create an authorization request in the database
///
/// Stores the authorization request with a 10-minute expiration.
///
/// # Arguments
/// * `ctx` - Application context with database pool
/// * `data` - Authorization request data
///
/// # Returns
/// The unique request_id (UUID)
async fn create_authorization_request(
    ctx: &AppContext,
    data: AuthorizationRequestData,
) -> PdsResult<String> {
    let request_id = Uuid::new_v4().to_string();
    let lease = Lease::from_now(Duration::minutes(10));
    let value = encode_request_data(&data);
    require_store(ctx)?
        .insert("oauth_flow_state", &request_id, &value, Some(lease))
        .await
        .map_err(map_distributed_err)?;
    Ok(request_id)
}

/// Get authorization request by request_id
///
/// # Arguments
/// * `ctx` - Application context
/// * `request_id` - Unique request identifier
///
/// # Returns
/// Authorization request if found and not expired
pub async fn get_authorization_request(
    ctx: &AppContext,
    request_id: &str,
) -> PdsResult<AuthorizationRequest> {
    let bytes = require_store(ctx)?
        .get("oauth_flow_state", request_id)
        .await
        .map_err(map_distributed_err)?
        .ok_or_else(|| {
            // The adapter's `get` filters out lease-expired and
            // already-consumed rows as `None`. Distinguishing
            // "never existed", "expired", and "consumed" would
            // require a richer adapter return type; the
            // pre-Arc-7 path collapsed expired into a separate
            // 401 ("Authorization request expired") while
            // not-found was 404. Step 2 collapses to a single
            // NotFound — the consent screen treats expiry and
            // never-existed identically (operator restarts the
            // flow either way) and the lookup site doesn't
            // benefit from the distinction.
            PdsError::NotFound(format!("Authorization request not found: {}", request_id))
        })?;
    decode_request(&bytes).map_err(|e| {
        PdsError::Internal(format!("authorization_request decode failed: {}", e))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_authorize_query_validation() {
        // Test that AuthorizeQuery can be deserialized
        let query = AuthorizeQuery {
            response_type: "code".to_string(),
            client_id: "test_client".to_string(),
            redirect_uri: "http://localhost:3000/callback".to_string(),
            scope: "atproto".to_string(),
            code_challenge: "test_challenge".to_string(),
            code_challenge_method: "S256".to_string(),
            state: Some("test_state".to_string()),
        };

        assert_eq!(query.response_type, "code");
        assert_eq!(query.client_id, "test_client");
        assert!(query.state.is_some());
    }
}
