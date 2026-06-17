//! OAuth-based admin authentication endpoints.
//!
//! Wraps proto-blue's OAuth client to authenticate operators via their
//! atproto identity. The flow:
//!
//! 1. `/admin-oauth/login` — discover the user's AS, build a PAR/PKCE
//!    authorization URL, stash the `AuthState` + server metadata under
//!    the `state` parameter, redirect.
//! 2. `/admin-oauth/callback` — look up the stash, exchange the code
//!    for a token, verify the resulting DID is on the admin list, mint
//!    PDS access/refresh tokens, render a small HTML page that stows
//!    them in localStorage.
//! 3. `/oauth/client-metadata.json` — public client-metadata document
//!    consumed by the AS during discovery.
use crate::AppContext;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Redirect,
    routing::{get, post},
    Json, Router,
};
use proto_blue::oauth::{
    types::{AuthState, OAuthClientMetadata, OAuthServerMetadata},
    OAuthClient,
};
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
#[derive(Clone)]
pub struct OAuthStateData {
    /// PKCE verifier + DPoP key + issuer, produced by `OAuthClient::authorize`.
    pub auth_state: AuthState,
    /// AS metadata fetched during discovery — the callback needs it again to
    /// drive token exchange (token_endpoint, iss-parameter support, etc.).
    pub server_metadata: OAuthServerMetadata,
    /// Optional handle hint, retained only for tracing.
    #[allow(dead_code)]
    pub handle: Option<String>,
}

/// Build OAuth routes
pub fn routes(state_store: OAuthStateStore) -> Router<AppContext> {
    let oauth_router = Router::new()
        .route("/admin-oauth/login", get(initiate_oauth))
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
            // Preserve the session id across refresh so the per-request
            // session lookup stays continuous (#271). Rotation of the
            // refresh token itself lands in #272; here we only reissue the
            // access token bound to the same `sid`.
            let sid = claims.get("sid").and_then(|v| v.as_str());
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
            tracing::debug!(did = %did, "admin access token refreshed (AS-only path)");
            return Ok(Json(RefreshResponse {
                access_token,
                refresh_token: None,
            }));
        }
    }

    // Path 2: account-backed admin refresh token (rotates).
    match ctx.account_manager.refresh_session(&req.refresh_token).await {
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

/// Build a fresh `OAuthClient` configured for this PDS's admin flow.
fn build_oauth_client(ctx: &AppContext) -> OAuthClient {
    let metadata = OAuthClientMetadata {
        client_id: ctx.config.authentication.oauth.client_id.clone(),
        redirect_uris: vec![ctx.config.authentication.oauth.redirect_uri.clone()],
        response_types: Some(vec!["code".to_string()]),
        grant_types: Some(vec![
            "authorization_code".to_string(),
            "refresh_token".to_string(),
        ]),
        scope: Some("atproto transition:generic".to_string()),
        token_endpoint_auth_method: Some("none".to_string()),
        token_endpoint_auth_signing_alg: None,
        application_type: Some("web".to_string()),
        dpop_bound_access_tokens: Some(true),
        client_name: Some("Aurora Locus Admin".to_string()),
        client_uri: None,
        logo_uri: None,
    };
    OAuthClient::new(metadata)
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

    let oauth_client = build_oauth_client(&ctx);

    // Discover the AS for this PDS.
    let pds_url = &ctx.config.authentication.oauth.pds_url;
    let server_metadata = oauth_client.discover_server(pds_url).await.map_err(|e| {
        tracing::error!("Failed to discover server metadata: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to discover OAuth server: {}", e),
        )
    })?;

    // Build authorization URL — proto-blue generates PKCE + DPoP keys + state internally.
    let (auth_url, auth_state) = oauth_client
        .authorize(&server_metadata)
        .await
        .map_err(|e| {
            tracing::error!("Failed to build authorization URL: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to build authorization URL: {}", e),
            )
        })?;

    // The state we'll receive back on the callback is `app_state` on AuthState.
    let state_key = auth_state.app_state.clone().ok_or_else(|| {
        tracing::error!("OAuth client returned AuthState without app_state");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "OAuth client returned no state parameter".to_string(),
        )
    })?;

    state_store
        .store(
            state_key,
            OAuthStateData {
                auth_state,
                server_metadata,
                handle: params.handle.clone(),
            },
        )
        .await;

    tracing::info!("Redirecting to authorization URL: {}", auth_url);
    Ok(Redirect::to(auth_url.as_str()))
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

/// Handle OAuth callback
async fn handle_oauth_callback(
    State(ctx): State<AppContext>,
    axum::Extension(state_store): axum::Extension<OAuthStateStore>,
    headers: axum::http::HeaderMap,
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

    let oauth_client = build_oauth_client(&ctx);

    // RFC 9207: when the AS supports it, verify the `iss` we got back matches
    // the one we stored before exchanging the code.
    let token_set = oauth_client
        .callback_with_iss(
            &code,
            params.iss.as_deref(),
            &state_data.auth_state,
            &state_data.server_metadata,
        )
        .await
        .map_err(|e| {
            tracing::error!("Failed to exchange code: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to exchange authorization code: {}", e),
            )
        })?;

    let did = token_set.sub.clone();
    tracing::info!("OAuth authentication successful for DID: {}", did);

    // Admin authorisation check. Authority comes from the admin_role
    // table only. The first SuperAdmin must be inserted directly into
    // admin_role per the bootstrap path in README "First Admin User";
    // subsequent grants flow through tools.aurora.superadmin.grantRole
    // and the audit chain.
    let admin_role = ctx.admin_role_manager.get_role(&did).await.map_err(|e| {
        tracing::error!("Failed to query admin role: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to check admin status".to_string(),
        )
    })?;

    let Some(ref admin_role) = admin_role else {
        tracing::warn!("User {} is not an admin on this PDS", did);
        return Err((
            StatusCode::FORBIDDEN,
            "User is not authorized as an admin on this PDS".to_string(),
        ));
    };

    let role = Some(admin_role.role.as_str().to_string());

    tracing::info!("Admin {} authenticated with role {:?}", did, role);

    // Mint PDS-side tokens (real session if the user has an account, otherwise a
    // 24h admin-scoped JWT for AS-only admins).
    let account_exists = ctx.account_manager.get_account(&did).await.is_ok();

    let (access_token, refresh_token) = if account_exists {
        let session = ctx
            .account_manager
            .create_session(&did, None)
            .await
            .map_err(|e| {
                tracing::error!("Failed to create session: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to create session: {}", e),
                )
            })?;

        (session.access_token, session.refresh_token)
    } else {
        use jsonwebtoken::{encode, EncodingKey, Header};
        use serde_json::json;

        // Record a server-side operator session (§8.1.7 / #271). It outlives
        // the 24h access token, so tie its lifetime to the 30d refresh
        // window. `refresh_id` is the rotation-chain head (#272). Both the
        // access and refresh tokens carry the resulting `sid` so the auth
        // path can validate/touch/revoke this session per request.
        let refresh_id = uuid::Uuid::new_v4().to_string();
        let source_ip =
            crate::rate_limit::extract_client_ip(&headers, ctx.rate_limiter.trust_proxy)
                .map(|ip| ip.to_string());
        let user_agent = headers
            .get(axum::http::header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let sid = ctx
            .operator_session_store
            .create(
                &did,
                source_ip.as_deref(),
                user_agent.as_deref(),
                &refresh_id,
                chrono::Duration::days(30),
            )
            .await
            .map_err(|e| {
                tracing::error!("Failed to create operator session: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Failed to create session".to_string(),
                )
            })?;

        let now = chrono::Utc::now().timestamp();

        let access_token =
            mint_admin_access_jwt(&ctx.config.authentication.jwt_secret, &did, Some(&sid))
                .map_err(|e| {
                    tracing::error!("Failed to create JWT: {}", e);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Failed to create token".to_string(),
                    )
                },
            )?;

        let refresh_claims = json!({
            "sub": did,
            "iat": now,
            "exp": now + 2592000, // 30 days
            "scope": "refresh",
            "sid": sid,
            "rid": refresh_id,
        });

        let refresh_token = encode(
            &Header::default(),
            &refresh_claims,
            &EncodingKey::from_secret(ctx.config.authentication.jwt_secret.as_bytes()),
        )
        .map_err(|e| {
            tracing::error!("Failed to create refresh token: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to create refresh token".to_string(),
            )
        })?;

        (access_token, refresh_token)
    };

    let html = format!(
        r#"<!DOCTYPE html>
<html>
<head>
    <title>Login Successful</title>
    <style>
        body {{
            font-family: system-ui, -apple-system, sans-serif;
            display: flex;
            justify-content: center;
            align-items: center;
            height: 100vh;
            margin: 0;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            color: white;
        }}
        .container {{
            text-align: center;
            padding: 2rem;
        }}
        .spinner {{
            width: 48px;
            height: 48px;
            border: 4px solid rgba(255,255,255,0.3);
            border-top-color: white;
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
        serde_json::to_string(&role).unwrap()
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
    use super::{handle_refresh, mint_admin_access_jwt, RefreshRequest};
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
        let dir = tempdir().unwrap().keep();
        let db_path = dir.join("test.db");
        let config = ServerConfig {
            service: ServiceConfig {
                hostname: "localhost".to_string(),
                port: 2583,
                service_did: "did:web:localhost".to_string(),
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
                auto_stream_events: false,
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
}
