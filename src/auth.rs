/// Authentication extractors and utilities
use crate::{
    account::ValidatedSession,
    admin::Role,
    api::middleware::extract_bearer_token,
    context::AppContext,
    error::PdsError,
    oauth::ScopeSet,
};
use axum::{
    async_trait,
    extract::FromRequestParts,
    http::request::Parts,
};
use uuid::Uuid;

/// Authentication method used for the request
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMethod {
    /// OAuth 2.1 token (modern)
    OAuth,
    /// Legacy JWT session token
    JWT,
}

/// Authenticated context - extracts and validates session from request
#[derive(Debug, Clone)]
pub struct AuthContext {
    pub did: String,
    pub session: ValidatedSession,
    pub auth_method: AuthMethod,
}

#[async_trait]
impl FromRequestParts<AppContext> for AuthContext {
    type Rejection = PdsError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppContext,
    ) -> Result<Self, Self::Rejection> {
        use std::time::Instant;

        // Extract bearer token from Authorization header
        let token = extract_bearer_token(&parts.headers)
            .ok_or_else(|| PdsError::Authentication("Missing authorization header".to_string()))?;

        let start = Instant::now();

        // Try OAuth validation first (modern standard)
        match validate_oauth_token(state, &token).await {
            Ok(oauth_token) => {
                let duration = start.elapsed().as_secs_f64();

                // Create session from OAuth token
                let session = ValidatedSession {
                    did: oauth_token.did.clone(),
                    session_id: oauth_token.token_id.clone(),
                    is_app_password: false,
                };

                // Record metrics
                crate::metrics::record_oauth_token_exchange("validation", "success", duration);

                // Store auth method in extensions for middleware
                parts.extensions.insert(AuthMethod::OAuth);

                Ok(AuthContext {
                    did: oauth_token.did,
                    session,
                    auth_method: AuthMethod::OAuth,
                })
            }
            Err(_) => {
                // Fallback to JWT validation for backward compatibility
                let session = state
                    .account_manager
                    .validate_access_token(&token)
                    .await?;

                let did = session.did.clone();
                let duration = start.elapsed().as_secs_f64();

                // Record metrics (JWT fallback)
                crate::metrics::record_oauth_token_exchange("jwt_fallback", "success", duration);

                // Store auth method in extensions for middleware
                parts.extensions.insert(AuthMethod::JWT);

                Ok(AuthContext {
                    did,
                    session,
                    auth_method: AuthMethod::JWT,
                })
            }
        }
    }
}

/// Optional authenticated context - does not fail if no auth provided
#[derive(Debug, Clone)]
pub struct OptionalAuthContext {
    pub auth: Option<AuthContext>,
}

#[async_trait]
impl FromRequestParts<AppContext> for OptionalAuthContext {
    type Rejection = PdsError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppContext,
    ) -> Result<Self, Self::Rejection> {
        use std::time::Instant;

        // Try to extract bearer token
        let token = extract_bearer_token(&parts.headers);

        let auth = if let Some(token) = token {
            let start = Instant::now();

            // Try OAuth validation first
            match validate_oauth_token(state, &token).await {
                Ok(oauth_token) => {
                    let duration = start.elapsed().as_secs_f64();

                    // Create session from OAuth token
                    let session = ValidatedSession {
                        did: oauth_token.did.clone(),
                        session_id: oauth_token.token_id.clone(),
                        is_app_password: false,
                    };

                    // Record metrics
                    crate::metrics::record_oauth_token_exchange("validation_optional", "success", duration);

                    // Store auth method in extensions for middleware
                    parts.extensions.insert(AuthMethod::OAuth);

                    Some(AuthContext {
                        did: oauth_token.did,
                        session,
                        auth_method: AuthMethod::OAuth,
                    })
                }
                Err(_) => {
                    // Fallback to JWT validation
                    match state.account_manager.validate_access_token(&token).await {
                        Ok(session) => {
                            let did = session.did.clone();
                            let duration = start.elapsed().as_secs_f64();

                            // Record metrics (JWT fallback)
                            crate::metrics::record_oauth_token_exchange("jwt_fallback_optional", "success", duration);

                            // Store auth method in extensions for middleware
                            parts.extensions.insert(AuthMethod::JWT);

                            Some(AuthContext {
                                did,
                                session,
                                auth_method: AuthMethod::JWT,
                            })
                        }
                        Err(_) => None,
                    }
                }
            }
        } else {
            None
        };

        Ok(OptionalAuthContext { auth })
    }
}

/// Admin authentication context - requires admin role
#[derive(Debug, Clone)]
pub struct AdminAuthContext {
    pub did: String,
    pub session: ValidatedSession,
    pub role: Role,
}

#[async_trait]
impl FromRequestParts<AppContext> for AdminAuthContext {
    type Rejection = PdsError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppContext,
    ) -> Result<Self, Self::Rejection> {
        // Extract bearer token
        let token = extract_bearer_token(&parts.headers)
            .ok_or_else(|| PdsError::Authentication("Missing authorization header".to_string()))?;

        // Try to validate as session token first
        let (did, session) = match state.account_manager.validate_access_token(&token).await {
            Ok(session) => {
                let did = session.did.clone();
                (did, session)
            }
            Err(_) => {
                // Session validation failed, try JWT validation for admin-only tokens
                tracing::debug!("AdminAuthContext: Session validation failed, trying JWT validation");

                let token_data = verify_jwt_token(&token, &state.config.authentication.jwt_secret)?;

                // Extract DID from JWT claims
                let claims = &token_data.claims;
                let did = claims.get("sub")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| PdsError::Authentication("Invalid JWT: missing 'sub' claim".to_string()))?
                    .to_string();

                // Check scope is admin
                let scope = claims.get("scope")
                    .and_then(|v| v.as_str());
                if scope != Some("admin") {
                    return Err(PdsError::Authentication("JWT token does not have admin scope".to_string()));
                }

                tracing::info!("AdminAuthContext: JWT validation successful for DID: {}", did);

                // Create a synthetic session for admin JWT tokens
                let session = ValidatedSession {
                    did: did.clone(),
                    session_id: format!("jwt-{}", Uuid::new_v4()),
                    is_app_password: false,
                };

                (did, session)
            }
        };

        tracing::debug!("AdminAuthContext: Checking admin role for DID: {}", did);

        // Check if DID is in configured admin DIDs list
        let is_configured_admin = state.config.authentication.admin_dids.contains(&did);

        // Try to get role from database
        let role = if let Some(admin_role) = state.admin_role_manager.get_role(&did).await? {
            // User has a role in the database
            tracing::info!("AdminAuthContext: User {} has role {} from database", did, admin_role.role.as_str());
            admin_role.role
        } else if is_configured_admin {
            // User is in configured admin DIDs, grant SuperAdmin
            tracing::info!("AdminAuthContext: User {} is configured admin, granting SuperAdmin", did);
            Role::SuperAdmin
        } else {
            // User is not an admin
            tracing::warn!("AdminAuthContext: User {} is not an admin", did);
            return Err(PdsError::Authorization(
                "Admin role required".to_string()
            ));
        };

        Ok(AdminAuthContext {
            did,
            session,
            role,
        })
    }
}

/// Macro to require specific admin role
/// Usage: require_admin_role!(auth, Role::SuperAdmin)?;
#[macro_export]
macro_rules! require_admin_role {
    ($auth:expr, $required:expr) => {
        if !$auth.role.can_act_as($required) {
            return Err($crate::error::PdsError::Authorization(format!(
                "Requires {} role or higher",
                $required.as_str()
            )));
        }
    };
}

/// Verify a JWT token with full validation
///
/// This performs:
/// 1. JWT signature verification
/// 2. Expiration checking
/// 3. Claims validation
pub fn verify_jwt_token(token: &str, jwt_secret: &str) -> Result<jsonwebtoken::TokenData<serde_json::Value>, PdsError> {
    use jsonwebtoken::{decode, DecodingKey, Validation, Algorithm};

    let decoding_key = DecodingKey::from_secret(jwt_secret.as_bytes());
    let mut validation = Validation::new(Algorithm::HS256);
    // Allow some clock skew (5 minutes)
    validation.leeway = 300;

    decode::<serde_json::Value>(token, &decoding_key, &validation)
        .map_err(|e| {
            tracing::warn!("JWT verification failed: {}", e);
            match e.kind() {
                jsonwebtoken::errors::ErrorKind::ExpiredSignature => {
                    PdsError::Authentication("Token has expired".to_string())
                }
                jsonwebtoken::errors::ErrorKind::InvalidSignature => {
                    PdsError::Authentication("Invalid token signature".to_string())
                }
                _ => PdsError::Authentication(format!("Invalid token: {}", e))
            }
        })
}

/// Simplified admin token verification for admin panel
/// This is a basic check - for more secure verification, use AdminAuthContext extractor
pub fn verify_admin_token(token: &str, jwt_secret: &str) -> Result<(), PdsError> {
    // Perform full JWT verification
    verify_jwt_token(token, jwt_secret)?;

    // Token is valid
    Ok(())
}

// ========== OAuth 2.1 + DPoP Authentication ==========

/// OAuth token information
///
/// Represents a validated OAuth access token with scopes and DPoP binding.
#[derive(Debug, Clone)]
pub struct OAuthToken {
    /// Account DID
    pub did: String,

    /// Token ID
    pub token_id: String,

    /// OAuth client ID
    pub client_id: String,

    /// Granted scopes (space-separated)
    pub scope: String,

    /// DPoP thumbprint (if token is DPoP-bound)
    pub dpop_thumbprint: Option<String>,

    /// Device ID (if token is device-bound)
    pub device_id: Option<String>,
}

/// OAuth authenticated context with scope enforcement
///
/// Extracts and validates OAuth access tokens from Authorization header.
/// Supports DPoP token binding for enhanced security.
///
/// # Usage
/// ```ignore
/// async fn handler(auth: OAuthAuthContext) -> Result<Json<Response>, PdsError> {
///     // auth.did - authenticated user's DID
///     // auth.scopes - parsed OAuth scopes
///     // auth.token - full token information
///
///     // Check scope manually
///     require_scope(&auth.token.scope, &AtProtoScope::RepoCreate)?;
///
///     // ... handler logic
/// }
/// ```
#[derive(Debug, Clone)]
pub struct OAuthAuthContext {
    pub did: String,
    pub token: OAuthToken,
    pub scopes: ScopeSet,
}

#[async_trait]
impl FromRequestParts<AppContext> for OAuthAuthContext {
    type Rejection = PdsError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppContext,
    ) -> Result<Self, Self::Rejection> {
        // Extract bearer token from Authorization header
        let access_token = extract_bearer_token(&parts.headers)
            .ok_or_else(|| PdsError::Authentication("Missing authorization header".to_string()))?;

        // TODO: Validate DPoP proof if present
        // For now, we'll just validate the access token
        // DPoP validation will be added in the next step

        // Try to find OAuth token in database
        let token_info = validate_oauth_token(state, &access_token).await?;

        // Parse scopes
        let scopes = ScopeSet::from_str(&token_info.scope)
            .map_err(|e| PdsError::Authentication(format!("Invalid token scopes: {}", e)))?;

        let did = token_info.did.clone();

        Ok(OAuthAuthContext {
            did,
            token: token_info,
            scopes,
        })
    }
}

/// Validate OAuth access token
///
/// Looks up the token in the database and returns token information.
/// This is a helper function used by OAuthAuthContext and middleware.
pub async fn validate_oauth_token(
    ctx: &AppContext,
    access_token: &str,
) -> Result<OAuthToken, PdsError> {
    // Query token table for this access token
    // Note: In the actual implementation, access tokens should be stored hashed
    // For now, we'll do a direct lookup

    let row = sqlx::query(
        r#"
        SELECT token_id, did, client_id, scope, dpop_thumbprint, device_id, expires_at
        FROM token
        WHERE token_id = ?
        "#,
    )
    .bind(access_token)
    .fetch_optional(&ctx.account_db)
    .await
    .map_err(|e| PdsError::Database(e))?
    .ok_or_else(|| PdsError::Authentication("Invalid or expired access token".to_string()))?;

    // Check if token is expired
    use sqlx::Row;
    let expires_at: chrono::DateTime<chrono::Utc> = row.get("expires_at");

    if expires_at < chrono::Utc::now() {
        return Err(PdsError::Authentication("Access token has expired".to_string()));
    }

    Ok(OAuthToken {
        token_id: row.get("token_id"),
        did: row.get("did"),
        client_id: row.get("client_id"),
        scope: row.get("scope"),
        dpop_thumbprint: row.get("dpop_thumbprint"),
        device_id: row.get("device_id"),
    })
}

/// Extract DPoP header from request
///
/// DPoP proof is sent in the "DPoP" HTTP header (not Authorization).
pub fn extract_dpop_header(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get("dpop")
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string())
}

/// Validate DPoP proof
///
/// Verifies that:
/// 1. DPoP proof JWT is well-formed and signed correctly
/// 2. JWK thumbprint matches the token's bound thumbprint
/// 3. htm (HTTP method) and htu (HTTP URI) match the request
/// 4. Proof is not expired and not reused (via jti)
///
/// This will be fully implemented when we integrate DPoP validation.
pub async fn validate_dpop_proof(
    _ctx: &AppContext,
    _dpop_proof: &str,
    _expected_thumbprint: &str,
    _http_method: &str,
    _http_uri: &str,
) -> Result<(), PdsError> {
    // TODO: Implement full DPoP proof validation
    // For now, we'll skip DPoP validation and mark it as a future task

    // Steps:
    // 1. Parse DPoP proof JWT
    // 2. Extract JWK from proof header
    // 3. Compute JWK thumbprint
    // 4. Verify thumbprint matches expected_thumbprint
    // 5. Verify proof signature using JWK
    // 6. Verify htm and htu claims
    // 7. Verify jti is unique (replay prevention)
    // 8. Verify proof is not expired

    Ok(())
}
