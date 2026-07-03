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
    http::{HeaderMap, HeaderValue, StatusCode},
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

/// Extract an atproto-OAuth bearer (`Authorization: DPoP <token>`). The DPoP
/// auth-scheme is what distinguishes an atproto-OAuth bearer (β.3) from a
/// `Bearer`-scheme session/JWT token, so the two auth paths never collide.
pub fn extract_dpop_bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("DPoP ").map(|t| t.to_string()))
}

/// Internal, **server-set-only** header the [`atproto_oauth_gate`] middleware
/// stamps with the DID an atproto-OAuth bearer resolved to (Arc 2 Phase ε.3).
/// The gate STRIPS any inbound value before optionally setting it, so a
/// downstream `fn(State, headers)` resolver (`require_auth` /
/// `require_auth_unified` — neither has request-line or extensions access) can
/// read + trust it. Trust is sound only because the gate is layered on the
/// router hosting every resolver caller, so no client-supplied value survives.
const OAUTH_RESOLVED_DID_HEADER: &str = "x-aurora-oauth-resolved-did";
/// Companion to [`OAUTH_RESOLVED_DID_HEADER`] carrying the (internal-form) scope
/// the OAuth path resolved to. ε.3 stubs this to the internal all-scope
/// (`atproto:*`); ε.4's scope-translation replaces the stub.
const OAUTH_RESOLVED_SCOPE_HEADER: &str = "x-aurora-oauth-resolved-scope";

/// Read the gate-resolved atproto-OAuth DID, if the middleware authenticated one.
fn oauth_resolved_did(headers: &HeaderMap) -> Option<String> {
    headers
        .get(OAUTH_RESOLVED_DID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

/// Read the gate-resolved internal-form scope.
fn oauth_resolved_scope(headers: &HeaderMap) -> Option<String> {
    headers
        .get(OAUTH_RESOLVED_SCOPE_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

/// Arc 2 Phase ε.3 — the general-XRPC atproto-OAuth bearer gate (registry-
/// gated). Runs as a router-level middleware so the whole XRPC surface becomes
/// OAuth-capable without per-handler edits: `fn(State, headers)` resolvers
/// (`require_auth`, `require_auth_unified`) can't host DPoP (no request line)
/// nor read extensions, so the gate does the DPoP + registry work here and
/// hands the resolved DID to them through a stripped-then-set internal header.
///
/// For a request bearing `Authorization: DPoP <token>` it enforces, in order:
/// the bearer validates (β.1 hash lookup); a DPoP proof is present and valid
/// (α₁: htm/htu/ath/exp/jti); the proof key matches the token's bound
/// thumbprint (β.3); AND the key is a registered, non-revoked device for the
/// bearer's DID (ε.2 registry — the new registry-gates-auth invariant). Any
/// failure fails closed (the request does not fall through to the session
/// path). A request with no `DPoP`-scheme auth passes through untouched.
///
/// Scope is STUBBED at ε.3: a bearer carrying the `atproto` scope is admitted
/// broadly (mapped to the internal `atproto:*` all-scope so handler-side
/// `enforce_scope` passes). ε.4's scope-translation replaces the stub.
pub async fn atproto_oauth_gate(
    State(ctx): State<AppContext>,
    mut req: Request,
    next: Next,
) -> Response {
    // Spoof defense: never trust an inbound value of the internal headers.
    req.headers_mut().remove(OAUTH_RESOLVED_DID_HEADER);
    req.headers_mut().remove(OAUTH_RESOLVED_SCOPE_HEADER);

    let Some(token) = extract_dpop_bearer(req.headers()) else {
        // Not an atproto-OAuth bearer — the Bearer/JWT session path handles it.
        return next.run(req).await;
    };

    // Extract everything the validation needs as OWNED values BEFORE any await:
    // a `&Request` cannot be held across an await point (`Body` is not `Sync`,
    // so the middleware future would be non-`Send` and fail `from_fn`'s bound).
    let method = req.method().as_str().to_string();
    let htu = format!("{}{}", ctx.service_url(), req.uri().path());
    let dpop_proof = req
        .headers()
        .get("dpop")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    match resolve_atproto_oauth(&ctx, &method, &htu, dpop_proof.as_deref(), &token).await {
        Ok((did, scope)) => {
            match (HeaderValue::from_str(&did), HeaderValue::from_str(&scope)) {
                (Ok(dv), Ok(sv)) => {
                    req.headers_mut().insert(OAUTH_RESOLVED_DID_HEADER, dv);
                    req.headers_mut().insert(OAUTH_RESOLVED_SCOPE_HEADER, sv);
                    next.run(req).await
                }
                _ => PdsError::Internal("resolved DID/scope not header-safe".to_string())
                    .into_response(),
            }
        }
        // A present-but-invalid atproto-OAuth bearer fails closed here.
        Err(e) => e.into_response(),
    }
}

/// The gate's validation core — takes owned request bits (never a `&Request`,
/// which would make the caller's future non-`Send`). Returns
/// `(did, internal_scope)` on success.
async fn resolve_atproto_oauth(
    ctx: &AppContext,
    method: &str,
    htu: &str,
    dpop_proof: Option<&str>,
    token: &str,
) -> PdsResult<(String, String)> {
    // 1. bearer → token row (β.1 access_token_hash lookup; missing/revoked/
    //    expired → Authentication error → 401).
    let token_info = crate::auth::validate_oauth_token(ctx, token).await?;

    // 2. DPoP proof required + verified against this request (htm/htu/ath).
    let proof = dpop_proof
        .ok_or_else(|| PdsError::Authentication("DPoP proof required".to_string()))?;
    let ath = crate::federation::dpop::compute_ath(token);
    let proof_jkt = ctx
        .dpop_verifier
        .verify_dpop_proof(proof, method, htu, Some(&ath))
        .await?;

    // 3. proof key == the token's bound DPoP thumbprint (β.3 binding).
    match token_info.dpop_thumbprint.as_deref() {
        Some(bound) if bound == proof_jkt => {}
        _ => {
            return Err(PdsError::Authentication(
                "DPoP key does not match the token's bound thumbprint".to_string(),
            ))
        }
    }

    // 4. registry gate (ε.2): the key must be a registered active device for
    //    the bearer's DID.
    let device = ctx
        .atproto_device_manager
        .get_device_by_jkt(&token_info.did, &proof_jkt)
        .await?
        .ok_or_else(|| PdsError::Authentication("device not registered".to_string()))?;

    // 5. best-effort activity tracking (never fails the request).
    let _ = ctx.atproto_device_manager.touch(&device.device_id).await;

    // 6. scope (ε.4 scope-α, translate-at-gate): require the base `atproto`
    //    scope, then translate the bearer's atproto-spec scopes into the
    //    internal-vocabulary scope string the handler-side `enforce_scope`
    //    evaluates (transition:generic → repo.* + blob.upload; base-only → no
    //    write capability; admin never granted).
    if !token_info.scope.split_whitespace().any(|s| s == "atproto") {
        return Err(PdsError::Authorization(
            "token lacks the required 'atproto' scope".to_string(),
        ));
    }
    let internal_scope = crate::oauth::atproto::scope::to_internal_scope(&token_info.scope);
    Ok((token_info.did, internal_scope))
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
    // Arc 2 ε.3: honor an atproto-OAuth identity the `atproto_oauth_gate`
    // middleware already validated (DPoP + registered device). The synthetic
    // session_id marks the OAuth origin; there is no `session` row.
    if let Some(did) = oauth_resolved_did(&headers) {
        return Ok(ValidatedSession {
            session_id: format!("atproto-oauth:{did}"),
            did,
            is_app_password: false,
        });
    }

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
    // Arc 2 ε.3: honor a gate-resolved atproto-OAuth identity (DPoP + registered
    // device already verified). The internal-form scope is the ε.3 stub
    // (`atproto:*`) until ε.4's scope-translation.
    if let Some(did) = oauth_resolved_did(&headers) {
        let scope = oauth_resolved_scope(&headers).unwrap_or_default();
        return Ok(UnifiedAuthContext::OAuth { did, scope });
    }

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

/// v0.9 Federation runtime-mutability arc §3.7 (#395) — request-layer
/// short-circuit. Refuses the inbound federation OPERATIONAL endpoints with 503
/// when `federation.enabled` resolves false (runtime override → config fallback),
/// effective immediately for incident response — before any restart tears the
/// subsystem down.
///
/// The `status` / `describePosture` posture endpoints are intentionally NOT gated
/// (they must keep reporting the off-state so peers can discover it), and
/// `com.aurora.dpop.getNonce` is excluded (DPoP is also OAuth's, not federation-
/// only). The per-request `resolve` DB read happens ONLY for the gated paths.
/// Scope is inbound only; outbound federation operations continue until restart.
pub async fn federation_enabled_gate(
    State(ctx): State<AppContext>,
    req: Request,
    next: Next,
) -> Response {
    const GATED: &[&str] = &[
        "/xrpc/com.aurora.federation.listInstances",
        "/xrpc/com.aurora.federation.refreshDiscovery",
        "/xrpc/com.aurora.federation.aggregateTimeline",
    ];
    if GATED.contains(&req.uri().path()) {
        if let Some((status, body)) =
            crate::api::aurora_admin::federation_inbound_gate_503(&ctx).await
        {
            return (status, body).into_response();
        }
    }
    next.run(req).await
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

#[cfg(test)]
mod epsilon_oauth_gate_tests {
    use super::*;
    use crate::federation::dpop::{DPopClaims, Jwk};
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

    async fn ctx() -> AppContext {
        crate::api::federation_peers::test_support::create_test_context_with(|_| {}).await
    }

    async fn seed_actor(ctx: &AppContext, did: &str) {
        sqlx::query("INSERT INTO actor (did, handle, created_at) VALUES ($1,$2,$3)")
            .bind(did)
            .bind(format!("{}.example.com", did.replace(':', "-")))
            .bind("2026-01-01T00:00:00Z")
            .execute(&ctx.account_db)
            .await
            .unwrap();
    }

    // ---- simple: the fn-resolvers honor the gate-set internal header ----

    #[tokio::test]
    async fn require_auth_honors_gate_resolved_did() {
        let ctx = ctx().await;
        let mut headers = HeaderMap::new();
        headers.insert(
            OAUTH_RESOLVED_DID_HEADER,
            HeaderValue::from_static("did:web:alice.example.com"),
        );
        let s = require_auth(State(ctx.clone()), headers).await.unwrap();
        assert_eq!(s.did, "did:web:alice.example.com");
        assert!(!s.is_app_password);
    }

    #[tokio::test]
    async fn require_auth_unified_honors_gate_resolved_did() {
        let ctx = ctx().await;
        let mut headers = HeaderMap::new();
        headers.insert(
            OAUTH_RESOLVED_DID_HEADER,
            HeaderValue::from_static("did:web:bob.example.com"),
        );
        headers.insert(OAUTH_RESOLVED_SCOPE_HEADER, HeaderValue::from_static("atproto:*"));
        let auth = require_auth_unified(State(ctx.clone()), headers).await.unwrap();
        assert!(auth.is_oauth());
        assert_eq!(auth.did(), "did:web:bob.example.com");
        assert_eq!(auth.oauth_scope(), Some("atproto:*"));
    }

    // ---- resolve_atproto_oauth: full validation core ----

    fn fresh_p256() -> (p256::ecdsa::SigningKey, Jwk) {
        use p256::ecdsa::SigningKey;
        use p256::EncodedPoint;
        let sk = SigningKey::random(&mut rand::rngs::OsRng);
        let enc: EncodedPoint = sk.verifying_key().to_encoded_point(false);
        let jwk = Jwk {
            kty: "EC".to_string(),
            crv: "P-256".to_string(),
            x: URL_SAFE_NO_PAD.encode(enc.x().unwrap()),
            y: URL_SAFE_NO_PAD.encode(enc.y().unwrap()),
        };
        (sk, jwk)
    }

    fn dpop_proof(sk: &p256::ecdsa::SigningKey, jwk: &Jwk, htu: &str, ath: &str) -> String {
        use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
        use p256::pkcs8::EncodePrivateKey;
        let claims = DPopClaims {
            jti: uuid::Uuid::new_v4().to_string(),
            htm: "POST".to_string(),
            htu: htu.to_string(),
            iat: chrono::Utc::now().timestamp(),
            exp: chrono::Utc::now().timestamp() + 120,
            ath: Some(ath.to_string()),
        };
        let pem = sk.to_pkcs8_pem(Default::default()).unwrap().to_string();
        let key = EncodingKey::from_ec_pem(pem.as_bytes()).unwrap();
        let mut h = Header::new(Algorithm::ES256);
        h.typ = Some("dpop+jwt".to_string());
        h.jwk = Some(serde_json::from_value(serde_json::to_value(jwk).unwrap()).unwrap());
        encode(&h, &claims, &key).unwrap()
    }

    /// Seed a token row + a registered device whose key == the proof key.
    /// Returns (bearer, jkt).
    async fn seed_token_and_device(ctx: &AppContext, did: &str, jwk: &Jwk, scope: &str) -> (String, String) {
        let jwk_json = serde_json::to_string(jwk).unwrap();
        let dev = ctx
            .atproto_device_manager
            .register_device(did, &jwk_json, Some("dev"), None)
            .await
            .unwrap();
        let bearer = format!("at_{}", uuid::Uuid::new_v4().simple());
        let hash = crate::oauth::access_token_hash(&bearer);
        sqlx::query(
            "INSERT INTO token (token_id, did, client_id, scope, created_at, updated_at, \
             expires_at, dpop_thumbprint, access_token_hash) VALUES ($1,$2,$3,$4,$5,$5,$6,$7,$8)",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(did)
        .bind("https://app/cm.json")
        .bind(scope)
        .bind("2026-01-01T00:00:00Z")
        .bind("2099-01-01T00:00:00Z")
        .bind(&dev.dpop_jkt)
        .bind(&hash)
        .execute(&ctx.account_db)
        .await
        .unwrap();
        (bearer, dev.dpop_jkt)
    }


    #[tokio::test]
    async fn resolve_happy_path_translates_scope() {
        let ctx = ctx().await;
        let did = "did:web:carol.example.com";
        seed_actor(&ctx, did).await;
        let (sk, jwk) = fresh_p256();
        // transition:generic → the internal repo.* + blob.upload capabilities.
        let (bearer, _jkt) =
            seed_token_and_device(&ctx, did, &jwk, "atproto transition:generic").await;

        let htu = format!("{}/xrpc/com.atproto.repo.createRecord", ctx.service_url());
        let ath = crate::federation::dpop::compute_ath(&bearer);
        let proof = dpop_proof(&sk, &jwk, &htu, &ath);

        let (rdid, rscope) = resolve_atproto_oauth(&ctx, "POST", &htu, Some(&proof), &bearer)
            .await
            .expect("resolve");
        assert_eq!(rdid, did);
        assert_eq!(rscope, "atproto:repo.* atproto:blob.upload"); // ε.4 scope-α
    }

    #[test]
    fn translated_scope_gates_enforce_scope_end_to_end() {
        // ε.3 (gate) → ε.4 (translation) → handler enforce_scope, tied together.
        use crate::oauth::atproto::scope::to_internal_scope;
        use crate::oauth::AtProtoScope;

        let generic = UnifiedAuthContext::OAuth {
            did: "did:web:x.example.com".to_string(),
            scope: to_internal_scope("atproto transition:generic"),
        };
        // transition:generic admits the repo-write family + blob upload…
        assert!(enforce_scope(&generic, &AtProtoScope::RepoCreate).is_ok());
        assert!(enforce_scope(&generic, &AtProtoScope::RepoDelete).is_ok());
        assert!(enforce_scope(&generic, &AtProtoScope::BlobUpload).is_ok());
        // …but never admin.
        assert!(enforce_scope(&generic, &AtProtoScope::AdminAll).is_err());

        // Base `atproto` alone grants no write capability.
        let base = UnifiedAuthContext::OAuth {
            did: "did:web:y.example.com".to_string(),
            scope: to_internal_scope("atproto"),
        };
        assert!(enforce_scope(&base, &AtProtoScope::RepoCreate).is_err());
        assert!(enforce_scope(&base, &AtProtoScope::BlobUpload).is_err());
    }

    #[tokio::test]
    async fn resolve_base_atproto_only_grants_no_write_scope() {
        let ctx = ctx().await;
        let did = "did:web:grace.example.com";
        seed_actor(&ctx, did).await;
        let (sk, jwk) = fresh_p256();
        // Base `atproto` only → empty internal scope (reads ok, writes 403).
        let (bearer, _jkt) = seed_token_and_device(&ctx, did, &jwk, "atproto").await;
        let htu = format!("{}/xrpc/com.atproto.repo.createRecord", ctx.service_url());
        let ath = crate::federation::dpop::compute_ath(&bearer);
        let proof = dpop_proof(&sk, &jwk, &htu, &ath);
        let (_did, rscope) = resolve_atproto_oauth(&ctx, "POST", &htu, Some(&proof), &bearer)
            .await
            .expect("resolve");
        assert_eq!(rscope, "");
    }

    #[tokio::test]
    async fn resolve_rejects_unregistered_device() {
        let ctx = ctx().await;
        let did = "did:web:dave.example.com";
        seed_actor(&ctx, did).await;
        let (sk, jwk) = fresh_p256();
        // Seed the token bound to the key's jkt, but do NOT register a device.
        let jkt = crate::federation::dpop::compute_jwk_thumbprint(
            &serde_json::to_value(&jwk).unwrap(),
        )
        .unwrap();
        let bearer = format!("at_{}", uuid::Uuid::new_v4().simple());
        let hash = crate::oauth::access_token_hash(&bearer);
        sqlx::query(
            "INSERT INTO token (token_id, did, client_id, scope, created_at, updated_at, \
             expires_at, dpop_thumbprint, access_token_hash) VALUES ($1,$2,$3,$4,$5,$5,$6,$7,$8)",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(did)
        .bind("https://app/cm.json")
        .bind("atproto")
        .bind("2026-01-01T00:00:00Z")
        .bind("2099-01-01T00:00:00Z")
        .bind(&jkt)
        .bind(&hash)
        .execute(&ctx.account_db)
        .await
        .unwrap();

        let htu = format!("{}/xrpc/com.atproto.repo.createRecord", ctx.service_url());
        let ath = crate::federation::dpop::compute_ath(&bearer);
        let proof = dpop_proof(&sk, &jwk, &htu, &ath);

        let err = resolve_atproto_oauth(&ctx, "POST", &htu, Some(&proof), &bearer).await.unwrap_err();
        assert!(matches!(err, PdsError::Authentication(ref m) if m.contains("device not registered")));
    }

    #[tokio::test]
    async fn resolve_rejects_missing_atproto_scope() {
        let ctx = ctx().await;
        let did = "did:web:erin.example.com";
        seed_actor(&ctx, did).await;
        let (sk, jwk) = fresh_p256();
        // Scope lacks the base `atproto` token.
        let (bearer, _jkt) = seed_token_and_device(&ctx, did, &jwk, "transition:generic").await;
        let htu = format!("{}/xrpc/com.atproto.repo.createRecord", ctx.service_url());
        let ath = crate::federation::dpop::compute_ath(&bearer);
        let proof = dpop_proof(&sk, &jwk, &htu, &ath);
        let err = resolve_atproto_oauth(&ctx, "POST", &htu, Some(&proof), &bearer).await.unwrap_err();
        assert!(matches!(err, PdsError::Authorization(_)));
    }
}
