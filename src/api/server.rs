/// com.atproto.server.* endpoints
use crate::{
    account::{
        CreateAccountRequest, CreateAccountResponse, CreateAppPasswordRequest,
        CreateAppPasswordResponse, CreateSessionRequest, ListAppPasswordsResponse,
        RefreshSessionRequest, RevokeAppPasswordRequest, SessionInfo, SessionResponse,
    },
    api::middleware,
    auth::AuthContext,
    context::AppContext,
    error::{PdsError, PdsResult},
    service_auth,
};
use axum::{
    extract::{Query, State},
    http::HeaderMap,
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;

/// Build server routes
pub fn routes() -> Router<AppContext> {
    Router::new()
        .route("/xrpc/com.atproto.server.createAccount", post(create_account))
        .route("/xrpc/com.atproto.server.createSession", post(create_session))
        .route("/xrpc/com.atproto.server.getSession", get(get_session))
        .route("/xrpc/com.atproto.server.deleteSession", post(delete_session))
        .route("/xrpc/com.atproto.server.refreshSession", post(refresh_session))
        .route("/xrpc/com.atproto.server.requestEmailConfirmation", post(request_email_confirmation))
        .route("/xrpc/com.atproto.server.confirmEmail", post(confirm_email))
        .route("/xrpc/com.atproto.server.requestPasswordReset", post(request_password_reset))
        .route("/xrpc/com.atproto.server.resetPassword", post(reset_password))
        .route("/xrpc/com.atproto.server.deleteAccount", post(delete_account))
        .route("/xrpc/com.atproto.server.createAppPassword", post(create_app_password))
        .route("/xrpc/com.atproto.server.listAppPasswords", get(list_app_passwords))
        .route("/xrpc/com.atproto.server.revokeAppPassword", post(revoke_app_password))
        .route("/xrpc/com.atproto.server.getServiceAuth", get(get_service_auth))
}

/// Create account endpoint
async fn create_account(
    State(ctx): State<AppContext>,
    Json(req): Json<CreateAccountRequest>,
) -> PdsResult<Json<CreateAccountResponse>> {
    tracing::info!("create_account: Starting account creation for handle: {}", req.handle);

    // Validate and use invite code if required
    if ctx.config.invites.required {
        tracing::debug!("create_account: Invite code required, validating");
        let code = req.invite_code.as_ref().ok_or_else(|| {
            crate::error::PdsError::Validation("Invite code required".to_string())
        })?;

        // Validate and mark code as used
        ctx.invite_manager.use_code(code, &req.handle).await
            .map_err(|e| {
                tracing::error!("create_account: Failed to use invite code: {}", e);
                e
            })?;
        tracing::debug!("create_account: Invite code validated successfully");
    }

    // Create account (pass None for invite_code since we already validated it)
    tracing::debug!("create_account: Creating account in database");
    let email = req.email.clone();
    let account = ctx
        .account_manager
        .create_account(req.handle.clone(), req.email, req.password, None)
        .await
        .map_err(|e| {
            tracing::error!("create_account: Failed to create account in database: {}", e);
            e
        })?;
    tracing::info!("create_account: Account created successfully, DID: {}", account.did);

    // Initialize repository for the new account
    tracing::debug!("create_account: Initializing repository for DID: {}", account.did);
    use crate::actor_store::RepositoryManager;
    let repo_mgr = RepositoryManager::with_validation_mode(
        account.did.clone(),
        (*ctx.actor_store).clone(),
        ctx.config.validation_mode,
    );
    repo_mgr.initialize().await
        .map_err(|e| {
            tracing::error!("create_account: Failed to initialize repository: {}", e);
            e
        })?;
    tracing::info!("create_account: Repository initialized successfully");

    // Generate and send email verification token if email was provided
    if email.is_some() && ctx.mailer.is_configured() {
        match ctx.account_manager.generate_email_verification_token(&account.did).await {
            Ok(token) => {
                // Send verification email
                let base_url = ctx.service_url();
                if let Err(e) = ctx.mailer.send_verification_email(
                    email.as_ref().unwrap(),
                    account.handle.as_deref().unwrap_or("unknown"),
                    &token,
                    &base_url
                ).await {
                    tracing::warn!("Failed to send verification email: {}", e);
                    // Don't fail account creation if email fails
                }
            }
            Err(e) => {
                tracing::warn!("Failed to generate verification token: {}", e);
            }
        }
    }

    // Create initial session
    tracing::debug!("create_account: Creating initial session");
    let session = ctx.account_manager.create_session(&account.did, None).await
        .map_err(|e| {
            tracing::error!("create_account: Failed to create session: {}", e);
            e
        })?;
    tracing::info!("create_account: Session created successfully");

    Ok(Json(CreateAccountResponse {
        did: account.did,
        handle: account.handle.unwrap_or_default(),
        access_jwt: session.access_token,
        refresh_jwt: session.refresh_token,
    }))
}

/// Create session (login) endpoint
async fn create_session(
    State(ctx): State<AppContext>,
    Json(req): Json<CreateSessionRequest>,
) -> PdsResult<Json<SessionResponse>> {
    // Try regular password authentication first
    let (account, session) = match ctx
        .account_manager
        .login(&req.identifier, &req.password)
        .await
    {
        Ok(result) => result,
        Err(_) => {
            // If regular password fails, try app password authentication
            ctx.account_manager
                .login_with_app_password(&req.identifier, &req.password)
                .await
                .map(|(account, session, _name)| (account, session))?
        }
    };

    Ok(Json(SessionResponse {
        did: account.did,
        handle: account.handle.unwrap_or_default(),
        access_jwt: session.access_token,
        refresh_jwt: session.refresh_token,
        email: account.email,
        email_confirmed: Some(account.email_confirmed_at.is_some()),
    }))
}

/// Get session info endpoint
async fn get_session(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> PdsResult<Json<SessionInfo>> {
    // Require authentication
    let validated = middleware::require_auth(State(ctx.clone()), headers).await?;

    // Get account info
    let account = ctx.account_manager.get_account(&validated.did).await?;

    Ok(Json(SessionInfo {
        did: account.did,
        handle: account.handle.unwrap_or_default(),
        email: account.email,
        email_confirmed: Some(account.email_confirmed_at.is_some()),
    }))
}

/// Delete session (logout) endpoint
async fn delete_session(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> PdsResult<Json<serde_json::Value>> {
    // Require authentication
    let validated = middleware::require_auth(State(ctx.clone()), headers).await?;

    // Delete session
    ctx.account_manager
        .delete_session(&validated.session_id)
        .await?;

    Ok(Json(serde_json::json!({})))
}

/// Refresh session endpoint
async fn refresh_session(
    State(ctx): State<AppContext>,
    Json(req): Json<RefreshSessionRequest>,
) -> PdsResult<Json<SessionResponse>> {
    // Refresh session
    let session = ctx
        .account_manager
        .refresh_session(&req.refresh_jwt)
        .await?;

    // Get account info
    let account = ctx.account_manager.get_account(&session.did).await?;

    Ok(Json(SessionResponse {
        did: account.did.clone(),
        handle: account.handle.clone().unwrap_or_default(),
        access_jwt: session.access_token,
        refresh_jwt: session.refresh_token,
        email: account.email,
        email_confirmed: Some(account.email_confirmed_at.is_some()),
    }))
}

/// Request email confirmation endpoint
///
/// Generates a new verification token and sends it via email
async fn request_email_confirmation(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> PdsResult<Json<serde_json::Value>> {
    // Require authentication
    let validated = middleware::require_auth(State(ctx.clone()), headers).await?;

    // Get account info to retrieve email
    let account = ctx.account_manager.get_account(&validated.did).await?;

    if account.email.is_none() {
        return Err(crate::error::PdsError::Validation(
            "Account does not have an email address".to_string(),
        ));
    }

    // Generate new verification token
    let token = ctx
        .account_manager
        .request_email_confirmation(&validated.did)
        .await?;

    // Send verification email if mailer is configured
    if ctx.mailer.is_configured() {
        let base_url = ctx.service_url();
        ctx.mailer
            .send_verification_email(
                account.email.as_ref().unwrap(),
                account.handle.as_deref().unwrap_or("unknown"),
                &token,
                &base_url,
            )
            .await?;
    } else {
        tracing::warn!("Email not configured, verification token generated but not sent");
    }

    Ok(Json(serde_json::json!({})))
}

/// Confirm email endpoint
///
/// Verifies the email address using the provided token
#[derive(serde::Deserialize)]
struct ConfirmEmailRequest {
    token: String,
}

async fn confirm_email(
    State(ctx): State<AppContext>,
    Json(req): Json<ConfirmEmailRequest>,
) -> PdsResult<Json<serde_json::Value>> {
    // Confirm email with the provided token
    ctx.account_manager.confirm_email(&req.token).await?;

    Ok(Json(serde_json::json!({})))
}

/// Request password reset endpoint
///
/// Generates a reset token and sends it via email (public endpoint, no auth required)
#[derive(serde::Deserialize)]
struct RequestPasswordResetRequest {
    identifier: String, // Email or handle
}

async fn request_password_reset(
    State(ctx): State<AppContext>,
    Json(req): Json<RequestPasswordResetRequest>,
) -> PdsResult<Json<serde_json::Value>> {
    // Generate reset token (returns token and email address)
    let (token, email) = ctx
        .account_manager
        .generate_password_reset_token(&req.identifier)
        .await?;

    // Get account to retrieve handle for email
    let account = ctx.account_manager.get_account_by_identifier(&req.identifier).await?;

    // Send password reset email if mailer is configured
    if ctx.mailer.is_configured() {
        let base_url = ctx.service_url();
        ctx.mailer
            .send_password_reset_email(&email, account.handle.as_deref().unwrap_or("unknown"), &token, &base_url)
            .await?;
    } else {
        tracing::warn!("Email not configured, reset token generated but not sent");
    }

    // Always return success even if account not found (security best practice - no enumeration)
    Ok(Json(serde_json::json!({})))
}

/// Reset password endpoint
///
/// Validates the token and updates the password
#[derive(serde::Deserialize)]
struct ResetPasswordRequest {
    token: String,
    password: String,
}

async fn reset_password(
    State(ctx): State<AppContext>,
    Json(req): Json<ResetPasswordRequest>,
) -> PdsResult<Json<serde_json::Value>> {
    // Reset password using the token
    ctx.account_manager
        .reset_password(&req.token, &req.password)
        .await?;

    Ok(Json(serde_json::json!({})))
}

/// Delete account endpoint
///
/// Marks account for deletion after password confirmation (30-day grace period)
#[derive(serde::Deserialize)]
struct DeleteAccountRequest {
    password: String,
}

async fn delete_account(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(req): Json<DeleteAccountRequest>,
) -> PdsResult<Json<serde_json::Value>> {
    // Require authentication
    let validated = middleware::require_auth(State(ctx.clone()), headers).await?;

    // Request account deletion with password confirmation
    ctx.account_manager
        .request_account_deletion(&validated.did, &req.password)
        .await?;

    Ok(Json(serde_json::json!({
        "message": "Account marked for deletion. You have 30 days to cancel this request by logging in again."
    })))
}

/// Create app password endpoint
///
/// Creates a new app-specific password for third-party applications.
/// The app password is only shown once in the response.
async fn create_app_password(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(req): Json<CreateAppPasswordRequest>,
) -> PdsResult<Json<CreateAppPasswordResponse>> {
    // Require authentication
    let validated = middleware::require_auth(State(ctx.clone()), headers).await?;

    // App passwords cannot be created using app password authentication
    if validated.is_app_password {
        return Err(crate::error::PdsError::Authorization(
            "Cannot create app password using app password authentication".to_string(),
        ));
    }

    // Create app password
    let privileged = req.privileged.unwrap_or(false);
    let app_password = ctx
        .account_manager
        .create_app_password(&validated.did, &req.name, privileged)
        .await?;

    Ok(Json(CreateAppPasswordResponse { app_password }))
}

/// List app passwords endpoint
///
/// Lists all app passwords for the authenticated user (without the actual passwords).
async fn list_app_passwords(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> PdsResult<Json<ListAppPasswordsResponse>> {
    // Require authentication
    let validated = middleware::require_auth(State(ctx.clone()), headers).await?;

    // List app passwords
    let passwords = ctx
        .account_manager
        .list_app_passwords(&validated.did)
        .await?;

    Ok(Json(ListAppPasswordsResponse { passwords }))
}

/// Revoke app password endpoint
///
/// Revokes an app password and invalidates all sessions created with it.
async fn revoke_app_password(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(req): Json<RevokeAppPasswordRequest>,
) -> PdsResult<Json<serde_json::Value>> {
    // Require authentication
    let validated = middleware::require_auth(State(ctx.clone()), headers).await?;

    // App passwords cannot revoke other app passwords (must use main password)
    if validated.is_app_password {
        return Err(crate::error::PdsError::Authorization(
            "Cannot revoke app password using app password authentication".to_string(),
        ));
    }

    // Revoke app password
    ctx.account_manager
        .revoke_app_password(&validated.did, &req.name)
        .await?;

    Ok(Json(serde_json::json!({})))
}

// Constants for service auth token expiration
const HOUR_IN_SECONDS: i64 = 3600;
const MINUTE_IN_SECONDS: i64 = 60;

/// Request parameters for getServiceAuth
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetServiceAuthQuery {
    /// Target service DID (audience)
    aud: String,
    /// Optional expiration timestamp (seconds since epoch)
    #[serde(default)]
    exp: Option<i64>,
    /// Optional lexicon method the token will be used for
    #[serde(default)]
    lxm: Option<String>,
}

/// Response for getServiceAuth
#[derive(serde::Serialize)]
struct GetServiceAuthResponse {
    token: String,
}

/// Get service auth endpoint
///
/// Generates a service auth JWT for authenticated server-to-server communication.
/// This allows the user's PDS to authenticate requests to other services on behalf of the user.
///
/// # Authentication
/// Requires a valid session token (Bearer token in Authorization header).
///
/// # Validation Rules
/// - Token must not be expired when requested
/// - Expiration cannot be > 1 hour in future
/// - Method-less tokens cannot have expiration > 1 minute in future
/// - Takendown accounts can only get tokens for account migration methods
/// - Some methods are "protected" and cannot use service auth
async fn get_service_auth(
    State(ctx): State<AppContext>,
    auth: AuthContext,
    Query(req): Query<GetServiceAuthQuery>,
) -> PdsResult<Json<GetServiceAuthResponse>> {
    let now = Utc::now().timestamp();

    // Validate expiration if provided
    let exp_duration = if let Some(exp) = req.exp {
        // Check that expiration is not in the past
        if exp <= now {
            return Err(PdsError::Validation(
                "Token expiration cannot be in the past".to_string(),
            ));
        }

        let duration = exp - now;

        // Check expiration is not too far in the future
        if duration > HOUR_IN_SECONDS {
            return Err(PdsError::Validation(
                format!(
                    "Token expiration too far in future: {} seconds (max: {} seconds)",
                    duration, HOUR_IN_SECONDS
                )
            ));
        }

        // Check method-less tokens have shorter expiration
        if req.lxm.is_none() && duration > MINUTE_IN_SECONDS {
            return Err(PdsError::Validation(
                format!(
                    "Tokens without a lexicon method cannot have expiration > {} seconds (requested: {} seconds)",
                    MINUTE_IN_SECONDS, duration
                )
            ));
        }

        Some(duration)
    } else {
        None
    };

    // Check if account is taken down
    // Takendown accounts can only get tokens for specific migration methods
    if let Ok(is_taken_down) = ctx.moderation_manager.is_taken_down(&auth.did).await {
        if is_taken_down {
            // Check if this is a migration-related method
            let is_migration_method = req.lxm.as_ref().map_or(false, |method| {
                method.starts_with("com.atproto.server.activateAccount")
                    || method.starts_with("com.atproto.identity.updateHandle")
            });

            if !is_migration_method {
                return Err(PdsError::Validation(
                    "Account is taken down. Only migration methods are allowed".to_string(),
                ));
            }
        }
    }

    // Check if method is protected (cannot use service auth)
    // Protected methods require direct user authentication
    if let Some(ref method) = req.lxm {
        let protected_methods: &[&str] = &[
            "com.atproto.server.createSession",
            "com.atproto.server.createAccount",
            "com.atproto.server.resetPassword",
            "com.atproto.server.deleteAccount",
        ];

        if protected_methods.iter().any(|&pm| method.starts_with(pm)) {
            return Err(PdsError::Validation(
                format!("Method '{}' is protected and cannot use service auth", method)
            ));
        }
    }

    // Get signing key from config
    let signing_key_hex = &ctx.config.authentication.repo_signing_key;
    let signing_key_bytes = hex::decode(signing_key_hex).map_err(|e| {
        PdsError::Internal(format!("Failed to decode signing key: {}", e))
    })?;

    if signing_key_bytes.len() != 32 {
        return Err(PdsError::Internal(
            "Signing key must be exactly 32 bytes".to_string(),
        ));
    }

    // Generate service auth JWT
    let token = service_auth::create_service_jwt(
        &auth.did,                           // Issuer DID (authenticated user)
        &req.aud,                            // Audience DID (target service)
        exp_duration,                        // Expiration duration in seconds
        req.lxm.as_deref(),                  // Optional lexicon method
        &signing_key_bytes,                  // Signing key
    )?;

    tracing::info!(
        did = %auth.did,
        aud = %req.aud,
        lxm = ?req.lxm,
        exp = ?req.exp,
        "service_auth_token_generated"
    );

    Ok(Json(GetServiceAuthResponse { token }))
}
