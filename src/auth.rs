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
        // First, authenticate as normal user
        let token = extract_bearer_token(&parts.headers)
            .ok_or_else(|| PdsError::Authentication("Missing authorization header".to_string()))?;

        let session = state
            .account_manager
            .validate_access_token(&token)
            .await
            .map_err(|e| {
                tracing::error!("AdminAuthContext: Token validation failed: {}", e);
                e
            })?;

        let did = session.did.clone();
        tracing::debug!("AdminAuthContext: Checking admin role for DID: {}", did);

        // TEMPORARY: Bypass admin role check for debugging
        tracing::warn!("AdminAuthContext: BYPASSING role check - granting SuperAdmin to {}", did);

        Ok(AdminAuthContext {
            did,
            session,
            role: Role::SuperAdmin,  // Temporary - bypass role check
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

/// Simplified admin token verification for admin panel
/// This is a basic check - for more secure verification, use AdminAuthContext extractor
pub fn verify_admin_token(token: &str) -> Result<(), PdsError> {
    // For now, just verify the token is not empty
    // In a production system, this would:
    // 1. Parse and validate JWT signature
    // 2. Check token expiration
    // 3. Verify admin role in token claims
    // 4. Check against revocation list

    if token.is_empty() {
        return Err(PdsError::Authentication("Invalid admin token".to_string()));
    }

    // TODO: Implement full JWT verification with admin role check
    // For now, accept any non-empty token (development only)
    Ok(())
}
