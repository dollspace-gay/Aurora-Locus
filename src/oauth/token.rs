//! OAuth 2.1 Token Endpoint
//!
//! Implements token issuance with PKCE verification and DPoP token binding:
//! - Authorization code grant (exchange code for tokens)
//! - Refresh token grant (get new access token)
//! - PKCE code_verifier verification (SHA-256)
//! - DPoP proof extraction and verification
//! - Token binding to DPoP thumbprint
//! - Token storage in database
//!
//! Flow (Authorization Code Grant):
//! 1. Client sends POST /oauth/token with code + code_verifier
//! 2. Server validates authorization code
//! 3. Server verifies PKCE: SHA256(code_verifier) == code_challenge
//! 4. Server extracts and verifies DPoP proof from header
//! 5. Server computes JWK thumbprint for token binding
//! 6. Server generates access_token + refresh_token
//! 7. Server stores tokens in database bound to DPoP thumbprint
//! 8. Server marks authorization code as used
//! 9. Server returns tokens with token_type=DPoP
//!
//! References:
//! - https://datatracker.ietf.org/doc/html/draft-ietf-oauth-v2-1-09#section-4.1.3
//! - https://datatracker.ietf.org/doc/html/rfc7636 (PKCE)
//! - https://datatracker.ietf.org/doc/html/rfc9449 (DPoP)

use crate::error::{PdsError, PdsResult};
use crate::oauth::consent::{get_request_by_code, mark_code_as_used};
use crate::oauth::models::{TokenRequest, TokenResponse};
use crate::oauth::token_rotation::TokenRotationManager;
use crate::AppContext;
use axum::{
    extract::State,
    http::HeaderMap,
    response::{IntoResponse, Json},
    Form,
};
use chrono::{Duration, Utc};
use sha2::{Digest, Sha256};
use tracing::{debug, warn};
use uuid::Uuid;

/// Token endpoint handler
///
/// POST /oauth/token
///
/// # Request Body (form-urlencoded)
/// - grant_type: "authorization_code" or "refresh_token"
/// - code: Authorization code (for authorization_code grant)
/// - code_verifier: PKCE verifier (for authorization_code grant)
/// - client_id: OAuth client identifier
/// - redirect_uri: Redirect URI (must match authorization request)
/// - refresh_token: Refresh token (for refresh_token grant)
///
/// # Headers
/// - DPoP: DPoP proof JWT (required for DPoP token binding)
///
/// # Returns
/// - 200 OK with TokenResponse (access_token, refresh_token, etc.)
/// - 400 Bad Request for validation errors
/// - 401 Unauthorized for authentication errors
pub async fn token_endpoint(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Form(request): Form<TokenRequest>,
) -> PdsResult<impl IntoResponse> {
    debug!("Token endpoint request: grant_type={}", request.grant_type);

    // Route to appropriate grant handler
    match request.grant_type.as_str() {
        "authorization_code" => handle_authorization_code_grant(&ctx, headers, request).await,
        "refresh_token" => handle_refresh_token_grant(&ctx, headers, request).await,
        _ => Err(PdsError::Validation(format!(
            "Unsupported grant_type: {}",
            request.grant_type
        ))),
    }
}

/// Handle authorization code grant
///
/// Exchanges authorization code for access + refresh tokens.
/// Performs PKCE verification and DPoP token binding.
async fn handle_authorization_code_grant(
    ctx: &AppContext,
    headers: HeaderMap,
    request: TokenRequest,
) -> PdsResult<Json<TokenResponse>> {
    let start_time = std::time::Instant::now();

    // Step 1: Validate required parameters
    let code = request.code.as_ref().ok_or_else(|| {
        crate::metrics::record_oauth_token_exchange(
            "authorization_code",
            "missing_code",
            start_time.elapsed().as_secs_f64(),
        );
        PdsError::Validation("code is required for authorization_code grant".to_string())
    })?;

    let code_verifier = request.code_verifier.as_ref().ok_or_else(|| {
        crate::metrics::record_oauth_token_exchange(
            "authorization_code",
            "missing_verifier",
            start_time.elapsed().as_secs_f64(),
        );
        PdsError::Validation("code_verifier is required for authorization_code grant".to_string())
    })?;

    let redirect_uri = request.redirect_uri.as_ref().ok_or_else(|| {
        crate::metrics::record_oauth_token_exchange(
            "authorization_code",
            "missing_redirect_uri",
            start_time.elapsed().as_secs_f64(),
        );
        PdsError::Validation("redirect_uri is required for authorization_code grant".to_string())
    })?;

    debug!("Processing authorization code grant: code={}", &code[..8]);

    // Step 2: Get authorization request by code
    let auth_request = get_request_by_code(ctx, code).await?;

    // Step 3: Validate client_id matches
    if auth_request.client_id != request.client_id {
        warn!(
            "Client ID mismatch: expected {}, got {}",
            auth_request.client_id, request.client_id
        );
        crate::metrics::record_oauth_token_exchange(
            "authorization_code",
            "client_mismatch",
            start_time.elapsed().as_secs_f64(),
        );
        return Err(PdsError::Authentication("Invalid client".to_string()));
    }

    // Step 4: Validate redirect_uri matches
    if &auth_request.redirect_uri != redirect_uri {
        warn!(
            "Redirect URI mismatch: expected {}, got {}",
            auth_request.redirect_uri, redirect_uri
        );
        crate::metrics::record_oauth_token_exchange(
            "authorization_code",
            "redirect_uri_mismatch",
            start_time.elapsed().as_secs_f64(),
        );
        return Err(PdsError::Authentication("Invalid redirect_uri".to_string()));
    }

    // Step 5: Verify PKCE code_verifier
    if let Err(e) = verify_pkce_challenge(code_verifier, &auth_request.code_challenge) {
        crate::metrics::record_oauth_pkce_failure("verification_failed");
        crate::metrics::record_oauth_token_exchange(
            "authorization_code",
            "pkce_failed",
            start_time.elapsed().as_secs_f64(),
        );
        return Err(e);
    }

    debug!("✓ PKCE verification successful");

    // Step 6: Extract and verify DPoP proof.
    // Three-state semantics per RFC 9449:
    //   - DPoP header absent           → Bearer token flow (Ok(None))
    //   - DPoP header present + valid  → DPoP-bound token (Ok(Some(thumbprint)))
    //   - DPoP header present + invalid → 400 invalid_dpop_proof
    // The previous shape silently downgraded "present but invalid"
    // to Bearer with a `warn!("allowing for development")`. RFC 9449
    // §5 requires invalid proofs to fail; that fail-open path is
    // gone.
    let dpop_thumbprint =
        extract_and_verify_dpop_proof(&headers, ctx).await.map_err(|e| {
            crate::metrics::record_oauth_dpop_failure("verification_failed");
            warn!("DPoP verification failed: {}", e);
            crate::metrics::record_oauth_token_exchange(
                "authorization_code",
                "dpop_invalid",
                start_time.elapsed().as_secs_f64(),
            );
            e
        })?;
    if let Some(t) = &dpop_thumbprint {
        debug!("✓ DPoP proof verified: thumbprint={}", t);
    }

    // Step 7: Generate access and refresh tokens
    let access_token = generate_token("at");
    let refresh_token = generate_token("rt");

    debug!("Generated tokens for user: {}", auth_request.did);

    // Step 8: Store tokens in database
    let token_id = Uuid::new_v4().to_string();
    let now = Utc::now();
    let _access_expires = now + Duration::hours(1); // 1 hour access token (TODO: store in DB)
    let refresh_expires = now + Duration::days(90); // 90 day refresh token

    // Persist the SHA-256 hash of the bearer, not the bearer itself (β.1 /
    // R1 F-3.1). `validate_oauth_token` looks the bearer up by this hash.
    let access_token_hash = crate::oauth::access_token_hash(&access_token);

    // Determine token type before moving dpop_thumbprint
    let has_dpop = dpop_thumbprint.is_some();

    sqlx::query(
        r#"
        INSERT INTO token (
            token_id, did, client_id, current_refresh_token,
            scope, created_at, updated_at, expires_at,
            dpop_thumbprint, device_id, access_token_hash
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        "#,
    )
    .bind(&token_id)
    .bind(&auth_request.did)
    .bind(&auth_request.client_id)
    .bind(&refresh_token)
    .bind(&auth_request.scope)
    .bind(now.to_rfc3339())
    .bind(now.to_rfc3339())
    .bind(refresh_expires.to_rfc3339())
    .bind(dpop_thumbprint)
    .bind(Option::<String>::None) // device_id (TODO: bind to device)
    .bind(&access_token_hash)
    .execute(&ctx.account_db)
    .await?;

    debug!("Stored token: token_id={}", token_id);

    // Step 9: Mark authorization code as used
    mark_code_as_used(ctx, code).await?;

    // Step 10: Return token response
    let token_type = if has_dpop { "DPoP" } else { "Bearer" };

    // Record successful token exchange metrics
    crate::metrics::record_oauth_token_exchange(
        "authorization_code",
        "success",
        start_time.elapsed().as_secs_f64(),
    );

    Ok(Json(TokenResponse {
        access_token,
        refresh_token,
        token_type: token_type.to_string(),
        expires_in: 3600, // 1 hour in seconds
        scope: auth_request.scope,
    }))
}

/// Handle refresh token grant
///
/// Exchanges refresh token for new access token.
/// Uses TokenRotationManager for secure token rotation.
async fn handle_refresh_token_grant(
    ctx: &AppContext,
    _headers: HeaderMap,
    request: TokenRequest,
) -> PdsResult<Json<TokenResponse>> {
    let start_time = std::time::Instant::now();

    // Step 1: Validate required parameters
    let refresh_token = request.refresh_token.as_ref().ok_or_else(|| {
        crate::metrics::record_oauth_token_exchange(
            "refresh_token",
            "missing_refresh_token",
            start_time.elapsed().as_secs_f64(),
        );
        PdsError::Validation("refresh_token is required for refresh_token grant".to_string())
    })?;

    debug!("Processing refresh token grant");

    // Step 2: Use TokenRotationManager to rotate the token
    let rotation_manager = TokenRotationManager::new(ctx.account_db.clone());
    let result = match rotation_manager
        .rotate_token(refresh_token, &request.client_id)
        .await
    {
        Ok(res) => {
            // Record successful token rotation
            crate::metrics::record_oauth_token_rotation("success");
            crate::metrics::record_oauth_token_exchange(
                "refresh_token",
                "success",
                start_time.elapsed().as_secs_f64(),
            );
            res
        }
        Err(e) => {
            // Record failed token rotation
            crate::metrics::record_oauth_token_rotation("failure");
            crate::metrics::record_oauth_token_exchange(
                "refresh_token",
                "rotation_failed",
                start_time.elapsed().as_secs_f64(),
            );
            return Err(e);
        }
    };

    // Step 3: Return token response
    // TokenRotationManager already returns RotationResult which matches TokenResponse
    Ok(Json(TokenResponse {
        access_token: result.access_token,
        refresh_token: result.refresh_token,
        token_type: result.token_type,
        expires_in: result.expires_in,
        scope: result.scope,
    }))
}

/// Verify PKCE code_verifier against code_challenge
///
/// PKCE verification ensures the client that initiated the authorization
/// is the same client exchanging the code for tokens.
///
/// Verification: SHA256(code_verifier) == code_challenge
///
/// # Arguments
/// * `code_verifier` - The PKCE verifier from client
/// * `code_challenge` - The SHA-256 challenge from authorization request
///
/// # Returns
/// Ok if verification succeeds, error otherwise
fn verify_pkce_challenge(code_verifier: &str, code_challenge: &str) -> PdsResult<()> {
    // Compute SHA-256 hash of code_verifier
    let mut hasher = Sha256::new();
    hasher.update(code_verifier.as_bytes());
    let hash = hasher.finalize();

    // Encode as base64url (no padding)
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    let computed_challenge = URL_SAFE_NO_PAD.encode(hash);

    // Compare with stored challenge
    if computed_challenge != code_challenge {
        warn!(
            "PKCE verification failed: computed={}, expected={}",
            &computed_challenge[..8],
            &code_challenge[..8]
        );
        return Err(PdsError::Authentication(
            "PKCE verification failed".to_string(),
        ));
    }

    Ok(())
}

/// Extract and verify a DPoP proof from request headers, if present.
///
/// Three-state outcome:
///   - `Ok(None)`           — no DPoP header sent; bearer-token flow
///   - `Ok(Some(thumbprint))` — proof present and verified; bind the
///     issued token to this JWK thumbprint
///   - `Err(_)`             — proof present but invalid; caller maps
///     to 400 invalid_dpop_proof
///
/// The verification dispatches to `ctx.dpop_verifier`, which performs
/// signature verification, claim validation (htm/htu/jti replay/exp),
/// and returns the JWK thumbprint per RFC 9449. No `ath` claim is
/// required here — issuance proofs predate the access token.
async fn extract_and_verify_dpop_proof(
    headers: &HeaderMap,
    ctx: &AppContext,
) -> PdsResult<Option<String>> {
    let Some(raw) = headers.get("DPoP") else {
        return Ok(None);
    };
    let dpop_proof = raw
        .to_str()
        .map_err(|_| PdsError::Authentication("Invalid DPoP header value".to_string()))?;
    // Token endpoint URI is the htu claim the proof must commit to.
    // Service URL + canonical /oauth/token path.
    let token_endpoint = format!("{}/oauth/token", ctx.service_url());
    let thumbprint = ctx
        .dpop_verifier
        .verify_dpop_proof(dpop_proof, "POST", &token_endpoint, None)
        .await?;
    Ok(Some(thumbprint))
}

/// Generate a random token
///
/// # Arguments
/// * `prefix` - Token prefix ("at" for access, "rt" for refresh)
///
/// # Returns
/// Token string with format: {prefix}_{uuid}
fn generate_token(prefix: &str) -> String {
    format!("{}_{}", prefix, Uuid::new_v4().to_string().replace("-", ""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pkce_verification() {
        // Test vector from RFC 7636
        let code_verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";

        // Compute challenge
        let mut hasher = Sha256::new();
        hasher.update(code_verifier.as_bytes());
        let hash = hasher.finalize();

        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use base64::Engine;
        let code_challenge = URL_SAFE_NO_PAD.encode(hash);

        // Verify
        let result = verify_pkce_challenge(code_verifier, &code_challenge);
        assert!(result.is_ok(), "PKCE verification should succeed");
    }

    #[test]
    fn test_pkce_verification_fails_wrong_verifier() {
        let code_challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        let wrong_verifier = "wrong_verifier_value";

        let result = verify_pkce_challenge(wrong_verifier, code_challenge);
        assert!(
            result.is_err(),
            "PKCE verification should fail with wrong verifier"
        );
    }

    #[test]
    fn test_generate_token() {
        // Layout: "<prefix>_" + 32-char dash-stripped UUIDv4 = 35 chars total.
        let access_token = generate_token("at");
        assert!(access_token.starts_with("at_"));
        assert_eq!(access_token.len(), 35);

        let refresh_token = generate_token("rt");
        assert!(refresh_token.starts_with("rt_"));
        assert_eq!(refresh_token.len(), 35);
    }

    #[test]
    fn test_token_request_deserialization() {
        let request = TokenRequest {
            grant_type: "authorization_code".to_string(),
            code: Some("test_code".to_string()),
            code_verifier: Some("test_verifier".to_string()),
            client_id: "test_client".to_string(),
            redirect_uri: Some("http://localhost".to_string()),
            refresh_token: None,
        };

        assert_eq!(request.grant_type, "authorization_code");
        assert!(request.code.is_some());
        assert!(request.code_verifier.is_some());
    }
}
