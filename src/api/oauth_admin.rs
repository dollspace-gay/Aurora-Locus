//! OAuth-based admin authentication endpoints.
//!
//! Drives the admin login ceremony against Aurora's OWN authorization server
//! with the Aurora-owned [`AdminOAuthClient`] (chainlink #439) — no
//! proto-blue-oauth in the loop, so the flow does not inherit that client's
//! DPoP-`exp` non-compliance (RFC 9449 §4.2). The flow:
//!
//! 1. `/admin-oauth/login` — build the client, generate PKCE + state, push a
//!    PAR to our own AS, stash the PKCE verifier under the `state` parameter,
//!    redirect the browser to the authorize URL.
//! 2. `/admin-oauth/callback` — look up the stash, exchange the code for a
//!    (throwaway) DPoP-bound token, resolve the authenticated DID from that
//!    token via the loopback validation path, verify the DID is a local admin,
//!    mint a real PDS account session, render a small HTML page that stows the
//!    session tokens in localStorage.
//! 3. `/oauth/client-metadata.json` — public client-metadata document the AS
//!    fetches to resolve + trust the client during PAR.
use crate::AppContext;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Redirect,
    routing::{get, post},
    Json, Router,
};
use crate::oauth_client::admin::AdminOAuthClient;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// In-memory stash of in-flight OAuth flows.
///
/// Keyed on the `state` parameter the AS will echo back. In production
/// this should be a distributed store (Redis, etc.) so flows survive
/// restarts and load-balance across replicas.
#[derive(Clone)]
pub struct OAuthStateStore {
    states: Arc<RwLock<HashMap<String, OAuthStateData>>>,
}

impl Default for OAuthStateStore {
    fn default() -> Self {
        Self::new()
    }
}

impl OAuthStateStore {
    pub fn new() -> Self {
        Self {
            states: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn store(&self, state: String, data: OAuthStateData) {
        let mut states = self.states.write().await;
        states.insert(state, data);
    }

    pub async fn get(&self, state: &str) -> Option<OAuthStateData> {
        let mut states = self.states.write().await;
        states.remove(state)
    }
}

/// Per-flow data we need to remember between `/login` and `/callback`.
///
/// The DPoP key is deliberately NOT carried across the round-trip: Aurora's AS
/// does not bind the PAR proof's key to the issued token (only the
/// token-exchange proof's key is bound), and the admin flow discards the OAuth
/// token immediately after resolving the DID — so a fresh ephemeral key at the
/// callback is sufficient and correct.
#[derive(Clone)]
pub struct OAuthStateData {
    /// PKCE verifier — replayed at the callback to complete the code exchange.
    pub code_verifier: String,
    /// The redirect_uri bound to this flow; must match at the token exchange.
    pub redirect_uri: String,
    /// Optional handle hint, retained only for tracing.
    #[allow(dead_code)]
    pub handle: Option<String>,
}

/// Build OAuth routes
pub fn routes(state_store: OAuthStateStore) -> Router<AppContext> {
    let oauth_router = Router::new()
        .route("/admin-oauth/login", get(initiate_oauth))
        .route("/admin-oauth/password-login", post(password_login_gate))
        .route("/admin-oauth/callback", get(handle_oauth_callback))
        .route("/admin-oauth/refresh", post(handle_refresh))
        .layer(axum::Extension(state_store));

    Router::new()
        .merge(oauth_router)
        .route("/oauth/client-metadata.json", get(client_metadata))
}

/// Mint a 24h HS256 `scope=admin` access JWT for `did`, carrying the
/// operator-session id `sid` (§8.1.7 / #271). Shared by the OAuth callback
/// (AS-only admin login) and `/admin-oauth/refresh` so both paths emit a
/// byte-identical access-token shape — and so a refreshed token preserves
/// the session's `sid`, keeping the per-request session lookup continuous
/// across refreshes. The auth path treats the `sid` as optional: tokens
/// minted before #271 simply have none and take the legacy stateless path.
fn mint_admin_access_jwt(
    jwt_secret: &str,
    did: &str,
    sid: Option<&str>,
) -> Result<String, jsonwebtoken::errors::Error> {
    use jsonwebtoken::{encode, EncodingKey, Header};
    use serde_json::json;

    let now = chrono::Utc::now().timestamp();
    let mut claims = json!({
        "sub": did,
        "iat": now,
        "exp": now + 86400, // 24 hours
        "scope": "admin",
    });
    // Only stamp `sid` when there's a session to bind to. A legacy refresh
    // token minted before #271 has none; its refreshed access token stays
    // sid-less and takes the auth path's legacy stateless branch.
    if let Some(sid) = sid {
        claims["sid"] = serde_json::Value::String(sid.to_string());
    }
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(jwt_secret.as_bytes()),
    )
}

/// Mint an HS256 `scope=refresh` JWT carrying the operator-session id `sid`
/// and rotation-chain id `rid` (#271/#272). `exp` is the absolute expiry
/// (unix seconds): a rotated refresh token keeps the *original* session's
/// expiry rather than sliding it forward, so a refresh token can never
/// outlive its operator_session row. Shared by login and the rotation path.
fn mint_admin_refresh_jwt(
    jwt_secret: &str,
    did: &str,
    sid: &str,
    rid: &str,
    exp: i64,
) -> Result<String, jsonwebtoken::errors::Error> {
    use jsonwebtoken::{encode, EncodingKey, Header};
    use serde_json::json;

    let now = chrono::Utc::now().timestamp();
    let claims = json!({
        "sub": did,
        "iat": now,
        "exp": exp,
        "scope": "refresh",
        "sid": sid,
        "rid": rid,
    });
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(jwt_secret.as_bytes()),
    )
}

/// Request body for `POST /admin-oauth/refresh`.
#[derive(Deserialize)]
struct RefreshRequest {
    refresh_token: String,
}

/// Response from `POST /admin-oauth/refresh`. `refresh_token` is present
/// only when the underlying refresh rotated it (account-backed admins);
/// the AS-only HS256 path does not rotate, so it is omitted there.
#[derive(Serialize, Debug)]
struct RefreshResponse {
    access_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
}

fn refresh_error(status: StatusCode, code: &str, message: &str) -> (StatusCode, Json<serde_json::Value>) {
    (
        status,
        Json(serde_json::json!({ "error": code, "message": message })),
    )
}

/// `POST /admin-oauth/refresh` — exchange a refresh token for a fresh
/// admin access token (Arc E first wave, §8.1.2 / #268).
///
/// Public by design: the refresh token IS the credential, and the (likely
/// expired) access token is not required. Two refresh-token shapes are
/// accepted, disambiguated only after signature verification so an
/// unverified `scope` claim can never drive a mint:
///
/// 1. **AS-only admins** — an HS256 `scope=refresh` JWT minted at login.
///    A fresh 24h `scope=admin` access token is issued; the refresh
///    token is NOT rotated. Rotation, a server-side refresh-token store,
///    and the SuperAdmin revocation surface land together in 0.9.3
///    (§8.1.7). Revocation in the meantime is enforced at use-time:
///    `finalize_admin_role` resolves the role live per request (#267),
///    so a revoked operator's freshly-minted access token still 403s on
///    its next request.
/// 2. **Account-backed admins** — an atproto account refresh token,
///    handled by `AccountManager::refresh_session`, which rotates it;
///    the new refresh token is returned for the client to store.
///
/// Returns 401 for an expired, malformed, or wrong-scope refresh token —
/// the only case in which the client falls back to interactive re-login.
async fn handle_refresh(
    State(ctx): State<AppContext>,
    Json(req): Json<RefreshRequest>,
) -> Result<Json<RefreshResponse>, (StatusCode, Json<serde_json::Value>)> {
    // Path 1: AS-only admin HS256 `scope=refresh` JWT. Verify the
    // signature against the PDS secret FIRST — only a server-minted token
    // passes — then trust its `scope`. A sig-valid token whose scope is
    // not "refresh" (e.g. an access token, or an account refresh token
    // that happens to verify) is NOT minted from here; it falls through.
    if let Ok(token_data) = crate::auth::verify_jwt_token(
        &req.refresh_token,
        &ctx.config.authentication.jwt_secret,
    ) {
        let claims = &token_data.claims;
        if claims.get("scope").and_then(|v| v.as_str()) == Some("refresh") {
            let did = claims
                .get("sub")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    refresh_error(
                        StatusCode::UNAUTHORIZED,
                        "InvalidToken",
                        "refresh token missing 'sub' claim",
                    )
                })?;
            let sid = claims.get("sid").and_then(|v| v.as_str());
            let rid = claims.get("rid").and_then(|v| v.as_str());

            // Post-#271 tokens carry both `sid` and `rid`: rotate-on-use
            // (#272). The presented refresh token is validated against, and
            // its `rid` rotated within, the live operator session; the old
            // refresh token becomes unusable (past the grace window). A
            // rejected rotation (revoked/expired session, stale/replayed
            // token past grace) bounces the client to interactive re-login.
            if let (Some(sid), Some(rid)) = (sid, rid) {
                let new_rid = match ctx.operator_session_store.rotate(sid, rid).await {
                    Ok(Some(new_rid)) => new_rid,
                    Ok(None) => {
                        return Err(refresh_error(
                            StatusCode::UNAUTHORIZED,
                            "InvalidToken",
                            "refresh token is no longer valid",
                        ));
                    }
                    Err(e) => {
                        tracing::error!("operator-session rotation failed: {}", e);
                        return Err(refresh_error(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "RotateFailed",
                            "failed to rotate session",
                        ));
                    }
                };
                let secret = &ctx.config.authentication.jwt_secret;
                let mint_err = |e: jsonwebtoken::errors::Error, what: &str| {
                    tracing::error!("failed to mint {} on refresh: {}", what, e);
                    refresh_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "MintFailed",
                        "failed to mint tokens",
                    )
                };
                let access_token = mint_admin_access_jwt(secret, did, Some(sid))
                    .map_err(|e| mint_err(e, "access token"))?;
                // Keep the session's original expiry — never slide it.
                let exp = claims
                    .get("exp")
                    .and_then(|v| v.as_i64())
                    .unwrap_or_else(|| chrono::Utc::now().timestamp() + 2592000);
                let refresh_token = mint_admin_refresh_jwt(secret, did, sid, &new_rid, exp)
                    .map_err(|e| mint_err(e, "refresh token"))?;
                tracing::debug!(did = %did, "admin tokens rotated (AS-only path)");
                return Ok(Json(RefreshResponse {
                    access_token,
                    refresh_token: Some(refresh_token),
                }));
            }

            // Legacy (pre-#271) refresh token: no rotation chain to advance.
            // Reissue a stateless access token, preserving any `sid`.
            let access_token =
                mint_admin_access_jwt(&ctx.config.authentication.jwt_secret, did, sid).map_err(
                    |e| {
                        tracing::error!("failed to mint admin access token on refresh: {}", e);
                        refresh_error(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "MintFailed",
                            "failed to mint access token",
                        )
                    },
                )?;
            tracing::debug!(did = %did, "admin access token refreshed (legacy AS-only path)");
            return Ok(Json(RefreshResponse {
                access_token,
                refresh_token: None,
            }));
        }
    }

    // Path 2: account-backed admin refresh token (rotates). Phase 4 · #442:
    // rotate with a 15m access token + the DID's role-based refresh (idle)
    // lifetime (honoring any per-account override), so admin sessions slide on
    // activity and expire on idle. The DID is read directly (not via
    // validate_refresh_token, whose fail-closed semantics would reject a
    // grace-period token that refresh_session handles); a non-admin token (no
    // role) keeps the regular refresh lifetimes.
    use crate::admin::security_config as sec;
    let did: Option<String> =
        sqlx::query_scalar("SELECT did FROM refresh_token WHERE token = $1")
            .bind(&req.refresh_token)
            .fetch_optional(&ctx.account_db)
            .await
            .ok()
            .flatten();
    let admin_role = match &did {
        Some(d) => ctx.admin_role_manager.get_role(d).await.ok().flatten(),
        None => None,
    };
    let refresh_result = match admin_role {
        Some(ar) => {
            let cfg = match &did {
                Some(d) => ctx.admin_security_store.get_config(d).await.ok().flatten(),
                None => None,
            };
            let refresh_secs =
                sec::compute_admin_session_lifetime_secs(ar.role, cfg.as_ref());
            ctx.account_manager
                .refresh_session_with(
                    &req.refresh_token,
                    chrono::Duration::seconds(sec::ADMIN_ACCESS_TOKEN_LIFETIME_SECS),
                    chrono::Duration::seconds(refresh_secs),
                )
                .await
        }
        None => ctx.account_manager.refresh_session(&req.refresh_token).await,
    };
    match refresh_result {
        Ok(session) => {
            tracing::debug!(did = %session.did, "admin access token refreshed (account path)");
            Ok(Json(RefreshResponse {
                access_token: session.access_token,
                refresh_token: Some(session.refresh_token),
            }))
        }
        Err(e) => {
            tracing::debug!("admin refresh rejected: {}", e);
            Err(refresh_error(
                StatusCode::UNAUTHORIZED,
                "InvalidToken",
                "invalid or expired refresh token",
            ))
        }
    }
}

/// Request body for `POST /admin-oauth/password-login`.
#[derive(Deserialize)]
struct AdminLoginRequest {
    /// Handle or email of the local admin account.
    identifier: String,
    password: String,
}

/// Response from `POST /admin-oauth/password-login`. Mirrors the token shape the
/// OAuth callback stows in localStorage, so the admin UI consumes both paths
/// identically (Bearer in `Authorization`).
#[derive(Serialize, Debug)]
struct AdminLoginResponse {
    access_token: String,
    refresh_token: String,
    did: String,
    role: String,
}

/// `POST /admin-oauth/password-login` — password-based admin login (chainlink
/// #434).
///
/// A Bearer-token alternative to the OAuth admin flow, which is blocked upstream
/// by a proto-blue-oauth DPoP-`exp` compliance bug (RFC 9449 §4.2). It reuses
/// the exact session mechanism the OAuth callback + holder OAuth use —
/// `account_manager.login` (timing-attack-mitigated password verification +
/// deactivation/takedown checks) then a v0.10 local-account admin-role gate —
/// and returns `account_manager.create_session` tokens as JSON. The admin UI
/// stores them in localStorage and sends `Authorization: Bearer`, validated by
/// the same `route_local_verify` → `validate_access_token` path as every other
/// admin request.
///
/// No cookie is set and no CSRF token is required: the response carries the
/// Bearer token in the JSON body (not an ambient cookie credential), so this
/// login is not a CSRF sink — the same posture as the OAuth callback.
/// Toggle gate in front of [`handle_password_login`] (#442). Password login is a
/// fallback, OFF by default; when disabled it 302-redirects to the OAuth login
/// landing (`/admin/`) — the friendly UX for a stale bookmark / muscle memory —
/// rather than exposing the credential endpoint. Enabled per-deployment via
/// `PDS_ADMIN_PASSWORD_LOGIN_ENABLED` (cached in config at boot). Kept separate
/// from the handler so the handler's own tests exercise the login logic directly.
async fn password_login_gate(
    State(ctx): State<AppContext>,
    body: Json<AdminLoginRequest>,
) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    if !ctx.config.authentication.password_login_enabled {
        return (
            StatusCode::FOUND,
            [(axum::http::header::LOCATION, "/admin/")],
        )
            .into_response();
    }
    match handle_password_login(State(ctx), body).await {
        Ok(json) => json.into_response(),
        Err((status, msg)) => (status, msg).into_response(),
    }
}

async fn handle_password_login(
    State(ctx): State<AppContext>,
    Json(req): Json<AdminLoginRequest>,
) -> Result<Json<AdminLoginResponse>, (StatusCode, String)> {
    // Verify the password and mint a session. `login` resolves the identifier to
    // a LOCAL account, rejects deactivated/taken-down accounts, verifies the
    // argon2 hash, and applies timing-attack mitigation. A generic 401 avoids
    // leaking whether the identifier or the password was wrong.
    let (account, session) = ctx
        .account_manager
        .login(&req.identifier, &req.password)
        .await
        .map_err(|e| {
            tracing::debug!("admin password login failed: {}", e);
            (StatusCode::UNAUTHORIZED, "Invalid credentials".to_string())
        })?;

    // v0.10 constitutional claim: admin/superadmin roles are restricted to local
    // accounts (`login` guarantees local). Require an admin_roles entry; a local
    // account without one is refused. Authority is the admin_role table, live per
    // request (#267).
    let admin_role = ctx
        .admin_role_manager
        .get_role(&account.did)
        .await
        .map_err(|e| {
            tracing::error!("Failed to query admin role: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to check admin status".to_string(),
            )
        })?;
    let Some(admin_role) = admin_role else {
        tracing::warn!(
            did = %account.did,
            "Password admin login rejected: account has no admin role on this PDS"
        );
        return Err((
            StatusCode::FORBIDDEN,
            "Account has no admin role on this PDS".to_string(),
        ));
    };

    tracing::info!(
        did = %account.did,
        role = %admin_role.role.as_str(),
        "Admin logged in via password"
    );
    Ok(Json(AdminLoginResponse {
        access_token: session.access_token,
        refresh_token: session.refresh_token,
        did: account.did,
        role: admin_role.role.as_str().to_string(),
    }))
}

/// Generate a PKCE `(code_verifier, S256 code_challenge)` pair (RFC 7636). The
/// verifier is 43 URL-safe chars (base64url of 32 random bytes), well within the
/// 43–128 range, and the challenge is `base64url(SHA-256(verifier))` — exactly
/// what the AS recomputes in `oauth::atproto::token`'s PKCE check.
fn generate_pkce() -> (String, String) {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    use rand::RngCore;
    use sha2::{Digest, Sha256};

    let mut seed = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut seed);
    let verifier = URL_SAFE_NO_PAD.encode(seed);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

/// Generate an opaque, high-entropy `state` parameter (CSRF/flow binding).
fn generate_state() -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    use rand::RngCore;

    let mut bytes = [0u8; 24];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Query parameters for OAuth initiation
#[derive(Deserialize)]
struct OAuthInitParams {
    /// Optional handle hint
    handle: Option<String>,
}

/// Initiate OAuth flow for admin login
async fn initiate_oauth(
    State(ctx): State<AppContext>,
    axum::Extension(state_store): axum::Extension<OAuthStateStore>,
    Query(params): Query<OAuthInitParams>,
) -> Result<Redirect, (StatusCode, String)> {
    tracing::info!("Initiating OAuth admin login");

    // #432 Fix 2 front-door: admin login is restricted to LOCAL accounts. When
    // a handle hint is supplied, reject a non-local one HERE — before the OAuth
    // round-trip — so the operator sees a clear error instead of failing at the
    // callback. Resolution is against the LOCAL account store only (not PLC), so
    // a handle that happens to resolve elsewhere is still refused. The no-hint
    // case is caught by Fix 1's callback gate (`authorize_local_admin`).
    if let Some(handle) = params.handle.as_deref() {
        if ctx.account_manager.get_account_by_identifier(handle).await.is_err() {
            tracing::warn!(
                handle = %handle,
                "Admin login rejected: handle has no local account on this PDS"
            );
            return Err((
                StatusCode::FORBIDDEN,
                "Admin login requires an account hosted on this PDS.".to_string(),
            ));
        }
    }

    // #432 Fix 2: admins are local accounts (see `authorize_local_admin`), so
    // the authorization server is always THIS PDS. `AdminOAuthClient::from_config`
    // targets `service.effective_public_url()` — the same value the AS metadata
    // publishes as `issuer` and did.json as `serviceEndpoint` — and generates a
    // fresh ephemeral DPoP key for this ceremony. The browser is then routed to
    // `{issuer}/oauth/atproto/authorize` (PAR-based) and lands back at
    // `/admin-oauth/callback`.
    let mut oauth_client = AdminOAuthClient::from_config(&ctx.config).map_err(|e| {
        tracing::error!("Failed to build admin OAuth client: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to build OAuth client: {}", e),
        )
    })?;

    // Generate PKCE + state locally (the Aurora-owned client does not hide these
    // the way proto-blue did), push the authorization request to our own AS, and
    // build the authorize URL from the returned request_uri.
    let (code_verifier, code_challenge) = generate_pkce();
    let state = generate_state();
    let redirect_uri = ctx.config.authentication.oauth.redirect_uri.clone();

    let par = oauth_client
        .pushed_authorization_request(&state, &code_challenge, &redirect_uri)
        .await
        .map_err(|e| {
            tracing::error!("Failed to push authorization request: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to push authorization request: {}", e),
            )
        })?;

    let auth_url = oauth_client.build_authorize_url(&par.request_uri);

    // Stash the PKCE verifier + redirect_uri under the state the AS will echo
    // back on the callback.
    state_store
        .store(
            state,
            OAuthStateData {
                code_verifier,
                redirect_uri,
                handle: params.handle.clone(),
            },
        )
        .await;

    tracing::info!("Redirecting to authorization URL");
    Ok(Redirect::to(&auth_url))
}

/// OAuth callback parameters
#[derive(Deserialize)]
struct OAuthCallbackParams {
    code: Option<String>,
    state: Option<String>,
    iss: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// Response for successful OAuth login
#[allow(dead_code)] // future structured-response variant
#[derive(Serialize)]
struct OAuthLoginResponse {
    access_token: String,
    refresh_token: String,
    did: String,
    is_admin: bool,
    role: Option<String>,
}

/// Resolve an authenticated OAuth login (`did`) into PDS-side admin session
/// tokens, enforcing the v0.10 constitutional claim that admin/superadmin roles
/// are restricted to LOCAL accounts — DIDs with a row in this PDS's `account`
/// table. Returns `(access_token, refresh_token, role)` from a real account
/// session, i.e. the tokens `route_local_verify` (`account_manager
/// ::validate_access_token`) accepts on every subsequent request.
///
/// Two rejections, both 403, in this order:
///  1. **Non-local DID** — the DID authenticated as a valid ATProto identity
///     but has no local account. Admin trust is a claim about accounts the
///     operator controls on THIS PDS, so it cannot hold a role here (even if a
///     legacy `admin_roles` row exists). Checked FIRST so the message is
///     accurate rather than a misleading "not an admin".
///  2. **Local non-admin** — a local account with no `admin_roles` entry.
///
/// This replaces the pre-v0.10 split that minted an "AS-only" HS256 admin JWT
/// backed by `operator_session_store` for admins without a local account. Those
/// tokens were not understood by `route_local_verify`, so every request after
/// login 401'd and the browser looped back to the login page. Constraining
/// admins to local accounts means the login always yields a real, validatable
/// account session and that path is gone.
async fn authorize_local_admin(
    ctx: &AppContext,
    did: &str,
) -> Result<(String, String, String), (StatusCode, String)> {
    // (1) Local-account gate — runs before the admin_roles lookup.
    if ctx.account_manager.get_account(did).await.is_err() {
        tracing::warn!(
            did = %did,
            "Admin OAuth login rejected: DID has no local account. Admin roles \
             are restricted to accounts hosted on this PDS."
        );
        return Err((
            StatusCode::FORBIDDEN,
            "Admin roles are restricted to accounts hosted on this PDS. \
             Log in with a local account."
                .to_string(),
        ));
    }

    // (2) Admin authorisation. Authority comes from the admin_role table only
    // (resolved live per request, #267). The first SuperAdmin is inserted
    // directly per the README "First Admin User" bootstrap; subsequent grants
    // flow through tools.aurora.superadmin.grantRole and the audit chain.
    let admin_role = ctx.admin_role_manager.get_role(did).await.map_err(|e| {
        tracing::error!("Failed to query admin role: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to check admin status".to_string(),
        )
    })?;
    let Some(admin_role) = admin_role else {
        tracing::warn!("User {} is not an admin on this PDS", did);
        return Err((
            StatusCode::FORBIDDEN,
            "User is not authorized as an admin on this PDS".to_string(),
        ));
    };
    let role = admin_role.role.as_str().to_string();
    tracing::info!("Admin {} authenticated with role {}", did, role);

    // A local account is guaranteed above, so this always yields a real account
    // session whose tokens `route_local_verify` validates. Phase 4 · #442: a 15m
    // access token + a role-based refresh (idle-timeout) lifetime, honoring any
    // per-account override. bound_ip is None here — IP binding wires in a later
    // commit.
    use crate::admin::security_config as sec;
    let security_config = ctx.admin_security_store.get_config(did).await.ok().flatten();
    let refresh_secs =
        sec::compute_admin_session_lifetime_secs(admin_role.role, security_config.as_ref());
    let session = ctx
        .account_manager
        .create_session_with(
            did,
            None,
            chrono::Duration::seconds(sec::ADMIN_ACCESS_TOKEN_LIFETIME_SECS),
            chrono::Duration::seconds(refresh_secs),
            None,
        )
        .await
        .map_err(|e| {
            tracing::error!("Failed to create session: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to create session: {}", e),
            )
        })?;

    Ok((session.access_token, session.refresh_token, role))
}

/// Handle OAuth callback
async fn handle_oauth_callback(
    State(ctx): State<AppContext>,
    axum::Extension(state_store): axum::Extension<OAuthStateStore>,
    Query(params): Query<OAuthCallbackParams>,
) -> Result<axum::response::Html<String>, (StatusCode, String)> {
    tracing::info!("Handling OAuth callback");

    if let Some(error) = params.error {
        let description = params
            .error_description
            .unwrap_or_else(|| "Unknown error".to_string());
        tracing::warn!("OAuth error: {} - {}", error, description);
        return Err((
            StatusCode::BAD_REQUEST,
            format!("OAuth error: {} - {}", error, description),
        ));
    }

    let code = params.code.ok_or_else(|| {
        tracing::error!("Missing authorization code");
        (
            StatusCode::BAD_REQUEST,
            "Missing authorization code".to_string(),
        )
    })?;

    let state = params.state.ok_or_else(|| {
        tracing::error!("Missing state parameter");
        (
            StatusCode::BAD_REQUEST,
            "Missing state parameter".to_string(),
        )
    })?;

    let state_data = state_store.get(&state).await.ok_or_else(|| {
        tracing::error!("Invalid or expired state");
        (
            StatusCode::BAD_REQUEST,
            "Invalid or expired state parameter".to_string(),
        )
    })?;

    // RFC 9207 (defense in depth): the admin flow only ever targets our own AS,
    // but if the AS echoed an `iss`, it must match the issuer we push to. A
    // mismatch signals a mix-up attempt.
    let expected_iss = ctx.service_url();
    if let Some(iss) = params.iss.as_deref() {
        if iss != expected_iss {
            tracing::warn!(got = %iss, expected = %expected_iss, "OAuth callback iss mismatch");
            return Err((
                StatusCode::BAD_REQUEST,
                "OAuth issuer mismatch".to_string(),
            ));
        }
    }

    // Exchange the code with the Aurora-owned client (fresh ephemeral DPoP key).
    // The AS binds the issued token to that key, but the admin flow uses the
    // token only to learn the authenticated DID and then discards it, so the
    // key need not survive beyond this call.
    let mut oauth_client = AdminOAuthClient::from_config(&ctx.config).map_err(|e| {
        tracing::error!("Failed to build admin OAuth client: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to build OAuth client: {}", e),
        )
    })?;

    let tokens = oauth_client
        .exchange_code_for_tokens(&code, &state_data.code_verifier, &state_data.redirect_uri)
        .await
        .map_err(|e| {
            tracing::error!("Failed to exchange code: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to exchange authorization code: {}", e),
            )
        })?;

    // Loopback DID resolution: Aurora's AS token response carries no `sub`, but
    // the token it just minted lives in this PDS's own `token` table, so we
    // validate it locally to learn the authenticated DID — the loopback
    // equivalent of the userinfo call an external client would make.
    let did = crate::auth::validate_oauth_token(&ctx, &tokens.access_token)
        .await
        .map_err(|e| {
            tracing::error!("Failed to resolve DID from the issued token: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to resolve authenticated identity".to_string(),
            )
        })?
        .did;
    tracing::info!("OAuth authentication successful for DID: {}", did);

    // v0.10 constitutional claim: admin/superadmin roles are restricted to LOCAL
    // accounts. Resolve the login into a real account session (the tokens the
    // request-auth path validates), rejecting non-local DIDs and non-admins.
    let (access_token, refresh_token, role) = authorize_local_admin(&ctx, &did).await?;

    // Resolve the deployment-default theme's token CSS to inline into the
    // transition screen (chainlink #441), so it paints the right theme instantly
    // instead of fetching /theme/active.css (which lagged and, uncached, served a
    // stale theme).
    let theme_id = crate::api::aurora_admin::deployment_default_theme(&ctx).await;
    let theme_css = ctx.theme_registry.resolve_token_css(&theme_id).unwrap_or_default();

    let html = format!(
        r#"<!DOCTYPE html>
<html>
<head>
    <title>Login Successful</title>
    <!-- Theme this transition screen with the SAME token layer as the login page
         and admin UI: the base alias tokens (static), then the deployment-default
         theme's resolved `:root` overrides INLINED below. Inlining — rather than
         linking /theme/active.css — means the correct theme paints from the first
         byte: no fetch to lag behind the 500ms redirect, and no stale cached
         theme to flash (chainlink #441). -->
    <link rel="stylesheet" href="/admin/styles/tokens.css">
    <style>{theme_css}</style>
    <style>
        body {{
            font-family: system-ui, -apple-system, sans-serif;
            display: flex;
            justify-content: center;
            align-items: center;
            height: 100vh;
            margin: 0;
            background: var(--color-surface-primary);
            color: var(--color-text-primary);
        }}
        .container {{
            text-align: center;
            padding: 2rem;
        }}
        .spinner {{
            width: 48px;
            height: 48px;
            border: 4px solid var(--color-surface-tertiary);
            border-top-color: var(--color-accent-primary);
            border-radius: 50%;
            animation: spin 0.8s linear infinite;
            margin: 0 auto 1rem;
        }}
        @keyframes spin {{
            to {{ transform: rotate(360deg); }}
        }}
    </style>
</head>
<body>
    <div class="container">
        <div class="spinner"></div>
        <h2>Login Successful!</h2>
        <p>Redirecting to admin panel...</p>
    </div>
    <script>
        // Store tokens in localStorage under the canonical key names
        // (§8.1.1 rename + §8.1.2 refresh consumer / #268). adminDid /
        // adminRole keep their names (out of scope for the §8.1.1 rename).
        localStorage.setItem('aurora-admin-token', {});
        localStorage.setItem('aurora-admin-refresh-token', {});
        localStorage.setItem('adminDid', {});
        localStorage.setItem('adminRole', {} || 'admin');

        // Redirect to admin panel
        setTimeout(() => {{
            window.location.href = '/admin/index.html';
        }}, 500);
    </script>
</body>
</html>"#,
        serde_json::to_string(&access_token).unwrap(),
        serde_json::to_string(&refresh_token).unwrap(),
        serde_json::to_string(&did).unwrap(),
        serde_json::to_string(&role).unwrap(),
        theme_css = theme_css,
    );

    Ok(axum::response::Html(html))
}

/// OAuth client metadata
#[derive(Serialize)]
struct ClientMetadataResponse {
    client_id: String,
    client_name: String,
    redirect_uris: Vec<String>,
    token_endpoint_auth_method: String,
    grant_types: Vec<String>,
    response_types: Vec<String>,
    scope: String,
    application_type: String,
    dpop_bound_access_tokens: bool,
}

/// Serve OAuth client metadata
async fn client_metadata(State(ctx): State<AppContext>) -> Json<ClientMetadataResponse> {
    Json(ClientMetadataResponse {
        client_id: ctx.config.authentication.oauth.client_id.clone(),
        client_name: "Aurora Locus Admin".to_string(),
        redirect_uris: vec![ctx.config.authentication.oauth.redirect_uri.clone()],
        token_endpoint_auth_method: "none".to_string(),
        grant_types: vec![
            "authorization_code".to_string(),
            "refresh_token".to_string(),
        ],
        response_types: vec!["code".to_string()],
        scope: "atproto transition:generic".to_string(),
        application_type: "web".to_string(),
        dpop_bound_access_tokens: true,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        authorize_local_admin, generate_pkce, handle_oauth_callback, handle_password_login,
        handle_refresh, initiate_oauth, mint_admin_access_jwt, password_login_gate,
        AdminLoginRequest, OAuthCallbackParams, OAuthInitParams, OAuthStateData, OAuthStateStore,
        RefreshRequest,
    };
    use crate::admin::roles::Role;
    use crate::config::*;
    use crate::AppContext;
    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::Json;
    use std::sync::Arc;
    use tempfile::tempdir;

    // Must match `jwt_secret` below so minted/signed tokens verify.
    const TEST_SECRET: &str = "test-secret-key-aurora-admin-test-32xx";

    async fn create_test_context() -> AppContext {
        build_test_context(None, true).await
    }

    /// As `create_test_context`, but with an explicit `service.public_url` — used
    /// by the loopback test to point the admin OAuth client at an in-process AS.
    async fn build_test_context(
        public_url: Option<String>,
        password_login_enabled: bool,
    ) -> AppContext {
        let dir = tempdir().unwrap().keep();
        let db_path = dir.join("test.db");
        let config = ServerConfig {
            service: ServiceConfig {
                hostname: "localhost".to_string(),
                port: 2583,
                service_did: "did:web:localhost".to_string(),
                version: "0.1.0-test".to_string(),
                blob_upload_limit: 5_242_880,
                public_url,
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
                jwt_secret: TEST_SECRET.to_string(),
                repo_signing_key: "a".repeat(64),
                plc_rotation_key: "b".repeat(64),
                oauth: OAuthConfig {
                    client_id: "http://localhost:3000/client-metadata.json".to_string(),
                    redirect_uri: "http://localhost:3000/oauth/callback".to_string(),
                    pds_url: "https://bsky.social".to_string(),
                },
                jwt_sunset_date: "Sat, 31 Dec 2024 23:59:59 GMT".to_string(),
                oauth_migration_guide_url: "https://docs.atproto.com/guides/oauth-migration"
                    .to_string(),
                password_login_enabled,
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
            validation_mode: crate::validation::ValidationMode::Required,
            distributed_state_mode: Default::default(),
            maintenance_pool: Default::default(),
            gc_sweep: Default::default(),
            bind_audit_orphan_marker: Default::default(),
            blob_metadata: Default::default(),
            entryway: None,
            lexicon: crate::config::LexiconConfig::default(),
            kryphocron: crate::config::KryphocronConfig::default(),
        };
        AppContext::new(
            config,
            Arc::new(crate::api::registry::RouteRegistry::default()),
        )
        .await
        .unwrap()
    }

    fn encode_jwt(claims: serde_json::Value) -> String {
        jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(TEST_SECRET.as_bytes()),
        )
        .unwrap()
    }

    /// The shared mint helper produces a valid 24h `scope=admin` JWT.
    #[test]
    fn mint_admin_access_jwt_produces_valid_admin_token() {
        let token = mint_admin_access_jwt(TEST_SECRET, "did:plc:minttest", None).unwrap();
        let td = crate::auth::verify_jwt_token(&token, TEST_SECRET).expect("verifies");
        assert_eq!(
            td.claims.get("scope").and_then(|v| v.as_str()),
            Some("admin")
        );
        assert_eq!(
            td.claims.get("sub").and_then(|v| v.as_str()),
            Some("did:plc:minttest")
        );
        let exp = td.claims.get("exp").and_then(|v| v.as_i64()).unwrap();
        let now = chrono::Utc::now().timestamp();
        assert!(exp > now + 86_000 && exp <= now + 86_400 + 5, "~24h expiry");
    }

    /// §8.1.2 happy path: a valid HS256 `scope=refresh` JWT yields a fresh
    /// access token that is itself a usable admin credential, and the
    /// refresh token is NOT rotated on the AS-only path.
    #[tokio::test]
    async fn refresh_hs256_scope_refresh_mints_usable_admin_token() {
        let ctx = create_test_context().await;
        let did = "did:plc:refreshtest";
        ctx.admin_role_manager
            .grant_role(did, Role::Admin, "did:plc:bootstrap", None)
            .await
            .unwrap();
        let now = chrono::Utc::now().timestamp();
        let refresh = encode_jwt(serde_json::json!({
            "sub": did, "iat": now, "exp": now + 2_592_000, "scope": "refresh",
        }));

        let resp = handle_refresh(
            State(ctx.clone()),
            Json(RefreshRequest {
                refresh_token: refresh,
            }),
        )
        .await
        .expect("refresh succeeds")
        .0;

        assert!(
            resp.refresh_token.is_none(),
            "AS-only path must not rotate the refresh token"
        );
        // The minted access token works as an admin credential and carries
        // the live role (proving end-to-end usability + §8.1.6 coupling).
        let auth = crate::auth::admin_auth_from_token(&ctx, &resp.access_token)
            .await
            .expect("minted token is a usable admin credential");
        assert_eq!(auth.did, did);
        assert_eq!(auth.role, Role::Admin);
    }

    /// An expired refresh token is rejected with 401 (falls through the
    /// HS256 path on the failed signature/expiry check, then the account
    /// path misses) — the client's only re-login trigger.
    #[tokio::test]
    async fn refresh_rejects_expired_refresh_token() {
        let ctx = create_test_context().await;
        let now = chrono::Utc::now().timestamp();
        let expired = encode_jwt(serde_json::json!({
            "sub": "did:plc:x", "iat": now - 100_000, "exp": now - 86_400, "scope": "refresh",
        }));
        let err = handle_refresh(
            State(ctx),
            Json(RefreshRequest {
                refresh_token: expired,
            }),
        )
        .await
        .expect_err("expired refresh must be rejected");
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);
    }

    /// A `scope=admin` access token presented to the refresh endpoint must
    /// NOT mint another access token (no access->access loop): it isn't
    /// `scope=refresh`, so it falls to the account path and 401s.
    #[tokio::test]
    async fn refresh_rejects_access_token_presented_as_refresh() {
        let ctx = create_test_context().await;
        let access = mint_admin_access_jwt(TEST_SECRET, "did:plc:x", None).unwrap();
        let err = handle_refresh(
            State(ctx),
            Json(RefreshRequest {
                refresh_token: access,
            }),
        )
        .await
        .expect_err("access token is not a refresh token");
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);
    }

    /// A non-JWT string is rejected with 401.
    #[tokio::test]
    async fn refresh_rejects_garbage_token() {
        let ctx = create_test_context().await;
        let err = handle_refresh(
            State(ctx),
            Json(RefreshRequest {
                refresh_token: "not.a.jwt".to_string(),
            }),
        )
        .await
        .expect_err("garbage must be rejected");
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);
    }

    /// #272 rotation-on-use end to end via the endpoint: a sid+rid refresh
    /// token rotates (returns a NEW refresh token), the new one rotates
    /// again (the chain advances), and a token whose `rid` was never the
    /// session's current head is rejected.
    #[tokio::test]
    async fn refresh_rotates_chain_and_rejects_stale_rid() {
        let ctx = create_test_context().await;
        let did = "did:plc:rotateop";
        let exp = chrono::Utc::now().timestamp() + 2_592_000;
        let sid = ctx
            .operator_session_store
            .create(did, None, None, "r1", chrono::Duration::days(30))
            .await
            .unwrap();

        let refresh_r1 = encode_jwt(serde_json::json!({
            "sub": did, "exp": exp, "scope": "refresh", "sid": sid, "rid": "r1",
        }));
        let resp1 = handle_refresh(
            State(ctx.clone()),
            Json(RefreshRequest {
                refresh_token: refresh_r1.clone(),
            }),
        )
        .await
        .expect("rotation succeeds")
        .0;
        let new_refresh = resp1.refresh_token.expect("rotation returns a new refresh token");
        assert_ne!(new_refresh, refresh_r1, "a new refresh token is issued");

        // The newly issued refresh token rotates again — the chain advances.
        let resp2 = handle_refresh(
            State(ctx.clone()),
            Json(RefreshRequest {
                refresh_token: new_refresh.clone(),
            }),
        )
        .await
        .expect("second rotation succeeds")
        .0;
        let new_refresh2 = resp2.refresh_token.expect("second rotation returns a token");
        assert_ne!(new_refresh2, new_refresh, "chain advanced again");

        // A token whose rid was never this session's head is rejected.
        let bogus = encode_jwt(serde_json::json!({
            "sub": did, "exp": exp, "scope": "refresh", "sid": sid, "rid": "never-current",
        }));
        let err = handle_refresh(
            State(ctx),
            Json(RefreshRequest {
                refresh_token: bogus,
            }),
        )
        .await
        .expect_err("stale rid must be rejected");
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);
    }

    /// Seed a local account (actor + account rows) so `get_account` /
    /// `create_session` resolve — bypasses createAccount's PLC round-trip.
    async fn seed_local_account(ctx: &AppContext, did: &str, handle: &str) {
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO actor (did, handle, created_at, takedown_ref, deactivated_at, delete_after)
             VALUES (?1, ?2, ?3, NULL, NULL, NULL)",
        )
        .bind(did)
        .bind(handle)
        .bind(&now)
        .execute(&ctx.account_db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO account (did, email, password_hash, email_confirmed_at, invites_disabled)
             VALUES (?1, ?2, 'test-hash', NULL, 0)",
        )
        .bind(did)
        .bind(Some("admin@example.test"))
        .execute(&ctx.account_db)
        .await
        .unwrap();
    }

    /// Seed a local account with a REAL argon2 password hash (so `login` can
    /// verify it) and optionally an admin role.
    async fn seed_password_admin(
        ctx: &AppContext,
        did: &str,
        handle: &str,
        password: &str,
        role: Option<Role>,
    ) {
        let hash = crate::auth::PasswordHasher::hash(password).unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO actor (did, handle, created_at, takedown_ref, deactivated_at, delete_after)
             VALUES (?1, ?2, ?3, NULL, NULL, NULL)",
        )
        .bind(did)
        .bind(handle)
        .bind(&now)
        .execute(&ctx.account_db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO account (did, email, password_hash, email_confirmed_at, invites_disabled)
             VALUES (?1, ?2, ?3, NULL, 0)",
        )
        .bind(did)
        .bind(Some("admin@example.test"))
        .bind(&hash)
        .execute(&ctx.account_db)
        .await
        .unwrap();
        if let Some(role) = role {
            ctx.admin_role_manager
                .grant_role(did, role, "system", None)
                .await
                .unwrap();
        }
    }

    // Fix 1 (#434): password-based admin login (Bearer).

    #[tokio::test]
    async fn password_login_happy_path_mints_validatable_admin_session() {
        let ctx = create_test_context().await;
        seed_password_admin(
            &ctx,
            "did:plc:pwadmin000000000000000000",
            "admin.localhost",
            "correct-horse-battery-staple",
            Some(Role::SuperAdmin),
        )
        .await;
        let resp = handle_password_login(
            State(ctx.clone()),
            Json(AdminLoginRequest {
                identifier: "admin.localhost".to_string(),
                password: "correct-horse-battery-staple".to_string(),
            }),
        )
        .await
        .expect("valid admin login must succeed");
        assert_eq!(resp.0.role, "superadmin");
        assert_eq!(resp.0.did, "did:plc:pwadmin000000000000000000");
        // Token validates via the same path route_local_verify uses.
        let validated = ctx
            .account_manager
            .validate_access_token(&resp.0.access_token)
            .await
            .expect("minted access token must validate");
        assert_eq!(validated.did, "did:plc:pwadmin000000000000000000");
    }

    #[tokio::test]
    async fn password_login_rejects_wrong_password() {
        let ctx = create_test_context().await;
        seed_password_admin(
            &ctx,
            "did:plc:pwadmin000000000000000000",
            "admin.localhost",
            "the-right-password",
            Some(Role::SuperAdmin),
        )
        .await;
        let err = handle_password_login(
            State(ctx),
            Json(AdminLoginRequest {
                identifier: "admin.localhost".to_string(),
                password: "the-WRONG-password".to_string(),
            }),
        )
        .await
        .expect_err("wrong password must be refused");
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);
        assert_eq!(err.1, "Invalid credentials");
    }

    #[tokio::test]
    async fn password_login_rejects_local_account_without_admin_role() {
        let ctx = create_test_context().await;
        seed_password_admin(
            &ctx,
            "did:plc:pwnonadmin0000000000000000",
            "bob.localhost",
            "bobs-password",
            None, // local account, no admin role
        )
        .await;
        let err = handle_password_login(
            State(ctx),
            Json(AdminLoginRequest {
                identifier: "bob.localhost".to_string(),
                password: "bobs-password".to_string(),
            }),
        )
        .await
        .expect_err("non-admin must be refused");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
        assert!(err.1.contains("no admin role"), "got: {}", err.1);
    }

    #[tokio::test]
    async fn password_login_rejects_unknown_identifier() {
        let ctx = create_test_context().await;
        let err = handle_password_login(
            State(ctx),
            Json(AdminLoginRequest {
                identifier: "ghost.localhost".to_string(),
                password: "whatever".to_string(),
            }),
        )
        .await
        .expect_err("unknown identifier must be refused");
        assert_eq!(err.0, StatusCode::UNAUTHORIZED);
        assert_eq!(err.1, "Invalid credentials");
    }

    // #442: password login is a fallback, OFF by default. The gate 302s to
    // /admin/ when disabled and delegates to the handler when enabled.

    #[tokio::test]
    async fn password_login_gate_redirects_to_admin_when_disabled() {
        use axum::http::header;
        let ctx = build_test_context(None, false).await;
        let resp = password_login_gate(
            State(ctx),
            Json(AdminLoginRequest {
                identifier: "admin.localhost".to_string(),
                password: "whatever".to_string(),
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FOUND);
        assert_eq!(
            resp.headers().get(header::LOCATION).unwrap().to_str().unwrap(),
            "/admin/",
            "disabled password login must 302 to the OAuth login landing, not expose the endpoint"
        );
    }

    #[tokio::test]
    async fn password_login_gate_delegates_when_enabled() {
        // Enabled: the gate passes through to the handler, which rejects an
        // unknown identifier with 401 (not a 302) — proving delegation.
        let ctx = build_test_context(None, true).await;
        let resp = password_login_gate(
            State(ctx),
            Json(AdminLoginRequest {
                identifier: "ghost.localhost".to_string(),
                password: "whatever".to_string(),
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // Fix 1 (#432): admin OAuth is constrained to local accounts.

    #[tokio::test]
    async fn initiate_oauth_rejects_non_local_handle_at_front_door() {
        // Fix 2 (#432) front-door: a handle hint with no local account is
        // refused before any OAuth round-trip (no network / discovery reached).
        let ctx = create_test_context().await;
        let res = initiate_oauth(
            State(ctx),
            axum::Extension(OAuthStateStore::new()),
            axum::extract::Query(OAuthInitParams {
                handle: Some("stranger.example.com".to_string()),
            }),
        )
        .await;
        let err = res.expect_err("non-local handle must be refused at the front door");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
        assert!(
            err.1.contains("hosted on this PDS"),
            "expected local-account message, got: {}",
            err.1
        );
    }

    #[tokio::test]
    async fn authorize_local_admin_rejects_non_local_did() {
        let ctx = create_test_context().await;
        // A DID that OAuth-authenticated but has NO local account — and even
        // carries a (legacy) admin_roles row — must still be refused, because
        // the local-account gate runs BEFORE the role lookup.
        let did = "did:plc:nonlocaladmin0000000000000";
        ctx.admin_role_manager
            .grant_role(did, Role::SuperAdmin, "system", None)
            .await
            .unwrap();
        let err = authorize_local_admin(&ctx, did)
            .await
            .expect_err("non-local DID must be refused");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
        assert!(
            err.1.contains("hosted on this PDS"),
            "expected local-account message, got: {}",
            err.1
        );
    }

    #[tokio::test]
    async fn authorize_local_admin_happy_path_mints_validatable_session() {
        let ctx = create_test_context().await;
        let did = "did:plc:localadmin00000000000000000";
        seed_local_account(&ctx, did, "admin.localhost").await;
        ctx.admin_role_manager
            .grant_role(did, Role::SuperAdmin, "system", None)
            .await
            .unwrap();

        let (access, refresh, role) = authorize_local_admin(&ctx, did)
            .await
            .expect("local admin must be authorized");
        assert_eq!(role, "superadmin");
        assert!(!refresh.is_empty());

        // The minted access token must validate via the exact path
        // route_local_verify uses — proving the login→401→login loop is gone.
        let validated = ctx
            .account_manager
            .validate_access_token(&access)
            .await
            .expect("session access token must validate via account_manager");
        assert_eq!(validated.did, did);
    }

    #[tokio::test]
    async fn authorize_local_admin_rejects_local_non_admin() {
        let ctx = create_test_context().await;
        let did = "did:plc:localnonadmin000000000000";
        seed_local_account(&ctx, did, "bob.localhost").await;
        // Local account, but NO admin_roles entry — the existing "not an admin"
        // 403 must still fire (no regression).
        let err = authorize_local_admin(&ctx, did)
            .await
            .expect_err("local non-admin must be refused");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
        assert!(
            err.1.contains("not authorized as an admin"),
            "expected not-an-admin message, got: {}",
            err.1
        );
    }

    // Phase 4 (#442): admin session hardening — the security-config store + the
    // sliding-refresh lifetime wired into the OAuth session mint.

    #[tokio::test]
    async fn admin_security_store_session_lifetime_round_trip() {
        let ctx = create_test_context().await;
        let did = "did:plc:sechardening00000000000000";
        seed_local_account(&ctx, did, "admin.localhost").await;
        let store = &ctx.admin_security_store;

        // No row → None (all defaults).
        assert!(store.get_config(did).await.unwrap().is_none());

        // Set → round-trips.
        store.set_session_lifetime(did, Some(3600)).await.unwrap();
        assert_eq!(
            store.get_config(did).await.unwrap().unwrap().session_lifetime_secs,
            Some(3600)
        );

        // Clear (None) → the override is cleared (row persists, value NULL).
        store.set_session_lifetime(did, None).await.unwrap();
        assert_eq!(
            store.get_config(did).await.unwrap().unwrap().session_lifetime_secs,
            None
        );

        // Out-of-bounds rejected at write time.
        assert!(store.set_session_lifetime(did, Some(1)).await.is_err());
        assert!(store
            .set_session_lifetime(did, Some(31 * 24 * 3600))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn admin_session_uses_15m_access_and_role_based_refresh() {
        let ctx = create_test_context().await;
        let did = "did:plc:seclifetime000000000000000";
        seed_local_account(&ctx, did, "admin.localhost").await;
        ctx.admin_role_manager
            .grant_role(did, Role::SuperAdmin, "system", None)
            .await
            .unwrap();

        let before = chrono::Utc::now();
        let (access, refresh, role) = authorize_local_admin(&ctx, did).await.unwrap();
        assert_eq!(role, "superadmin");

        // Access token expires ~15 minutes out (fixed for all roles).
        let access_exp: String =
            sqlx::query_scalar("SELECT expires_at FROM session WHERE access_token = $1")
                .bind(&access)
                .fetch_one(&ctx.account_db)
                .await
                .unwrap();
        let access_secs = (parse_rfc3339(&access_exp) - before).num_seconds();
        assert!(
            (14 * 60..=16 * 60).contains(&access_secs),
            "access token should be ~15m, got {access_secs}s"
        );

        // Refresh token expires ~1 hour out (superadmin role default).
        let refresh_exp: String =
            sqlx::query_scalar("SELECT expires_at FROM refresh_token WHERE token = $1")
                .bind(&refresh)
                .fetch_one(&ctx.account_db)
                .await
                .unwrap();
        let refresh_secs = (parse_rfc3339(&refresh_exp) - before).num_seconds();
        assert!(
            (59 * 60..=61 * 60).contains(&refresh_secs),
            "superadmin refresh should be ~1h, got {refresh_secs}s"
        );
    }

    #[tokio::test]
    async fn admin_session_honors_lifetime_override() {
        let ctx = create_test_context().await;
        let did = "did:plc:secoverride0000000000000000";
        seed_local_account(&ctx, did, "admin.localhost").await;
        ctx.admin_role_manager
            .grant_role(did, Role::SuperAdmin, "system", None)
            .await
            .unwrap();
        // Override the superadmin 1h default to 2h.
        ctx.admin_security_store
            .set_session_lifetime(did, Some(7200))
            .await
            .unwrap();

        let before = chrono::Utc::now();
        let (_access, refresh, _role) = authorize_local_admin(&ctx, did).await.unwrap();
        let refresh_exp: String =
            sqlx::query_scalar("SELECT expires_at FROM refresh_token WHERE token = $1")
                .bind(&refresh)
                .fetch_one(&ctx.account_db)
                .await
                .unwrap();
        let refresh_secs = (parse_rfc3339(&refresh_exp) - before).num_seconds();
        assert!(
            (119 * 60..=121 * 60).contains(&refresh_secs),
            "overridden refresh should be ~2h, got {refresh_secs}s"
        );
    }

    fn parse_rfc3339(s: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(s)
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    // Phase 3 (#439): full loopback — the callback exchanges a real code against
    // Aurora's own in-process AS with an Aurora-built DPoP proof, resolves the
    // DID from the issued token, and mints a validatable admin session.

    /// Pull a `localStorage.setItem('<key>', "<json>")` value out of the success
    /// HTML the callback renders.
    fn extract_localstorage_value(html: &str, key: &str) -> String {
        let needle = format!("localStorage.setItem('{key}', ");
        let start = html.find(&needle).expect("localStorage key present") + needle.len();
        let rest = &html[start..];
        let end = rest.find(");").expect("setItem statement terminator");
        serde_json::from_str::<String>(rest[..end].trim()).expect("value is a JSON string")
    }

    #[tokio::test]
    async fn callback_completes_loopback_exchange_and_mints_validatable_session() {
        use crate::oauth::atproto::request_store::{self, AtprotoAuthorizationRequest};

        // Bind the in-process AS first so we know its port before building ctx.
        // Address it as `localhost` (not `127.0.0.1`) so the WebAuthn RP config
        // derived from the public URL is valid — a bare IP is rejected — while
        // still resolving to this loopback listener.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let as_url = format!("http://localhost:{port}");

        // A context whose public URL IS that in-process AS (Aurora talks to its
        // own AS); both the admin client and the AS handlers derive their URLs
        // from this, so their DPoP htu values line up.
        let ctx = build_test_context(Some(as_url.clone()), true).await;

        // Serve Aurora's real AS routes on the same ctx (shared account_db).
        let app = crate::oauth::atproto::routes().with_state(ctx.clone());
        let (shutdown, rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = rx.await;
                })
                .await
                .ok();
        });

        // Seed a local superadmin + a redeemable authorization code bound to it
        // (as consent would have, but seeded directly so the test needn't drive
        // the browser-session authorize step).
        let did = "did:plc:loopadmin0000000000000000";
        seed_local_account(&ctx, did, "admin.localhost").await;
        ctx.admin_role_manager
            .grant_role(did, Role::SuperAdmin, "system", None)
            .await
            .unwrap();

        let code = "loopback-authorization-code";
        let (verifier, challenge) = generate_pkce();
        let redirect_uri = ctx.config.authentication.oauth.redirect_uri.clone();
        let now = chrono::Utc::now();
        request_store::insert(
            &ctx.account_db,
            &AtprotoAuthorizationRequest {
                request_id: "loop-req".to_string(),
                request_uri: None,
                client_id: ctx.config.authentication.oauth.client_id.clone(),
                redirect_uri: redirect_uri.clone(),
                scope: "atproto transition:generic".to_string(),
                state: None,
                code_challenge: challenge,
                code_challenge_method: "S256".to_string(),
                did: Some(did.to_string()),
                code_hash: Some(crate::oauth::access_token_hash(code)),
                code_used_at: None,
                denied_at: None,
                created_at: now.to_rfc3339(),
                expires_at: (now + chrono::Duration::minutes(10)).to_rfc3339(),
            },
        )
        .await
        .unwrap();

        // Pre-store the flow state exactly as `initiate_oauth` would.
        let state_store = OAuthStateStore::new();
        let state = "loopback-state";
        state_store
            .store(
                state.to_string(),
                OAuthStateData {
                    code_verifier: verifier,
                    redirect_uri,
                    handle: None,
                },
            )
            .await;

        // Drive the callback: it exchanges the code over HTTP against the
        // in-process AS (Aurora-built DPoP proof, incl. the exp claim upstream
        // omits), resolves the DID via the loopback validation path, and mints a
        // real account session.
        let html = handle_oauth_callback(
            State(ctx.clone()),
            axum::Extension(state_store),
            axum::extract::Query(OAuthCallbackParams {
                code: Some(code.to_string()),
                state: Some(state.to_string()),
                iss: None,
                error: None,
                error_description: None,
            }),
        )
        .await
        .expect("loopback callback must succeed")
        .0;

        // The HTML stows a session token that validates via the exact path
        // route_local_verify uses.
        let token = extract_localstorage_value(&html, "aurora-admin-token");
        let validated = ctx
            .account_manager
            .validate_access_token(&token)
            .await
            .expect("minted session token must validate");
        assert_eq!(validated.did, did);
        assert!(html.contains("superadmin"));

        // The transition screen is themed with the shared token layer (chainlink
        // #440/#441): the base alias tokens load statically and the theme is
        // INLINED, so there is no /theme/active.css fetch to lag or serve a stale
        // cached theme — and never the old hardcoded off-brand gradient.
        assert!(
            html.contains(r#"href="/admin/styles/tokens.css""#),
            "base token layer must load"
        );
        assert!(
            !html.contains(r#"href="/theme/active.css""#),
            "the FOUC-prone dynamic theme link must be gone (inlined instead)"
        );
        assert!(html.contains("var(--color-surface-primary)"));
        assert!(html.contains("var(--color-accent-primary)"));
        assert!(
            !html.contains("667eea") && !html.contains("764ba2"),
            "the hardcoded off-brand gradient must be gone"
        );

        let _ = shutdown.send(());
    }
}
