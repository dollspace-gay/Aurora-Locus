/// OAuth-based admin authentication endpoints
use crate::{error::{PdsError, PdsResult}, AppContext};
use atproto::oauth::OAuthClient;
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect},
    routing::get,
    Json, Router,
};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Storage for OAuth state and PKCE verifiers
/// In production, this should be Redis or another distributed store
#[derive(Clone)]
pub struct OAuthStateStore {
    states: Arc<RwLock<HashMap<String, OAuthStateData>>>,
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

#[derive(Clone)]
pub struct OAuthStateData {
    pub code_verifier: String,
    pub handle: Option<String>,
    pub token_endpoint: String,
}

/// Build OAuth routes
pub fn routes(state_store: OAuthStateStore) -> Router<AppContext> {
    // Create a router with the OAuth state store, then layer it over the app context
    let oauth_router = Router::new()
        .route("/admin-oauth/login", get(initiate_oauth))
        .route("/admin-oauth/callback", get(handle_oauth_callback))
        .layer(axum::Extension(state_store));

    Router::new()
        .merge(oauth_router)
        .route("/oauth/client-metadata.json", get(client_metadata))
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
    use atproto::oauth::{OAuthClient, PkceParams};

    tracing::info!("Initiating OAuth admin login");

    // Create OAuth client
    let oauth_client = OAuthClient::new(
        ctx.config.authentication.oauth.client_id.clone(),
        ctx.config.authentication.oauth.redirect_uri.clone(),
    )
    .map_err(|e| {
        tracing::error!("Failed to create OAuth client: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("OAuth initialization failed: {}", e),
        )
    })?;

    // Generate PKCE parameters
    let pkce = PkceParams::generate();

    // Discover server metadata to get token endpoint
    let handle = params.handle.as_deref().unwrap_or("user.bsky.social");
    let pds_url = &ctx.config.authentication.oauth.pds_url;

    let server_metadata = oauth_client
        .discover_server_metadata(pds_url)
        .await
        .map_err(|e| {
            tracing::error!("Failed to discover server metadata: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to discover OAuth server: {}", e),
            )
        })?;

    let token_endpoint = server_metadata.token_endpoint.clone();

    // Build authorization URL
    let auth_url = oauth_client
        .build_authorization_url(
            pds_url,
            handle,
            &pkce,
        )
        .await
        .map_err(|e| {
            tracing::error!("Failed to build authorization URL: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to build authorization URL: {}", e),
            )
        })?;

    // Store PKCE verifier for later use
    // Extract state from the URL
    let url = Url::parse(&auth_url).map_err(|e| {
        tracing::error!("Invalid authorization URL: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Invalid authorization URL".to_string(),
        )
    })?;

    let state = url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.to_string())
        .ok_or_else(|| {
            tracing::error!("No state parameter in authorization URL");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Missing state parameter".to_string(),
            )
        })?;

    // Store state data including token endpoint
    state_store
        .store(
            state,
            OAuthStateData {
                code_verifier: pkce.code_verifier,
                handle: params.handle.clone(),
                token_endpoint,
            },
        )
        .await;

    tracing::info!("Redirecting to authorization URL: {}", auth_url);
    Ok(Redirect::to(&auth_url))
}

/// OAuth callback parameters
#[derive(Deserialize)]
struct OAuthCallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// Response for successful OAuth login
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

    // Check for errors
    if let Some(error) = params.error {
        let description = params.error_description.unwrap_or_else(|| "Unknown error".to_string());
        tracing::warn!("OAuth error: {} - {}", error, description);
        return Err((
            StatusCode::BAD_REQUEST,
            format!("OAuth error: {} - {}", error, description),
        ));
    }

    // Get authorization code and state
    let code = params.code.ok_or_else(|| {
        tracing::error!("Missing authorization code");
        (StatusCode::BAD_REQUEST, "Missing authorization code".to_string())
    })?;

    let state = params.state.ok_or_else(|| {
        tracing::error!("Missing state parameter");
        (StatusCode::BAD_REQUEST, "Missing state parameter".to_string())
    })?;

    // Retrieve stored PKCE verifier
    let state_data = state_store.get(&state).await.ok_or_else(|| {
        tracing::error!("Invalid or expired state");
        (
            StatusCode::BAD_REQUEST,
            "Invalid or expired state parameter".to_string(),
        )
    })?;

    // Create OAuth client
    let oauth_client = OAuthClient::new(
        ctx.config.authentication.oauth.client_id.clone(),
        ctx.config.authentication.oauth.redirect_uri.clone(),
    )
    .map_err(|e| {
        tracing::error!("Failed to create OAuth client: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("OAuth client error: {}", e),
        )
    })?;

    // Use the token endpoint discovered during authorization
    let token_endpoint = &state_data.token_endpoint;

    // Exchange code for tokens
    let oauth_session = oauth_client
        .exchange_code(&code, &state_data.code_verifier, token_endpoint)
        .await
        .map_err(|e| {
            tracing::error!("Failed to exchange code: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to exchange authorization code: {}", e),
            )
        })?;

    let did = oauth_session.did.to_string();
    tracing::info!("OAuth authentication successful for DID: {}", did);

    // Check if this DID is authorized as admin
    let is_configured_admin = ctx.config.authentication.admin_dids.contains(&did);

    let admin_role = ctx
        .admin_role_manager
        .get_role(&did)
        .await
        .map_err(|e| {
            tracing::error!("Failed to query admin role: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to check admin status".to_string(),
            )
        })?;

    let is_admin = is_configured_admin || admin_role.is_some();

    if !is_admin {
        tracing::warn!("User {} is not an admin on this PDS", did);
        return Err((
            StatusCode::FORBIDDEN,
            "User is not authorized as an admin on this PDS".to_string(),
        ));
    }

    let role = if let Some(ref admin_role) = admin_role {
        Some(admin_role.role.as_str().to_string())
    } else {
        Some("superadmin".to_string())
    };

    tracing::info!("Admin {} authenticated with role {:?}", did, role);

    // Check if account exists on this PDS
    let account_exists = ctx.account_manager.get_account(&did).await.is_ok();

    // Create session tokens
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
        // Create temporary admin-only JWT tokens
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

    // Return HTML page that stores tokens and redirects to admin panel
    let html = format!(r#"<!DOCTYPE html>
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
async fn client_metadata(
    State(ctx): State<AppContext>,
) -> Json<ClientMetadataResponse> {
    Json(ClientMetadataResponse {
        client_id: ctx.config.authentication.oauth.client_id.clone(),
        client_name: "Aurora Locus Admin".to_string(),
        redirect_uris: vec![ctx.config.authentication.oauth.redirect_uri.clone()],
        token_endpoint_auth_method: "none".to_string(), // Public client
        grant_types: vec!["authorization_code".to_string(), "refresh_token".to_string()],
        response_types: vec!["code".to_string()],
        scope: "atproto transition:generic".to_string(),
        application_type: "web".to_string(),
        dpop_bound_access_tokens: true,
    })
}
