// Allow dead_code for middleware - many auth/logging helpers are defined for future routes
#![allow(dead_code)]

//! Authentication and authorization middleware

use crate::{
    account::ValidatedSession,
    context::AppContext,
    error::{PdsError, PdsResult},
    metrics,
    oauth::AtProtoScope,
};
use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::time::Instant;
use tracing::{error, info, warn};

/// Extract bearer token from Authorization header
pub fn extract_bearer_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer ").map(|t| t.to_string()))
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
    let token = extract_bearer_token(&headers).ok_or_else(|| {
        warn!("authentication_failed: missing authorization header");
        metrics::record_error("AuthenticationFailed", "middleware");
        PdsError::Authentication("Missing authorization header".to_string())
    })?;

    match ctx.account_manager.validate_access_token(&token).await {
        Ok(session) => {
            info!(
                did = %session.did,
                session_id = %session.session_id,
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

    /// OAuth 2.1 authenticated user with scopes
    OAuth { did: String, scope: String },
}

impl UnifiedAuthContext {
    /// Get the authenticated DID
    pub fn did(&self) -> &str {
        match self {
            UnifiedAuthContext::Local(session) => &session.did,
            UnifiedAuthContext::CrossPDS { did } => did,
            UnifiedAuthContext::OAuth { did, .. } => did,
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

    /// Check if this is OAuth authentication
    pub fn is_oauth(&self) -> bool {
        matches!(self, UnifiedAuthContext::OAuth { .. })
    }

    /// Get OAuth scope if this is OAuth authentication
    pub fn oauth_scope(&self) -> Option<&str> {
        match self {
            UnifiedAuthContext::OAuth { scope, .. } => Some(scope),
            _ => None,
        }
    }
}

/// Require authentication — Arc 12 §5.3.3 tuple-routed, §5.3.4
/// non-forwarded variant. Thin wrapper around
/// `AppContext::verify_jwt_with_allowlist` (§5.3.4.1 shared helper);
/// audience allowlist is `[ctx.service_did()]`.
pub async fn require_auth_unified(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> PdsResult<UnifiedAuthContext> {
    let token = extract_bearer_token(&headers).ok_or_else(|| {
        warn!("authentication_failed: missing authorization header");
        metrics::record_error("AuthenticationFailed", "middleware");
        PdsError::Authentication("Missing authorization header".to_string())
    })?;
    let service_did = ctx.service_did().to_string();
    ctx.verify_jwt_with_allowlist(&token, &[service_did.as_str()])
        .await
}

/// Arc 12 §5.3.4 forwarded variant — thin wrapper around the same
/// `verify_jwt_with_allowlist` helper with the multi-audience
/// allowlist that forwarded routes need (`[service_did, entryway_did]`
/// when entryway is configured; `[service_did]` only in standalone
/// mode, in which case forwarded routes degrade to the same
/// allowlist as the non-forwarded variant — see §5.3.8 standalone
/// passthrough).
pub async fn require_auth_forwarded(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> PdsResult<UnifiedAuthContext> {
    let token = extract_bearer_token(&headers).ok_or_else(|| {
        warn!("authentication_failed: missing authorization header");
        metrics::record_error("AuthenticationFailed", "middleware");
        PdsError::Authentication("Missing authorization header".to_string())
    })?;
    let service_did = ctx.service_did().to_string();
    let entryway_did = ctx.entryway_did().map(str::to_string);
    let mut allowlist: Vec<&str> = vec![service_did.as_str()];
    if let Some(eid) = entryway_did.as_deref() {
        allowlist.push(eid);
    }
    ctx.verify_jwt_with_allowlist(&token, &allowlist).await
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
            let is_admin = ctx
                .admin_role_manager
                .get_role(&session.did)
                .await
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
                            "Account has been taken down due to terms of service violations"
                                .to_string(),
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
        return rand::random::<u8>() < 26; // ~10% (26/256)
    }

    // Log everything else
    true
}

/// Database query logging wrapper
///
/// Logs slow queries (>100ms) and records metrics
pub async fn log_db_query<T, F>(operation: &str, table: &str, query: F) -> PdsResult<T>
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
        if ctx
            .account_manager
            .validate_access_token(&token)
            .await
            .is_ok()
        {
            // Local auth succeeded - continue with normal flow
            return Ok(next.run(req).await);
        }

        // Local auth failed - try service auth (cross-PDS)
        if let Some(service_auth) = &ctx.federation_auth {
            let service_did = ctx.service_did();

            // Verify JWT with this PDS's DID as audience
            match service_auth
                .authenticator
                .verify_service_jwt(&token, service_did)
                .await
            {
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
    let token = extract_bearer_token(&headers).ok_or_else(|| {
        warn!("service_auth_failed: missing_authorization_header");
        metrics::record_error("ServiceAuthMissing", "middleware");
        PdsError::Authentication("Missing authorization header".to_string())
    })?;

    let service_auth = ctx.federation_auth.as_ref().ok_or_else(|| {
        warn!("service_auth_failed: federation_disabled");
        PdsError::Authentication("Federation not enabled".to_string())
    })?;

    let service_did = ctx.service_did();

    // Verify JWT
    let claims = service_auth
        .authenticator
        .verify_service_jwt(&token, service_did)
        .await?;

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
            return Err(PdsError::Authentication(
                "Replay attack detected".to_string(),
            ));
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

/// Enforce OAuth scope requirement on UnifiedAuthContext
///
/// For OAuth authentication, this enforces that the required scope is present.
/// For other authentication types (local session, cross-PDS), this check is skipped.
///
/// # Arguments
/// * `auth` - The unified authentication context
/// * `required_scope` - The scope required for this operation
///
/// # Returns
/// Ok(()) if authorized, Err if scope is missing
///
/// # Example
/// ```ignore
/// let auth = middleware::require_auth_unified(State(ctx), headers).await?;
/// enforce_scope(&auth, &AtProtoScope::RepoCreate)?;
/// ```
pub fn enforce_scope(auth: &UnifiedAuthContext, required_scope: &AtProtoScope) -> PdsResult<()> {
    match auth {
        UnifiedAuthContext::OAuth { scope, .. } => {
            // For OAuth, enforce scope check
            crate::oauth::require_scope(scope, required_scope)
        }
        UnifiedAuthContext::Local(_) | UnifiedAuthContext::CrossPDS { .. } => {
            // Legacy authentication types have implicit full access
            Ok(())
        }
    }
}

/// Emit v0.3-wire-shape deprecation headers on a response built
/// from a legacy-shape request. Increments the
/// `aurora_legacy_wire_ingest_total` counter and emits a tracing
/// info event.
///
/// Per V04_DESIGN §5.3.6 + Step 0 Q12 recon. Structurally parallel
/// to the JWT-deprecation middleware below, but called inline from
/// each dual-shape handler rather than wired as a layer — there are
/// only two dual-shape endpoints (`emitEvent`, `updateSubjectStatus`)
/// and an inline helper is less surface area than a full middleware
/// for that footprint. The Deserialize impls can't set request
/// extensions (no request context), and the handler-set marker
/// can't be read post-`next.run` (request consumed), so the
/// JWT-style middleware shape doesn't fit cleanly anyway.
///
/// Header set, mirroring the JWT pattern:
/// - `Deprecation: true`
/// - `Sunset: <date>` (only when `PDS_V03_WIRE_SUNSET_DATE` is set
///   to a real HTTP-date string; omitted when the env var is unset
///   or "deprecated").
/// - `Warning: 299 - "<message>"` naming the legacy fields + endpoint.
/// - `X-Wire-Migration-Guide` pointing at the operator doc.
pub fn emit_legacy_wire_headers(
    response: &mut Response,
    endpoint: &str,
    shape: &str,
    fields: &[&str],
) {
    use axum::http::HeaderValue;

    let headers = response.headers_mut();
    headers.insert("Deprecation", HeaderValue::from_static("true"));

    let sunset_raw =
        std::env::var("PDS_V03_WIRE_SUNSET_DATE").unwrap_or_else(|_| "deprecated".to_string());
    if sunset_raw != "deprecated" {
        if let Ok(val) = HeaderValue::from_str(&sunset_raw) {
            headers.insert("Sunset", val);
        }
    }

    let warning = format!(
        "299 - \"v0.3 wire shape is canonical; legacy fields [{}] on {} are deprecated\"",
        fields.join(", "),
        endpoint,
    );
    if let Ok(val) = HeaderValue::from_str(&warning) {
        headers.insert("Warning", val);
    }

    headers.insert(
        "X-Wire-Migration-Guide",
        HeaderValue::from_static("/docs/operator/v03-wire-deprecation-rollout.md"),
    );

    for field in fields {
        crate::metrics::record_legacy_wire_ingest(endpoint, shape, field);
    }

    info!(
        endpoint = endpoint,
        shape = shape,
        fields = ?fields,
        "legacy_wire_shape_ingested"
    );
}

/// Detect a JWT-shaped bearer token by its structural signature.
///
/// JWTs are three base64url segments separated by `.` (header,
/// payload, signature). Aurora-Locus's OAuth bearer tokens are
/// opaque strings without that structure. This structural check
/// is sufficient for the deprecation-header path because false
/// positives (a non-JWT token that happens to have two dots) are
/// rare and the cost of an extra Deprecation header on a
/// non-JWT request is negligible.
///
/// Arc 6 Step 8: this detection is **in-middleware** rather than
/// reading `req.extensions().get::<AuthMethod>()` because the
/// `FromRequestParts` extractor that sets that extension runs
/// INSIDE `next.run(req)` — by the time the middleware reads
/// `req.extensions()` before `next.run`, the extension isn't set
/// yet, and after `next.run` the request has been consumed. The
/// extractor-route was the original design but never worked; the
/// structural-detection route is what actually fires at runtime.
fn token_looks_like_jwt(token: &str) -> bool {
    let parts: Vec<&str> = token.split('.').collect();
    parts.len() == 3 && parts.iter().all(|p| !p.is_empty())
}

/// Add JWT deprecation headers to responses
///
/// This middleware checks if the request used JWT authentication (vs OAuth 2.1)
/// and adds deprecation warning headers to the response to inform clients
/// that JWT auth will be sunset in favor of OAuth.
///
/// Headers added for JWT authentication:
/// - Deprecation: true
/// - Sunset: <date> (from config)
/// - Warning: "299 - \"JWT authentication is deprecated. Migrate to OAuth 2.1.\""
/// - X-OAuth-Migration-Guide: <url> (from config)
///
/// Arc 6 Step 8 wired this middleware into the router stack (it
/// was previously defined but never registered). The detection
/// path uses the structural `token_looks_like_jwt` helper because
/// the extractor-set request extension isn't visible from a
/// middleware in axum's layer model — see the helper's doc.
pub async fn jwt_deprecation_headers(
    State(ctx): State<AppContext>,
    req: Request,
    next: Next,
) -> Response {
    // Detect JWT auth from the Authorization header structure
    // directly. Requests without an Authorization header (or
    // with a non-JWT token) flow through unchanged.
    let is_jwt = extract_bearer_token(req.headers())
        .as_deref()
        .map(token_looks_like_jwt)
        .unwrap_or(false);

    // Run the request handler
    let mut response = next.run(req).await;

    // Add deprecation headers if JWT was used
    if is_jwt {
        let headers = response.headers_mut();

        headers.insert("Deprecation", "true".parse().unwrap());

        headers.insert(
            "Sunset",
            ctx.config.authentication.jwt_sunset_date.parse().unwrap(),
        );

        headers.insert(
            "Warning",
            "299 - \"JWT authentication is deprecated. Migrate to OAuth 2.1.\""
                .parse()
                .unwrap(),
        );

        headers.insert(
            "X-OAuth-Migration-Guide",
            ctx.config
                .authentication
                .oauth_migration_guide_url
                .parse()
                .unwrap(),
        );

        // Record metrics
        metrics::record_jwt_deprecation_warning();

        info!("jwt_deprecation_warning_sent");
    }

    response
}

// ============================================================================
// Namespace-keyed scope-check middleware (chainlink #84 / Phase 2.3.1).
//
// Runs before AdminAuthContext extraction on admin namespaces. Composition:
//
//   Request → namespace_scope_check_middleware → AdminAuthContext → handler
//
// Behaviour:
//   - Path outside admin namespaces (com.atproto.admin.* / tools.aurora.*):
//     no-op pass-through.
//   - Path inside an admin namespace, OAuth-authenticated: scope-check;
//     reject with 403 if scope is missing.
//   - Path inside an admin namespace, session/JWT-authenticated (or no
//     auth header): pass through. Session tokens predate the OAuth scope
//     hierarchy and grant implicit full access; AdminAuthContext still
//     enforces role downstream.
//
// The decision logic is split into a pure helper (`classify_namespace_scope`)
// so it's testable without an AppContext; the axum wrapper does the
// (DB-backed) OAuth-token lookup before dispatching.
// ============================================================================

/// Outcome of the namespace scope check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NamespaceScopeOutcome {
    /// Request passes — either not an admin path, or not OAuth-authenticated
    /// (downstream auth handles role enforcement).
    Pass,
    /// OAuth scope satisfies the namespace requirement.
    Allow,
    /// OAuth scope is insufficient — reject with 403.
    Deny(String),
}

/// Pure decision: classify a request based on path and (optional) OAuth
/// scope claim. See module-level docs for behaviour.
pub fn classify_namespace_scope(
    path: &str,
    oauth_scope: Option<&str>,
) -> NamespaceScopeOutcome {
    use crate::oauth::{enforce_namespace_scope, required_scopes_for_path, ScopeSet};
    use std::str::FromStr;

    if required_scopes_for_path(path).is_none() {
        return NamespaceScopeOutcome::Pass;
    }

    let Some(scope_str) = oauth_scope else {
        // Admin path but no OAuth claim — session/JWT path. Let the
        // downstream AdminAuthContext extractor handle role enforcement.
        return NamespaceScopeOutcome::Pass;
    };

    let scopes = match ScopeSet::from_str(scope_str) {
        Ok(s) => s,
        Err(_) => return NamespaceScopeOutcome::Deny("Invalid scope claim".to_string()),
    };

    match enforce_namespace_scope(path, &scopes) {
        Ok(()) => NamespaceScopeOutcome::Allow,
        Err(e) => NamespaceScopeOutcome::Deny(e.to_string()),
    }
}

/// Axum middleware: enforces namespace scope rules on admin paths.
///
/// Looks up the OAuth scope by validating the bearer token against the
/// token table; if the token is not an OAuth token (e.g. session/JWT),
/// `validate_oauth_token` returns Err and we treat this as the
/// session-token case (pass-through).
pub async fn namespace_scope_check(
    State(ctx): State<AppContext>,
    req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path().to_string();

    let oauth_scope = match extract_bearer_token(req.headers()) {
        Some(token) => crate::auth::validate_oauth_token(&ctx, &token)
            .await
            .ok()
            .map(|t| t.scope),
        None => None,
    };

    match classify_namespace_scope(&path, oauth_scope.as_deref()) {
        NamespaceScopeOutcome::Pass | NamespaceScopeOutcome::Allow => next.run(req).await,
        NamespaceScopeOutcome::Deny(msg) => {
            warn!(path = %path, "namespace_scope_check: denied — {}", msg);
            metrics::record_error("NamespaceScopeDenied", "middleware");
            (StatusCode::FORBIDDEN, msg).into_response()
        }
    }
}

#[cfg(test)]
mod namespace_scope_tests {
    use super::*;

    #[test]
    fn non_admin_path_passes_regardless_of_scope() {
        // Routes outside admin namespaces are not subject to namespace scope rules.
        assert_eq!(
            classify_namespace_scope("/xrpc/com.atproto.repo.createRecord", None),
            NamespaceScopeOutcome::Pass
        );
        assert_eq!(
            classify_namespace_scope(
                "/xrpc/com.atproto.repo.createRecord",
                Some("atproto:read")
            ),
            NamespaceScopeOutcome::Pass
        );
        assert_eq!(
            classify_namespace_scope("/health", None),
            NamespaceScopeOutcome::Pass
        );
    }

    #[test]
    fn session_token_admin_path_passes_through() {
        // No OAuth claim → session/JWT path → defer to AdminAuthContext.
        for path in [
            "/xrpc/tools.aurora.ops.pauseSequencer",
            "/xrpc/tools.aurora.moderator.listEvents",
            "/xrpc/com.atproto.admin.searchAccounts",
        ] {
            assert_eq!(
                classify_namespace_scope(path, None),
                NamespaceScopeOutcome::Pass,
                "path {} should pass for session-token (no OAuth scope)",
                path
            );
        }
    }

    #[test]
    fn oauth_admin_moderation_blocked_from_ops() {
        let outcome = classify_namespace_scope(
            "/xrpc/tools.aurora.ops.pauseSequencer",
            Some("atproto:admin.moderation"),
        );
        assert!(
            matches!(outcome, NamespaceScopeOutcome::Deny(_)),
            "moderation scope on ops path should be Deny, got {:?}",
            outcome
        );
    }

    #[test]
    fn oauth_admin_server_blocked_from_moderation_tier() {
        for prefix in [
            "tools.aurora.moderator.",
            "tools.aurora.admin.",
            "tools.aurora.superadmin.",
        ] {
            let path = format!("/xrpc/{}listEvents", prefix);
            let outcome = classify_namespace_scope(&path, Some("atproto:admin.server"));
            assert!(
                matches!(outcome, NamespaceScopeOutcome::Deny(_)),
                "server scope on {} should be Deny, got {:?}",
                path,
                outcome
            );
        }
    }

    #[test]
    fn oauth_admin_wildcard_satisfies_any_admin_path() {
        for path in [
            "/xrpc/tools.aurora.ops.pauseSequencer",
            "/xrpc/tools.aurora.moderator.listEvents",
            "/xrpc/tools.aurora.admin.grantRole",
            "/xrpc/tools.aurora.superadmin.purgeAccount",
            "/xrpc/com.atproto.admin.searchAccounts",
        ] {
            assert_eq!(
                classify_namespace_scope(path, Some("atproto:admin.*")),
                NamespaceScopeOutcome::Allow,
                "admin.* should Allow {}",
                path
            );
        }
    }

    #[test]
    fn oauth_correct_scope_satisfies_namespace() {
        // The "happy path" specific scopes.
        assert_eq!(
            classify_namespace_scope(
                "/xrpc/tools.aurora.ops.pauseSequencer",
                Some("atproto:admin.server")
            ),
            NamespaceScopeOutcome::Allow
        );
        assert_eq!(
            classify_namespace_scope(
                "/xrpc/tools.aurora.moderator.listEvents",
                Some("atproto:admin.moderation")
            ),
            NamespaceScopeOutcome::Allow
        );
    }

    #[test]
    fn oauth_com_atproto_admin_accepts_either_scope() {
        // Bsky-PDS parity baseline: server OR moderation accepted.
        let path = "/xrpc/com.atproto.admin.searchAccounts";
        assert_eq!(
            classify_namespace_scope(path, Some("atproto:admin.server")),
            NamespaceScopeOutcome::Allow
        );
        assert_eq!(
            classify_namespace_scope(path, Some("atproto:admin.moderation")),
            NamespaceScopeOutcome::Allow
        );
    }

    #[test]
    fn oauth_unrelated_scope_blocked_from_admin() {
        // Defense-in-depth: a non-admin scope (e.g., atproto:read) doesn't
        // accidentally satisfy any admin namespace.
        assert!(matches!(
            classify_namespace_scope(
                "/xrpc/tools.aurora.ops.pauseSequencer",
                Some("atproto:read atproto:write")
            ),
            NamespaceScopeOutcome::Deny(_)
        ));
    }

    // ---------- Arc 6 Step 8: JWT structural detection ----------

    #[test]
    fn token_looks_like_jwt_accepts_three_segment_token() {
        // header.payload.signature shape — what JWTs use.
        assert!(token_looks_like_jwt("aaa.bbb.ccc"));
        assert!(token_looks_like_jwt(
            "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4ifQ.fakeSig"
        ));
    }

    #[test]
    fn token_looks_like_jwt_rejects_non_jwt_shapes() {
        // Opaque OAuth tokens — no dots.
        assert!(!token_looks_like_jwt("opaque_token_uuid_like"));
        assert!(!token_looks_like_jwt(""));
        // Wrong segment count.
        assert!(!token_looks_like_jwt("aaa.bbb"));
        assert!(!token_looks_like_jwt("aaa.bbb.ccc.ddd"));
        // Empty segments — JWTs require non-empty header/payload/signature.
        assert!(!token_looks_like_jwt("aaa..ccc"));
        assert!(!token_looks_like_jwt(".bbb.ccc"));
        assert!(!token_looks_like_jwt("aaa.bbb."));
    }
}
