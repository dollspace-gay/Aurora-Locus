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
            .await?;

        let did = session.did.clone();

        // Check admin role
        let admin_role = state
            .admin_role_manager
            .get_role(&did)
            .await?
            .ok_or_else(|| PdsError::Authorization("Admin access required".to_string()))?;

        Ok(AdminAuthContext {
            did,
            session,
            role: admin_role.role,
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
