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

/// Unified authentication context (Phase 4)
///
/// Supports both local authentication (via session tokens) and
/// cross-PDS authentication (via service auth JWTs).
#[derive(Debug, Clone)]
pub enum UnifiedAuthContext {
    /// Local user authenticated via session token
    Local(ValidatedSession),

    /// Cross-PDS user authenticated via service auth JWT
    CrossPDS { did: String },
}

impl UnifiedAuthContext {
    /// Get the authenticated DID
    pub fn did(&self) -> &str {
        match self {
            UnifiedAuthContext::Local(session) => &session.did,
            UnifiedAuthContext::CrossPDS { did } => did,
        }
    }

    /// Check if this is local authentication
    pub fn is_local(&self) -> bool {
        matches!(self, UnifiedAuthContext::Local(_))
    }

    /// Check if this is cross-PDS authentication
    pub fn is_cross_pds(&self) -> bool {
        matches!(self, UnifiedAuthContext::CrossPDS { .. })
    }
}

/// Require authentication (local or cross-PDS) - Phase 4
///
/// This tries local authentication first, then falls back to service auth.
/// Use this for endpoints that should accept both local and cross-PDS requests.
pub async fn require_auth_unified(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> PdsResult<UnifiedAuthContext> {
    let token = extract_bearer_token(&headers)
        .ok_or_else(|| {
            warn!("authentication_failed: missing authorization header");
            metrics::record_error("AuthenticationFailed", "middleware");
            PdsError::Authentication("Missing authorization header".to_string())
        })?;

    // Try local auth first
    match ctx.account_manager.validate_access_token(&token).await {
        Ok(session) => {
            info!(
                did = %session.did,
                is_app_password = session.is_app_password,
                auth_type = "local",
                "authentication_successful"
            );
            return Ok(UnifiedAuthContext::Local(session));
        }
        Err(_) => {
            // Local auth failed, try service auth (cross-PDS)
        }
    }

    // Try service auth (cross-PDS)
    if let Some(service_auth) = &ctx.federation_auth {
        let service_did = ctx.service_did();

        match service_auth.authenticator.verify_service_jwt(&token, service_did).await {
            Ok(claims) => {
                // Verify and consume nonce
                if let Some(nonce_store) = &ctx.nonce_store {
                    match nonce_store.check_and_record(&claims.jti).await {
                        Ok(true) => {
                            info!(
                                did = %claims.iss,
                                auth_type = "cross_pds",
                                "authentication_successful"
                            );
                            return Ok(UnifiedAuthContext::CrossPDS { did: claims.iss });
                        }
                        Ok(false) => {
                            warn!(
                                jti = %claims.jti,
                                "service_auth_failed: replay_attack"
                            );
                            metrics::record_error("ServiceAuthReplayAttack", "middleware");
                            return Err(PdsError::Authentication("Replay attack detected".to_string()));
                        }
                        Err(e) => {
                            error!(
                                error = %e,
                                "service_auth_failed: nonce_check_error"
                            );
                        }
                    }
                }

                // If nonce store is not available, allow but log warning
                warn!("service_auth: nonce_store_not_available, replay_prevention_disabled");
                return Ok(UnifiedAuthContext::CrossPDS { did: claims.iss });
            }
            Err(e) => {
                warn!(
                    error = %e,
                    "service_auth_failed"
                );
            }
        }
    }

    // Both auth methods failed
    warn!("authentication_failed: invalid_token");
    metrics::record_error("AuthenticationFailed", "middleware");
    Err(PdsError::Authentication("Invalid or expired token".to_string()))
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

// ========== Phase 4: Cross-PDS Service Authentication ==========

/// Service authentication context for cross-PDS requests
///
/// This represents an authenticated request from another PDS on behalf of a user.
/// Unlike ValidatedSession (local auth), this is verified via DID-based JWT signatures.
#[derive(Debug, Clone)]
pub struct ServiceAuthContext {
    /// The user's DID (from JWT iss claim)
    pub did: String,

    /// The JWT ID (nonce) for replay prevention
    pub jti: String,

    /// Optional endpoint identifier (from JWT lxm claim)
    pub endpoint: Option<String>,

    /// Timestamp when the JWT was issued
    pub issued_at: i64,

    /// Timestamp when the JWT expires
    pub expires_at: i64,
}

/// Extract and verify service auth JWT from another PDS
///
/// This middleware:
/// 1. Extracts Bearer token from Authorization header
/// 2. Tries local auth first (ValidatedSession)
/// 3. Falls back to service auth if local auth fails
/// 4. Verifies JWT cryptographically via DID resolution
/// 5. Checks nonce for replay prevention
/// 6. Adds ServiceAuthContext to request extensions
///
/// Rate limiting for cross-PDS requests should be stricter (handled separately).
pub async fn service_auth(
    State(ctx): State<AppContext>,
    mut req: Request,
    next: Next,
) -> Result<Response, PdsError> {
    let headers = req.headers().clone();

    // Check if this is a service auth request
    if let Some(token) = extract_bearer_token(&headers) {
        // Try local auth first
        if ctx.account_manager.validate_access_token(&token).await.is_ok() {
            // Local auth succeeded - continue with normal flow
            return Ok(next.run(req).await);
        }

        // Local auth failed - try service auth (cross-PDS)
        if let Some(service_auth) = &ctx.federation_auth {
            let service_did = ctx.service_did();

            // Verify JWT with this PDS's DID as audience
            match service_auth.authenticator.verify_service_jwt(&token, service_did).await {
                Ok(claims) => {
                    // JWT is valid - check nonce for replay prevention
                    if let Some(nonce_store) = &ctx.nonce_store {
                        match nonce_store.check_and_record(&claims.jti).await {
                            Ok(true) => {
                                // Nonce is new - request is valid
                                let service_context = ServiceAuthContext {
                                    did: claims.iss.clone(),
                                    jti: claims.jti.clone(),
                                    endpoint: claims.lxm.clone(),
                                    issued_at: claims.iat,
                                    expires_at: claims.exp,
                                };

                                // Log cross-PDS request
                                info!(
                                    user_did = %claims.iss,
                                    jti = %claims.jti,
                                    endpoint = ?claims.lxm,
                                    "service_auth_successful: cross-PDS request"
                                );

                                // Add to request extensions
                                req.extensions_mut().insert(service_context);

                                // TODO: Apply stricter rate limiting for cross-PDS requests
                                // This should be ~10x stricter than local requests

                                return Ok(next.run(req).await);
                            }
                            Ok(false) => {
                                // Replay attack detected
                                warn!(
                                    jti = %claims.jti,
                                    user_did = %claims.iss,
                                    "service_auth_failed: replay_attack_detected"
                                );
                                metrics::record_error("ServiceAuthReplayAttack", "middleware");
                                return Err(PdsError::Authentication(
                                    "Replay attack detected".to_string(),
                                ));
                            }
                            Err(e) => {
                                error!(
                                    error = %e,
                                    "service_auth_failed: nonce_check_error"
                                );
                                // Continue without service auth on nonce store error
                            }
                        }
                    } else {
                        // No nonce store configured - allow but log warning
                        warn!("service_auth: nonce_store_not_configured");

                        let service_context = ServiceAuthContext {
                            did: claims.iss.clone(),
                            jti: claims.jti.clone(),
                            endpoint: claims.lxm.clone(),
                            issued_at: claims.iat,
                            expires_at: claims.exp,
                        };

                        req.extensions_mut().insert(service_context);
                        return Ok(next.run(req).await);
                    }
                }
                Err(e) => {
                    // Service auth failed
                    warn!(
                        error = %e,
                        "service_auth_failed: jwt_verification_failed"
                    );
                    // Continue without auth - some endpoints don't require it
                }
            }
        }
    }

    // No auth or auth failed - continue anyway (some endpoints are public)
    Ok(next.run(req).await)
}

/// Require service auth - extract ServiceAuthContext or return 401
///
/// Use this extractor in route handlers that require cross-PDS authentication.
pub async fn require_service_auth(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> PdsResult<ServiceAuthContext> {
    let token = extract_bearer_token(&headers)
        .ok_or_else(|| {
            warn!("service_auth_failed: missing_authorization_header");
            metrics::record_error("ServiceAuthMissing", "middleware");
            PdsError::Authentication("Missing authorization header".to_string())
        })?;

    let service_auth = ctx.federation_auth.as_ref()
        .ok_or_else(|| {
            warn!("service_auth_failed: federation_disabled");
            PdsError::Authentication("Federation not enabled".to_string())
        })?;

    let service_did = ctx.service_did();

    // Verify JWT
    let claims = service_auth.authenticator.verify_service_jwt(&token, service_did).await?;

    // Check nonce if nonce store is configured
    if let Some(nonce_store) = &ctx.nonce_store {
        let is_new = nonce_store.check_and_record(&claims.jti).await?;

        if !is_new {
            warn!(
                jti = %claims.jti,
                user_did = %claims.iss,
                "service_auth_failed: replay_attack"
            );
            metrics::record_error("ServiceAuthReplayAttack", "middleware");
            return Err(PdsError::Authentication("Replay attack detected".to_string()));
        }
    }

    info!(
        user_did = %claims.iss,
        jti = %claims.jti,
        "service_auth_required: authenticated"
    );

    Ok(ServiceAuthContext {
        did: claims.iss,
        jti: claims.jti,
        endpoint: claims.lxm,
        issued_at: claims.iat,
        expires_at: claims.exp,
    })
}
