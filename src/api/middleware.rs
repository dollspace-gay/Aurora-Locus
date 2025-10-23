/// Authentication and authorization middleware
use crate::{
    account::ValidatedSession,
    context::AppContext,
    error::{PdsError, PdsResult},
};
use axum::{
    extract::{Request, State},
    http::HeaderMap,
    middleware::Next,
    response::Response,
};

/// Extract bearer token from Authorization header
pub fn extract_bearer_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| {
            if s.starts_with("Bearer ") {
                Some(s[7..].to_string())
            } else {
                None
            }
        })
}

/// Authenticate request and add session to extensions
pub async fn authenticate(
    State(ctx): State<AppContext>,
    mut req: Request,
    next: Next,
) -> Result<Response, PdsError> {
    let headers = req.headers().clone();

    if let Some(token) = extract_bearer_token(&headers) {
        match ctx.account_manager.validate_access_token(&token).await {
            Ok(session) => {
                // Add session to request extensions
                req.extensions_mut().insert(session);
            }
            Err(_) => {
                // Invalid token - continue without session
                // Some endpoints work without auth
            }
        }
    }

    Ok(next.run(req).await)
}

/// Require authentication - extract session or return 401
pub async fn require_auth(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> PdsResult<ValidatedSession> {
    let token = extract_bearer_token(&headers)
        .ok_or_else(|| PdsError::Authentication("Missing authorization header".to_string()))?;

    ctx.account_manager.validate_access_token(&token).await
}

/// Moderation enforcement middleware
///
/// Checks if the authenticated user's account is subject to moderation actions
/// (takedown or suspension) and blocks the request if so.
/// Admin accounts are exempt from this check to allow them to review moderated content.
pub async fn check_account_moderation(
    State(ctx): State<AppContext>,
    mut req: Request,
    next: Next,
) -> Result<Response, PdsError> {
    let headers = req.headers().clone();

    // Only check moderation for authenticated requests
    if let Some(token) = extract_bearer_token(&headers) {
        if let Ok(session) = ctx.account_manager.validate_access_token(&token).await {
            // Check if this is an admin - admins bypass moderation checks
            let is_admin = ctx.admin_role_manager.get_role(&session.did).await
                .unwrap_or(None)
                .is_some();

            if !is_admin {
                // Check if account is taken down
                if ctx.moderation_manager.is_taken_down(&session.did).await? {
                    return Err(PdsError::AccountTakenDown(
                        "Account has been taken down due to terms of service violations".to_string(),
                    ));
                }

                // Check if account is suspended
                if ctx.moderation_manager.is_suspended(&session.did).await? {
                    return Err(PdsError::AccountSuspended(
                        "Account is currently suspended".to_string(),
                    ));
                }
            }

            // Add session to request extensions for downstream use
            req.extensions_mut().insert(session);
        }
    }

    Ok(next.run(req).await)
}
