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
    routing::get,
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
        .layer(axum::Extension(state_store));

    Router::new()
        .merge(oauth_router)
        .route("/oauth/client-metadata.json", get(client_metadata))
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

        let now = chrono::Utc::now().timestamp();
        let claims = json!({
            "sub": did,
            "iat": now,
            "exp": now + 86400, // 24 hours
            "scope": "admin",
        });

        let access_token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(ctx.config.authentication.jwt_secret.as_bytes()),
        )
        .map_err(|e| {
            tracing::error!("Failed to create JWT: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to create token".to_string(),
            )
        })?;

        let refresh_claims = json!({
            "sub": did,
            "iat": now,
            "exp": now + 2592000, // 30 days
            "scope": "refresh",
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
        // Store tokens in localStorage
        localStorage.setItem('adminToken', {});
        localStorage.setItem('adminRefreshToken', {});
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
