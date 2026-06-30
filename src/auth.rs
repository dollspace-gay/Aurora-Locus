// Allow dead_code for auth module - many auth contexts are defined for future protected routes
#![allow(dead_code)]

//! Authentication extractors and utilities

use crate::{
    account::ValidatedSession, admin::Role, api::middleware::extract_bearer_token,
    context::AppContext, error::PdsError, oauth::ScopeSet,
};
use axum::{async_trait, extract::FromRequestParts, http::request::Parts};
use uuid::Uuid;


/// Parse RFC3339 timestamp string to DateTime<Utc>. Required for sqlx::Any
/// since chrono types don't implement Type<Any>. See chainlink #76.
fn parse_ts(s: &str) -> Result<chrono::DateTime<chrono::Utc>, crate::error::PdsError> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .map_err(|e| crate::error::PdsError::Internal(format!("Invalid timestamp: {}", e)))
}

/// A DID the caller has authenticated as authorized to write to — produced by
/// the request-auth chokepoints (e.g. `authenticated_did_for_repo`, which
/// validates that the request's authenticated identity owns the target repo).
///
/// Arc H §7.2.5 / #280 rev4 (LB-5/H-7), scoped per #281: this newtype makes the
/// kryphocron dedicated-endpoint write boundary — including the `graph.block`
/// cascade entry point (`createBlock`, #282) — take a *deliberately-constructed*
/// DID rather than a bare `&str`, so a write DID sourced from request input
/// can't be passed implicitly to the write helpers.
///
/// **Scope note (per the #281 §16 clarification).** rev4's literal "type-proof
/// Boundary-1 everywhere via `for_writer`" was reduced to *this* boundary,
/// because `RepositoryManager::for_writer` lives in `actor_store` and cannot
/// depend on the `api` auth types (layering), and because a legitimate
/// non-request writer exists (the rewrite-on-rotate system job). So this is a
/// low-level, `api`-free newtype constructed from an already-validated DID
/// string; `for_writer` still takes `String`. The global, auth-object-bound
/// version is future work (precondition: resolve the layering bar / introduce a
/// low-level principal type). Boundary-1 for #280 holds because `createBlock` —
/// the only #280 cascade entry point — consumes this type at its boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedDid(String);

impl AuthenticatedDid {
    /// Construct from a DID the caller has already authenticated **and**
    /// authorized — i.e. a request-auth chokepoint has validated
    /// `requested_repo == auth.did()`. The kryphocron `authenticated_did_for_repo`
    /// chokepoint is the producer; do not call this with a DID taken straight
    /// from request input without that validation.
    pub fn from_authenticated(did: impl Into<String>) -> Self {
        Self(did.into())
    }

    /// The underlying DID string.
    pub fn value(&self) -> &str {
        &self.0
    }
}

/// Authentication method used for the request
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMethod {
    /// OAuth 2.1 token (modern)
    OAuth,
    /// Legacy JWT session token
    Jwt,
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
                let session = state.account_manager.validate_access_token(&token).await?;

                let did = session.did.clone();
                let duration = start.elapsed().as_secs_f64();

                // Record metrics (JWT fallback)
                crate::metrics::record_oauth_token_exchange("jwt_fallback", "success", duration);

                // Store auth method in extensions for middleware
                parts.extensions.insert(AuthMethod::Jwt);

                Ok(AuthContext {
                    did,
                    session,
                    auth_method: AuthMethod::Jwt,
                })
            }
        }
    }
}

/// Arc 12 §5.3.4 forwarded-routes auth extractor. Thin wrapper
/// around `AppContext::verify_jwt_with_allowlist` with the
/// `[service_did, entryway_did]` allowlist (degrades to
/// `[service_did]` in standalone mode where `entryway_did()` is
/// `None`). Used by the four §5.3.8 forwarded mint-pattern handlers
/// (`signPlcOperation`, `updateHandle`, `getSession`).
///
/// Returns only the resolved DID — handlers don't need the inner
/// session/oauth/cross-pds variant distinction for forwarding
/// decisions (the variant is purely informational at this layer).
#[derive(Debug, Clone)]
pub struct AuthContextForwarded {
    pub did: String,
}

#[async_trait]
impl FromRequestParts<AppContext> for AuthContextForwarded {
    type Rejection = PdsError;

    async fn from_request_parts(
        parts: &mut Parts,
        ctx: &AppContext,
    ) -> Result<Self, Self::Rejection> {
        let token = extract_bearer_token(&parts.headers)
            .ok_or_else(|| PdsError::Authentication("Missing authorization header".to_string()))?;
        let service_did = ctx.service_did().to_string();
        let entryway_did_owned = ctx.entryway_did().map(str::to_string);
        let mut allowlist: Vec<&str> = vec![service_did.as_str()];
        if let Some(eid) = entryway_did_owned.as_deref() {
            allowlist.push(eid);
        }
        let auth = ctx.verify_jwt_with_allowlist(&token, &allowlist).await?;
        Ok(Self {
            did: auth.did().to_string(),
        })
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
                    crate::metrics::record_oauth_token_exchange(
                        "validation_optional",
                        "success",
                        duration,
                    );

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
                            crate::metrics::record_oauth_token_exchange(
                                "jwt_fallback_optional",
                                "success",
                                duration,
                            );

                            // Store auth method in extensions for middleware
                            parts.extensions.insert(AuthMethod::Jwt);

                            Some(AuthContext {
                                did,
                                session,
                                auth_method: AuthMethod::Jwt,
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
        let token = extract_bearer_token(&parts.headers)
            .ok_or_else(|| PdsError::Authentication("Missing authorization header".to_string()))?;
        admin_auth_from_token(state, &token).await
    }
}

/// Pre-Step-2 layering had two paths (local session, then HS256
/// admin JWT). Step 2 (§5.4.2) adds a third: ES256K service-auth
/// JWTs, gated by a four-case pre-check (§5.3.1) so non-ES256K
/// tokens never trigger the resolver. The full layering is:
///
/// 1. Local session (`account_manager.validate_access_token`).
/// 2. HS256 admin JWT (`verify_jwt_token`, scope=admin).
/// 3. ES256K pre-check (`pre_check_es256k`) — four explicit
///    fall-through cases, NO `?` propagation.
/// 4. `verify_service_jwt` against `state.identity_resolver`.
/// 5. Role lookup against `admin_role_manager`.
///
/// Layers 1 and 2 short-circuit on success. Layer 1's failure falls
/// to layer 2; layer 2's failure falls to layer 3. A successful
/// HS256 JWT with non-admin scope is treated as a definitive layer-2
/// rejection (401) — falling through to layer 3 would only re-reject
/// it as `alg=HS256 not ES256K`, with no observable benefit and a
/// less specific log line.
///
/// Extracted to a free function so tests can invoke it directly with
/// a token + AppContext rather than building HTTP `Parts`.
pub(crate) async fn admin_auth_from_token(
    state: &AppContext,
    token: &str,
) -> Result<AdminAuthContext, PdsError> {
    // Layer 1: local session
    match state.account_manager.validate_access_token(token).await {
        Ok(session) => {
            let did = session.did.clone();
            return finalize_admin_role(state, did, session).await;
        }
        Err(_) => {
            tracing::debug!(token_prefix = %mask_token(token), "local session token rejected");
        }
    }

    // Layer 2: HS256 admin JWT
    match verify_jwt_token(token, &state.config.authentication.jwt_secret) {
        Ok(token_data) => {
            let claims = &token_data.claims;
            let did = claims
                .get("sub")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    tracing::debug!(reason = "missing-sub", "HS256 admin token rejected");
                    PdsError::Authentication("HS256 admin token missing 'sub' claim".to_string())
                })?
                .to_string();
            let scope = claims.get("scope").and_then(|v| v.as_str());
            if scope != Some("admin") {
                tracing::debug!(reason = "scope-not-admin", "HS256 admin token rejected");
                return Err(PdsError::Authentication(
                    "HS256 token does not have admin scope".to_string(),
                ));
            }
            // §8.1.7 / #271: tokens minted once the operator-session store
            // landed carry a `sid`. When present, the session must be live —
            // not revoked, not expired — on EVERY request; this is what makes
            // a SuperAdmin force-logout (#273) take effect on the operator's
            // very next request, and bumps the session's last-active stamp.
            // Tokens without a `sid` (minted before #271) take the legacy
            // stateless path unchanged, so a deploy doesn't bounce live
            // sessions.
            let sid = claims
                .get("sid")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty());
            if let Some(sid) = sid {
                if !state.operator_session_store.validate_and_touch(sid).await? {
                    tracing::debug!(reason = "session-invalid", "HS256 admin token rejected");
                    return Err(PdsError::Authentication(
                        "operator session is no longer valid".to_string(),
                    ));
                }
            }
            let session = ValidatedSession {
                did: did.clone(),
                session_id: sid
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("jwt-{}", Uuid::new_v4())),
                is_app_password: false,
            };
            return finalize_admin_role(state, did, session).await;
        }
        Err(e) => {
            tracing::debug!(
                reason = %hs256_rejection_category(&e),
                "HS256 admin token rejected"
            );
            // fall through to layer 3
        }
    }

    // Layer 3: ES256K pre-check (§5.3.1). Each rejection variant is
    // a non-error fall-through — no `?` propagation; the pre-check
    // returns a `Result<(), PreCheckRejection>` that we dispatch on.
    match pre_check_es256k(token) {
        Err(PreCheckRejection::NotJwtShaped) => {
            tracing::debug!("service-auth pre-check: token is not JWT-shaped");
            return Err(PdsError::Authentication("Invalid auth token".to_string()));
        }
        Err(PreCheckRejection::NoValidAlgField) => {
            tracing::debug!("service-auth pre-check: header lacks valid alg field");
            return Err(PdsError::Authentication("Invalid auth token".to_string()));
        }
        Err(PreCheckRejection::AlgNotEs256k(received)) => {
            tracing::debug!(
                received_alg = %received,
                "service-auth pre-check: alg={} not ES256K",
                received
            );
            return Err(PdsError::Authentication("Invalid auth token".to_string()));
        }
        Ok(()) => {}
    }

    // Layer 4: ES256K service-auth verification against the resolver.
    // §5.3.1 specifies `expected_aud = state.service_did()`; §5.5.6
    // documents this as byte-for-byte strict-equal (no normalization).
    //
    // Cluster 2 Member 2.2 (#144) — site 7 of 8 (admin extractor, the
    // only live caller of the free-fn). Pre-#144 this match
    // unconditionally wrapped Err as `PdsError::Authentication`,
    // destroying the typed `ServiceAuthError::DidTombstoned` (emitted
    // by site 5's resolver match arm) and bypassing site 4's
    // `From<ServiceAuthError> for PdsError` typed-routing impl. After
    // the fix the typed variant propagates via `.into()` so
    // IntoResponse for PdsError::DidTombstoned maps it to HTTP 400
    // `{"error": "DidTombstoned", ...}`. All other ServiceAuthError
    // variants preserve today's `PdsError::Authentication(format!(...))`
    // shape byte-identical — the wrap string isn't grep'd by any
    // test/runbook/metric. The tracing log line via
    // log_service_auth_error stays unchanged for all variants
    // (including DidTombstoned, which gets its own arm post-#144 site 6).
    let claims = match crate::service_auth::verify_service_jwt(
        token,
        state.service_did(),
        state.identity_resolver.as_ref(),
    )
    .await
    {
        Ok(c) => c,
        Err(e) => {
            log_service_auth_error(&e, state.service_did());
            return match e {
                crate::service_auth::ServiceAuthError::DidTombstoned(_) => Err(e.into()),
                other => Err(PdsError::Authentication(format!(
                    "service-auth verification failed: {}",
                    other
                ))),
            };
        }
    };

    // Synthetic session — service-auth tokens aren't backed by a
    // local-session row. session_id is unique per request so audit
    // logs can correlate.
    let session = ValidatedSession {
        did: claims.iss.clone(),
        session_id: format!("svc-{}", Uuid::new_v4()),
        is_app_password: false,
    };
    finalize_admin_role(state, claims.iss, session).await
}

/// Look up the admin role for `did`. Returns 403 (`Authorization`)
/// when authentication succeeded but no role exists — distinct from
/// the 401 `Authentication` errors above so operators can tell
/// "wrong token" from "valid token, not authorized".
async fn finalize_admin_role(
    state: &AppContext,
    did: String,
    session: ValidatedSession,
) -> Result<AdminAuthContext, PdsError> {
    match state.admin_role_manager.get_role(&did).await? {
        Some(admin_role) => {
            tracing::debug!(
                did = %did,
                role = %admin_role.role.as_str(),
                "admin role lookup succeeded"
            );
            Ok(AdminAuthContext {
                did,
                session,
                role: admin_role.role,
            })
        }
        None => {
            tracing::info!("authorization: DID={} has no role", did);
            Err(PdsError::Authorization(format!(
                "Admin role required for {}",
                did
            )))
        }
    }
}

/// §5.3.1 pre-check rejection variants. Each one is a non-error
/// fall-through from the perspective of `admin_auth_from_token` —
/// the load-bearing property is that `verify_service_jwt` and the
/// resolver are unreachable when `pre_check_es256k` returns `Err`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PreCheckRejection {
    /// Token doesn't split into 3 base64url-shaped segments, or the
    /// header doesn't decode/parse as JSON.
    NotJwtShaped,
    /// Header is parseable JSON but `alg` is absent or not a string.
    /// Treated identically to alg-mismatch per §5.3.1: the resolver
    /// must not be reached.
    NoValidAlgField,
    /// Header has a string `alg`, but it isn't `ES256K`.
    AlgNotEs256k(String),
}

/// Defensive header inspection — no `unwrap`, no `?` for the parse
/// cases. All four step-3 outcomes are explicit results the caller
/// dispatches on.
fn pre_check_es256k(token: &str) -> Result<(), PreCheckRejection> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(PreCheckRejection::NotJwtShaped);
    }
    let header_bytes = match URL_SAFE_NO_PAD.decode(parts[0]) {
        Ok(b) => b,
        Err(_) => return Err(PreCheckRejection::NotJwtShaped),
    };
    let header_json: serde_json::Value = match serde_json::from_slice(&header_bytes) {
        Ok(v) => v,
        Err(_) => return Err(PreCheckRejection::NotJwtShaped),
    };
    let alg = match header_json.get("alg").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return Err(PreCheckRejection::NoValidAlgField),
    };
    if alg != "ES256K" {
        return Err(PreCheckRejection::AlgNotEs256k(alg.to_string()));
    }
    Ok(())
}

/// First 8 chars of the token as a debug-correlatable prefix. Tokens
/// here are bearer tokens (session IDs, JWTs); first 8 chars is
/// either an opaque ID prefix or the JWT header's `eyJhbGci...`
/// boilerplate. Either way, not enough to authenticate with on its
/// own.
fn mask_token(token: &str) -> String {
    let head: String = token.chars().take(8).collect();
    if token.chars().count() > 8 {
        format!("{}…", head)
    } else {
        head
    }
}

/// Categorise an HS256 verification failure for logging without
/// echoing the underlying jsonwebtoken error message (which can
/// include token contents).
fn hs256_rejection_category(err: &PdsError) -> &'static str {
    match err {
        PdsError::Authentication(msg) => {
            if msg.contains("expired") {
                "expired"
            } else if msg.contains("signature") {
                "bad-signature"
            } else {
                "invalid"
            }
        }
        _ => "other",
    }
}

/// Per-cause log-line dispatch for `verify_service_jwt` failures
/// (§5.3.5). Each line is distinguishable in a log search; sensitive
/// fields (token, signing keys, internal state) are not emitted.
/// The audience-mismatch line is the §5.5.6 known-limitation
/// diagnostic — both expected and received audiences are visible to
/// the operator.
fn log_service_auth_error(err: &crate::service_auth::ServiceAuthError, expected_aud: &str) {
    use crate::service_auth::ServiceAuthError;
    match err {
        ServiceAuthError::AudienceMismatch { expected, received } => {
            tracing::debug!(
                "service-auth: expected aud={}, received aud={}",
                expected,
                received
            );
            // `expected` from the error == `expected_aud` we passed
            // in — just defensively reference both so the param isn't
            // dead under future refactors.
            let _ = expected_aud;
        }
        ServiceAuthError::Expired => {
            tracing::debug!("service-auth: token expired");
        }
        ServiceAuthError::SignatureVerificationFailed => {
            tracing::debug!("service-auth: signature verification failed");
        }
        ServiceAuthError::ResolverError(detail) => {
            tracing::debug!("service-auth: resolver error: {}", detail);
        }
        ServiceAuthError::InvalidPublicKey(detail) => {
            tracing::debug!("service-auth: invalid public key: {}", detail);
        }
        ServiceAuthError::InvalidSignatureFormat(detail) => {
            tracing::debug!("service-auth: invalid signature format: {}", detail);
        }
        ServiceAuthError::InvalidClaims(detail) => {
            tracing::debug!("service-auth: invalid claims: {}", detail);
        }
        ServiceAuthError::InvalidExpirationWindow(detail) => {
            tracing::debug!("service-auth: invalid expiration window: {}", detail);
        }
        // Pre-check is supposed to reject these before
        // verify_service_jwt is called. If they surface here, the
        // contract is violated — log at warn so it shows up.
        ServiceAuthError::NotJwtShaped(detail) | ServiceAuthError::UnsupportedAlg(detail) => {
            tracing::warn!(
                "service-auth: pre-check leak — verify_service_jwt rejected for {}",
                detail
            );
        }
        ServiceAuthError::MissingOrInvalidAlg => {
            tracing::warn!(
                "service-auth: pre-check leak — verify_service_jwt rejected for missing-or-invalid alg"
            );
        }
        // Cluster 2 Member 2.2 (#144). The match above is exhaustive
        // (no catch-all) by design — adding ServiceAuthError variants
        // FORCES a tracing arm here at compile time. This is
        // tracing-only; the message string is not grep'd by any
        // test/runbook/metric, so detail/wording is free to evolve.
        // The actual wire shape (HTTP 400 + `{"error":"DidTombstoned",
        // ...}`) is produced by the From<ServiceAuthError> for
        // PdsError impl + IntoResponse for PdsError::DidTombstoned
        // (src/error.rs:620-624).
        ServiceAuthError::DidTombstoned(did) => {
            tracing::debug!("service-auth: issuer DID tombstoned: {}", did);
        }
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
pub fn verify_jwt_token(
    token: &str,
    jwt_secret: &str,
) -> Result<jsonwebtoken::TokenData<serde_json::Value>, PdsError> {
    use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};

    let decoding_key = DecodingKey::from_secret(jwt_secret.as_bytes());
    let mut validation = Validation::new(Algorithm::HS256);
    // Allow some clock skew (5 minutes)
    validation.leeway = 300;

    decode::<serde_json::Value>(token, &decoding_key, &validation).map_err(|e| {
        tracing::warn!("JWT verification failed: {}", e);
        match e.kind() {
            jsonwebtoken::errors::ErrorKind::ExpiredSignature => {
                PdsError::Authentication("Token has expired".to_string())
            }
            jsonwebtoken::errors::ErrorKind::InvalidSignature => {
                PdsError::Authentication("Invalid token signature".to_string())
            }
            _ => PdsError::Authentication(format!("Invalid token: {}", e)),
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

        // Try to find OAuth token in database
        let token_info = validate_oauth_token(state, &access_token).await?;

        // DPoP proof-of-possession check (RFC 9449 §7).
        //
        // Tokens that were issued bound to a DPoP key carry a non-NULL
        // `dpop_thumbprint`. On every resource request for those
        // tokens, the request MUST present a DPoP proof whose JWK
        // hashes to the same thumbprint, and whose `ath` claim is
        // `base64url(SHA-256(access_token))` to bind the proof to
        // this specific token (§4.3).
        //
        // Bearer-only tokens (no thumbprint) accept the request
        // without a DPoP header — backward compat for clients that
        // never opted in.
        if let Some(bound_thumbprint) = token_info.dpop_thumbprint.as_deref() {
            let dpop_proof = parts
                .headers
                .get("dpop")
                .ok_or_else(|| {
                    PdsError::Authentication(
                        "DPoP proof required for DPoP-bound token".to_string(),
                    )
                })?
                .to_str()
                .map_err(|_| {
                    PdsError::Authentication("Invalid DPoP header value".to_string())
                })?;

            // Reconstruct the request method/URI the way the proof
            // would have committed to them. parts.uri here is the
            // path-and-query the server received; htu in proof is the
            // canonical request URI minus query string. Build absolute
            // URL from service_url() so the comparison can match what
            // a well-formed client computed.
            let method = parts.method.as_str().to_string();
            let uri = format!(
                "{}{}",
                state.service_url(),
                parts.uri.path()
            );
            let expected_ath = crate::federation::dpop::compute_ath(&access_token);
            let proof_thumbprint = state
                .dpop_verifier
                .verify_dpop_proof(dpop_proof, &method, &uri, Some(&expected_ath))
                .await?;
            if proof_thumbprint != bound_thumbprint {
                return Err(PdsError::Authentication(
                    "DPoP proof key does not match the token's bound thumbprint".to_string(),
                ));
            }
        }

        // Parse scopes
        let scopes = token_info
            .scope
            .parse::<ScopeSet>()
            .map_err(|e| PdsError::Authentication(format!("Invalid token scopes: {}", e)))?;

        let did = token_info.did.clone();

        Ok(OAuthAuthContext {
            did,
            token: token_info,
            scopes,
        })
    }
}

/// Claims extracted from an entryway-issued external access token
/// per Arc 12 §5.3.3.
#[derive(Debug, Clone)]
pub struct AccessTokenClaims {
    /// Subject DID (the user the token authenticates).
    pub did: String,
    /// Issuer DID — the entryway DID per §5.3.3.1.
    pub iss: String,
    /// Audience — must match one of the caller-supplied
    /// `expected_audiences`.
    pub aud: String,
    /// Issued-at unix timestamp.
    pub iat: i64,
    /// Expires-at unix timestamp.
    pub exp: i64,
    /// OAuth scope claim. Optional because some entryway-mint
    /// shapes omit it for service-only tokens.
    pub scope: Option<String>,
    /// OAuth client_id claim. Optional for the same reason.
    pub client_id: Option<String>,
}

/// Verify an external access token issued by the entryway per
/// Arc 12 §5.3.3.
///
/// Performs:
/// 1. JWT-shape pre-check (3 base64url segments).
/// 2. Header decode + `alg == ES256K` verification. (The
///    §5.3.3 algorithm-allowlist guard at the tuple-routing
///    front-door also enforces this; the check here is
///    defense-in-depth so the function is safe to call
///    independently.)
/// 3. Payload decode + claims extraction.
/// 4. ES256K signature verification over `header.payload`
///    against `entryway_jwt_public_key`.
/// 5. Claim validation: `exp` not past, `iat` not future,
///    `aud` ∈ `expected_audiences`. `iss` extracted but
///    NOT trust-checked here (caller is responsible per
///    §5.3.3 routing — this function trusts the public key
///    is the entryway's).
///
/// Returns `PdsError::Authentication(...)` on any failure.
/// On success: `AccessTokenClaims` with did/iss/aud/iat/exp +
/// optional scope/client_id.
///
/// **Signature semantics.** k256 ECDSA over the SHA-256 of
/// `header.payload` (ATProto / JWS ES256K convention).
/// Signature bytes are DER-encoded per ATProto convention
/// (same as `verify_service_jwt`'s signature handling).
pub async fn validate_external_access_token(
    token: &str,
    entryway_jwt_public_key: &k256::ecdsa::VerifyingKey,
    expected_audiences: &[&str],
) -> Result<AccessTokenClaims, PdsError> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use k256::ecdsa::{signature::Verifier, Signature};

    // 1. Shape pre-check.
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(PdsError::Authentication(
            "external access token not JWT-shaped (expected 3 segments)".to_string(),
        ));
    }
    let header_b64 = parts[0];
    let claims_b64 = parts[1];
    let signature_b64 = parts[2];

    // 2. Header decode + alg check.
    let header_bytes = URL_SAFE_NO_PAD.decode(header_b64).map_err(|_| {
        PdsError::Authentication("external access token header base64 decode failed".to_string())
    })?;
    let header_json: serde_json::Value = serde_json::from_slice(&header_bytes).map_err(|_| {
        PdsError::Authentication("external access token header not parseable JSON".to_string())
    })?;
    let alg = header_json
        .get("alg")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            PdsError::Authentication(
                "external access token header missing/invalid alg".to_string(),
            )
        })?;
    if alg != "ES256K" {
        return Err(PdsError::Authentication(format!(
            "external access token alg {:?} not ES256K",
            alg
        )));
    }

    // 3. Payload decode + claim extraction.
    let claims_bytes = URL_SAFE_NO_PAD.decode(claims_b64).map_err(|_| {
        PdsError::Authentication("external access token payload base64 decode failed".to_string())
    })?;
    let claims_json: serde_json::Value = serde_json::from_slice(&claims_bytes).map_err(|_| {
        PdsError::Authentication("external access token payload not parseable JSON".to_string())
    })?;

    let sub = claims_json
        .get("sub")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            PdsError::Authentication("external access token missing sub claim".to_string())
        })?
        .to_string();
    let iss = claims_json
        .get("iss")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            PdsError::Authentication("external access token missing iss claim".to_string())
        })?
        .to_string();
    let aud = claims_json
        .get("aud")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            PdsError::Authentication("external access token missing aud claim".to_string())
        })?
        .to_string();
    let iat = claims_json
        .get("iat")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| {
            PdsError::Authentication(
                "external access token missing/invalid iat claim".to_string(),
            )
        })?;
    let exp = claims_json
        .get("exp")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| {
            PdsError::Authentication(
                "external access token missing/invalid exp claim".to_string(),
            )
        })?;
    let scope = claims_json
        .get("scope")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let client_id = claims_json
        .get("client_id")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    // 4. Signature verification (ES256K, DER-encoded per
    // ATProto convention).
    let signature_bytes = URL_SAFE_NO_PAD.decode(signature_b64).map_err(|_| {
        PdsError::Authentication(
            "external access token signature base64 decode failed".to_string(),
        )
    })?;
    let signature = Signature::from_der(&signature_bytes).map_err(|_| {
        PdsError::Authentication("external access token signature DER parse failed".to_string())
    })?;
    let signing_input = format!("{}.{}", header_b64, claims_b64);
    entryway_jwt_public_key
        .verify(signing_input.as_bytes(), &signature)
        .map_err(|_| {
            PdsError::Authentication(
                "external access token signature verification failed".to_string(),
            )
        })?;

    // 5. Claim validation.
    let now = chrono::Utc::now().timestamp();
    // Allow ±300s skew matching verify_jwt_token's leeway.
    const SKEW_SECS: i64 = 300;
    if exp + SKEW_SECS < now {
        return Err(PdsError::Authentication(
            "external access token expired".to_string(),
        ));
    }
    if iat > now + SKEW_SECS {
        return Err(PdsError::Authentication(
            "external access token iat is in the future".to_string(),
        ));
    }
    if !expected_audiences.contains(&aud.as_str()) {
        return Err(PdsError::Authentication(format!(
            "external access token aud {:?} not in expected audiences {:?}",
            aud, expected_audiences
        )));
    }

    Ok(AccessTokenClaims {
        did: sub,
        iss,
        aud,
        iat,
        exp,
        scope,
        client_id,
    })
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
    .map_err(PdsError::Database)?
    .ok_or_else(|| PdsError::Authentication("Invalid or expired access token".to_string()))?;

    // Check if token is expired
    use sqlx::Row;
    let expires_at: chrono::DateTime<chrono::Utc> = parse_ts(&row.get::<String, _>("expires_at"))?;

    if expires_at < chrono::Utc::now() {
        return Err(PdsError::Authentication(
            "Access token has expired".to_string(),
        ));
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

/// Argon2id password hashing.
///
/// Vendored from the previously embedded `atproto::server_auth::PasswordHasher`
/// because proto-blue is a client SDK and does not include server-side password
/// hashing. Argon2id is the OWASP-recommended algorithm.
pub struct PasswordHasher;

impl PasswordHasher {
    /// Hash a password using Argon2id with a fresh random salt.
    pub fn hash(password: &str) -> Result<String, PdsError> {
        use argon2::password_hash::{rand_core::OsRng, PasswordHasher as _, SaltString};
        let salt = SaltString::generate(&mut OsRng);
        argon2::Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| PdsError::Internal(format!("password hash failed: {}", e)))
    }

    /// Verify a password against a previously stored Argon2 hash.
    ///
    /// Returns `Ok(true)` on a match, `Ok(false)` on a clean mismatch.
    /// Returns `Err` only if the stored hash is malformed.
    pub fn verify(password: &str, hash: &str) -> Result<bool, PdsError> {
        use argon2::password_hash::{PasswordHash, PasswordVerifier};
        let parsed = PasswordHash::new(hash)
            .map_err(|e| PdsError::Internal(format!("malformed password hash: {}", e)))?;
        Ok(argon2::Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok())
    }
}

// ============================================================
// Arc 12 §5.3.4.1 — shared verify helper used by both middleware
// variants (`require_auth_unified` + `require_auth_forwarded`).
// Step 1.3 extracts the tuple-routing logic that landed in
// `require_auth_unified` during Step 0.6.3 into this module so the
// two middleware functions can call it as
// `ctx.verify_jwt_with_allowlist(token, audience_allowlist)` and
// differ only in their allowlist.
// ============================================================

/// §5.3.3 algorithm allowlist. Anything outside this set — including
/// `alg=none` — is rejected before tuple lookup. Single source of
/// truth shared with `validate_external_access_token`'s defensive
/// alg check.
fn alg_in_allowlist(alg: &str) -> bool {
    matches!(alg, "HS256" | "ES256K" | "ES256")
}

/// Decode the JWT header's `alg` (required) and `kid` (optional).
/// Returns the human-readable failure reason on the `Err` side so the
/// caller can log it without echoing token bytes.
fn decode_jwt_header_alg_kid(
    header_b64: &str,
) -> Result<(String, Option<String>), &'static str> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let header_bytes = URL_SAFE_NO_PAD
        .decode(header_b64)
        .map_err(|_| "header base64 decode failed")?;
    let header_json: serde_json::Value =
        serde_json::from_slice(&header_bytes).map_err(|_| "header not parseable JSON")?;
    let alg = header_json
        .get("alg")
        .and_then(|v| v.as_str())
        .ok_or("header missing/invalid alg")?
        .to_string();
    let kid = header_json
        .get("kid")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    Ok((alg, kid))
}

/// Decode the JWT payload's `iss` claim. Missing / non-string iss
/// → `Ok(None)`. Base64/JSON parse failure → `Err`.
fn decode_jwt_iss_only(claims_b64: &str) -> Result<Option<String>, &'static str> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let claims_bytes = URL_SAFE_NO_PAD
        .decode(claims_b64)
        .map_err(|_| "claims base64 decode failed")?;
    let claims_json: serde_json::Value =
        serde_json::from_slice(&claims_bytes).map_err(|_| "claims not parseable JSON")?;
    Ok(claims_json
        .get("iss")
        .and_then(|v| v.as_str())
        .map(str::to_string))
}

/// "Known entryway kid" predicate per §5.3.3. The Aurora-Locus
/// `EntrywayConfig` (§5.4 Step 1.1) does not enumerate kid values —
/// per the design's "kid is a routing hint, not a trust gate"
/// principle, any non-local kid is taken as an entryway-routing hint
/// when entryway mode is configured. Signature verification against
/// `EntrywayConfig.jwt_public_key` is the actual safety floor.
fn is_known_entryway_kid(ctx: &AppContext, kid: &str) -> bool {
    ctx.config.entryway.is_some() && kid != "aurora-local-v1"
}

/// §5.3.4.1 implementation. Called via
/// `AppContext::verify_jwt_with_allowlist`. Routes a bearer token
/// through the §5.3.3 tuple table; the destination routes that check
/// audience (`route_external_verify`, `route_service_auth_fallback`)
/// receive the caller-supplied `audience_allowlist`.
pub async fn verify_jwt_with_allowlist_impl(
    ctx: &AppContext,
    token: &str,
    audience_allowlist: &[&str],
) -> Result<crate::api::middleware::UnifiedAuthContext, PdsError> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return route_opaque_oauth(ctx, token).await;
    }

    let (alg, kid) = match decode_jwt_header_alg_kid(parts[0]) {
        Ok(pair) => pair,
        Err(reason) => {
            tracing::warn!(
                reason = %reason,
                "authentication_failed: jwt header decode"
            );
            crate::metrics::record_error("AuthenticationFailed", "middleware");
            return Err(PdsError::Authentication("Invalid token".to_string()));
        }
    };

    if !alg_in_allowlist(&alg) {
        tracing::warn!(
            alg = %alg,
            "authentication_failed: alg not in allowlist"
        );
        crate::metrics::record_error("AuthenticationFailed", "middleware");
        return Err(PdsError::Authentication("Invalid algorithm".to_string()));
    }

    let iss = decode_jwt_iss_only(parts[1]).unwrap_or(None);

    match (alg.as_str(), kid.as_deref()) {
        ("HS256", Some("aurora-local-v1") | None) => route_local_verify(ctx, token).await,
        ("HS256", Some(unknown_kid)) => {
            tracing::warn!(
                kid = %unknown_kid,
                "authentication_failed: HS256 with unrecognized kid"
            );
            crate::metrics::record_error("AuthenticationFailed", "middleware");
            Err(PdsError::Authentication("Invalid token".to_string()))
        }
        ("ES256K", Some(k)) if is_known_entryway_kid(ctx, k) => {
            route_external_verify(ctx, token, audience_allowlist).await
        }
        ("ES256K", _) | ("ES256", _) => {
            route_service_auth_fallback(ctx, token, iss.as_deref(), audience_allowlist).await
        }
        _ => {
            tracing::warn!(
                alg = %alg,
                kid = ?kid,
                "authentication_failed: unhandled (alg, kid) combination"
            );
            crate::metrics::record_error("AuthenticationFailed", "middleware");
            Err(PdsError::Authentication("Invalid token".to_string()))
        }
    }
}

/// Opaque-bearer dispatch: existing OAuth DB-lookup path. Per §5.3.3,
/// this is the only route opaque-shaped tokens take.
async fn route_opaque_oauth(
    ctx: &AppContext,
    token: &str,
) -> Result<crate::api::middleware::UnifiedAuthContext, PdsError> {
    match validate_oauth_token(ctx, token).await {
        Ok(token_info) => {
            tracing::info!(
                did = %token_info.did,
                client_id = %token_info.client_id,
                auth_type = "oauth",
                "authentication_successful"
            );
            Ok(crate::api::middleware::UnifiedAuthContext::OAuth {
                did: token_info.did,
                scope: token_info.scope,
            })
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "authentication_failed: oauth token invalid"
            );
            crate::metrics::record_error("AuthenticationFailed", "middleware");
            Err(PdsError::Authentication(
                "Invalid or expired token".to_string(),
            ))
        }
    }
}

/// Local-verify dispatch (HS256 + `aurora-local-v1` | absent kid).
/// Aurora-Locus stores minted access tokens in the session table, so
/// this validation is a DB lookup of the full JWT string — the
/// equivalent of an HS256 signature check against the local secret
/// (only this server could have stored that exact byte string).
async fn route_local_verify(
    ctx: &AppContext,
    token: &str,
) -> Result<crate::api::middleware::UnifiedAuthContext, PdsError> {
    match ctx.account_manager.validate_access_token(token).await {
        Ok(session) => {
            tracing::info!(
                did = %session.did,
                is_app_password = session.is_app_password,
                auth_type = "local",
                "authentication_successful"
            );
            Ok(crate::api::middleware::UnifiedAuthContext::Local(session))
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "authentication_failed: local session invalid"
            );
            crate::metrics::record_error("AuthenticationFailed", "middleware");
            Err(PdsError::Authentication(
                "Invalid or expired token".to_string(),
            ))
        }
    }
}

/// ES256K + entryway-kid dispatch. Verifies the JWT against the
/// configured entryway pubkey via `validate_external_access_token`.
/// `audience_allowlist` is honored by the inner function's aud
/// check. When `EntrywayConfig` is `None` this branch is unreachable
/// (`is_known_entryway_kid` returns `false`); the explicit guard
/// here is defense-in-depth.
async fn route_external_verify(
    ctx: &AppContext,
    token: &str,
    audience_allowlist: &[&str],
) -> Result<crate::api::middleware::UnifiedAuthContext, PdsError> {
    let entryway = ctx.config.entryway.as_ref().ok_or_else(|| {
        tracing::warn!("authentication_failed: external entryway-verify reached without EntrywayConfig");
        crate::metrics::record_error("AuthenticationFailed", "middleware");
        PdsError::Authentication("Entryway verification not configured".to_string())
    })?;

    let claims = validate_external_access_token(
        token,
        &entryway.jwt_public_key,
        audience_allowlist,
    )
    .await?;

    tracing::info!(
        did = %claims.did,
        iss = %claims.iss,
        auth_type = "entryway_external",
        "authentication_successful"
    );
    Ok(crate::api::middleware::UnifiedAuthContext::CrossPDS { did: claims.did })
}

/// Trusted service-auth fallback (§5.3.3.1).
///
/// Order is load-bearing: iss-trust check runs *before* any PLC fetch
/// or signature verification, so unknown / non-DID / empty iss
/// uniformly reject without any network call.
///
/// The `audience_allowlist` is iterated; verify_service_jwt is called
/// once per audience and the first success is accepted. Per §5.3.4
/// `require_auth_unified` passes a single audience (the PDS DID);
/// `require_auth_forwarded` passes both (PDS DID + entryway DID).
async fn route_service_auth_fallback(
    ctx: &AppContext,
    token: &str,
    iss: Option<&str>,
    audience_allowlist: &[&str],
) -> Result<crate::api::middleware::UnifiedAuthContext, PdsError> {
    let iss_str = iss.unwrap_or("");
    if !ctx.is_trusted_iss(iss_str) {
        tracing::warn!(
            iss = %iss_str,
            "authentication_failed: iss not in trusted-iss allowlist"
        );
        crate::metrics::record_error("AuthenticationFailed", "middleware");
        return Err(PdsError::Authentication("Invalid token".to_string()));
    }

    let service_auth = ctx.federation_auth.as_ref().ok_or_else(|| {
        tracing::warn!("authentication_failed: federation_auth not configured");
        crate::metrics::record_error("AuthenticationFailed", "middleware");
        PdsError::Authentication("Service auth not configured".to_string())
    })?;

    let mut last_err: Option<crate::error::PdsError> = None;
    for &aud in audience_allowlist {
        match service_auth.authenticator.verify_service_jwt(token, aud).await {
            Ok(claims) => {
                if let Some(nonce_store) = &ctx.nonce_store {
                    match nonce_store.check_and_record(&claims.jti).await {
                        Ok(true) => {
                            tracing::info!(
                                did = %claims.iss,
                                aud = %aud,
                                auth_type = "cross_pds",
                                "authentication_successful"
                            );
                            return Ok(crate::api::middleware::UnifiedAuthContext::CrossPDS {
                                did: claims.iss,
                            });
                        }
                        Ok(false) => {
                            tracing::warn!(jti = %claims.jti, "service_auth_failed: replay_attack");
                            crate::metrics::record_error("ServiceAuthReplayAttack", "middleware");
                            return Err(PdsError::Authentication(
                                "Replay attack detected".to_string(),
                            ));
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "service_auth_failed: nonce_check_error");
                            return Err(PdsError::Authentication("Invalid token".to_string()));
                        }
                    }
                } else {
                    tracing::warn!("service_auth: nonce_store_not_available, replay_prevention_disabled");
                    return Ok(crate::api::middleware::UnifiedAuthContext::CrossPDS {
                        did: claims.iss,
                    });
                }
            }
            Err(e) => {
                tracing::debug!(aud = %aud, error = %e, "service_auth: aud-allowlist iteration failed");
                last_err = Some(e);
                continue;
            }
        }
    }

    if let Some(e) = last_err {
        tracing::warn!(error = %e, "service_auth_failed");
        // Cluster 2 Member 2.2 (#144) — site 8 of 8 (cross-PDS
        // fallback, the only live caller of the federation method
        // `service_auth.authenticator.verify_service_jwt`). Pre-#144
        // this final return unconditionally emitted
        // `PdsError::Authentication("Invalid token")`, discarding the
        // typed `last_err` that site 1's federation-method fix
        // propagated. After the fix, when `last_err` is
        // `PdsError::DidTombstoned`, propagate it unchanged so
        // IntoResponse maps to HTTP 400 `{"error": "DidTombstoned",
        // ...}`. All other typed variants preserve today's hardcoded
        // `"Invalid token"` Authentication wrap (the aud-allowlist
        // iteration uses string-equality on the wrap message; not
        // grep'd by any test/runbook/metric, but conservative).
        crate::metrics::record_error("AuthenticationFailed", "middleware");
        return match e {
            PdsError::DidTombstoned(_) => Err(e),
            _ => Err(PdsError::Authentication("Invalid token".to_string())),
        };
    } else {
        tracing::warn!("service_auth_failed: empty audience_allowlist");
    }
    crate::metrics::record_error("AuthenticationFailed", "middleware");
    Err(PdsError::Authentication("Invalid token".to_string()))
}

#[cfg(test)]
mod password_tests {
    use super::PasswordHasher;

    #[test]
    fn hash_then_verify_correct_returns_true() {
        let hash = PasswordHasher::hash("hunter2_correct_horse").unwrap();
        assert!(PasswordHasher::verify("hunter2_correct_horse", &hash).unwrap());
    }

    #[test]
    fn verify_wrong_password_returns_false() {
        let hash = PasswordHasher::hash("right").unwrap();
        assert!(!PasswordHasher::verify("wrong", &hash).unwrap());
    }

    #[test]
    fn verify_malformed_hash_errors() {
        let result = PasswordHasher::verify("anything", "not-a-real-hash");
        assert!(result.is_err());
    }

    #[test]
    fn two_hashes_of_same_password_differ_due_to_salt() {
        let h1 = PasswordHasher::hash("same").unwrap();
        let h2 = PasswordHasher::hash("same").unwrap();
        assert_ne!(h1, h2);
        assert!(PasswordHasher::verify("same", &h1).unwrap());
        assert!(PasswordHasher::verify("same", &h2).unwrap());
    }
}

#[cfg(test)]
mod identity_resolver_slot_smoke_tests {
    //! Step 0.6 smoke test — proves the `AppContext::identity_resolver`
    //! slot type accepts a non-`IdentityResolver` impl of
    //! `IdentityResolverApi`. Step 2's extractor tests rely on
    //! constructing an `AppContext` with a counting mock swapped in;
    //! this test only proves the slot's type permits that swap.
    use crate::identity::resolver::test_doubles::MockIdentityResolver;
    use crate::identity::IdentityResolverApi;
    use std::sync::Arc;

    #[test]
    fn arc_of_mock_coerces_into_identity_resolver_slot() {
        let mock: Arc<MockIdentityResolver> = Arc::new(MockIdentityResolver::new());
        let slot: Arc<dyn IdentityResolverApi> = mock;
        // Compile-time assertion of trait-object coercion is the
        // payload; touching `slot` keeps the binding non-trivially
        // used so the test isn't elided as a no-op.
        assert!(Arc::strong_count(&slot) >= 1);
    }
}

#[cfg(test)]
mod admin_auth_third_path_tests {
    //! Step 2 (§5.4.2) tests for the ES256K third path on
    //! `AdminAuthContext`. Exercises `admin_auth_from_token` directly
    //! so the test surface is the auth logic, not HTTP plumbing.
    //!
    //! Pre-check tests (§5.3.1) are load-bearing: each one must
    //! observe `mock.resolve_did_calls() == 0`. A non-zero reading
    //! means the alg boundary leaked and the design needs revision.
    use super::admin_auth_from_token;
    use crate::admin::roles::Role;
    use crate::config::*;
    use crate::context::AppContext;
    use crate::error::PdsError;
    use crate::identity::did_document::{DidDocument, VerificationMethod};
    use crate::identity::resolver::test_doubles::MockIdentityResolver;
    use crate::service_auth::create_service_jwt;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use k256::ecdsa::{signature::Signer, Signature, SigningKey, VerifyingKey};
    use std::path::PathBuf;
    use std::sync::Arc;
    use tempfile::tempdir;
    // `traced_test` injects a local `logs_contain(val: &str) -> bool`
    // function into each test scope (per tracing-test 0.2 macro).
    use tracing_test::traced_test;

    const TEST_SERVICE_DID: &str = "did:web:localhost";
    const TEST_ISS: &str = "did:plc:test1234";

    /// Match the test-context construction used by `aurora_admin`'s
    /// tests so all managers wire up correctly. Returns an owned
    /// AppContext whose `identity_resolver` slot is replaced by an
    /// `Arc<MockIdentityResolver>` after construction; the mock is
    /// returned alongside so the test can script DIDs and read
    /// invocation counters.
    async fn build_test_ctx_with_mock() -> (AppContext, Arc<MockIdentityResolver>) {
        let dir = tempdir().unwrap().keep();
        let db_path = dir.join("test.db");
        let config = ServerConfig {
            service: ServiceConfig {
                hostname: "localhost".to_string(),
                port: 2583,
                service_did: TEST_SERVICE_DID.to_string(),
                version: "0.1.0-test".to_string(),
                blob_upload_limit: 5_242_880,
                public_url: None,
                max_blob_fetch_size: 50_000_000,
                blob_fetch_timeout_seconds: 30,
                blob_fetch_max_retries: 3,
                accepting_imports: true,
                max_import_size: None,
            },
            storage: StorageConfig {
                data_directory: dir.clone(),
                account_db: db_path.clone(),
                sequencer_db: dir.join("sequencer.db"),
                did_cache_db: dir.join("did_cache.db"),
                actor_store_directory: dir.join("actors"),
                blobstore: BlobstoreConfig::Disk {
                    location: dir.join("blobs"),
                    tmp_location: dir.join("temp"),
                },
            },
            database: Default::default(),
            authentication: AuthConfig {
                jwt_secret: "test-secret-key-aurora-admin-test-32xx".to_string(),
                repo_signing_key: "a".repeat(64),
                plc_rotation_key: "b".repeat(64),
                oauth: OAuthConfig {
                    client_id: "http://localhost:3000/client-metadata.json".to_string(),
                    redirect_uri: "http://localhost:3000/oauth/callback".to_string(),
                    pds_url: "https://bsky.social".to_string(),
                },
                jwt_sunset_date: "Sat, 31 Dec 2024 23:59:59 GMT".to_string(),
                oauth_migration_guide_url:
                    "https://docs.atproto.com/guides/oauth-migration".to_string(),
                oauth_features: Default::default(),
            },
            identity: IdentityConfig {
                did_plc_url: "https://plc.directory".to_string(),
                service_handle_domains: vec![".localhost".to_string()],
                did_cache_stale_ttl: 3600,
                did_cache_max_ttl: 86400,
                recovery_did_key: None,
            },
            email: None,
            invites: InviteConfig {
                required: false,
                interval: 604800,
                epoch: "2024-01-01T00:00:00Z".to_string(),
            },
            rate_limit: RateLimitConfig {
                enabled: false,
                global_requests_per_minute: 3000,
                exempt_admin_assets: true,
            buckets_retention_days: 7,
            },
            logging: LoggingConfig {
                level: "info".to_string(),
            },
            federation: FederationConfig {
                enabled: false,
                relay_urls: vec![],
                appview_url: None,
                firehose_enabled: false,
                crawl_enabled: false,
                public_url: Some("http://localhost:2583".to_string()),
                peer_pds: vec![],
            },
            validation_mode: PathBuf::from("required")
                .into_os_string()
                .to_string_lossy()
                .parse()
                .unwrap_or(crate::validation::ValidationMode::Required),
            distributed_state_mode: Default::default(),
            maintenance_pool: Default::default(),
            gc_sweep: Default::default(),
            bind_audit_orphan_marker: Default::default(),
            blob_metadata: Default::default(),
            entryway: None,
            lexicon: crate::config::LexiconConfig::default(),
            kryphocron: crate::config::KryphocronConfig::default(),
        };
        let mut ctx = AppContext::new(
            config,
            Arc::new(crate::api::registry::RouteRegistry::default()),
        )
        .await
        .unwrap();
        let mock: Arc<MockIdentityResolver> = Arc::new(MockIdentityResolver::new());
        ctx.identity_resolver = mock.clone();
        (ctx, mock)
    }

    fn multibase_encode(verifying_key: &VerifyingKey) -> String {
        let sec1 = verifying_key.to_encoded_point(true);
        let mut buf = vec![0xe7_u8, 0x01_u8];
        buf.extend_from_slice(sec1.as_bytes());
        format!("z{}", bs58::encode(&buf).into_string())
    }

    fn did_doc_with_key(did: &str, verifying_key: &VerifyingKey) -> DidDocument {
        DidDocument {
            context: None,
            id: did.to_string(),
            also_known_as: vec![],
            service: vec![],
            verification_method: vec![VerificationMethod {
                id: format!("{}#atproto", did),
                key_type: "Multikey".to_string(),
                controller: did.to_string(),
                public_key_multibase: Some(multibase_encode(verifying_key)),
            }],
        }
    }

    fn manual_jwt(header_json: &str, claims_json: &str, signature: &[u8]) -> String {
        let header_b64 = URL_SAFE_NO_PAD.encode(header_json.as_bytes());
        let claims_b64 = URL_SAFE_NO_PAD.encode(claims_json.as_bytes());
        let sig_b64 = URL_SAFE_NO_PAD.encode(signature);
        format!("{}.{}.{}", header_b64, claims_b64, sig_b64)
    }

    fn future_exp() -> i64 {
        chrono::Utc::now().timestamp() + 60
    }

    fn past_exp() -> i64 {
        chrono::Utc::now().timestamp() - 60
    }

    fn well_formed_claims_json(iss: &str, aud: &str, exp: i64) -> String {
        format!(
            r#"{{"iss":"{}","aud":"{}","exp":{}}}"#,
            iss, aud, exp
        )
    }

    /// Construct a fresh ES256K signing keypair, script the resolver
    /// with a matching DID document under `iss`, and return the
    /// signing key bytes ready for `create_service_jwt`.
    fn script_iss_with_fresh_key(
        mock: &MockIdentityResolver,
        iss: &str,
    ) -> k256::ecdsa::SigningKey {
        let signing_key = SigningKey::random(&mut rand::thread_rng());
        let verifying_key = *signing_key.verifying_key();
        mock.script_did(iss, did_doc_with_key(iss, &verifying_key));
        signing_key
    }

    // ---------- Happy path ----------

    #[traced_test]
    #[tokio::test]
    async fn extracts_service_auth_identity_with_valid_role() {
        let (ctx, mock) = build_test_ctx_with_mock().await;
        let signing_key = script_iss_with_fresh_key(&mock, TEST_ISS);
        ctx.admin_role_manager
            .grant_role(TEST_ISS, Role::Admin, "did:plc:bootstrap", None)
            .await
            .expect("grant_role");

        let token = create_service_jwt(
            TEST_ISS,
            TEST_SERVICE_DID,
            Some(60),
            None,
            &signing_key.to_bytes(),
        )
        .expect("create_service_jwt");

        let auth = admin_auth_from_token(&ctx, &token)
            .await
            .expect("happy path");

        assert_eq!(auth.did, TEST_ISS);
        assert_eq!(auth.role, Role::Admin);
        assert!(mock.resolve_did_calls() >= 1);
    }

    // ---------- Authorization (403) ----------

    #[traced_test]
    #[tokio::test]
    async fn rejects_with_403_when_role_lookup_returns_none() {
        let (ctx, mock) = build_test_ctx_with_mock().await;
        let signing_key = script_iss_with_fresh_key(&mock, TEST_ISS);
        // No grant_role call — the DID has no admin role.

        let token = create_service_jwt(
            TEST_ISS,
            TEST_SERVICE_DID,
            Some(60),
            None,
            &signing_key.to_bytes(),
        )
        .unwrap();

        let result = admin_auth_from_token(&ctx, &token).await;

        match result {
            Err(PdsError::Authorization(_)) => {}
            other => panic!("expected Authorization (403), got {:?}", other),
        }
        assert!(logs_contain(&format!("authorization: DID={} has no role", TEST_ISS)));
    }

    // ---------- 401 — verify_service_jwt rejections ----------

    #[traced_test]
    #[tokio::test]
    async fn rejects_with_401_on_audience_mismatch_with_log() {
        let (ctx, mock) = build_test_ctx_with_mock().await;
        let signing_key = script_iss_with_fresh_key(&mock, TEST_ISS);

        // Token's `aud` is intentionally wrong.
        let token = create_service_jwt(
            TEST_ISS,
            "did:plc:wrongAudience",
            Some(60),
            None,
            &signing_key.to_bytes(),
        )
        .unwrap();

        let result = admin_auth_from_token(&ctx, &token).await;

        match result {
            Err(PdsError::Authentication(_)) => {}
            other => panic!("expected Authentication (401), got {:?}", other),
        }
        // The §5.5.6 known-limitation diagnostic: both audiences
        // must be visible to the operator.
        assert!(logs_contain(&format!("expected aud={}", TEST_SERVICE_DID)));
        assert!(logs_contain("received aud=did:plc:wrongAudience"));
    }

    #[traced_test]
    #[tokio::test]
    async fn rejects_with_401_on_expired_token() {
        let (ctx, mock) = build_test_ctx_with_mock().await;
        let signing_key = script_iss_with_fresh_key(&mock, TEST_ISS);
        let verifying_key = *signing_key.verifying_key();
        // Re-script with the verifying key just to be defensive
        // about the helper's state — already done by
        // script_iss_with_fresh_key, but harmless.
        let _ = verifying_key;

        // create_service_jwt rejects past-exp via claims.validate;
        // assemble manually with a real ES256K signature so the
        // path threads through resolver + signature verify and
        // fails only at the final expiry check.
        let header = r#"{"alg":"ES256K","typ":"JWT"}"#;
        let claims = well_formed_claims_json(TEST_ISS, TEST_SERVICE_DID, past_exp());
        let header_b64 = URL_SAFE_NO_PAD.encode(header.as_bytes());
        let claims_b64 = URL_SAFE_NO_PAD.encode(claims.as_bytes());
        let signing_input = format!("{}.{}", header_b64, claims_b64);
        let sig: Signature = signing_key.sign(signing_input.as_bytes());
        let sig_b64 = URL_SAFE_NO_PAD.encode(sig.to_der().as_bytes());
        let token = format!("{}.{}.{}", header_b64, claims_b64, sig_b64);

        let result = admin_auth_from_token(&ctx, &token).await;

        match result {
            Err(PdsError::Authentication(_)) => {}
            other => panic!("expected Authentication (401), got {:?}", other),
        }
        assert!(logs_contain("service-auth: token expired"));
    }

    // ---------- 401 — pre-check rejections ----------
    //
    // Each pre-check test asserts `mock.resolve_did_calls() == 0` —
    // load-bearing per §5.3.1 / §5.4.2. A non-zero reading means the
    // alg boundary leaked.

    #[traced_test]
    #[tokio::test]
    async fn rejects_with_401_on_alg_mismatch_pre_check_skips_resolver() {
        let (ctx, mock) = build_test_ctx_with_mock().await;
        let header = r#"{"alg":"RS256","typ":"JWT"}"#;
        let claims = well_formed_claims_json(TEST_ISS, TEST_SERVICE_DID, future_exp());
        let token = manual_jwt(header, &claims, b"junk-signature-bytes");

        let result = admin_auth_from_token(&ctx, &token).await;

        match result {
            Err(PdsError::Authentication(_)) => {}
            other => panic!("expected Authentication (401), got {:?}", other),
        }
        assert!(logs_contain("service-auth pre-check: alg=RS256 not ES256K"));
        assert_eq!(
            mock.resolve_did_calls(),
            0,
            "pre-check leaked: resolver was reached on alg-mismatch path"
        );
    }

    #[traced_test]
    #[tokio::test]
    async fn rejects_with_401_on_opaque_non_jwt_pre_check_skips_resolver() {
        let (ctx, mock) = build_test_ctx_with_mock().await;
        let token = "not-a-jwt";

        let result = admin_auth_from_token(&ctx, token).await;

        match result {
            Err(PdsError::Authentication(_)) => {}
            other => panic!("expected Authentication (401), got {:?}", other),
        }
        assert!(logs_contain("service-auth pre-check: token is not JWT-shaped"));
        assert_eq!(
            mock.resolve_did_calls(),
            0,
            "pre-check leaked: resolver was reached on non-JWT-shaped token path"
        );
    }

    #[traced_test]
    #[tokio::test]
    async fn rejects_with_401_on_missing_alg_field_pre_check_skips_resolver() {
        let (ctx, mock) = build_test_ctx_with_mock().await;
        let header = r#"{"typ":"JWT"}"#;
        let claims = well_formed_claims_json(TEST_ISS, TEST_SERVICE_DID, future_exp());
        let token = manual_jwt(header, &claims, b"junk-signature-bytes");

        let result = admin_auth_from_token(&ctx, &token).await;

        match result {
            Err(PdsError::Authentication(_)) => {}
            other => panic!("expected Authentication (401), got {:?}", other),
        }
        assert!(logs_contain(
            "service-auth pre-check: header lacks valid alg field"
        ));
        assert_eq!(
            mock.resolve_did_calls(),
            0,
            "pre-check leaked: resolver was reached on missing-alg path"
        );
    }

    #[traced_test]
    #[tokio::test]
    async fn rejects_with_401_on_non_string_alg_pre_check_skips_resolver() {
        let (ctx, mock) = build_test_ctx_with_mock().await;
        let header = r#"{"alg":123,"typ":"JWT"}"#;
        let claims = well_formed_claims_json(TEST_ISS, TEST_SERVICE_DID, future_exp());
        let token = manual_jwt(header, &claims, b"junk-signature-bytes");

        let result = admin_auth_from_token(&ctx, &token).await;

        match result {
            Err(PdsError::Authentication(_)) => {}
            other => panic!("expected Authentication (401), got {:?}", other),
        }
        assert!(logs_contain(
            "service-auth pre-check: header lacks valid alg field"
        ));
        assert_eq!(
            mock.resolve_did_calls(),
            0,
            "pre-check leaked: resolver was reached on non-string-alg path"
        );
    }

    // ---------- Layer-2 regression ----------

    /// Confirms the layer-2 HS256 admin path still works post-Step-2
    /// fall-through refactor. The token is a vanilla HS256 JWT
    /// signed with the test JWT secret with `scope=admin`; layer 1
    /// fails (no local session row), layer 2 succeeds, role lookup
    /// succeeds.
    #[traced_test]
    #[tokio::test]
    async fn layer_2_hs256_admin_still_works_after_fall_through_refactor() {
        let (ctx, mock) = build_test_ctx_with_mock().await;
        ctx.admin_role_manager
            .grant_role("did:plc:hs256admin", Role::Admin, "did:plc:bootstrap", None)
            .await
            .expect("grant_role");

        // Build an HS256 admin JWT.
        let secret = &ctx.config.authentication.jwt_secret;
        let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
        let exp = chrono::Utc::now().timestamp() + 60;
        let claims = serde_json::json!({
            "sub": "did:plc:hs256admin",
            "scope": "admin",
            "exp": exp,
        });
        let token = jsonwebtoken::encode(
            &header,
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes()),
        )
        .unwrap();

        let auth = admin_auth_from_token(&ctx, &token).await.expect("layer 2");
        assert_eq!(auth.did, "did:plc:hs256admin");
        assert_eq!(auth.role, Role::Admin);
        // Layer 2 short-circuited; the resolver was never reached.
        assert_eq!(
            mock.resolve_did_calls(),
            0,
            "layer 2 success must not reach the identity resolver"
        );
    }

    // ---------- §8.1.6 role-change invalidation (satisfied-by-architecture) ----------

    /// Verification gate for design §8.1.6 (Arc E first wave, #267): a
    /// mid-session role change takes effect on the operator's *next
    /// request* with no operator action and no token re-issuance.
    ///
    /// The design assumed roles were embedded in the session token, so it
    /// specified a `401 token-stale` -> `refresh-required` -> transparent-
    /// refresh dance to pick up new claims. Aurora-Locus never embeds the
    /// role: `finalize_admin_role` resolves it live from `admin_role_manager
    /// .get_role(did)` on every request (the token only carries `scope`).
    /// So the gate is met structurally — there is no stale claim to
    /// invalidate. This test mints ONE token and reuses it across an
    /// upgrade-equivalent change and a full revoke, asserting each takes
    /// effect immediately on the very next `admin_auth_from_token` call.
    #[traced_test]
    #[tokio::test]
    async fn role_change_takes_effect_on_next_request_without_reauth() {
        let (ctx, _mock) = build_test_ctx_with_mock().await;
        let did = "did:plc:rolechange";

        ctx.admin_role_manager
            .grant_role(did, Role::Moderator, "did:plc:bootstrap", None)
            .await
            .expect("initial grant");

        // One token for the whole "session" — never re-minted below.
        let secret = &ctx.config.authentication.jwt_secret;
        let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
        let exp = chrono::Utc::now().timestamp() + 3600;
        let claims = serde_json::json!({ "sub": did, "scope": "admin", "exp": exp });
        let token = jsonwebtoken::encode(
            &header,
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes()),
        )
        .unwrap();

        // Baseline: the session resolves to the granted role.
        let auth = admin_auth_from_token(&ctx, &token).await.expect("baseline");
        assert_eq!(auth.role, Role::Moderator);

        // SuperAdmin changes the role mid-session (revoke + re-grant is the
        // role-change path — a second grant on an active role conflicts).
        ctx.admin_role_manager
            .revoke_role(did, "did:plc:superadmin", None)
            .await
            .expect("revoke for change");
        ctx.admin_role_manager
            .grant_role(did, Role::Admin, "did:plc:superadmin", None)
            .await
            .expect("re-grant elevated");

        // Same token, next request: the new role is in effect immediately.
        let auth = admin_auth_from_token(&ctx, &token)
            .await
            .expect("post-change");
        assert_eq!(
            auth.role,
            Role::Admin,
            "role change must take effect on next request without re-auth"
        );

        // Full revocation also takes effect immediately: next request 403s.
        ctx.admin_role_manager
            .revoke_role(did, "did:plc:superadmin", None)
            .await
            .expect("full revoke");
        match admin_auth_from_token(&ctx, &token).await {
            Err(PdsError::Authorization(_)) => {}
            other => panic!("expected Authorization (403) after revoke, got {:?}", other),
        }
    }

    // ---------- §8.1.7 operator-session enforcement in the auth path (#271) ----------

    /// Mint an HS256 admin token carrying a `sid` claim, for the
    /// operator-session enforcement tests below.
    fn admin_token_with_sid(secret: &str, did: &str, sid: &str) -> String {
        let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
        let claims = serde_json::json!({
            "sub": did,
            "scope": "admin",
            "exp": chrono::Utc::now().timestamp() + 3600,
            "sid": sid,
        });
        jsonwebtoken::encode(
            &header,
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes()),
        )
        .unwrap()
    }

    /// A token bound to a live operator session authenticates, and the
    /// per-request lookup bumps the session's last-active stamp.
    #[traced_test]
    #[tokio::test]
    async fn admin_token_with_live_session_is_accepted() {
        let (ctx, _mock) = build_test_ctx_with_mock().await;
        let did = "did:plc:liveop";
        ctx.admin_role_manager
            .grant_role(did, Role::Admin, "did:plc:bootstrap", None)
            .await
            .expect("grant_role");
        let sid = ctx
            .operator_session_store
            .create(did, Some("203.0.113.9"), None, "rid-1", chrono::Duration::days(30))
            .await
            .expect("create session");

        let token = admin_token_with_sid(&ctx.config.authentication.jwt_secret, did, &sid);
        let auth = admin_auth_from_token(&ctx, &token).await.expect("live session");
        assert_eq!(auth.role, Role::Admin);
        assert_eq!(auth.session.session_id, sid, "session_id reflects the sid");
    }

    /// The gate that backs SuperAdmin force-logout (#273): once a session is
    /// revoked, a token bound to it is rejected on the very next request even
    /// though its signature, scope, and role are all still valid.
    #[traced_test]
    #[tokio::test]
    async fn admin_token_with_revoked_session_is_rejected() {
        let (ctx, _mock) = build_test_ctx_with_mock().await;
        let did = "did:plc:revokedop";
        ctx.admin_role_manager
            .grant_role(did, Role::Admin, "did:plc:bootstrap", None)
            .await
            .expect("grant_role");
        let sid = ctx
            .operator_session_store
            .create(did, None, None, "rid-1", chrono::Duration::days(30))
            .await
            .expect("create session");
        let token = admin_token_with_sid(&ctx.config.authentication.jwt_secret, did, &sid);

        // Live first.
        admin_auth_from_token(&ctx, &token).await.expect("live before revoke");

        // Simulate the #273 force-logout writer.
        sqlx::query("UPDATE operator_session SET revoked = TRUE WHERE id = $1")
            .bind(&sid)
            .execute(&ctx.account_db)
            .await
            .expect("revoke");

        match admin_auth_from_token(&ctx, &token).await {
            Err(PdsError::Authentication(_)) => {}
            other => panic!("expected Authentication (401) after revoke, got {:?}", other),
        }
    }

    /// A token whose `sid` names no session row (stale/forged) is rejected —
    /// the store returns false for an unknown id.
    #[traced_test]
    #[tokio::test]
    async fn admin_token_with_unknown_session_is_rejected() {
        let (ctx, _mock) = build_test_ctx_with_mock().await;
        let did = "did:plc:ghostop";
        ctx.admin_role_manager
            .grant_role(did, Role::Admin, "did:plc:bootstrap", None)
            .await
            .expect("grant_role");
        let token = admin_token_with_sid(
            &ctx.config.authentication.jwt_secret,
            did,
            "00000000-0000-0000-0000-000000000000",
        );
        match admin_auth_from_token(&ctx, &token).await {
            Err(PdsError::Authentication(_)) => {}
            other => panic!("expected Authentication (401) for unknown sid, got {:?}", other),
        }
    }
}

#[cfg(test)]
mod authenticated_did_tests {
    use super::AuthenticatedDid;

    #[test]
    fn from_authenticated_round_trips() {
        let d = AuthenticatedDid::from_authenticated("did:plc:abc123".to_string());
        assert_eq!(d.value(), "did:plc:abc123");
        // Accepts &str via Into.
        let d2 = AuthenticatedDid::from_authenticated("did:plc:abc123");
        assert_eq!(d, d2);
    }
}
