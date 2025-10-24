/// Authentication extractors and utilities
use crate::{
    account::ValidatedSession,
    admin::Role,
    api::middleware::extract_bearer_token,
    context::AppContext,
    error::PdsError,
};
use axum::{
    async_trait,
    extract::FromRequestParts,
    http::request::Parts,
};
use uuid::Uuid;

/// Authenticated context - extracts and validates session from request
#[derive(Debug, Clone)]
pub struct AuthContext {
    pub did: String,
    pub session: ValidatedSession,
}

#[async_trait]
impl FromRequestParts<AppContext> for AuthContext {
    type Rejection = PdsError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppContext,
    ) -> Result<Self, Self::Rejection> {
        // Extract bearer token from Authorization header
        let token = extract_bearer_token(&parts.headers)
            .ok_or_else(|| PdsError::Authentication("Missing authorization header".to_string()))?;

        // Validate token
        let session = state
            .account_manager
            .validate_access_token(&token)
            .await?;

        let did = session.did.clone();

        Ok(AuthContext { did, session })
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
        // Try to extract bearer token
        let token = extract_bearer_token(&parts.headers);

        let auth = if let Some(token) = token {
            // Try to validate
            match state.account_manager.validate_access_token(&token).await {
                Ok(session) => {
                    let did = session.did.clone();
                    Some(AuthContext { did, session })
                }
                Err(_) => None,
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
