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
            let session = ValidatedSession {
                did: did.clone(),
                session_id: format!("jwt-{}", Uuid::new_v4()),
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
            return Err(PdsError::Authentication(format!(
                "service-auth verification failed: {}",
                e
            )));
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
                auto_stream_events: false,
            },
            validation_mode: PathBuf::from("required")
                .into_os_string()
                .to_string_lossy()
                .parse()
                .unwrap_or(crate::validation::ValidationMode::Required),
            distributed_state_mode: Default::default(),
            maintenance_pool: Default::default(),
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
}
