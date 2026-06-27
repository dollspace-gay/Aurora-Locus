/// com.atproto.server.* endpoints
use crate::{
    account::{
        CreateAccountRequest, CreateAccountResponse, CreateAppPasswordRequest,
        CreateAppPasswordResponse, CreateSessionRequest, ListAppPasswordsResponse,
        RevokeAppPasswordRequest, SessionInfo, SessionResponse,
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
use serde::Serialize;

/// Build server routes
pub fn routes() -> Router<AppContext> {
    Router::new()
        // Server metadata
        .route(
            "/xrpc/com.atproto.server.describeServer",
            get(describe_server),
        )
        // Account management
        .route(
            "/xrpc/com.atproto.server.createAccount",
            post(create_account),
        )
        .route(
            "/xrpc/com.atproto.server.createSession",
            post(create_session),
        )
        .route("/xrpc/com.atproto.server.getSession", get(get_session))
        .route(
            "/xrpc/com.atproto.server.deleteSession",
            post(delete_session),
        )
        .route(
            "/xrpc/com.atproto.server.refreshSession",
            post(refresh_session),
        )
        .route(
            "/xrpc/com.atproto.server.requestEmailConfirmation",
            post(request_email_confirmation),
        )
        .route("/xrpc/com.atproto.server.confirmEmail", post(confirm_email))
        .route(
            "/xrpc/com.atproto.server.requestPasswordReset",
            post(request_password_reset),
        )
        .route(
            "/xrpc/com.atproto.server.resetPassword",
            post(reset_password),
        )
        .route(
            "/xrpc/com.atproto.server.requestEmailUpdate",
            post(request_email_update),
        )
        .route("/xrpc/com.atproto.server.updateEmail", post(update_email))
        .route(
            "/xrpc/com.atproto.server.requestAccountDelete",
            post(request_account_delete),
        )
        .route(
            "/xrpc/com.atproto.server.deleteAccount",
            post(delete_account),
        )
        .route(
            "/xrpc/com.atproto.server.activateAccount",
            post(activate_account),
        )
        .route(
            "/xrpc/com.atproto.server.deactivateAccount",
            post(deactivate_account),
        )
        .route(
            "/xrpc/com.atproto.server.checkAccountStatus",
            get(check_account_status),
        )
        // App passwords
        .route(
            "/xrpc/com.atproto.server.createAppPassword",
            post(create_app_password),
        )
        .route(
            "/xrpc/com.atproto.server.listAppPasswords",
            get(list_app_passwords),
        )
        .route(
            "/xrpc/com.atproto.server.revokeAppPassword",
            post(revoke_app_password),
        )
        // Invite codes
        .route(
            "/xrpc/com.atproto.server.getAccountInviteCodes",
            get(get_account_invite_codes),
        )
        .route(
            "/xrpc/com.atproto.server.createInviteCode",
            post(create_invite_code),
        )
        .route(
            "/xrpc/com.atproto.server.createInviteCodes",
            post(create_invite_codes),
        )
        // Service auth
        .route(
            "/xrpc/com.atproto.server.getServiceAuth",
            get(get_service_auth),
        )
        // Signing key reservation
        .route(
            "/xrpc/com.atproto.server.reserveSigningKey",
            post(reserve_signing_key),
        )
}

/// Create account endpoint
async fn create_account(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(req): Json<CreateAccountRequest>,
) -> PdsResult<Json<CreateAccountResponse>> {
    tracing::info!(
        "create_account: Starting account creation for handle: {}",
        req.handle
    );

    // IP-based rate limiting for account creation (100 per 5 minutes per IP)
    // This is handled by the endpoint-specific middleware limits, but we add
    // an additional per-IP check here for extra protection
    if let Some(client_ip) =
        crate::rate_limit::extract_client_ip(&headers, ctx.rate_limiter.trust_proxy)
    {
        tracing::debug!(
            "create_account: Checking IP-based rate limit for IP: {}",
            client_ip
        );
        ctx.rate_limiter.check_ip(&client_ip)?;
    }

    // If a DID is provided, verify we have a reserved keypair for it
    // This validates that the caller went through the proper account creation flow
    let reserved_signing_key = if let Some(ref did) = req.did {
        tracing::debug!(
            "create_account: Looking up reserved keypair for DID: {}",
            did
        );
        let keypair = ctx.actor_store.get_reserved_keypair(did).await?;
        if keypair.is_none() {
            tracing::warn!("create_account: No reserved keypair found for DID: {}", did);
            // Note: In strict mode, we would reject this. For now, we allow it
            // since the account manager will generate a new DID anyway.
        }
        keypair.map(|kp| kp.did())
    } else {
        None
    };

    // Validate and use invite code if required
    if ctx.config.invites.required {
        tracing::debug!("create_account: Invite code required, validating");
        let code = req.invite_code.as_ref().ok_or_else(|| {
            crate::error::PdsError::Validation("Invite code required".to_string())
        })?;

        // Validate and mark code as used
        ctx.invite_manager
            .use_code(code, &req.handle)
            .await
            .map_err(|e| {
                tracing::error!("create_account: Failed to use invite code: {}", e);
                e
            })?;
        tracing::debug!("create_account: Invite code validated successfully");
    }

    // Create account (pass None for invite_code since we already validated it).
    // Arc 13 §6.3.3 / Step 2.2: pass through the optional
    // `recovery_key` input so it ends up first in the genesis op's
    // rotation_keys per §6.3.3 priority order.
    tracing::debug!("create_account: Creating account in database");
    let email = req.email.clone();
    let account = ctx
        .account_manager
        .create_account(req.handle.clone(), req.email, req.password, None, req.recovery_key)
        .await
        .map_err(|e| {
            tracing::error!(
                "create_account: Failed to create account in database: {}",
                e
            );
            e
        })?;
    tracing::info!(
        "create_account: Account created successfully, DID: {}",
        account.did
    );

    // Clear the reserved keypair now that the account is created
    if let Some(ref signing_key) = reserved_signing_key {
        if let Err(e) = ctx
            .actor_store
            .clear_reserved_keypair(signing_key, req.did.as_deref())
            .await
        {
            tracing::warn!("create_account: Failed to clear reserved keypair: {}", e);
            // Don't fail account creation for cleanup failure
        }
    }

    // Initialize repository for the new account
    tracing::debug!(
        "create_account: Initializing repository for DID: {}",
        account.did
    );
    use crate::actor_store::RepositoryManager;
    let repo_mgr = RepositoryManager::with_validation_mode(
        account.did.clone(),
        (*ctx.actor_store).clone(),
        ctx.config.validation_mode,
    );
    repo_mgr.initialize().await.map_err(|e| {
        tracing::error!("create_account: Failed to initialize repository: {}", e);
        e
    })?;
    tracing::info!("create_account: Repository initialized successfully");

    // Arc 15 §8.3.8: four-emit sequence (identity → account Active
    // → commit genesis → sync). Best-effort: emission failures are
    // logged but do NOT fail account creation (the account exists;
    // consumers will resync on next connection).
    if let Some(ref handle) = account.handle {
        if let Err(e) = crate::api::account_emit::create_account_emit_sequence(
            &ctx,
            &account.did,
            handle,
        )
        .await
        {
            tracing::warn!(
                did = %account.did,
                error = %e,
                "create_account: four-emit sequence failed (account created OK)"
            );
        }
    }

    // Default audience auto-create (#334 / §6.6.2 item 2 / §7.3.3): when the
    // deployment's default-audience-mode is a participating mode, author a
    // policy.audience record on the new account carrying that initial mode (the
    // holder populates its members later). The `nobody` default authors nothing
    // (the prior behavior). Best-effort: a failure is logged but never fails
    // account creation — the account and its repo already exist.
    if let Some(mode) = crate::kryphocron_policy::default_audience_mode(&ctx.account_db).await {
        match crate::api::kryphocron_endpoints::author_default_audience(&ctx, &account.did, &mode)
            .await
        {
            Ok(uri) => tracing::info!(
                did = %account.did,
                mode = %mode,
                uri = %uri,
                "create_account: authored default policy.audience"
            ),
            Err(e) => tracing::warn!(
                did = %account.did,
                mode = %mode,
                error = %e,
                "create_account: default audience auto-create failed (account created OK)"
            ),
        }
    }

    // Generate and send email verification token if email was provided
    if let Some(email_val) = &email {
        if ctx.mailer.is_configured() {
            match ctx
                .account_manager
                .generate_email_verification_token(&account.did)
                .await
            {
                Ok(token) => {
                    // Send verification email
                    let base_url = ctx.service_url();
                    if let Err(e) = ctx
                        .mailer
                        .send_verification_email(
                            email_val,
                            account.handle.as_deref().unwrap_or("unknown"),
                            &token,
                            &base_url,
                        )
                        .await
                    {
                        tracing::warn!("Failed to send verification email: {}", e);
                        // Don't fail account creation if email fails
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to generate verification token: {}", e);
                }
            }
        }
    }

    // Create initial session
    tracing::debug!("create_account: Creating initial session");
    let session = ctx
        .account_manager
        .create_session(&account.did, None)
        .await
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
    headers: HeaderMap,
    Json(req): Json<CreateSessionRequest>,
) -> PdsResult<Json<SessionResponse>> {
    // Identifier+IP rate limiting for login attempts (30 per 5min per identifier+IP)
    // Prevents brute-force attacks on specific accounts from specific IPs
    if let Some(client_ip) =
        crate::rate_limit::extract_client_ip(&headers, ctx.rate_limiter.trust_proxy)
    {
        tracing::debug!(
            "create_session: Checking identifier+IP rate limit for {}",
            req.identifier
        );
        ctx.rate_limiter
            .check_identifier_ip(&req.identifier, &client_ip)?;
    }

    // Try regular password authentication first. If that returns a
    // routine auth failure (NotFound / Authentication), fall through
    // silently to the app-password path — that's the intended dual-login
    // shape (single endpoint serves regular + app passwords). Anything
    // else (database errors, decode failures, internal errors) must NOT
    // be silently swallowed; #130 caught a PG-only TIMESTAMPTZ decode
    // failure masked here for weeks, surfacing as a generic NotFound
    // with zero log signal. Emit at warn for non-auth errors so the
    // next decode/database failure is grep-visible.
    let (account, session) = match ctx
        .account_manager
        .login(&req.identifier, &req.password)
        .await
    {
        Ok(result) => result,
        Err(err) => {
            if !matches!(
                err,
                PdsError::NotFound(_) | PdsError::Authentication(_)
            ) {
                tracing::warn!(
                    error = %err,
                    "primary login path errored unexpectedly; falling back to app-password"
                );
            }
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
    // Arc 12 §5.3.4: forwarded-routes auth path (single-aud in
    // standalone mode; multi-aud accepting entryway-issued tokens in
    // entryway mode).
    let unified = middleware::require_auth_forwarded(State(ctx.clone()), headers.clone()).await?;
    let did = unified.did().to_string();

    // #297 — the caller's operator role, for the admin UI's live tier
    // resolution. Looked up locally from `admin_roles` regardless of entryway
    // mode (the role table is this PDS's, not the entryway's); `None` for
    // regular accounts → the field is omitted (standard session shape). Fail
    // soft: a lookup error degrades to no role rather than failing the session.
    let role = ctx
        .admin_role_manager
        .get_role(&did)
        .await
        .ok()
        .flatten()
        .map(|r| r.role.as_str().to_string());

    // Arc 12 §5.3.8 mint-pattern forward. Entryway is the canonical
    // source of session info in entryway mode (it owns the account
    // identity).
    if let Some(entryway) = ctx.entryway_client.as_ref() {
        let fwd_headers = ctx
            .entryway_auth_headers(&did, "com.atproto.server.getSession")
            .await?;
        let mut resp: SessionInfo = entryway
            .xrpc_get_json("com.atproto.server.getSession", fwd_headers, &[])
            .await?;
        // The entryway owns identity but not this PDS's operator roles — graft
        // the locally-resolved role onto the forwarded session.
        resp.role = role;
        return Ok(Json(resp));
    }

    // Standalone path.
    let account = ctx.account_manager.get_account(&did).await?;

    Ok(Json(SessionInfo {
        did: account.did,
        handle: account.handle.unwrap_or_default(),
        email: account.email,
        email_confirmed: Some(account.email_confirmed_at.is_some()),
        role,
    }))
}

/// Forensic debug log for session endpoints that receive no usable refresh
/// token (Arc 4 Q6, design §3.2/§3.3). Coarse by design — fires on any
/// no-bearer-token / wrong-token-type request. Production-silent under the
/// default `aurora_locus=info` filter; visible with
/// `RUST_LOG=aurora_locus::api::server=debug`.
fn log_no_valid_refresh_token(endpoint: &str, reason: &str) {
    tracing::debug!(
        target: "aurora_locus::api::server",
        event = "aurora_session_endpoint_no_valid_refresh_token",
        endpoint,
        reason,
    );
}

/// Delete session (logout) endpoint.
///
/// atproto authenticates `deleteSession` with the **refresh** token in
/// `Authorization: Bearer` (Arc 4 §3.3) — not an access token. The underlying
/// `delete_session` chokepoint atomically revokes the refresh token (Q8).
async fn delete_session(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> PdsResult<Json<serde_json::Value>> {
    let token = middleware::extract_bearer_token(&headers).ok_or_else(|| {
        log_no_valid_refresh_token("deleteSession", "no_bearer_token");
        PdsError::Authentication("Missing or invalid Authorization header".to_string())
    })?;

    let identity = ctx.account_manager.validate_refresh_token(&token).await?;

    // Forensic record of the credential being revoked (production-silent under
    // the default `aurora_locus=info` filter; visible at debug).
    tracing::debug!(
        target: "aurora_locus::api::server",
        event = "aurora_session_deleted",
        did = %identity.did,
        token_id = %identity.token_id,
        "deleteSession: revoking session and its refresh token",
    );

    ctx.account_manager
        .delete_session(&identity.session_id)
        .await?;

    Ok(Json(serde_json::json!({})))
}

/// Refresh session endpoint.
///
/// atproto authenticates `refreshSession` with the refresh token in
/// `Authorization: Bearer` (Arc 4 §3.2) — not a JSON body. Downstream
/// rotate/mint is unchanged (M4).
async fn refresh_session(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> PdsResult<Json<SessionResponse>> {
    let token = middleware::extract_bearer_token(&headers).ok_or_else(|| {
        log_no_valid_refresh_token("refreshSession", "no_bearer_token");
        PdsError::Authentication("Missing or invalid Authorization header".to_string())
    })?;

    let session = ctx.account_manager.refresh_session(&token).await?;

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

    // Per-user rate limiting for email confirmation requests (10 per hour per DID)
    // Prevents email spam
    ctx.rate_limiter.check_did_endpoint(
        &validated.did,
        "/xrpc/com.atproto.server.requestEmailConfirmation",
    )?;

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
#[derive(serde::Serialize, serde::Deserialize)]
struct RequestPasswordResetRequest {
    identifier: String, // Email or handle
}

async fn request_password_reset(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(req): Json<RequestPasswordResetRequest>,
) -> PdsResult<Json<serde_json::Value>> {
    // Arc 12 §5.3.8 passthru-pattern forward. requestPasswordReset
    // is token-free: filter incoming headers per §5.3.6 via
    // `entryway_passthru_headers` and forward the body as-is. The
    // entryway is the canonical source of password-reset state in
    // entryway mode.
    if let Some(entryway_client) = ctx.entryway_client.as_ref() {
        let fwd_headers =
            crate::federation::entryway_passthru_headers(&headers, None)?;
        let resp: serde_json::Value = entryway_client
            .xrpc_post_json(
                "com.atproto.server.requestPasswordReset",
                fwd_headers,
                &req,
            )
            .await?;
        return Ok(Json(resp));
    }

    // Standalone path (unchanged).
    // IP-based rate limiting for password reset (50 per 5 minutes per IP)
    // Prevents email spam and denial of service
    if let Some(client_ip) =
        crate::rate_limit::extract_client_ip(&headers, ctx.rate_limiter.trust_proxy)
    {
        tracing::debug!(
            "request_password_reset: Checking IP-based rate limit for IP: {}",
            client_ip
        );
        ctx.rate_limiter.check_ip(&client_ip)?;
    }

    // Generate reset token (returns token and email address)
    let (token, email) = ctx
        .account_manager
        .generate_password_reset_token(&req.identifier)
        .await?;

    // Get account to retrieve handle for email
    let account = ctx
        .account_manager
        .get_account_by_identifier(&req.identifier)
        .await?;

    // Send password reset email if mailer is configured
    if ctx.mailer.is_configured() {
        let base_url = ctx.service_url();
        ctx.mailer
            .send_password_reset_email(
                &email,
                account.handle.as_deref().unwrap_or("unknown"),
                &token,
                &base_url,
            )
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

/// Request email update endpoint
///
/// Initiates the email update flow. If the user's email is already confirmed,
/// generates a token and sends it to the current email address.
/// Returns whether a token is required to complete the update.
async fn request_email_update(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> PdsResult<Json<serde_json::Value>> {
    // Require authentication
    let validated = middleware::require_auth(State(ctx.clone()), headers).await?;

    // Per-user rate limiting (15 per day, 5 per hour)
    ctx.rate_limiter.check_did_endpoint(
        &validated.did,
        "/xrpc/com.atproto.server.requestEmailUpdate",
    )?;

    // Get account info
    let account = ctx.account_manager.get_account(&validated.did).await?;

    // Account must have email
    let email = account.email.ok_or_else(|| {
        crate::error::PdsError::Validation("Account does not have an email address".to_string())
    })?;

    // Token is required only if email is already confirmed
    let token_required = account.email_confirmed_at.is_some();

    if token_required {
        // Generate token and send to current email
        let token = ctx
            .account_manager
            .generate_email_update_token(&validated.did)
            .await?;

        if ctx.mailer.is_configured() {
            ctx.mailer
                .send_email_update_email(
                    &email,
                    account.handle.as_deref().unwrap_or("user"),
                    &token,
                )
                .await?;

            tracing::info!(
                did = %validated.did,
                "email_update_token_sent"
            );
        } else {
            tracing::warn!(
                did = %validated.did,
                token = %token,
                "Email not configured, email update token generated but not sent"
            );
        }
    }

    Ok(Json(serde_json::json!({
        "tokenRequired": token_required
    })))
}

/// Update email endpoint
///
/// Updates the account's email address. If the current email was confirmed,
/// a token from requestEmailUpdate is required.
#[derive(serde::Deserialize)]
struct UpdateEmailRequest {
    /// The new email address
    email: String,
    /// The confirmation token (required if current email was confirmed)
    token: Option<String>,
}

async fn update_email(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(req): Json<UpdateEmailRequest>,
) -> PdsResult<Json<serde_json::Value>> {
    // Require authentication
    let validated = middleware::require_auth(State(ctx.clone()), headers).await?;

    // v0.8 arc 3 (#184) — reject ':' in the email (see
    // AccountManager::validate_email). Separate guard before the existing
    // check so the charset-specific message fires (M-5).
    if req.email.contains(':') {
        return Err(crate::error::PdsError::Validation(
            "Email address must not contain ':'".to_string(),
        ));
    }

    // Basic email format validation
    if !req.email.contains('@') || req.email.len() < 3 {
        return Err(crate::error::PdsError::Validation(
            "Invalid email format".to_string(),
        ));
    }

    // Get account info
    let account = ctx.account_manager.get_account(&validated.did).await?;

    // If email was confirmed, token is required
    if account.email_confirmed_at.is_some() {
        let token = req.token.ok_or_else(|| {
            crate::error::PdsError::Validation("Confirmation token required".to_string())
        })?;

        // Validate token
        ctx.account_manager
            .validate_email_update_token(&validated.did, &token)
            .await?;
    }

    // Update email
    ctx.account_manager
        .update_email(&validated.did, &req.email)
        .await?;

    Ok(Json(serde_json::json!({})))
}

/// Request account delete endpoint
///
/// Initiates the account deletion flow by generating a token and sending it via email.
/// The user must provide this token (along with password and DID) to the deleteAccount endpoint.
/// Rate limited to 15 per day, 5 per hour per DID.
async fn request_account_delete(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> PdsResult<Json<serde_json::Value>> {
    // Require authentication
    let validated = middleware::require_auth(State(ctx.clone()), headers).await?;

    // Per-user rate limiting for account deletion requests (5 per hour per DID)
    // Prevents email spam and abuse
    ctx.rate_limiter.check_did_endpoint(
        &validated.did,
        "/xrpc/com.atproto.server.requestAccountDelete",
    )?;

    // Get account info to retrieve email
    let account = ctx.account_manager.get_account(&validated.did).await?;

    // Account must have email to receive deletion token
    let email = account.email.ok_or_else(|| {
        crate::error::PdsError::Validation("Account does not have an email address".to_string())
    })?;

    // Generate deletion token
    let token = ctx
        .account_manager
        .generate_account_delete_token(&validated.did)
        .await?;

    // Send deletion confirmation email
    if ctx.mailer.is_configured() {
        ctx.mailer
            .send_account_delete_email(&email, account.handle.as_deref().unwrap_or("user"), &token)
            .await?;

        tracing::info!(
            did = %validated.did,
            "account_delete_token_sent"
        );
    } else {
        tracing::warn!(
            did = %validated.did,
            token = %token,
            "Email not configured, deletion token generated but not sent"
        );
    }

    Ok(Json(serde_json::json!({})))
}

/// Delete account endpoint
///
/// Permanently deletes an account after verifying the deletion token and password.
/// This is the ATProto-compliant flow:
/// 1. User calls requestAccountDelete to receive a token via email
/// 2. User calls deleteAccount with did, password, and token to confirm deletion
#[derive(serde::Deserialize)]
struct DeleteAccountRequest {
    /// The DID of the account to delete
    did: String,
    /// The account password for verification
    password: String,
    /// The deletion token received via email from requestAccountDelete
    token: String,
}

async fn delete_account(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(req): Json<DeleteAccountRequest>,
) -> PdsResult<Json<serde_json::Value>> {
    // Per-? tracing pattern from Arc 13 #71 closure (chainlink #86):
    // every error-propagation point logs at tracing::error! with an
    // `at_step` field so operators get a one-line cause on any
    // future failure path.

    // IP-based rate limiting for account deletion (50 per 5 minutes)
    if let Some(client_ip) =
        crate::rate_limit::extract_client_ip(&headers, ctx.rate_limiter.trust_proxy)
    {
        ctx.rate_limiter.check_ip(&client_ip).map_err(|e| {
            tracing::error!(
                at_step = "delete_account:rate_limit",
                did = %req.did,
                client_ip = %client_ip,
                error = %e,
                "deleteAccount: per-IP rate limit rejected"
            );
            e
        })?;
    }

    // Get account (include deactivated and taken down accounts)
    let account = ctx.account_manager.get_account(&req.did).await.map_err(|e| {
        tracing::error!(
            at_step = "delete_account:get_account",
            did = %req.did,
            error = %e,
            "deleteAccount: account lookup failed"
        );
        e
    })?;

    // Verify password - must have local account credentials
    let password_hash = account.password_hash.ok_or_else(|| {
        tracing::error!(
            at_step = "delete_account:no_password_hash",
            did = %req.did,
            "deleteAccount: account has no local password_hash (federated actor?)"
        );
        crate::error::PdsError::Authorization("No local account credentials".to_string())
    })?;

    let valid =
        crate::auth::PasswordHasher::verify(&req.password, &password_hash).map_err(|e| {
            tracing::error!(
                at_step = "delete_account:password_verify",
                did = %req.did,
                error = %e,
                "deleteAccount: password verification call failed"
            );
            crate::error::PdsError::Internal(format!("Password verification failed: {}", e))
        })?;

    if !valid {
        tracing::error!(
            at_step = "delete_account:password_invalid",
            did = %req.did,
            "deleteAccount: invalid password"
        );
        return Err(crate::error::PdsError::Authentication(
            "Invalid did or password".to_string(),
        ));
    }

    // Validate deletion token (chainlink #86 root cause was here:
    // sqlx::Any bool/BIGINT mismatch on the `used` column read).
    ctx.account_manager
        .validate_account_delete_token(&req.did, &req.token)
        .await
        .map_err(|e| {
            tracing::error!(
                at_step = "delete_account:validate_token",
                did = %req.did,
                error_kind = ?std::mem::discriminant(&e),
                error = %e,
                "deleteAccount: token validation failed"
            );
            e
        })?;

    // Mark token as used
    ctx.account_manager
        .mark_delete_token_used(&req.token)
        .await
        .map_err(|e| {
            tracing::error!(
                at_step = "delete_account:mark_token_used",
                did = %req.did,
                error = %e,
                "deleteAccount: failed to mark token used"
            );
            e
        })?;

    // Delete actor store data (repository, blobs, etc.)
    // This should be done before deleting account records.
    if let Err(e) = ctx.actor_store.destroy(&req.did).await {
        tracing::warn!(
            at_step = "delete_account:actor_store_destroy",
            did = %req.did,
            error = %e,
            "deleteAccount: actor store cleanup failed; continuing with account row delete"
        );
        // Continue with account deletion even if actor store cleanup fails.
    }

    // Permanently delete account from database
    ctx.account_manager
        .delete_account_permanent(&req.did)
        .await
        .map_err(|e| {
            tracing::error!(
                at_step = "delete_account:delete_permanent",
                did = %req.did,
                error = %e,
                "deleteAccount: permanent delete failed (actor store may be already destroyed)"
            );
            e
        })?;

    // Arc 15 §8.3.3: emit Deleted #account event + wipe prior history.
    // Two-await non-atomic per §8.5.5 (matches bsky-PDS pattern;
    // consumers duplicate-suppress on did).
    let deletion_seq = ctx
        .sequencer
        .sequence_account(crate::sequencer::events::AccountEvent::from_status(
            req.did.clone(),
            crate::sequencer::events::AccountStatus::Deleted,
        ))
        .await
        .map_err(|e| {
            tracing::error!(
                at_step = "delete_account:sequence_deletion_event",
                did = %req.did,
                error = %e,
                "deleteAccount: failed to emit #account Deleted"
            );
            e
        })?;
    ctx.sequencer
        .delete_all_for_user(&req.did, &[deletion_seq])
        .await
        .map_err(|e| {
            tracing::error!(
                at_step = "delete_account:wipe_prior_events",
                did = %req.did,
                deletion_seq,
                error = %e,
                "deleteAccount: retention wipe failed (deletion event emitted OK)"
            );
            e
        })?;

    tracing::info!(
        did = %req.did,
        deletion_seq,
        "account_deleted_permanently"
    );

    Ok(Json(serde_json::json!({})))
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
            return Err(PdsError::Validation(format!(
                "Token expiration too far in future: {} seconds (max: {} seconds)",
                duration, HOUR_IN_SECONDS
            )));
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
            let is_migration_method = req.lxm.as_ref().is_some_and(|method| {
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
            return Err(PdsError::Validation(format!(
                "Method '{}' is protected and cannot use service auth",
                method
            )));
        }
    }

    // Cluster 2 Member 2.1 (#143): sign with the per-account
    // atproto_signing_key (Arc 13 §6.3.2 per-DID key surface) so the
    // JWT signature matches the issuer DID's published verification
    // method on receiver-side resolution. Pre-fix: signed with the
    // server-wide ctx.config.authentication.repo_signing_key while
    // claiming auth.did as iss — receiver fetches auth.did's
    // published key, gets a per-account key, signature fails. Same
    // bug shape as Arc 18 / #117 (record-write signer correctness),
    // one layer up. Same key surface used by genesis-commit signing
    // (Arc 15, api/account_emit.rs::create_account_emit_sequence)
    // and Arc 18's record-write fix.
    let signing_key_bytes = ctx
        .account_manager
        .get_atproto_signing_key_bytes(&auth.did)
        .await?;

    if signing_key_bytes.len() != 32 {
        return Err(PdsError::Internal(
            "Signing key must be exactly 32 bytes".to_string(),
        ));
    }

    // Generate service auth JWT
    let token = service_auth::create_service_jwt(
        &auth.did,          // Issuer DID (authenticated user)
        &req.aud,           // Audience DID (target service)
        exp_duration,       // Expiration duration in seconds
        req.lxm.as_deref(), // Optional lexicon method
        &signing_key_bytes, // Per-account signing key (chainlink #143)
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

// ==================== New Endpoints for XRPC Parity ====================

/// Response for describeServer
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DescribeServerResponse {
    /// Server DID
    did: String,
    /// Available authentication methods
    available_user_domains: Vec<String>,
    /// Invite code required for registration
    #[serde(skip_serializing_if = "Option::is_none")]
    invite_code_required: Option<bool>,
    /// Phone verification required
    #[serde(skip_serializing_if = "Option::is_none")]
    phone_verification_required: Option<bool>,
    /// Links (e.g., privacy policy, terms of service)
    #[serde(skip_serializing_if = "Option::is_none")]
    links: Option<DescribeServerLinks>,
    /// Aurora-Locus additive extension (#344): minimal federation posture for
    /// any ATProto peer. NOT part of upstream's canonical describeServer —
    /// Bluesky's spec does not define this field; Aurora-Locus declares it
    /// (response-additive, so clients that don't know it ignore it). It always
    /// reflects the substrate's *enforced* federation state; future
    /// runtime-mutable federation policy won't change this field's semantic. If
    /// upstream ever adds a `federation` field with a different shape, realign.
    /// Richer Aurora-aware posture (relay URLs, appview/public URL) lives on the
    /// federation-scoped `com.aurora.federation.describePosture`.
    #[serde(skip_serializing_if = "Option::is_none")]
    federation: Option<FederationDescribe>,
}

/// The `federation` extension block on `describeServer` (#344). Minimal by
/// design: when federation is off, only `enabled: false` is emitted (URLs and
/// flags are omitted — the off-posture is itself the information a peer needs).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FederationDescribe {
    enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    firehose_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    crawl_enabled: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DescribeServerLinks {
    #[serde(skip_serializing_if = "Option::is_none")]
    privacy_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    terms_of_service: Option<String>,
}

/// Describe server endpoint
///
/// Returns server metadata, DID, and available authentication methods.
/// This is a public endpoint used for federation discovery.
async fn describe_server(State(ctx): State<AppContext>) -> PdsResult<Json<DescribeServerResponse>> {
    // v0.9 Federation runtime-mutability arc Phase A (#387): firehose_enabled
    // resolves the runtime override (→ env-config fallback) so the advertised
    // describe flag reflects operator changes without a restart.
    let firehose_enabled = crate::api::aurora_admin::resolve_federation_flag(
        &ctx,
        crate::api::aurora_admin::FEDERATION_FIREHOSE_ENABLED_KEY,
        ctx.config.federation.firehose_enabled,
    )
    .await;
    // Phase A (#388): crawl_enabled resolves the runtime override (→ config).
    let crawl_enabled = crate::api::aurora_admin::resolve_federation_flag(
        &ctx,
        crate::api::aurora_admin::FEDERATION_CRAWL_ENABLED_KEY,
        ctx.config.federation.crawl_enabled,
    )
    .await;
    Ok(Json(DescribeServerResponse {
        did: ctx.config.service.service_did.clone(),
        available_user_domains: ctx.config.identity.service_handle_domains.clone(),
        invite_code_required: Some(ctx.config.invites.required),
        phone_verification_required: Some(false), // Not implemented yet
        links: None,                              // TODO: Add from config if available
        federation: {
            let fc = &ctx.config.federation;
            // enabled always present; flags only when on (off-posture = enabled:false alone).
            Some(FederationDescribe {
                enabled: fc.enabled,
                firehose_enabled: fc.enabled.then_some(firehose_enabled),
                crawl_enabled: fc.enabled.then_some(crawl_enabled),
            })
        },
    }))
}

/// Response for getAccountInviteCodes
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GetAccountInviteCodesResponse {
    codes: Vec<InviteCodeInfo>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InviteCodeInfo {
    code: String,
    available: i32,
    disabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    for_account: Option<String>,
    created_at: String,
    uses: Vec<InviteCodeUse>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InviteCodeUse {
    used_by: String,
    used_at: String,
}

/// Get account invite codes endpoint
///
/// Returns all invite codes allocated to or created by the authenticated user.
async fn get_account_invite_codes(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> PdsResult<Json<GetAccountInviteCodesResponse>> {
    // Require authentication
    let validated = middleware::require_auth(State(ctx.clone()), headers).await?;

    // Get user's invite codes
    let codes = ctx
        .account_manager
        .list_invite_codes(&validated.did)
        .await?;

    // Build response with usage information
    let mut code_infos = Vec::new();
    for code in codes {
        // Get usage history for each code
        let uses_raw = ctx
            .account_manager
            .get_invite_code_usage(&code.code)
            .await?;
        let uses = uses_raw
            .into_iter()
            .map(|u| InviteCodeUse {
                used_by: u.used_by,
                used_at: u.used_at.to_rfc3339(),
            })
            .collect();

        code_infos.push(InviteCodeInfo {
            code: code.code,
            available: code.available_uses,
            disabled: code.disabled,
            for_account: code.created_for,
            created_at: code.created_at.to_rfc3339(),
            uses,
        });
    }

    Ok(Json(GetAccountInviteCodesResponse { codes: code_infos }))
}

/// Request for createInviteCode
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateInviteCodeRequest {
    use_count: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    for_account: Option<String>,
}

/// Response for createInviteCode
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateInviteCodeResponse {
    code: String,
}

/// Create invite code endpoint
///
/// Allows authenticated users to create invite codes (if they have allocation).
async fn create_invite_code(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(req): Json<CreateInviteCodeRequest>,
) -> PdsResult<Json<CreateInviteCodeResponse>> {
    // Require authentication
    let validated = middleware::require_auth(State(ctx.clone()), headers).await?;

    // Validate use count
    if req.use_count < 1 {
        return Err(PdsError::Validation(
            "Use count must be at least 1".to_string(),
        ));
    }

    if req.use_count > 10 {
        return Err(PdsError::Validation(
            "Use count cannot exceed 10".to_string(),
        ));
    }

    // Create invite code
    let code = ctx
        .account_manager
        .create_invite_code(&validated.did, req.use_count, req.for_account)
        .await?;

    Ok(Json(CreateInviteCodeResponse { code }))
}

/// Request for createInviteCodes (bulk creation)
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateInviteCodesRequest {
    code_count: i32,
    use_count: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    for_accounts: Option<Vec<String>>,
}

/// Response for createInviteCodes
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateInviteCodesResponse {
    codes: Vec<AccountInviteCode>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountInviteCode {
    code: String,
    available: i32,
    disabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    for_account: Option<String>,
    created_at: String,
    created_by: String,
}

/// Create invite codes endpoint (bulk)
///
/// Allows authenticated users (or admins) to create multiple invite codes at once.
async fn create_invite_codes(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(req): Json<CreateInviteCodesRequest>,
) -> PdsResult<Json<CreateInviteCodesResponse>> {
    // Require authentication
    let validated = middleware::require_auth(State(ctx.clone()), headers).await?;

    // Validate counts
    if req.code_count < 1 || req.code_count > 100 {
        return Err(PdsError::Validation(
            "Code count must be between 1 and 100".to_string(),
        ));
    }

    if req.use_count < 1 || req.use_count > 10 {
        return Err(PdsError::Validation(
            "Use count must be between 1 and 10".to_string(),
        ));
    }

    // Check if for_accounts matches code_count (if provided)
    if let Some(ref accounts) = req.for_accounts {
        if accounts.len() != req.code_count as usize {
            return Err(PdsError::Validation(
                "Number of for_accounts must match code_count".to_string(),
            ));
        }
    }

    // Create codes
    let mut codes = Vec::new();
    for i in 0..req.code_count {
        let for_account = req
            .for_accounts
            .as_ref()
            .and_then(|a| a.get(i as usize).cloned());
        let code = ctx
            .account_manager
            .create_invite_code(&validated.did, req.use_count, for_account.clone())
            .await?;

        let now = Utc::now();
        codes.push(AccountInviteCode {
            code,
            available: req.use_count,
            disabled: false,
            for_account,
            created_at: now.to_rfc3339(),
            created_by: validated.did.clone(),
        });
    }

    Ok(Json(CreateInviteCodesResponse { codes }))
}

/// Body for activate_account. All fields optional — empty body
/// triggers the spec-compliant JWT-only path; populated
/// {handle, password} triggers the Aurora recovery path
/// (chainlink #82: deactivation invalidates JWTs, so the
/// post-deactivation reactivation can't authenticate via JWT
/// and needs an unauth path with credentials).
#[derive(Debug, Default, serde::Deserialize)]
struct ActivateAccountRequest {
    #[serde(default)]
    handle: Option<String>,
    #[serde(default)]
    password: Option<String>,
}

/// Activate (reactivate) account endpoint.
///
/// Two auth paths (chainlink #82 — documented Aurora divergence from
/// atproto spec):
///
/// 1. **JWT path (spec)**: bearer token in `authorization` header.
///    Used when the caller still holds a valid pre-deactivation
///    session (rare — `deactivate_account` invalidates sessions).
///    Body may be empty or `{}`.
///
/// 2. **Credentials path (Aurora recovery)**: empty/missing
///    `authorization` header + body `{handle, password}`. Required
///    because `AccountManager::deactivate_account` deletes every
///    session+refresh_token row for the DID, and `login`
///    short-circuits on `deactivated_at != NULL`. Without this
///    path, deactivation would be a one-way trapdoor.
async fn activate_account(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    body: Option<Json<ActivateAccountRequest>>,
) -> PdsResult<Json<serde_json::Value>> {
    let body = body.map(|Json(b)| b).unwrap_or_default();

    // Resolve the DID: try JWT first; fall back to {handle, password}.
    let did =
        match middleware::require_auth(State(ctx.clone()), headers).await {
            Ok(validated) => validated.did,
            Err(jwt_err) => {
                let handle = body.handle.as_deref().ok_or_else(|| {
                    PdsError::Authentication(format!(
                        "JWT auth failed ({}) and no {{handle, password}} body \
                         supplied for recovery path",
                        jwt_err
                    ))
                })?;
                let password = body.password.as_deref().ok_or_else(|| {
                    PdsError::Validation(
                        "password required when activating via {handle, password} \
                         recovery path"
                            .to_string(),
                    )
                })?;
                let account = ctx
                    .account_manager
                    .get_account_by_identifier(handle)
                    .await?;
                let hash = account.password_hash.clone().ok_or_else(|| {
                    PdsError::Authorization(
                        "No local credentials for this handle".to_string(),
                    )
                })?;
                let valid = crate::auth::PasswordHasher::verify(password, &hash)
                    .map_err(|e| {
                        PdsError::Internal(format!(
                            "Password verification failed: {}",
                            e
                        ))
                    })?;
                if !valid {
                    return Err(PdsError::Authentication(
                        "Invalid handle or password".to_string(),
                    ));
                }
                account.did
            }
        };

    // Reactivate temporarily deactivated account.
    // If account is pending deletion (has delete_after set), use cancel_account_deletion instead.
    let account = ctx.account_manager.get_account(&did).await?;

    let message = if account.delete_after.is_some() {
        ctx.account_manager.cancel_account_deletion(&did).await?;
        "Account deletion cancelled and account reactivated"
    } else {
        ctx.account_manager.reactivate_account(&did).await?;
        "Account reactivated successfully"
    };

    // Arc 15 §8.3.5: three-emit reactivation — account → identity →
    // sync. Concurrent-write interleaving tolerated (round-1 F9
    // closure); `#sync` acts as resync surface.
    //
    // 1. Account (Pattern B — status from freshly-read row).
    let acc_post = ctx.account_manager.get_account(&did).await?;
    let (active, status) =
        crate::api::sync_helpers::get_account_status(&acc_post);
    ctx.sequencer
        .sequence_account(crate::sequencer::events::AccountEvent {
            did: did.clone(),
            active,
            status,
        })
        .await?;

    // 2. Identity — handle present (handle-change semantics is the
    // None-case per §8.3.7; reactivation always carries the handle).
    ctx.sequencer
        .sequence_identity(crate::sequencer::events::IdentityEvent {
            did: did.clone(),
            handle: acc_post.handle.clone(),
        })
        .await?;

    // 3. Sync — project current commit state via Sub-step 0.3(a)
    // helper. Best-effort: if the actor has no commit yet (edge
    // case), skip the sync emit rather than fail the reactivation.
    let repo_mgr = crate::actor_store::RepositoryManager::with_sequencer(
        did.clone(),
        ctx.actor_store.as_ref().clone(),
        ctx.sequencer.clone(),
    );
    match repo_mgr.current_sync_event_data().await {
        Ok(sync_data) => {
            let sync_evt = crate::sequencer::events::SyncEvent::from_sync_data(
                did.clone(),
                sync_data,
            )?;
            ctx.sequencer.sequence_sync(sync_evt).await?;
        }
        Err(e) => {
            tracing::warn!(
                did = %did,
                error = %e,
                "reactivate: no current sync data — skipping #sync emit",
            );
        }
    }

    Ok(Json(serde_json::json!({ "message": message })))
}

/// Body for deactivate_account per atproto lexicon
/// `com.atproto.server.deactivateAccount`: only optional
/// `deleteAfter` (datetime). JWT auth supplies the DID; no
/// password, no token in body (chainlink #83 spec-divergence fix).
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeactivateAccountRequest {
    #[serde(default)]
    delete_after: Option<String>,
}

/// Deactivate account endpoint per `com.atproto.server.deactivateAccount`.
///
/// JWT-auth + optional `deleteAfter` body field (chainlink #83 — the
/// pre-fix `{did, password, token}` body was off-spec). DID comes
/// from JWT; password isn't re-verified (JWT possession is the
/// identity proof). Reactivation path via `activateAccount` per
/// chainlink #82.
async fn deactivate_account(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    body: Option<Json<DeactivateAccountRequest>>,
) -> PdsResult<Json<serde_json::Value>> {
    // Require authentication.
    let validated = middleware::require_auth(State(ctx.clone()), headers).await?;
    let req = body.map(|Json(b)| b).unwrap_or_default();

    // `deleteAfter` is the lexicon-spec optional field. Aurora-Locus
    // doesn't schedule deferred deletion in v0.5 (the field exists in
    // the schema as `actor.delete_after` and is set via
    // `requestAccountDelete` / `delete_account` flows). If present
    // here, log + ignore — v0.6+ work to honor it from deactivate.
    if let Some(delete_after) = req.delete_after.as_deref() {
        tracing::info!(
            did = %validated.did,
            delete_after,
            "deactivate_account: deleteAfter requested but not yet honored in v0.5 \
             (deactivation only; use requestAccountDelete for scheduled deletion)"
        );
    }

    // Temporarily deactivate account (NO deletion scheduled).
    ctx.account_manager
        .deactivate_account(&validated.did)
        .await?;

    // Arc 15 §8.3.4: emit Deactivated #account event (Pattern B —
    // status derived from freshly-read post-mutation row).
    let acc_post = ctx.account_manager.get_account(&validated.did).await?;
    let (active, status) =
        crate::api::sync_helpers::get_account_status(&acc_post);
    debug_assert_eq!(
        status,
        Some(crate::sequencer::events::AccountStatus::Deactivated)
    );
    ctx.sequencer
        .sequence_account(crate::sequencer::events::AccountEvent {
            did: validated.did.clone(),
            active,
            status,
        })
        .await?;

    Ok(Json(serde_json::json!({
        "message": "Account temporarily deactivated. You can reactivate anytime by logging in or calling activateAccount."
    })))
}

/// Response for checkAccountStatus
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CheckAccountStatusResponse {
    activated: bool,
    valid_did: bool,
    repo_commit: Option<String>,
    repo_rev: Option<String>,
    repo_blocks: Option<i64>,
    indexed_records: Option<i64>,
    private_state_values: Option<i64>,
    expected_blobs: Option<i64>,
    imported_blobs: Option<i64>,
}

/// Check account status endpoint
///
/// Returns detailed status information about the account.
async fn check_account_status(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> PdsResult<Json<CheckAccountStatusResponse>> {
    // Require authentication
    let validated = middleware::require_auth(State(ctx.clone()), headers).await?;

    // Get account info
    let account = ctx.account_manager.get_account(&validated.did).await?;

    // Check if account is active (not deactivated or taken down)
    let activated = account.deactivated_at.is_none() && account.takedown_ref.is_none();

    // TODO: Get repo statistics from actor store
    // For now, return basic info
    Ok(Json(CheckAccountStatusResponse {
        activated,
        valid_did: true,
        repo_commit: None,
        repo_rev: None,
        repo_blocks: None,
        indexed_records: None,
        private_state_values: None,
        expected_blobs: None,
        imported_blobs: None,
    }))
}

/// Request for reserveSigningKey
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReserveSigningKeyRequest {
    /// The DID to reserve the signing key for (optional)
    /// If provided and a key was already reserved for this DID, returns the existing key's DID
    #[serde(default)]
    did: Option<String>,
}

/// Response for reserveSigningKey
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReserveSigningKeyResponse {
    /// The did:key identifier of the reserved signing key
    signing_key: String,
}

/// Reserve signing key endpoint
///
/// Reserves a secp256k1 signing keypair for account creation.
/// If a DID is provided and a key was already reserved for it, returns the existing key's did:key.
/// Otherwise, creates a new keypair and returns its did:key identifier.
///
/// This endpoint does not require authentication - it is called during account creation
/// before the account exists.
async fn reserve_signing_key(
    State(ctx): State<AppContext>,
    Json(req): Json<ReserveSigningKeyRequest>,
) -> PdsResult<Json<ReserveSigningKeyResponse>> {
    tracing::debug!(
        did = ?req.did,
        "reserve_signing_key: Reserving signing key"
    );

    // Validate DID format if provided
    if let Some(ref did) = req.did {
        if !did.starts_with("did:") {
            return Err(PdsError::Validation("Invalid DID format".to_string()));
        }
    }

    // Reserve or retrieve keypair
    let signing_key = ctx.actor_store.reserve_keypair(req.did.as_deref()).await?;

    tracing::info!(
        signing_key = %signing_key,
        did = ?req.did,
        "reserve_signing_key: Signing key reserved"
    );

    Ok(Json(ReserveSigningKeyResponse { signing_key }))
}

#[cfg(test)]
mod describe_server_federation_tests {
    use super::*;

    // #344 — the describeServer `federation` extension wire shape. When
    // federation is off, the block is `{enabled: false}` alone (flags omitted —
    // the off-posture is itself the signal a peer needs).
    #[test]
    fn federation_off_emits_enabled_false_only() {
        let v = serde_json::to_value(FederationDescribe {
            enabled: false,
            firehose_enabled: None,
            crawl_enabled: None,
        })
        .unwrap();
        assert_eq!(v["enabled"], false);
        assert_eq!(
            v.as_object().unwrap().len(),
            1,
            "off-posture is enabled:false alone: {v}"
        );
    }

    #[test]
    fn federation_on_emits_camelcase_flags() {
        let v = serde_json::to_value(FederationDescribe {
            enabled: true,
            firehose_enabled: Some(true),
            crawl_enabled: Some(false),
        })
        .unwrap();
        assert_eq!(v["enabled"], true);
        assert_eq!(v["firehoseEnabled"], true);
        assert_eq!(v["crawlEnabled"], false);
    }
}
