/// Authentication and authorization middleware
use crate::{
    account::ValidatedSession,
    context::AppContext,
    error::{PdsError, PdsResult},
    metrics,
};
use axum::{
    extract::{Request, State},
    http::HeaderMap,
    middleware::Next,
    response::Response,
};
use std::time::Instant;
use tracing::{error, info, warn};

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
        .ok_or_else(|| {
            warn!("authentication_failed: missing authorization header");
            metrics::record_error("AuthenticationFailed", "middleware");
            PdsError::Authentication("Missing authorization header".to_string())
        })?;

    match ctx.account_manager.validate_access_token(&token).await {
        Ok(session) => {
            info!(
                did = %session.did,
                is_app_password = session.is_app_password,
                "authentication_successful"
            );
            Ok(session)
        }
        Err(e) => {
            warn!(
                error = %e,
                "authentication_failed: invalid token"
            );
            metrics::record_error("AuthenticationFailed", "middleware");
            Err(e)
        }
    }
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
                // Check if account is taken down (ignore database errors)
                match ctx.moderation_manager.is_taken_down(&session.did).await {
                    Ok(true) => {
                        warn!(
                            did = %session.did,
                            "moderation_blocked: account_taken_down"
                        );
                        return Err(PdsError::AccountTakenDown(
                            "Account has been taken down due to terms of service violations".to_string(),
                        ));
                    }
                    Err(e) => {
                        // Log but don't fail the request if moderation check fails
                        warn!(
                            did = %session.did,
                            error = %e,
                            "moderation_check_failed: is_taken_down"
                        );
                    }
                    Ok(false) => {}
                }

                // Check if account is suspended (ignore database errors)
                match ctx.moderation_manager.is_suspended(&session.did).await {
                    Ok(true) => {
                        warn!(
                            did = %session.did,
                            "moderation_blocked: account_suspended"
                        );
                        return Err(PdsError::AccountSuspended(
                            "Account is currently suspended".to_string(),
                        ));
                    }
                    Err(e) => {
                        // Log but don't fail the request if moderation check fails
                        warn!(
                            did = %session.did,
                            error = %e,
                            "moderation_check_failed: is_suspended"
                        );
                    }
                    Ok(false) => {}
                }
            } else {
                info!(
                    did = %session.did,
                    "admin_access_granted"
                );
            }

            // Add session to request extensions for downstream use
            req.extensions_mut().insert(session);
        }
    }

    Ok(next.run(req).await)
}

/// Request ID for tracing
#[derive(Debug, Clone)]
pub struct RequestId(pub String);

impl RequestId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}

impl Default for RequestId {
    fn default() -> Self {
        Self::new()
    }
}

/// Request logging middleware with request IDs and metrics
///
/// Features:
/// - Assigns unique request ID to each request
/// - Logs request/response with structured data
/// - Tracks request duration
/// - Logs slow requests (>1s)
/// - Records metrics
/// - Supports log sampling for high-volume endpoints
pub async fn request_logging(
    State(_ctx): State<AppContext>,
    mut req: Request,
    next: Next,
) -> Result<Response, PdsError> {
    let request_id = RequestId::new();
    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let start = Instant::now();

    // Add request ID to extensions for downstream access
    req.extensions_mut().insert(request_id.clone());

    // Sample logging for high-volume endpoints
    let should_log = should_log_request(&path);

    if should_log {
        info!(
            request_id = %request_id.0,
            method = %method,
            path = %path,
            "request_started"
        );
    }

    // Process request
    let response = next.run(req).await;
    let duration = start.elapsed();
    let duration_secs = duration.as_secs_f64();
    let status = response.status().as_u16();

    // Record metrics
    metrics::record_http_request(&method, &path, status, duration_secs);

    // Log slow requests (>1 second)
    if duration_secs > 1.0 {
        warn!(
            request_id = %request_id.0,
            method = %method,
            path = %path,
            status = status,
            duration_ms = duration.as_millis(),
            "slow_request"
        );
    } else if should_log {
        info!(
            request_id = %request_id.0,
            method = %method,
            path = %path,
            status = status,
            duration_ms = duration.as_millis(),
            "request_completed"
        );
    }

    Ok(response)
}

/// Determine if request should be logged (sampling for high-volume endpoints)
fn should_log_request(path: &str) -> bool {
    // Always log admin and moderation endpoints
    if path.contains("/admin/") || path.contains("/moderation/") {
        return true;
    }

    // Always log authentication endpoints
    if path.contains("/session") || path.contains("/account") {
        return true;
    }

    // Sample 10% of feed/timeline requests (high volume)
    if path.contains("/feed/") || path.contains("/timeline") {
        return rand::random::<u8>() < 26 // ~10% (26/256)
    }

    // Log everything else
    true
}

/// Database query logging wrapper
///
/// Logs slow queries (>100ms) and records metrics
pub async fn log_db_query<T, F>(
    operation: &str,
    table: &str,
    query: F,
) -> PdsResult<T>
where
    F: std::future::Future<Output = PdsResult<T>>,
{
    let start = Instant::now();
    let result = query.await;
    let duration = start.elapsed();
    let duration_secs = duration.as_secs_f64();

    // Record metrics
    metrics::record_db_query(operation, table, duration_secs);

    // Log slow queries (>100ms)
    if duration.as_millis() > 100 {
        warn!(
            operation = operation,
            table = table,
            duration_ms = duration.as_millis(),
            "slow_query"
        );
    }

    result
}
