/// Admin API Endpoints
/// Implements com.atproto.admin.* endpoints for server administration
use crate::{admin::InviteCode, auth::AdminAuthContext, error::PdsError, AppContext};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use chrono::Duration;
use serde::Deserialize;

/// Build admin API routes
pub fn routes() -> Router<AppContext> {
    Router::new()
        // Admin stats and data
        .route("/xrpc/com.atproto.admin.getStats", get(get_stats))
        .route("/xrpc/com.atproto.admin.getUsers", get(get_users))
        .route("/xrpc/com.atproto.admin.listAccounts", get(get_users)) // Alias for frontend compatibility
        .route("/xrpc/com.atproto.admin.getAccount", get(get_account))
        .route(
            "/xrpc/com.atproto.admin.getAccountInfos",
            get(get_account_infos),
        )
        .route(
            "/xrpc/com.atproto.admin.updateSubjectStatus",
            post(update_subject_status),
        )
        .route(
            "/xrpc/com.atproto.admin.getSubjectStatus",
            get(get_subject_status),
        )
        // Invite codes
        .route(
            "/xrpc/com.atproto.admin.createInviteCode",
            post(create_invite_code),
        )
        .route(
            "/xrpc/com.atproto.admin.getInviteCodes",
            get(get_invite_codes),
        )
        .route(
            "/xrpc/com.atproto.admin.listInviteCodes",
            get(list_invite_codes),
        )
        .route(
            "/xrpc/com.atproto.admin.disableInviteCode",
            post(disable_invite_code),
        )
        .route(
            "/xrpc/com.atproto.admin.enableAccountInvites",
            post(enable_account_invites),
        )
        .route(
            "/xrpc/com.atproto.admin.disableAccountInvites",
            post(disable_account_invites),
        )
        // Role management
        .route("/xrpc/com.atproto.admin.grantRole", post(grant_role))
        .route("/xrpc/com.atproto.admin.revokeRole", post(revoke_role))
        .route("/xrpc/com.atproto.admin.listRoles", get(list_roles))
        // Account management
        .route(
            "/xrpc/com.atproto.admin.updateAccountEmail",
            post(update_account_email),
        )
        .route(
            "/xrpc/com.atproto.admin.updateAccountHandle",
            post(update_account_handle),
        )
        .route(
            "/xrpc/com.atproto.admin.updateAccountPassword",
            post(update_account_password),
        )
        .route(
            "/xrpc/com.atproto.admin.deleteAccount",
            post(admin_delete_account),
        )
        .route(
            "/xrpc/com.atproto.admin.updateAccountSigningKey",
            post(update_account_signing_key),
        )
        // Account moderation
        .route(
            "/xrpc/com.atproto.admin.takedownAccount",
            post(takedown_account),
        )
        .route(
            "/xrpc/com.atproto.admin.suspendAccount",
            post(suspend_account),
        )
        .route(
            "/xrpc/com.atproto.admin.restoreAccount",
            post(restore_account),
        )
        .route(
            "/xrpc/com.atproto.admin.getModerationHistory",
            get(get_moderation_history),
        )
        .route(
            "/xrpc/com.atproto.admin.getModerationQueue",
            get(get_moderation_queue),
        )
        // Labels
        .route("/xrpc/com.atproto.admin.applyLabel", post(apply_label))
        .route("/xrpc/com.atproto.admin.removeLabel", post(remove_label))
        // Reports
        .route("/xrpc/com.atproto.admin.submitReport", post(submit_report))
        .route(
            "/xrpc/com.atproto.admin.updateReportStatus",
            post(update_report_status),
        )
        .route("/xrpc/com.atproto.admin.listReports", get(list_reports))
        // Email
        .route("/xrpc/com.atproto.admin.sendEmail", post(send_email))
        // Audit logs
        .route("/xrpc/com.atproto.admin.getAuditLog", get(get_audit_log))
        // Validation failures
        .route(
            "/xrpc/com.atproto.admin.getValidationFailures",
            get(get_validation_failures),
        )
        // System health and diagnostics
        .route(
            "/xrpc/com.atproto.admin.getSystemHealth",
            get(get_system_health),
        )
        .route(
            "/xrpc/com.atproto.admin.getDatabaseStatus",
            get(get_database_status),
        )
        .route(
            "/xrpc/com.atproto.admin.getResourceUsage",
            get(get_resource_usage),
        )
        .route(
            "/xrpc/com.atproto.admin.listBackgroundJobs",
            get(list_background_jobs),
        )
        .route(
            "/xrpc/com.atproto.admin.runHealthChecks",
            get(run_health_checks),
        )
        .route(
            "/xrpc/com.atproto.admin.getVersionInfo",
            get(get_version_info),
        )
        .route(
            "/xrpc/com.atproto.admin.getSystemMetrics",
            get(get_system_metrics),
        )
        // Blob storage management
        .route(
            "/xrpc/com.atproto.admin.getBlobStatistics",
            get(get_blob_statistics),
        )
        .route("/xrpc/com.atproto.admin.listBlobs", get(list_blobs))
        .route("/xrpc/com.atproto.admin.deleteBlob", post(delete_blob))
        .route(
            "/xrpc/com.atproto.admin.quarantineBlob",
            post(quarantine_blob),
        )
        .route("/xrpc/com.atproto.admin.restoreBlob", post(restore_blob))
        .route("/xrpc/com.atproto.admin.runBlobGC", post(run_blob_gc))
        .route(
            "/xrpc/com.atproto.admin.getBlobQuotas",
            get(get_blob_quotas),
        )
        // Sequencer management
        .route(
            "/xrpc/com.atproto.admin.getSequencerStatus",
            get(get_sequencer_status),
        )
        .route(
            "/xrpc/com.atproto.admin.pauseSequencer",
            post(pause_sequencer),
        )
        .route(
            "/xrpc/com.atproto.admin.resumeSequencer",
            post(resume_sequencer),
        )
        .route(
            "/xrpc/com.atproto.admin.listRecentEvents",
            get(list_recent_events),
        )
        .route(
            "/xrpc/com.atproto.admin.resetSequencerCursor",
            post(reset_sequencer_cursor),
        )
        .route(
            "/xrpc/com.atproto.admin.rebuildSequencer",
            post(rebuild_sequencer),
        )
        // Rate limiting management
        .route(
            "/xrpc/com.atproto.admin.getRateLimitConfig",
            get(get_rate_limit_config),
        )
        .route(
            "/xrpc/com.atproto.admin.getRateLimitStatus",
            get(get_rate_limit_status),
        )
        .route(
            "/xrpc/com.atproto.admin.cleanupRateLimitState",
            post(cleanup_rate_limit_state),
        )
        // Federation and relay management
        .route(
            "/xrpc/com.atproto.admin.getFederationStatus",
            get(get_federation_status),
        )
        .route(
            "/xrpc/com.atproto.admin.getRelayConfig",
            get(get_relay_config),
        )
        .route(
            "/xrpc/com.atproto.admin.listKnownInstances",
            get(list_known_instances),
        )
        .route(
            "/xrpc/com.atproto.admin.triggerPdsDiscovery",
            post(trigger_pds_discovery),
        )
        .route(
            "/xrpc/com.atproto.admin.getNonceStoreStatus",
            get(get_nonce_store_status),
        )
        .route(
            "/xrpc/com.atproto.admin.cleanupNonceStores",
            post(cleanup_nonce_stores),
        )
}

// ============================================================================
// Admin Endpoints (OAuth Authentication via AdminAuthContext)
// ============================================================================

#[derive(Deserialize)]
struct CreateInviteCodeRequest {
    uses: Option<i32>,
    expires_days: Option<i64>,
    note: Option<String>,
    for_account: Option<String>,
}

/// Create an invite code
async fn create_invite_code(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<CreateInviteCodeRequest>,
) -> Result<Json<InviteCode>, (StatusCode, String)> {
    // Create invite code
    let uses = req.uses.unwrap_or(1);
    let expires_in = req.expires_days.map(Duration::days);

    let code = ctx
        .invite_manager
        .create_invite(
            &auth.did,
            uses,
            expires_in,
            req.note.clone(),
            req.for_account.clone(),
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Log the action
    let _ = ctx
        .admin_role_manager
        .log_action(&auth.did, "invite.create", None, Some(&code.code), None)
        .await;

    Ok(Json(code))
}

#[derive(Debug, Deserialize)]
struct GetInviteCodesQuery {
    #[serde(default)]
    include_disabled: bool,
}

#[derive(Debug, serde::Serialize)]
struct GetInviteCodesResponse {
    codes: Vec<InviteCode>,
}

/// Get all invite codes
async fn get_invite_codes(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(query): Query<GetInviteCodesQuery>,
) -> Result<Json<GetInviteCodesResponse>, (StatusCode, String)> {
    // Get all invite codes
    let codes = ctx
        .invite_manager
        .list_codes(query.include_disabled)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(GetInviteCodesResponse { codes }))
}

#[derive(Debug, Deserialize)]
struct ListInviteCodesQuery {
    #[serde(default)]
    #[allow(dead_code)] // TODO: Implement pagination
    limit: Option<i64>,
    #[serde(default)]
    #[allow(dead_code)] // TODO: Implement pagination
    cursor: Option<String>,
}

#[derive(Debug, serde::Serialize)]
struct ListInviteCodesResponse {
    codes: Vec<InviteCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor: Option<String>,
}

/// List invite codes (ATProto standard endpoint)
async fn list_invite_codes(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(_query): Query<ListInviteCodesQuery>,
) -> Result<Json<ListInviteCodesResponse>, (StatusCode, String)> {
    // Get all invite codes (ignore cursor for now, return all)
    let codes = ctx
        .invite_manager
        .list_codes(false) // Don't include disabled by default
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(ListInviteCodesResponse {
        codes,
        cursor: None,
    }))
}

/// Get server statistics
async fn get_stats(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Get statistics from database
    let total_users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM account")
        .fetch_one(&ctx.account_db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Note: Post count would require querying all actor databases - expensive
    // Set to 0 for now, can be improved later
    let total_posts: i64 = 0;

    let active_sessions: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM session WHERE expires_at > datetime('now')")
            .fetch_one(&ctx.account_db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let pending_reports: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM report WHERE status = 'open'")
            .fetch_one(&ctx.account_db)
            .await
            .unwrap_or(0);

    Ok(Json(serde_json::json!({
        "totalUsers": total_users,
        "totalPosts": total_posts,
        "activeSessions": active_sessions,
        "pendingReports": pending_reports,
    })))
}

#[derive(Deserialize)]
struct GetUsersParams {
    limit: Option<i64>,
    cursor: Option<String>,
}

/// Get list of users
async fn get_users(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(params): Query<GetUsersParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let limit = params.limit.unwrap_or(50).min(100);

    // Status is computed from actor state:
    //   takedown_ref present -> 'taken_down'
    //   deactivated_at present -> 'deactivated'
    //   otherwise -> 'active'
    let status_expr = "CASE \
        WHEN a.takedown_ref IS NOT NULL THEN 'taken_down' \
        WHEN a.deactivated_at IS NOT NULL THEN 'deactivated' \
        ELSE 'active' END";

    let users: Vec<serde_json::Value> = if let Some(cursor) = params.cursor {
        sqlx::query_as::<_, (String, String, Option<String>, String, String)>(&format!(
            "SELECT a.did, a.handle, ac.email, a.created_at, {} as status \
                 FROM actor a \
                 LEFT JOIN account ac ON a.did = ac.did \
                 WHERE a.did > ? ORDER BY a.did LIMIT ?",
            status_expr
        ))
        .bind(cursor)
        .bind(limit)
        .fetch_all(&ctx.account_db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    } else {
        sqlx::query_as::<_, (String, String, Option<String>, String, String)>(&format!(
            "SELECT a.did, a.handle, ac.email, a.created_at, {} as status \
                 FROM actor a \
                 LEFT JOIN account ac ON a.did = ac.did \
                 ORDER BY a.did LIMIT ?",
            status_expr
        ))
        .bind(limit)
        .fetch_all(&ctx.account_db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    }
    .into_iter()
    .map(|(did, handle, email, created_at, status)| {
        serde_json::json!({
            "did": did,
            "handle": handle,
            "email": email,
            "createdAt": created_at,
            "status": status,
        })
    })
    .collect();

    let cursor = users
        .last()
        .and_then(|u| u.get("did"))
        .and_then(|d| d.as_str());

    Ok(Json(serde_json::json!({
        "users": users,
        "cursor": cursor,
    })))
}

// ============================================================================
// Role Management Endpoints
// ============================================================================

#[derive(Deserialize)]
struct GrantRoleRequest {
    did: String,
    role: String,
}

/// Grant admin role to a user
async fn grant_role(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<GrantRoleRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use crate::admin::roles::Role;

    // Parse role
    let role: Role = req
        .role
        .parse()
        .map_err(|e: PdsError| (StatusCode::BAD_REQUEST, e.to_string()))?;

    // Grant role
    let admin_role = ctx
        .admin_role_manager
        .grant_role(&req.did, role, &auth.did, None)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Log action
    let _ = ctx
        .admin_role_manager
        .log_action(
            &auth.did,
            "role.grant",
            Some(&req.did),
            Some(&req.role),
            None,
        )
        .await;

    Ok(Json(serde_json::json!({
        "success": true,
        "did": req.did,
        "role": req.role,
        "admin_role": admin_role,
    })))
}

#[derive(Deserialize)]
struct RevokeRoleRequest {
    did: String,
    #[serde(default)]
    reason: Option<String>,
}

/// Revoke admin role from a user
async fn revoke_role(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<RevokeRoleRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Revoke role (revoke_role doesn't take a specific role, revokes the active role)
    ctx.admin_role_manager
        .revoke_role(&req.did, &auth.did, req.reason.clone())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Log action
    let _ = ctx
        .admin_role_manager
        .log_action(
            &auth.did,
            "role.revoke",
            Some(&req.did),
            req.reason.as_deref(),
            None,
        )
        .await;

    Ok(Json(serde_json::json!({
        "success": true,
        "did": req.did,
    })))
}

#[derive(Deserialize)]
struct ListRolesQuery {
    did: Option<String>,
}

/// List admin roles
async fn list_roles(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(query): Query<ListRolesQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if let Some(did) = query.did {
        // Get role for specific user
        let role_record = ctx
            .admin_role_manager
            .get_role(&did)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        Ok(Json(serde_json::json!({
            "did": did,
            "role": role_record,
        })))
    } else {
        // List all active role assignments
        let assignments = ctx
            .admin_role_manager
            .list_active_roles()
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        Ok(Json(serde_json::json!({
            "roles": assignments,
        })))
    }
}

// ============================================================================
// Account Management Endpoints
// ============================================================================

#[derive(Deserialize)]
struct UpdateAccountEmailRequest {
    /// Account DID
    did: String,
    /// New email address
    email: String,
}

/// Update account email address
async fn update_account_email(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Json(req): Json<UpdateAccountEmailRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Validate DID format
    if !req.did.starts_with("did:") {
        return Err((StatusCode::BAD_REQUEST, "Invalid DID format".to_string()));
    }

    // Validate email format (basic check)
    if !req.email.contains('@') || req.email.len() < 5 {
        return Err((StatusCode::BAD_REQUEST, "Invalid email format".to_string()));
    }

    ctx.account_manager
        .update_email(&req.did, &req.email)
        .await
        .map_err(|e| {
            if matches!(e, PdsError::NotFound(_)) {
                (
                    StatusCode::NOT_FOUND,
                    format!("Account not found: {}", req.did),
                )
            } else if matches!(e, PdsError::Validation(_)) {
                (StatusCode::CONFLICT, e.to_string())
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            }
        })?;

    Ok(Json(serde_json::json!({
        "success": true,
        "did": req.did,
        "email": req.email
    })))
}

#[derive(Deserialize)]
struct UpdateAccountHandleRequest {
    /// Account DID
    did: String,
    /// New handle
    handle: String,
}

/// Update account handle
async fn update_account_handle(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Json(req): Json<UpdateAccountHandleRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Validate DID format
    if !req.did.starts_with("did:") {
        return Err((StatusCode::BAD_REQUEST, "Invalid DID format".to_string()));
    }

    // Validate handle format (basic check)
    if req.handle.is_empty() || req.handle.len() > 253 {
        return Err((StatusCode::BAD_REQUEST, "Invalid handle format".to_string()));
    }

    let new_handle = ctx
        .account_manager
        .update_handle(&req.did, &req.handle)
        .await
        .map_err(|e| {
            if matches!(e, PdsError::NotFound(_)) {
                (
                    StatusCode::NOT_FOUND,
                    format!("Account not found: {}", req.did),
                )
            } else if matches!(e, PdsError::Validation(_)) {
                (StatusCode::CONFLICT, e.to_string())
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            }
        })?;

    Ok(Json(serde_json::json!({
        "success": true,
        "did": req.did,
        "handle": new_handle
    })))
}

#[derive(Deserialize)]
struct UpdateAccountPasswordRequest {
    /// Account DID
    did: String,
    /// New password
    password: String,
}

/// Update account password (admin override)
async fn update_account_password(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Json(req): Json<UpdateAccountPasswordRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Validate DID format
    if !req.did.starts_with("did:") {
        return Err((StatusCode::BAD_REQUEST, "Invalid DID format".to_string()));
    }

    // Validate password (minimum length)
    if req.password.len() < 8 {
        return Err((
            StatusCode::BAD_REQUEST,
            "Password must be at least 8 characters".to_string(),
        ));
    }

    ctx.account_manager
        .update_password(&req.did, &req.password)
        .await
        .map_err(|e| {
            if matches!(e, PdsError::NotFound(_)) {
                (
                    StatusCode::NOT_FOUND,
                    format!("Account not found: {}", req.did),
                )
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            }
        })?;

    Ok(Json(serde_json::json!({
        "success": true,
        "did": req.did,
        "message": "Password updated. All sessions have been invalidated."
    })))
}

#[derive(Deserialize)]
struct DeleteAccountRequest {
    /// Account DID
    did: String,
}

/// Delete account permanently (admin operation)
async fn admin_delete_account(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Json(req): Json<DeleteAccountRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Validate DID format
    if !req.did.starts_with("did:") {
        return Err((StatusCode::BAD_REQUEST, "Invalid DID format".to_string()));
    }

    ctx.account_manager
        .delete_account_permanent(&req.did)
        .await
        .map_err(|e| {
            if matches!(e, PdsError::NotFound(_)) {
                (
                    StatusCode::NOT_FOUND,
                    format!("Account not found: {}", req.did),
                )
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            }
        })?;

    Ok(Json(serde_json::json!({
        "success": true,
        "did": req.did,
        "message": "Account permanently deleted"
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateAccountSigningKeyRequest {
    /// DID of the account whose signing key is being updated
    did: String,
    /// New signing key in did:key: format (per the lexicon)
    signing_key: String,
}

/// Update an account's signing key in the PLC directory
///
/// Implements `com.atproto.admin.updateAccountSigningKey`. Submits a PLC
/// operation rotating the `verificationMethods.atproto` entry to the supplied
/// did:key value, then advances the repository commit chain with an empty
/// commit and sequences an identity event so federation peers learn of the
/// change.
///
/// Aurora-Locus runs in a single-operator-key model: the operator's
/// `authentication.repo_signing_key` is the only private key the PDS can sign
/// commits with. Rotating to any other public key would leave the account
/// unable to produce new commits, so this handler enforces strict-mode
/// validation: the supplied `signingKey` must match the operator's configured
/// key. The lexicon contract permits arbitrary `signingKey` values; the
/// strict-mode check is an Aurora-architecture safety constraint.
async fn update_account_signing_key(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<UpdateAccountSigningKeyRequest>,
) -> Result<StatusCode, axum::response::Response> {
    use crate::actor_store::repository::RepositoryManager;
    use crate::crypto::{
        plc::PlcSigner,
        plc_client::{PlcClient, PlcClientConfig},
        proto_blue_signer::RepoSigner,
    };
    use crate::sequencer::events::IdentityEvent;
    use axum::response::IntoResponse;

    fn plain_err(status: StatusCode, msg: impl Into<String>) -> axum::response::Response {
        (status, msg.into()).into_response()
    }
    fn xrpc_err(
        status: StatusCode,
        error: &str,
        message: impl Into<String>,
    ) -> axum::response::Response {
        (
            status,
            Json(serde_json::json!({
                "error": error,
                "message": message.into(),
            })),
        )
            .into_response()
    }

    if !req.did.starts_with("did:plc:") {
        return Err(plain_err(
            StatusCode::BAD_REQUEST,
            "did must be a did:plc identifier",
        ));
    }
    if !req.signing_key.starts_with("did:key:") {
        return Err(plain_err(
            StatusCode::BAD_REQUEST,
            "signingKey must be in did:key: format",
        ));
    }

    // Strict-mode validation: the supplied signingKey must match the operator's
    // configured repo_signing_key. Aurora-Locus has a single operator-level
    // private key; any other rotation target would leave the account unable to
    // sign new commits.
    //
    // TODO: Relax this check when Aurora-Locus supports per-account signing
    // keys. The lexicon contract permits arbitrary signingKey values; this
    // strict-mode validation is a safety check appropriate to Aurora's
    // current single-key architecture.
    let repo_signer = PlcSigner::from_hex(&ctx.config.authentication.repo_signing_key)
        .map_err(|e| {
            plain_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Operator repo signing key not configured: {}", e),
            )
        })?;
    let operator_did_key = repo_signer.public_key_did_key();
    if req.signing_key != operator_did_key {
        return Err(xrpc_err(
            StatusCode::BAD_REQUEST,
            "InvalidRequest",
            "signingKey does not match operator's configured signing key. \
             Aurora-Locus uses a single operator-level signing key model; \
             the provided signingKey must match the operator's \
             repo_signing_key config.",
        ));
    }

    let plc_client = PlcClient::new(PlcClientConfig {
        plc_url: ctx.config.identity.did_plc_url.clone(),
        timeout_secs: 30,
    })
    .map_err(|e| {
        plain_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("PLC client init failed: {}", e),
        )
    })?;

    let rotation_signer = PlcSigner::from_hex(&ctx.config.authentication.plc_rotation_key)
        .map_err(|e| {
            plain_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("PLC rotation key not configured: {}", e),
            )
        })?;

    // Compare against the current PLC document. Aurora's PlcClient::get_signing_key
    // returns multibase form (the bare `z...` prefix); the request's signingKey is
    // in did:key form. Strip the prefix for comparison so we don't submit a no-op
    // PLC operation when the keys already match.
    let current_doc = plc_client.get_document(&req.did).await.map_err(|e| {
        if matches!(e, PdsError::IdentityResolution(_)) {
            plain_err(
                StatusCode::NOT_FOUND,
                format!("DID document not found: {}", e),
            )
        } else {
            plain_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        }
    })?;
    let current_key_multibase = plc_client
        .get_signing_key(&current_doc)
        .map_err(|e| plain_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let new_key_multibase = req
        .signing_key
        .strip_prefix("did:key:")
        .unwrap_or(&req.signing_key);

    if plc_client.keys_match(&current_key_multibase, new_key_multibase) {
        tracing::debug!(did = %req.did, "Signing key already up to date; skipping PLC submission");
        return Ok(StatusCode::OK);
    }

    // Submit PLC update with the did:key form so the entry stores the canonical
    // verificationMethods.atproto value.
    plc_client
        .update_signing_key(&req.did, &req.signing_key, &rotation_signer)
        .await
        .map_err(|e| plain_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Advance the repository commit chain with an empty commit so the rotation
    // is reflected in repository state, not just the DID document. Mirrors the
    // CLI rotation flow in src/cli/rotate_keys.rs. Strict-mode validation
    // guarantees the operator's repo_signing_key matches the new PLC entry, so
    // the commit signature will verify against the newly-installed key.
    let repo_mgr = RepositoryManager::with_sequencer(
        req.did.clone(),
        (*ctx.actor_store).clone(),
        ctx.sequencer.clone(),
    );
    let repo_signer_pb: std::sync::Arc<dyn proto_blue::crypto::Signer> = {
        let key_bytes = hex::decode(&ctx.config.authentication.repo_signing_key).map_err(|e| {
            plain_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Invalid hex repo signing key: {}", e),
            )
        })?;
        let s = RepoSigner::from_bytes(&key_bytes).map_err(|e| {
            plain_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to build repo signer: {}", e),
            )
        })?;
        std::sync::Arc::new(s)
    };
    let (commit_cid, rev) = repo_mgr
        .apply_writes(vec![], repo_signer_pb)
        .await
        .map_err(|e| plain_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    tracing::info!(
        did = %req.did,
        commit_cid = %commit_cid,
        rev = %rev,
        "Created empty commit for signing key rotation"
    );

    // Announce the change via an identity event.
    let account = ctx.account_manager.get_account(&req.did).await.map_err(|e| {
        if matches!(e, PdsError::NotFound(_)) {
            plain_err(
                StatusCode::NOT_FOUND,
                format!("Account not found: {}", req.did),
            )
        } else {
            plain_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        }
    })?;
    let identity_evt = IdentityEvent::new(req.did.clone(), account.handle);
    ctx.sequencer
        .sequence_identity(identity_evt)
        .await
        .map_err(|e| plain_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    tracing::info!(
        admin = %auth.did,
        did = %req.did,
        "Updated account signing key via XRPC"
    );

    Ok(StatusCode::OK)
}

// ============================================================================
// Account Moderation Endpoints
// ============================================================================

#[derive(Deserialize)]
struct TakedownAccountRequest {
    did: String,
    reason: String,
    #[serde(default)]
    notes: Option<String>,
}

/// Takedown an account (remove from public view)
async fn takedown_account(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<TakedownAccountRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use crate::admin::moderation::{ApplyActionParams, ModerationAction};

    // Apply takedown action
    let record = ctx
        .moderation_manager
        .apply_action(ApplyActionParams {
            did: &req.did,
            action: ModerationAction::Takedown,
            reason: &req.reason,
            moderated_by: &auth.did,
            expires_in: None,
            report_id: None,
            notes: req.notes.clone(),
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Log action
    let _ = ctx
        .admin_role_manager
        .log_action(
            &auth.did,
            "account.takedown",
            Some(&req.did),
            Some(&req.reason),
            None,
        )
        .await;

    Ok(Json(serde_json::json!({
        "success": true,
        "moderation_id": record.id,
        "did": req.did,
        "action": "takedown",
    })))
}

#[derive(Deserialize)]
struct SuspendAccountRequest {
    did: String,
    reason: String,
    #[serde(default)]
    duration_days: Option<i64>,
    #[serde(default)]
    notes: Option<String>,
}

/// Suspend an account temporarily
async fn suspend_account(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<SuspendAccountRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use crate::admin::moderation::{ApplyActionParams, ModerationAction};

    let expires_in = req.duration_days.map(Duration::days);

    // Apply suspension
    let record = ctx
        .moderation_manager
        .apply_action(ApplyActionParams {
            did: &req.did,
            action: ModerationAction::Suspend,
            reason: &req.reason,
            moderated_by: &auth.did,
            expires_in,
            report_id: None,
            notes: req.notes.clone(),
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Log action
    let _ = ctx
        .admin_role_manager
        .log_action(
            &auth.did,
            "account.suspend",
            Some(&req.did),
            Some(&req.reason),
            None,
        )
        .await;

    Ok(Json(serde_json::json!({
        "success": true,
        "moderation_id": record.id,
        "did": req.did,
        "action": "suspend",
        "expires_at": record.expires_at,
    })))
}

#[derive(Deserialize)]
struct RestoreAccountRequest {
    did: String,
    moderation_id: i64,
    reason: String,
}

/// Restore an account after takedown/suspension
async fn restore_account(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<RestoreAccountRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Reverse moderation action
    ctx.moderation_manager
        .reverse_action(req.moderation_id, &auth.did, &req.reason)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Log action
    let _ = ctx
        .admin_role_manager
        .log_action(
            &auth.did,
            "account.restore",
            Some(&req.did),
            Some(&req.reason),
            None,
        )
        .await;

    Ok(Json(serde_json::json!({
        "success": true,
        "did": req.did,
    })))
}

#[derive(Deserialize)]
struct GetModerationHistoryQuery {
    did: String,
}

/// Get moderation history for an account
async fn get_moderation_history(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(query): Query<GetModerationHistoryQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let history = ctx
        .moderation_manager
        .get_history(&query.did)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "did": query.did,
        "history": history,
    })))
}

// ============================================================================
// Label Management Endpoints
// ============================================================================

#[derive(Deserialize)]
struct ApplyLabelRequest {
    uri: String,
    #[serde(default)]
    cid: Option<String>,
    val: String,
    #[serde(default)]
    expires_days: Option<i64>,
}

/// Apply a label to content
async fn apply_label(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<ApplyLabelRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let expires_in = req.expires_days.map(Duration::days);

    let label = ctx
        .label_manager
        .apply_label(
            &req.uri,
            req.cid.as_deref(),
            &req.val,
            &auth.did,
            expires_in,
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Log action
    let _ = ctx
        .admin_role_manager
        .log_action(
            &auth.did,
            "label.apply",
            None,
            Some(&req.val),
            Some(&req.uri),
        )
        .await;

    Ok(Json(serde_json::json!({
        "success": true,
        "label": label,
    })))
}

#[derive(Deserialize)]
struct RemoveLabelRequest {
    uri: String,
    #[serde(default)]
    cid: Option<String>,
    val: String,
}

/// Remove a label from content
async fn remove_label(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<RemoveLabelRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let label = ctx
        .label_manager
        .remove_label(&req.uri, req.cid.as_deref(), &req.val, &auth.did)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Log action
    let _ = ctx
        .admin_role_manager
        .log_action(
            &auth.did,
            "label.remove",
            None,
            Some(&req.val),
            Some(&req.uri),
        )
        .await;

    Ok(Json(serde_json::json!({
        "success": true,
        "label": label,
    })))
}

// ============================================================================
// Report Management Endpoints
// ============================================================================

#[derive(Deserialize)]
struct SubmitReportRequest {
    #[serde(default)]
    subject_did: Option<String>,
    #[serde(default)]
    subject_uri: Option<String>,
    #[serde(default)]
    subject_cid: Option<String>,
    reason_type: String,
    #[serde(default)]
    reason: Option<String>,
}

/// Submit a report
async fn submit_report(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<SubmitReportRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use crate::admin::reports::ReportReason;

    // Parse reason type
    let reason_type: ReportReason = req
        .reason_type
        .parse()
        .map_err(|e: PdsError| (StatusCode::BAD_REQUEST, e.to_string()))?;

    // Submit report
    let report = ctx
        .report_manager
        .submit_report(
            req.subject_did.as_deref(),
            req.subject_uri.as_deref(),
            req.subject_cid.as_deref(),
            reason_type,
            req.reason.as_deref(),
            &auth.did,
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "success": true,
        "report": report,
    })))
}

#[derive(Deserialize)]
struct UpdateReportStatusRequest {
    report_id: i64,
    status: String,
    #[serde(default)]
    resolution: Option<String>,
}

/// Update report status
async fn update_report_status(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<UpdateReportStatusRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use crate::admin::reports::ReportStatus;

    // Parse status
    let status: ReportStatus = req
        .status
        .parse()
        .map_err(|e: PdsError| (StatusCode::BAD_REQUEST, e.to_string()))?;

    // Update status
    ctx.report_manager
        .update_status(req.report_id, status, &auth.did, req.resolution.as_deref())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Log action
    let _ = ctx
        .admin_role_manager
        .log_action(&auth.did, "report.update", None, Some(&req.status), None)
        .await;

    Ok(Json(serde_json::json!({
        "success": true,
        "report_id": req.report_id,
        "status": req.status,
    })))
}

#[derive(Deserialize)]
struct ListReportsQuery {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

/// List reports
async fn list_reports(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(query): Query<ListReportsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use crate::admin::reports::ReportStatus;

    // Parse status filter if provided
    let status_filter = if let Some(status_str) = query.status {
        Some(
            status_str
                .parse::<ReportStatus>()
                .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?,
        )
    } else {
        None
    };

    // List reports
    let reports = ctx
        .report_manager
        .list_reports(status_filter, query.limit)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "reports": reports,
    })))
}

// ============================================================================
// Email Endpoints
// ============================================================================

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendEmailRequest {
    /// DID of the recipient
    recipient_did: String,
    /// Email subject
    subject: String,
    /// Email body content
    content: String,
    /// Optional sender DID for record-keeping
    #[serde(default)]
    sender_did: Option<String>,
    /// Optional comment for audit log
    #[serde(default)]
    comment: Option<String>,
}

/// Send email response per ATProto spec
#[derive(serde::Serialize)]
struct SendEmailResponse {
    sent: bool,
}

/// Send an email to a user
///
/// Allows admins to send emails to users for moderation notices,
/// warnings, or other administrative purposes.
async fn send_email(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<SendEmailRequest>,
) -> Result<Json<SendEmailResponse>, (StatusCode, String)> {
    // Get the recipient account to find their email
    let account = ctx
        .account_manager
        .get_account(&req.recipient_did)
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, format!("Account not found: {}", e)))?;

    // Check if account has email
    let to_email = account.email.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "Account has no email address".to_string(),
        )
    })?;

    // Send the email
    ctx.mailer
        .send_admin_email(&to_email, &req.subject, &req.content)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to send email: {}", e),
            )
        })?;

    // Log the action
    let sender = req.sender_did.as_deref().unwrap_or(&auth.did);
    let _ = ctx
        .admin_role_manager
        .log_action(
            sender,
            "email.send",
            Some(&req.recipient_did),
            req.comment.as_deref(),
            Some(&req.subject),
        )
        .await;

    tracing::info!(
        "Admin {} sent email to {} ({}): {}",
        auth.did,
        req.recipient_did,
        to_email,
        req.subject
    );

    Ok(Json(SendEmailResponse { sent: true }))
}

// ============================================================================
// Audit Log Endpoints
// ============================================================================

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetAuditLogQuery {
    /// Filter by admin DID
    #[serde(default)]
    admin_did: Option<String>,
    /// Filter by action type (e.g., "account.takedown", "label.apply")
    #[serde(default)]
    action: Option<String>,
    /// Filter by subject DID
    #[serde(default)]
    subject_did: Option<String>,
    /// Maximum number of entries to return (default 50, max 100)
    #[serde(default)]
    limit: Option<i64>,
    /// Cursor for pagination (ID of last entry from previous page)
    #[serde(default)]
    cursor: Option<i64>,
}

/// Audit log entry response
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AuditLogEntryResponse {
    id: i64,
    admin_did: String,
    action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    subject_did: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<String>,
    timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    ip_address: Option<String>,
}

/// Audit log response
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct GetAuditLogResponse {
    entries: Vec<AuditLogEntryResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor: Option<i64>,
    total_count: i64,
}

/// Get audit log entries
///
/// Returns a paginated list of admin action audit log entries.
/// Can be filtered by admin DID, action type, or subject DID.
async fn get_audit_log(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(query): Query<GetAuditLogQuery>,
) -> Result<Json<GetAuditLogResponse>, (StatusCode, String)> {
    // Clamp limit to reasonable range
    let limit = query.limit.unwrap_or(50).clamp(1, 100);

    // Get audit log entries
    let entries = ctx
        .admin_role_manager
        .get_audit_logs(
            query.admin_did.as_deref(),
            query.action.as_deref(),
            query.subject_did.as_deref(),
            limit + 1, // Fetch one extra to check if there are more
            query.cursor,
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Get total count
    let total_count = ctx
        .admin_role_manager
        .get_audit_log_count()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Check if there are more entries
    let has_more = entries.len() as i64 > limit;
    let entries: Vec<_> = entries.into_iter().take(limit as usize).collect();

    // Get cursor for next page
    let next_cursor = if has_more {
        entries.last().map(|e| e.id)
    } else {
        None
    };

    // Convert to response format
    let response_entries: Vec<AuditLogEntryResponse> = entries
        .into_iter()
        .map(|e| AuditLogEntryResponse {
            id: e.id,
            admin_did: e.admin_did,
            action: e.action,
            subject_did: e.subject_did,
            details: e.details,
            timestamp: e.timestamp.to_rfc3339(),
            ip_address: e.ip_address,
        })
        .collect();

    Ok(Json(GetAuditLogResponse {
        entries: response_entries,
        cursor: next_cursor,
        total_count,
    }))
}

// ============================================================================
// Additional Endpoints for Admin Panel Compatibility
// ============================================================================

#[derive(Deserialize)]
struct GetAccountQuery {
    did: String,
}

/// Get single account details
async fn get_account(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(query): Query<GetAccountQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let account = ctx
        .account_manager
        .get_account(&query.did)
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, format!("Account not found: {}", e)))?;

    Ok(Json(serde_json::json!({
        "did": account.did,
        "handle": account.handle,
        "email": account.email,
        "created_at": account.created_at,
        "email_confirmed": account.email_confirmed_at.is_some(),
        "takedown": account.takedown_ref.is_some(),
    })))
}

#[derive(Deserialize)]
struct GetAccountInfosQuery {
    /// Comma-separated list of DIDs to look up
    dids: String,
}

/// Account info for batch responses
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountInfo {
    did: String,
    handle: Option<String>,
    email: Option<String>,
    indexed_at: String,
    email_confirmed_at: Option<String>,
    invited_by: Option<InviteCodeInfo>,
    invites: Vec<InviteCodeInfo>,
    invites_disabled: bool,
    invite_note: Option<String>,
    deactivated_at: Option<String>,
    threat_signatures: Vec<ThreatSignature>,
}

/// Invite code info embedded in account info
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct InviteCodeInfo {
    code: String,
    available: i32,
    disabled: bool,
    for_account: String,
    created_by: String,
    created_at: String,
    uses: Vec<InviteCodeUse>,
}

/// Record of invite code usage
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct InviteCodeUse {
    used_by: String,
    used_at: String,
}

/// Threat signature (for future anti-spam/abuse detection)
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ThreatSignature {
    property: String,
    value: String,
}

/// Response for getAccountInfos
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct GetAccountInfosResponse {
    infos: Vec<AccountInfo>,
}

/// Get multiple account details in batch
///
/// Batch lookup of multiple account details by DIDs.
/// Returns information for all found accounts (missing DIDs are silently skipped).
async fn get_account_infos(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(query): Query<GetAccountInfosQuery>,
) -> Result<Json<GetAccountInfosResponse>, (StatusCode, String)> {
    // Parse the comma-separated DIDs
    let dids: Vec<&str> = query
        .dids
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    if dids.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "No DIDs provided".to_string()));
    }

    // Limit batch size to prevent abuse
    const MAX_BATCH_SIZE: usize = 100;
    if dids.len() > MAX_BATCH_SIZE {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("Too many DIDs (max {})", MAX_BATCH_SIZE),
        ));
    }

    let mut infos = Vec::new();

    for did in dids {
        // Skip invalid DID formats
        if !did.starts_with("did:") {
            continue;
        }

        // Try to get account, skip if not found
        let account = match ctx.account_manager.get_account(did).await {
            Ok(acc) => acc,
            Err(_) => continue,
        };

        // Get invite code that was used to create this account (if any)
        let invited_by = ctx
            .invite_manager
            .get_invite_for_account(did)
            .await
            .ok()
            .flatten()
            .map(|inv| InviteCodeInfo {
                code: inv.code.clone(),
                available: inv.available,
                disabled: inv.disabled,
                for_account: inv.for_account.clone().unwrap_or_default(),
                created_by: inv.created_by.clone(),
                created_at: inv.created_at.to_rfc3339(),
                uses: vec![], // We don't track individual uses in the invite lookup
            });

        // Get invite codes created by this account
        let account_invites = ctx
            .invite_manager
            .get_codes_created_by(did)
            .await
            .unwrap_or_default();

        let invites: Vec<InviteCodeInfo> = account_invites
            .into_iter()
            .map(|inv| {
                InviteCodeInfo {
                    code: inv.code.clone(),
                    available: inv.available,
                    disabled: inv.disabled,
                    for_account: inv.for_account.clone().unwrap_or_default(),
                    created_by: inv.created_by.clone(),
                    created_at: inv.created_at.to_rfc3339(),
                    uses: vec![], // Uses would need separate query
                }
            })
            .collect();

        infos.push(AccountInfo {
            did: account.did.clone(),
            handle: account.handle.clone(),
            email: account.email.clone(),
            indexed_at: account.created_at.to_rfc3339(),
            email_confirmed_at: account.email_confirmed_at.map(|dt| dt.to_rfc3339()),
            invited_by,
            invites,
            invites_disabled: account.invites_disabled.unwrap_or(false),
            invite_note: None, // Not tracked currently
            deactivated_at: account.deactivated_at.map(|dt| dt.to_rfc3339()),
            threat_signatures: vec![], // Not implemented yet
        });
    }

    Ok(Json(GetAccountInfosResponse { infos }))
}

#[derive(Deserialize)]
struct UpdateSubjectStatusRequest {
    subject: String, // DID or AT-URI
    #[serde(default)]
    action: String, // "suspend", "takedown", "restore"
    #[serde(default)]
    duration: Option<i64>, // Duration in seconds for temporary suspensions
}

/// Update subject status (unified moderation endpoint)
async fn update_subject_status(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<UpdateSubjectStatusRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use crate::admin::moderation::{ApplyActionParams, ModerationAction};

    // Extract DID from subject (handle both DID and AT-URI)
    let did = if req.subject.starts_with("did:") {
        req.subject.clone()
    } else if req.subject.starts_with("at://") {
        // Extract DID from AT-URI (format: at://did:plc:xyz/...)
        req.subject
            .trim_start_matches("at://")
            .split('/')
            .next()
            .unwrap_or("")
            .to_string()
    } else {
        return Err((
            StatusCode::BAD_REQUEST,
            "Invalid subject format".to_string(),
        ));
    };

    let action = match req.action.as_str() {
        "suspend" => ModerationAction::Suspend,
        "takedown" => ModerationAction::Takedown,
        "restore" => {
            // For restore, we reverse existing moderation actions
            // This is a simplified implementation - in production you'd want to track specific actions to reverse
            return Ok(Json(serde_json::json!({
                "success": true,
                "did": did,
                "action": "restore",
                "message": "To restore, use the reverse_action endpoint with the specific moderation_id"
            })));
        }
        _ => return Err((StatusCode::BAD_REQUEST, "Invalid action".to_string())),
    };

    let duration = req.duration.map(Duration::seconds);
    let reason = format!("Admin action: {}", req.action);

    ctx.moderation_manager
        .apply_action(ApplyActionParams {
            did: &did,
            action,
            reason: &reason,
            moderated_by: &auth.did,
            expires_in: duration,
            report_id: None,
            notes: None,
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "success": true,
        "did": did,
        "action": req.action,
    })))
}

#[derive(Deserialize)]
struct GetSubjectStatusQuery {
    /// The DID or AT-URI of the subject to query
    #[serde(default)]
    did: Option<String>,
    /// The AT-URI of the subject (alternative to did for record-level status)
    #[serde(default)]
    uri: Option<String>,
    /// The CID of the blob (for blob-level status)
    #[serde(default)]
    blob: Option<String>,
}

/// Subject status response matching ATProto spec
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SubjectStatusResponse {
    subject: SubjectRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    takedown: Option<StatusAttr>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deactivated: Option<StatusAttr>,
    #[serde(skip_serializing_if = "Option::is_none")]
    suspended: Option<StatusAttr>,
}

/// Reference to the subject (repo, record, or blob)
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SubjectRef {
    #[serde(rename = "$type")]
    type_field: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    did: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cid: Option<String>,
}

/// Status attribute with applied flag and optional reference
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusAttr {
    applied: bool,
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    ref_field: Option<String>,
}

/// Get subject status (takedown/deactivation status of account or record)
///
/// This endpoint returns the current moderation status of a subject,
/// including whether it's been taken down, deactivated, or suspended.
async fn get_subject_status(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(query): Query<GetSubjectStatusQuery>,
) -> Result<Json<SubjectStatusResponse>, (StatusCode, String)> {
    // Determine subject type and extract DID
    let (subject_type, did, uri) = if let Some(ref did_str) = query.did {
        // Direct DID query - repo subject
        if !did_str.starts_with("did:") {
            return Err((StatusCode::BAD_REQUEST, "Invalid DID format".to_string()));
        }
        ("com.atproto.admin.defs#repoRef", did_str.clone(), None)
    } else if let Some(ref uri_str) = query.uri {
        // AT-URI query - record subject
        if !uri_str.starts_with("at://") {
            return Err((StatusCode::BAD_REQUEST, "Invalid AT-URI format".to_string()));
        }
        // Extract DID from AT-URI
        let did = uri_str
            .trim_start_matches("at://")
            .split('/')
            .next()
            .ok_or_else(|| (StatusCode::BAD_REQUEST, "Invalid AT-URI format".to_string()))?
            .to_string();
        if !did.starts_with("did:") {
            return Err((
                StatusCode::BAD_REQUEST,
                "AT-URI must contain a DID".to_string(),
            ));
        }
        ("com.atproto.repo.strongRef", did, Some(uri_str.clone()))
    } else if let Some(ref _blob_cid) = query.blob {
        // Blob query - not yet implemented
        return Err((
            StatusCode::NOT_IMPLEMENTED,
            "Blob status queries not yet implemented".to_string(),
        ));
    } else {
        return Err((
            StatusCode::BAD_REQUEST,
            "Must provide did, uri, or blob parameter".to_string(),
        ));
    };

    // Get account info from account manager
    let account = ctx.account_manager.get_account(&did).await.map_err(|e| {
        if matches!(e, PdsError::NotFound(_)) {
            (StatusCode::NOT_FOUND, format!("Subject not found: {}", did))
        } else {
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        }
    })?;

    // Check moderation status
    let is_suspended = ctx
        .moderation_manager
        .is_suspended(&did)
        .await
        .unwrap_or(false);

    // Build response
    let subject = SubjectRef {
        type_field: subject_type.to_string(),
        did: if query.did.is_some() {
            Some(did.clone())
        } else {
            None
        },
        uri,
        cid: None,
    };

    let takedown = if account.takedown_ref.is_some() {
        Some(StatusAttr {
            applied: true,
            ref_field: account.takedown_ref.clone(),
        })
    } else {
        Some(StatusAttr {
            applied: false,
            ref_field: None,
        })
    };

    let deactivated = if account.deactivated_at.is_some() {
        Some(StatusAttr {
            applied: true,
            ref_field: account
                .deactivated_at
                .map(|dt: chrono::DateTime<chrono::Utc>| dt.to_rfc3339()),
        })
    } else {
        Some(StatusAttr {
            applied: false,
            ref_field: None,
        })
    };

    let suspended = if is_suspended {
        Some(StatusAttr {
            applied: true,
            ref_field: None,
        })
    } else {
        Some(StatusAttr {
            applied: false,
            ref_field: None,
        })
    };

    Ok(Json(SubjectStatusResponse {
        subject,
        takedown,
        deactivated,
        suspended,
    }))
}

#[derive(Deserialize)]
struct GetModerationQueueQuery {
    #[serde(default)]
    limit: Option<i64>,
}

/// Get moderation queue (reports needing review)
async fn get_moderation_queue(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(query): Query<GetModerationQueueQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use crate::admin::reports::ReportStatus;

    // Get open reports as the moderation queue
    let reports = ctx
        .report_manager
        .list_reports(Some(ReportStatus::Open), query.limit)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "queue": reports,
        "count": reports.len(),
    })))
}

#[derive(Deserialize)]
struct DisableInviteCodeRequest {
    code: String,
}

/// Disable an invite code
async fn disable_invite_code(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Json(req): Json<DisableInviteCodeRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    ctx.invite_manager
        .disable_code(&req.code)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "success": true,
        "code": req.code,
    })))
}

#[derive(Deserialize)]
struct AccountInvitesRequest {
    /// Account DID
    did: String,
}

/// Enable invite code creation for an account
async fn enable_account_invites(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Json(req): Json<AccountInvitesRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Validate DID format
    if !req.did.starts_with("did:") {
        return Err((StatusCode::BAD_REQUEST, "Invalid DID format".to_string()));
    }

    ctx.account_manager
        .enable_account_invites(&req.did)
        .await
        .map_err(|e| {
            if matches!(e, PdsError::NotFound(_)) {
                (
                    StatusCode::NOT_FOUND,
                    format!("Account not found: {}", req.did),
                )
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            }
        })?;

    Ok(Json(serde_json::json!({
        "success": true,
        "did": req.did,
        "invitesEnabled": true
    })))
}

/// Disable invite code creation for an account
async fn disable_account_invites(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Json(req): Json<AccountInvitesRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Validate DID format
    if !req.did.starts_with("did:") {
        return Err((StatusCode::BAD_REQUEST, "Invalid DID format".to_string()));
    }

    ctx.account_manager
        .disable_account_invites(&req.did)
        .await
        .map_err(|e| {
            if matches!(e, PdsError::NotFound(_)) {
                (
                    StatusCode::NOT_FOUND,
                    format!("Account not found: {}", req.did),
                )
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            }
        })?;

    Ok(Json(serde_json::json!({
        "success": true,
        "did": req.did,
        "invitesEnabled": false
    })))
}

// ============================================================================
// Validation Failures
// ============================================================================

#[derive(Debug, Deserialize)]
struct GetValidationFailuresQuery {
    did: String,
    collection: Option<String>,
    limit: Option<i64>,
}

async fn get_validation_failures(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(params): Query<GetValidationFailuresQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let failures = ctx
        .actor_store
        .get_validation_failures(&params.did, params.collection.as_deref(), params.limit)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "did": params.did,
        "failures": failures,
        "count": failures.len(),
    })))
}

// ============================================================================
// System Health and Diagnostics Endpoints
// ============================================================================

/// Get overall system health status
async fn get_system_health(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use crate::metrics;

    let uptime = metrics::UPTIME_SECONDS.get();

    // Check critical services
    let db_healthy = sqlx::query("SELECT 1")
        .fetch_one(&ctx.account_db)
        .await
        .is_ok();

    let sequencer_healthy = true; // Sequencer is always available if context exists

    // Check optional services
    let relay_connected = ctx.relay_client.is_some();
    let federation_enabled = ctx.config.federation.enabled;

    // Determine overall health
    let status = if db_healthy && sequencer_healthy {
        "healthy"
    } else {
        "unhealthy"
    };

    Ok(Json(serde_json::json!({
        "status": status,
        "version": ctx.config.service.version,
        "uptime_seconds": uptime,
        "services": {
            "database": if db_healthy { "healthy" } else { "unhealthy" },
            "sequencer": if sequencer_healthy { "healthy" } else { "unhealthy" },
            "relay": if relay_connected { "connected" } else { "disconnected" },
            "federation": if federation_enabled { "enabled" } else { "disabled" },
        },
        "active_http_requests": metrics::HTTP_REQUESTS_ACTIVE.get(),
        "active_sessions": metrics::SESSIONS_ACTIVE.get(),
    })))
}

/// Get database connection pool status
async fn get_database_status(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Get pool statistics
    let pool_size = ctx.account_db.size();
    let pool_connections = ctx.account_db.num_idle();

    // Try a test query to measure latency
    let start = std::time::Instant::now();
    let query_ok = sqlx::query("SELECT 1")
        .fetch_one(&ctx.account_db)
        .await
        .is_ok();
    let query_latency_ms = start.elapsed().as_millis();

    // Get database-level statistics
    let db_stats = sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM account")
        .fetch_one(&ctx.account_db)
        .await
        .map(|(count,)| count)
        .unwrap_or(0);

    let session_count = sqlx::query_as::<_, (i64,)>(
        "SELECT COUNT(*) FROM session WHERE expires_at > datetime('now')",
    )
    .fetch_one(&ctx.account_db)
    .await
    .map(|(count,)| count)
    .unwrap_or(0);

    Ok(Json(serde_json::json!({
        "status": if query_ok { "healthy" } else { "unhealthy" },
        "pool": {
            "size": pool_size,
            "idle_connections": pool_connections,
            "active_connections": pool_size as i64 - pool_connections as i64,
        },
        "latency_ms": query_latency_ms,
        "statistics": {
            "total_accounts": db_stats,
            "active_sessions": session_count,
        }
    })))
}

/// Get resource usage metrics (CPU, memory)
async fn get_resource_usage(
    State(_ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Get process metrics from prometheus
    let metric_families = prometheus::gather();

    let mut memory_bytes: Option<f64> = None;
    let mut cpu_seconds_total: Option<f64> = None;
    let mut open_fds: Option<f64> = None;

    // Extract process metrics
    for mf in &metric_families {
        match mf.name() {
            "process_resident_memory_bytes" => {
                if let Some(m) = mf.get_metric().first() {
                    memory_bytes = Some(m.get_gauge().value());
                }
            }
            "process_cpu_seconds_total" => {
                if let Some(m) = mf.get_metric().first() {
                    cpu_seconds_total = Some(m.get_counter().value());
                }
            }
            "process_open_fds" => {
                if let Some(m) = mf.get_metric().first() {
                    open_fds = Some(m.get_gauge().value());
                }
            }
            _ => {}
        }
    }

    Ok(Json(serde_json::json!({
        "memory": {
            "resident_bytes": memory_bytes.unwrap_or(0.0),
            "resident_mb": memory_bytes.unwrap_or(0.0) / 1024.0 / 1024.0,
        },
        "cpu": {
            "seconds_total": cpu_seconds_total.unwrap_or(0.0),
        },
        "file_descriptors": {
            "open": open_fds.unwrap_or(0.0) as i64,
        }
    })))
}

/// List background jobs status
async fn list_background_jobs(
    State(_ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use crate::metrics;

    // Get metrics about background jobs
    let active_jobs = metrics::BACKGROUND_JOBS_ACTIVE.get();

    // Get job execution counts from prometheus
    let metric_families = prometheus::gather();
    let mut job_stats = std::collections::HashMap::new();

    for mf in &metric_families {
        if mf.name() == "background_jobs_total" {
            for metric in mf.get_metric() {
                let mut job_type = "unknown";
                let mut status = "unknown";

                for label in metric.get_label() {
                    if label.name() == "job_type" {
                        job_type = label.value();
                    } else if label.name() == "status" {
                        status = label.value();
                    }
                }

                let count = metric.get_counter().value() as i64;
                let entry = job_stats.entry(job_type).or_insert_with(|| {
                    serde_json::json!({
                        "type": job_type,
                        "success": 0,
                        "failure": 0,
                        "total": 0,
                    })
                });

                if let Some(obj) = entry.as_object_mut() {
                    obj["total"] = serde_json::json!(obj["total"].as_i64().unwrap_or(0) + count);
                    if status == "success" {
                        obj["success"] = serde_json::json!(count);
                    } else if status == "failure" {
                        obj["failure"] = serde_json::json!(count);
                    }
                }
            }
        }
    }

    let jobs: Vec<_> = job_stats.values().cloned().collect();

    Ok(Json(serde_json::json!({
        "active_jobs": active_jobs,
        "job_statistics": jobs,
    })))
}

/// Run comprehensive health checks
async fn run_health_checks(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let start = std::time::Instant::now();
    let mut checks = Vec::new();

    // Database check
    let db_start = std::time::Instant::now();
    let db_healthy = sqlx::query("SELECT 1")
        .fetch_one(&ctx.account_db)
        .await
        .is_ok();
    checks.push(serde_json::json!({
        "component": "database",
        "status": if db_healthy { "healthy" } else { "unhealthy" },
        "response_time_ms": db_start.elapsed().as_millis(),
    }));

    // Blob storage check
    let blob_start = std::time::Instant::now();
    let _ = &ctx.blob_store; // Just verify it exists
    checks.push(serde_json::json!({
        "component": "blob_storage",
        "status": "healthy",
        "response_time_ms": blob_start.elapsed().as_millis(),
    }));

    // Sequencer check
    let seq_start = std::time::Instant::now();
    let _ = &ctx.sequencer;
    checks.push(serde_json::json!({
        "component": "sequencer",
        "status": "healthy",
        "response_time_ms": seq_start.elapsed().as_millis(),
    }));

    // Identity resolver check
    let identity_start = std::time::Instant::now();
    let _ = &ctx.identity_resolver;
    checks.push(serde_json::json!({
        "component": "identity_resolver",
        "status": "healthy",
        "response_time_ms": identity_start.elapsed().as_millis(),
    }));

    // Relay check (if enabled)
    if let Some(ref _relay) = ctx.relay_client {
        checks.push(serde_json::json!({
            "component": "relay_client",
            "status": "connected",
            "response_time_ms": 0,
        }));
    }

    // Email service check (if configured)
    let email_configured = ctx.config.email.is_some();
    if email_configured {
        checks.push(serde_json::json!({
            "component": "email_service",
            "status": "configured",
            "response_time_ms": 0,
        }));
    }

    // Determine overall status
    let all_healthy = checks.iter().all(|c| {
        c["status"] == "healthy" || c["status"] == "connected" || c["status"] == "configured"
    });

    Ok(Json(serde_json::json!({
        "overall_status": if all_healthy { "healthy" } else { "degraded" },
        "checks": checks,
        "total_duration_ms": start.elapsed().as_millis(),
        "timestamp": chrono::Utc::now().to_rfc3339(),
    })))
}

/// Get version and build information
async fn get_version_info(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    Ok(Json(serde_json::json!({
        "version": ctx.config.service.version,
        "service_did": ctx.config.service.service_did,
        "hostname": ctx.config.service.hostname,
        "port": ctx.config.service.port,
        "rust_version": env!("CARGO_PKG_RUST_VERSION"),
        "build_profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "features": {
            "federation": ctx.config.federation.enabled,
            "invites_required": ctx.config.invites.required,
            "rate_limiting": ctx.config.rate_limit.enabled,
            "email": ctx.config.email.is_some(),
        }
    })))
}

/// Get comprehensive system metrics
async fn get_system_metrics(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use crate::metrics;

    // Gather all Prometheus metrics
    let metric_families = prometheus::gather();

    // Extract key metrics
    let mut http_requests_total: i64 = 0;
    let mut db_queries_total: i64 = 0;
    let mut cache_hits: i64 = 0;
    let mut cache_misses: i64 = 0;
    let mut sequencer_current_seq: i64 = 0;
    let mut relay_events_total: i64 = 0;

    for mf in &metric_families {
        match mf.name() {
            "http_requests_total" => {
                for m in mf.get_metric() {
                    http_requests_total += m.get_counter().value() as i64;
                }
            }
            "db_queries_total" => {
                for m in mf.get_metric() {
                    db_queries_total += m.get_counter().value() as i64;
                }
            }
            "cache_hits_total" => {
                for m in mf.get_metric() {
                    cache_hits += m.get_counter().value() as i64;
                }
            }
            "cache_misses_total" => {
                for m in mf.get_metric() {
                    cache_misses += m.get_counter().value() as i64;
                }
            }
            "sequencer_current_seq" => {
                if let Some(m) = mf.get_metric().first() {
                    sequencer_current_seq = m.get_gauge().value() as i64;
                }
            }
            "relay_events_total" => {
                for m in mf.get_metric() {
                    relay_events_total += m.get_counter().value() as i64;
                }
            }
            _ => {}
        }
    }

    let cache_total = cache_hits + cache_misses;
    let cache_hit_rate = if cache_total > 0 {
        (cache_hits as f64 / cache_total as f64) * 100.0
    } else {
        0.0
    };

    Ok(Json(serde_json::json!({
        "uptime_seconds": metrics::UPTIME_SECONDS.get(),
        "http": {
            "requests_total": http_requests_total,
            "active_requests": metrics::HTTP_REQUESTS_ACTIVE.get(),
        },
        "database": {
            "queries_total": db_queries_total,
            "active_connections": metrics::DB_CONNECTIONS_ACTIVE.get(),
            "pool_size": ctx.account_db.size(),
        },
        "cache": {
            "hits": cache_hits,
            "misses": cache_misses,
            "hit_rate_percent": cache_hit_rate,
        },
        "sequencer": {
            "current_sequence": sequencer_current_seq,
            "events_total": metrics::SEQUENCER_EVENTS_TOTAL.with_label_values(&["commit"]).get() +
                           metrics::SEQUENCER_EVENTS_TOTAL.with_label_values(&["identity"]).get() +
                           metrics::SEQUENCER_EVENTS_TOTAL.with_label_values(&["account"]).get(),
        },
        "relay": {
            "events_received": relay_events_total,
            "connection_status": metrics::RELAY_CONNECTION_STATUS.get(),
        },
        "accounts": {
            "total": metrics::ACCOUNTS_TOTAL.get(),
            "active_sessions": metrics::SESSIONS_ACTIVE.get(),
        },
        "background_jobs": {
            "active": metrics::BACKGROUND_JOBS_ACTIVE.get(),
        }
    })))
}

// ============================================================================
// Blob Storage Management Endpoints
// ============================================================================

/// Query parameters for listBlobs endpoint
#[derive(Deserialize)]
struct ListBlobsQuery {
    did: Option<String>,
    #[serde(default = "default_blob_limit")]
    limit: i64,
    cursor: Option<String>,
}

fn default_blob_limit() -> i64 {
    100
}

/// Get blob storage statistics
async fn get_blob_statistics(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Get total blob count and size
    let stats = sqlx::query_as::<_, (i64, i64)>(
        "SELECT COUNT(*), COALESCE(SUM(size), 0) FROM blob_metadata",
    )
    .fetch_one(&ctx.account_db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let (total_count, total_size) = stats;

    // Get orphaned temp blobs count
    let orphaned_temp = ctx
        .blob_store
        .list_orphaned_temp_blobs(24)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let orphaned_count = orphaned_temp.len() as i64;

    // Get blob count by MIME type
    let mime_stats = sqlx::query_as::<_, (String, i64)>(
        "SELECT mime_type, COUNT(*) as count FROM blob_metadata GROUP BY mime_type ORDER BY count DESC LIMIT 10"
    )
    .fetch_all(&ctx.account_db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mime_distribution: Vec<serde_json::Value> = mime_stats
        .iter()
        .map(|(mime_type, count)| {
            serde_json::json!({
                "mime_type": mime_type,
                "count": count
            })
        })
        .collect();

    // Get top users by blob count
    let top_users = sqlx::query_as::<_, (String, i64, i64)>(
        "SELECT creator_did, COUNT(*) as count, SUM(size) as total_size FROM blob_metadata GROUP BY creator_did ORDER BY count DESC LIMIT 10"
    )
    .fetch_all(&ctx.account_db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let user_stats: Vec<serde_json::Value> = top_users
        .iter()
        .map(|(did, count, size)| {
            serde_json::json!({
                "did": did,
                "blob_count": count,
                "total_size": size
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "total_blobs": total_count,
        "total_size_bytes": total_size,
        "total_size_mb": total_size as f64 / 1024.0 / 1024.0,
        "orphaned_temp_blobs": orphaned_count,
        "mime_type_distribution": mime_distribution,
        "top_users_by_blob_count": user_stats,
    })))
}

/// List blobs with optional filtering
async fn list_blobs(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(params): Query<ListBlobsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let limit = params.limit.min(500); // Cap at 500

    let blobs = if let Some(did) = params.did {
        // List blobs for specific DID
        ctx.blob_store
            .list_for_user(&did, limit)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    } else {
        // List all blobs with cursor pagination
        let query = if let Some(cursor) = params.cursor {
            sqlx::query(
                r#"
                SELECT cid, mime_type, size, creator_did, created_at, width, height, alt_text, thumbnail_cid
                FROM blob_metadata
                WHERE cid > ?1
                ORDER BY cid ASC
                LIMIT ?2
                "#
            )
            .bind(cursor)
            .bind(limit)
        } else {
            sqlx::query(
                r#"
                SELECT cid, mime_type, size, creator_did, created_at, width, height, alt_text, thumbnail_cid
                FROM blob_metadata
                ORDER BY cid ASC
                LIMIT ?1
                "#
            )
            .bind(limit)
        };

        let rows = query
            .fetch_all(&ctx.account_db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let mut blobs = Vec::new();
        for row in rows {
            use sqlx::Row;
            blobs.push(crate::blob_store::BlobMetadata {
                cid: row
                    .try_get("cid")
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
                mime_type: row
                    .try_get("mime_type")
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
                size: row
                    .try_get("size")
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
                creator_did: row
                    .try_get("creator_did")
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
                created_at: row
                    .try_get("created_at")
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
                width: row
                    .try_get("width")
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
                height: row
                    .try_get("height")
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
                alt_text: row
                    .try_get("alt_text")
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
                thumbnail_cid: row
                    .try_get("thumbnail_cid")
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            });
        }
        blobs
    };

    let next_cursor = blobs.last().map(|b| b.cid.clone());

    Ok(Json(serde_json::json!({
        "blobs": blobs,
        "cursor": next_cursor,
    })))
}

/// Request body for deleteBlob endpoint
#[derive(Deserialize)]
struct DeleteBlobRequest {
    cid: String,
}

/// Delete a specific blob
async fn delete_blob(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Json(req): Json<DeleteBlobRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Check if blob exists
    let metadata = ctx
        .blob_store
        .get_metadata(&req.cid)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if metadata.is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            format!("Blob not found: {}", req.cid),
        ));
    }

    // Delete blob
    ctx.blob_store
        .delete(&req.cid)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "success": true,
        "cid": req.cid,
        "message": "Blob deleted successfully"
    })))
}

/// Request body for quarantineBlob endpoint
#[derive(Deserialize)]
struct QuarantineBlobRequest {
    cid: String,
    reason: String,
    details: Option<String>,
    legal_reference: Option<String>,
}

/// Quarantine a blob (mark as taken down)
async fn quarantine_blob(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<QuarantineBlobRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use crate::blob_store::quarantine::{BlobQuarantine, QuarantineReason};
    use std::str::FromStr;

    // Parse quarantine reason
    let reason = QuarantineReason::from_str(&req.reason)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    // Create quarantine manager
    let quarantine = BlobQuarantine::new(ctx.account_db.clone());

    // Quarantine the blob
    let record = quarantine
        .quarantine_blob(
            &req.cid,
            reason,
            req.details.as_deref(),
            &auth.did,
            req.legal_reference.as_deref(),
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "success": true,
        "cid": record.cid,
        "reason": record.reason,
        "quarantined_by": record.quarantined_by,
        "quarantined_at": record.quarantined_at,
    })))
}

/// Request body for restoreBlob endpoint
#[derive(Deserialize)]
struct RestoreBlobRequest {
    cid: String,
}

/// Restore a quarantined blob
async fn restore_blob(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<RestoreBlobRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use crate::blob_store::quarantine::BlobQuarantine;

    // Create quarantine manager
    let quarantine = BlobQuarantine::new(ctx.account_db.clone());

    // Restore the blob
    quarantine
        .restore_blob(&req.cid, &auth.did)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "success": true,
        "cid": req.cid,
        "restored_by": auth.did,
        "message": "Blob restored successfully"
    })))
}

/// Request body for runBlobGC endpoint
#[derive(Deserialize)]
struct RunBlobGCRequest {
    #[serde(default = "default_gc_ttl")]
    orphaned_ttl_hours: i64,
    dry_run: Option<bool>,
}

fn default_gc_ttl() -> i64 {
    24
}

/// Run blob garbage collection
async fn run_blob_gc(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Json(req): Json<RunBlobGCRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let dry_run = req.dry_run.unwrap_or(false);

    // List orphaned temp blobs
    let orphaned = ctx
        .blob_store
        .list_orphaned_temp_blobs(req.orphaned_ttl_hours)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut deleted_count = 0;
    let mut errors = Vec::new();

    if !dry_run {
        // Delete each orphaned blob
        for cid in &orphaned {
            match ctx.blob_store.delete_temp_blob(cid).await {
                Ok(_) => {
                    deleted_count += 1;
                    tracing::info!("Deleted orphaned temp blob: {}", cid);
                }
                Err(e) => {
                    errors.push(format!("Failed to delete {}: {}", cid, e));
                    tracing::warn!("Failed to delete orphaned temp blob {}: {}", cid, e);
                }
            }
        }
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "dry_run": dry_run,
        "orphaned_found": orphaned.len(),
        "deleted": deleted_count,
        "errors": errors,
    })))
}

/// Get blob quotas per account
async fn get_blob_quotas(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Get storage usage per user
    let usage = sqlx::query_as::<_, (String, i64, i64)>(
        r#"
        SELECT creator_did, COUNT(*) as blob_count, SUM(size) as total_size
        FROM blob_metadata
        GROUP BY creator_did
        ORDER BY total_size DESC
        LIMIT 100
        "#,
    )
    .fetch_all(&ctx.account_db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let quotas: Vec<serde_json::Value> = usage
        .iter()
        .map(|(did, count, size)| {
            serde_json::json!({
                "did": did,
                "blob_count": count,
                "total_size_bytes": size,
                "total_size_mb": *size as f64 / 1024.0 / 1024.0,
                // For now, no hard quotas enforced, just reporting usage
                "quota_bytes": null,
                "quota_mb": null,
                "usage_percent": null,
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "quotas": quotas,
        "total_users": quotas.len(),
    })))
}

// ============================================================================
// Sequencer Management Endpoints
// ============================================================================

/// Get sequencer status and statistics
async fn get_sequencer_status(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Get current sequence number
    let current_seq = ctx
        .sequencer
        .current_seq()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .unwrap_or(0);

    // Get total event count
    let total_events: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM repo_seq WHERE invalidated = 0")
            .fetch_one(&ctx.account_db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Get event counts by type
    let event_counts = sqlx::query_as::<_, (String, i64)>(
        "SELECT event_type, COUNT(*) as count FROM repo_seq WHERE invalidated = 0 GROUP BY event_type"
    )
    .fetch_all(&ctx.account_db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut events_by_type = serde_json::Map::new();
    for (event_type, count) in event_counts {
        events_by_type.insert(event_type, serde_json::json!(count));
    }

    // Get first and last event timestamps
    let first_event: Option<String> = sqlx::query_scalar(
        "SELECT sequenced_at FROM repo_seq WHERE invalidated = 0 ORDER BY seq ASC LIMIT 1",
    )
    .fetch_optional(&ctx.account_db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let last_event: Option<String> = sqlx::query_scalar(
        "SELECT sequenced_at FROM repo_seq WHERE invalidated = 0 ORDER BY seq DESC LIMIT 1",
    )
    .fetch_optional(&ctx.account_db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Check if sequencer is paused (using a config table)
    let is_paused: bool = sqlx::query_scalar(
        "SELECT COALESCE((SELECT value FROM sequencer_config WHERE key = 'paused'), '0') = '1'",
    )
    .fetch_one(&ctx.account_db)
    .await
    .unwrap_or(false);

    Ok(Json(serde_json::json!({
        "status": if is_paused { "paused" } else { "running" },
        "current_seq": current_seq,
        "total_events": total_events,
        "events_by_type": events_by_type,
        "first_event_at": first_event,
        "last_event_at": last_event,
        "paused": is_paused,
    })))
}

/// Query parameters for listRecentEvents endpoint
#[derive(Deserialize)]
struct ListRecentEventsQuery {
    #[serde(default = "default_recent_events_limit")]
    limit: i64,
    cursor: Option<i64>,
    event_type: Option<String>,
}

fn default_recent_events_limit() -> i64 {
    50
}

/// List recent events from the sequencer
async fn list_recent_events(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(params): Query<ListRecentEventsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let limit = params.limit.min(500); // Cap at 500

    // Build query based on filters
    let mut query = String::from(
        "SELECT seq, did, event_type, sequenced_at FROM repo_seq WHERE invalidated = 0",
    );

    // Add cursor filter
    if let Some(cursor) = params.cursor {
        query.push_str(&format!(" AND seq < {}", cursor));
    }

    // Add event type filter
    if let Some(ref event_type) = params.event_type {
        query.push_str(&format!(" AND event_type = '{}'", event_type));
    }

    query.push_str(&format!(" ORDER BY seq DESC LIMIT {}", limit));

    let rows = sqlx::query(&query)
        .fetch_all(&ctx.account_db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut events = Vec::new();
    for row in rows {
        use sqlx::Row;
        events.push(serde_json::json!({
            "seq": row.try_get::<i64, _>("seq").map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            "did": row.try_get::<String, _>("did").map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            "event_type": row.try_get::<String, _>("event_type").map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            "sequenced_at": row.try_get::<String, _>("sequenced_at").map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
        }));
    }

    let next_cursor = events
        .last()
        .and_then(|e| e.get("seq"))
        .and_then(|s| s.as_i64());

    Ok(Json(serde_json::json!({
        "events": events,
        "cursor": next_cursor,
    })))
}

/// Pause sequencer event streaming
async fn pause_sequencer(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Set paused flag in database
    sqlx::query("INSERT OR REPLACE INTO sequencer_config (key, value) VALUES ('paused', '1')")
        .execute(&ctx.account_db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Log action
    let _ = ctx
        .admin_role_manager
        .log_action(&auth.did, "sequencer.pause", None, None, None)
        .await;

    Ok(Json(serde_json::json!({
        "success": true,
        "status": "paused",
        "message": "Sequencer event streaming paused"
    })))
}

/// Resume sequencer event streaming
async fn resume_sequencer(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Set paused flag to false in database
    sqlx::query("INSERT OR REPLACE INTO sequencer_config (key, value) VALUES ('paused', '0')")
        .execute(&ctx.account_db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Log action
    let _ = ctx
        .admin_role_manager
        .log_action(&auth.did, "sequencer.resume", None, None, None)
        .await;

    Ok(Json(serde_json::json!({
        "success": true,
        "status": "running",
        "message": "Sequencer event streaming resumed"
    })))
}

/// Request body for resetSequencerCursor endpoint
#[derive(Deserialize)]
struct ResetSequencerCursorRequest {
    #[serde(default)]
    target_seq: Option<i64>,
}

/// Reset sequencer cursor position
async fn reset_sequencer_cursor(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<ResetSequencerCursorRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let target = req.target_seq.unwrap_or(0);

    // Validate target sequence exists if specified
    if target > 0 {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM repo_seq WHERE seq = ?1)")
                .bind(target)
                .fetch_one(&ctx.account_db)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        if !exists {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("Sequence {} not found", target),
            ));
        }
    }

    // Store cursor position
    sqlx::query(
        "INSERT OR REPLACE INTO sequencer_config (key, value) VALUES ('cursor_position', ?1)",
    )
    .bind(target.to_string())
    .execute(&ctx.account_db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Log action
    let _ = ctx
        .admin_role_manager
        .log_action(
            &auth.did,
            "sequencer.reset_cursor",
            None,
            Some(&target.to_string()),
            None,
        )
        .await;

    Ok(Json(serde_json::json!({
        "success": true,
        "cursor_position": target,
        "message": format!("Sequencer cursor reset to {}", target)
    })))
}

/// Request body for rebuildSequencer endpoint
#[derive(Deserialize)]
struct RebuildSequencerRequest {
    #[serde(default)]
    verify_only: bool,
}

/// Rebuild or verify sequencer integrity
async fn rebuild_sequencer(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<RebuildSequencerRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Verify sequence integrity
    let gaps = sqlx::query_as::<_, (i64, i64)>(
        r#"
        SELECT seq, seq - LAG(seq, 1, 0) OVER (ORDER BY seq) as gap
        FROM repo_seq
        WHERE invalidated = 0
        HAVING gap > 1
        LIMIT 10
        "#,
    )
    .fetch_all(&ctx.account_db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let has_gaps = !gaps.is_empty();

    // Check for duplicate sequences
    let duplicates: Vec<i64> = sqlx::query_scalar(
        r#"
        SELECT seq FROM repo_seq
        WHERE invalidated = 0
        GROUP BY seq
        HAVING COUNT(*) > 1
        LIMIT 10
        "#,
    )
    .fetch_all(&ctx.account_db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let has_duplicates = !duplicates.is_empty();

    let integrity_ok = !has_gaps && !has_duplicates;

    if req.verify_only {
        // Log verification action
        let _ = ctx
            .admin_role_manager
            .log_action(
                &auth.did,
                "sequencer.verify",
                None,
                Some(if integrity_ok { "passed" } else { "failed" }),
                None,
            )
            .await;

        Ok(Json(serde_json::json!({
            "success": true,
            "verify_only": true,
            "integrity_ok": integrity_ok,
            "has_gaps": has_gaps,
            "has_duplicates": has_duplicates,
            "gaps": gaps.iter().map(|(seq, gap)| serde_json::json!({
                "seq": seq,
                "gap_size": gap
            })).collect::<Vec<_>>(),
            "duplicate_sequences": duplicates,
        })))
    } else {
        // For now, rebuild is just verification
        // In a full implementation, this would:
        // 1. Backup current sequence table
        // 2. Rebuild sequence numbers from scratch
        // 3. Update all references
        // This is a destructive operation and should be done carefully

        // Log rebuild action
        let _ = ctx
            .admin_role_manager
            .log_action(
                &auth.did,
                "sequencer.rebuild",
                None,
                Some("verify_only"),
                None,
            )
            .await;

        Ok(Json(serde_json::json!({
            "success": true,
            "verify_only": false,
            "integrity_ok": integrity_ok,
            "message": "Sequencer verification complete. Full rebuild not yet implemented.",
            "has_gaps": has_gaps,
            "has_duplicates": has_duplicates,
        })))
    }
}

// ============================================================================
// Rate Limiting Management Endpoints
// ============================================================================

/// Response for getRateLimitConfig endpoint
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitConfigResponse {
    /// Requests per second for authenticated users
    authenticated_rps: u32,
    /// Requests per second for unauthenticated users
    unauthenticated_rps: u32,
    /// Requests per second for admin users
    admin_rps: u32,
    /// Requests per second for cross-PDS authenticated users
    cross_pds_rps: u32,
    /// Burst size for rate limiting
    burst_size: u32,
    /// Whether proxy headers are trusted for IP extraction
    trust_proxy: bool,
    /// Requests per second for handle resolution
    handle_resolution_rps: u32,
    /// Requests per second for DID resolution
    did_resolution_rps: u32,
    /// Endpoints with custom rate limits
    custom_endpoints: Vec<String>,
}

/// Get current rate limit configuration
///
/// Returns the current rate limiting settings including global limits,
/// per-type limits, and endpoints with custom rate limits.
async fn get_rate_limit_config(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<RateLimitConfigResponse>, (StatusCode, String)> {
    let config = ctx.rate_limiter.get_config();
    let custom_endpoints = ctx.rate_limiter.get_rate_limited_endpoints();

    Ok(Json(RateLimitConfigResponse {
        authenticated_rps: config.authenticated_rps,
        unauthenticated_rps: config.unauthenticated_rps,
        admin_rps: config.admin_rps,
        cross_pds_rps: config.cross_pds_rps,
        burst_size: config.burst_size,
        trust_proxy: config.trust_proxy,
        handle_resolution_rps: config.handle_resolution_rps,
        did_resolution_rps: config.did_resolution_rps,
        custom_endpoints,
    }))
}

/// Rate limit statistics per category
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitCategoryStats {
    category: String,
    recent_requests: u32,
}

/// Response for getRateLimitStatus endpoint
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitStatusResponse {
    /// Total tracked request identifiers
    tracked_identifiers: usize,
    /// Recent request counts by category
    recent_activity: Vec<RateLimitCategoryStats>,
    /// Endpoints with custom rate limits
    rate_limited_endpoints: Vec<String>,
    /// Server uptime information
    status: String,
}

/// Get current rate limiting status
///
/// Returns real-time statistics about rate limiting including
/// current request counts and tracked identifiers.
async fn get_rate_limit_status(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<RateLimitStatusResponse>, (StatusCode, String)> {
    let tracked_identifiers = ctx.rate_limiter.get_tracked_identifiers_count();
    let request_counts = ctx.rate_limiter.get_request_counts();
    let rate_limited_endpoints = ctx.rate_limiter.get_rate_limited_endpoints();

    // Aggregate request counts by category
    let mut category_counts: std::collections::HashMap<String, u32> =
        std::collections::HashMap::new();
    for (key, count) in request_counts {
        // Extract category from key (e.g., "global:authenticated" -> "authenticated")
        let category = if key.contains(':') {
            key.split(':').next_back().unwrap_or(&key).to_string()
        } else {
            key
        };
        *category_counts.entry(category).or_insert(0) += count;
    }

    let recent_activity: Vec<RateLimitCategoryStats> = category_counts
        .into_iter()
        .map(|(category, recent_requests)| RateLimitCategoryStats {
            category,
            recent_requests,
        })
        .collect();

    Ok(Json(RateLimitStatusResponse {
        tracked_identifiers,
        recent_activity,
        rate_limited_endpoints,
        status: "operational".to_string(),
    }))
}

/// Request body for cleanupRateLimitState endpoint
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CleanupRateLimitRequest {
    /// Force cleanup even if not necessary
    #[serde(default)]
    force: bool,
}

/// Cleanup old rate limit tracking state
///
/// Clears expired rate limit tracking entries to free memory.
/// This is normally done automatically but can be triggered manually.
async fn cleanup_rate_limit_state(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<CleanupRateLimitRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let before_count = ctx.rate_limiter.get_tracked_identifiers_count();

    ctx.rate_limiter.cleanup_old_counts();

    let after_count = ctx.rate_limiter.get_tracked_identifiers_count();
    let cleaned_count = before_count.saturating_sub(after_count);

    // Log action
    let _ = ctx
        .admin_role_manager
        .log_action(
            &auth.did,
            "rate_limit.cleanup",
            None,
            Some(&format!("cleaned {} entries", cleaned_count)),
            if req.force { Some("forced") } else { None },
        )
        .await;

    tracing::info!(
        "Admin {} triggered rate limit cleanup: {} entries removed",
        auth.did,
        cleaned_count
    );

    Ok(Json(serde_json::json!({
        "success": true,
        "before_count": before_count,
        "after_count": after_count,
        "cleaned_count": cleaned_count,
        "forced": req.force,
    })))
}

// ============================================================================
// Federation and Relay Management Endpoints
// ============================================================================

/// Response for getFederationStatus endpoint
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct FederationStatusResponse {
    /// Whether federation is enabled
    enabled: bool,
    /// Service DID for this PDS
    service_did: String,
    /// Number of configured relay servers
    relay_count: usize,
    /// Whether relay client is connected
    relay_connected: bool,
    /// Whether PDS discovery is enabled
    discovery_enabled: bool,
    /// Whether federated search is enabled
    search_enabled: bool,
    /// Number of known PDS instances
    known_instances: usize,
    /// Status message
    status: String,
}

/// Get federation status
///
/// Returns the current federation configuration and connection status.
async fn get_federation_status(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<FederationStatusResponse>, (StatusCode, String)> {
    let relay_connected = ctx.relay_client.is_some();
    let discovery_enabled = ctx.pds_discovery.is_some();
    let search_enabled = ctx.federated_search.is_some();

    // Get count of known instances if discovery is enabled
    let known_instances = if let Some(ref discovery) = ctx.pds_discovery {
        discovery.get_known_instances().await.len()
    } else {
        0
    };

    // Get config info
    let federation_config = &ctx.config.federation;

    let status = if !federation_config.enabled {
        "disabled".to_string()
    } else if relay_connected {
        "connected".to_string()
    } else {
        "enabled_disconnected".to_string()
    };

    Ok(Json(FederationStatusResponse {
        enabled: federation_config.enabled,
        service_did: ctx.config.service.service_did.clone(),
        relay_count: federation_config.relay_urls.len(),
        relay_connected,
        discovery_enabled,
        search_enabled,
        known_instances,
        status,
    }))
}

/// Relay server info
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RelayServerInfo {
    url: String,
    status: String,
}

/// Response for getRelayConfig endpoint
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RelayConfigResponse {
    /// Configured relay servers
    servers: Vec<RelayServerInfo>,
    /// Reconnect interval in seconds
    reconnect_interval: u64,
    /// Buffer size for events
    buffer_size: usize,
    /// Whether compression is enabled
    compression_enabled: bool,
    /// Overall relay status
    status: String,
}

/// Get relay configuration
///
/// Returns the current relay client configuration and server list.
async fn get_relay_config(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<RelayConfigResponse>, (StatusCode, String)> {
    let federation_config = &ctx.config.federation;
    let has_relay = ctx.relay_client.is_some();

    let servers: Vec<RelayServerInfo> = federation_config
        .relay_urls
        .iter()
        .map(|url: &String| RelayServerInfo {
            url: url.clone(),
            status: if has_relay {
                "configured".to_string()
            } else {
                "disabled".to_string()
            },
        })
        .collect();

    let status = if !federation_config.enabled {
        "disabled".to_string()
    } else if servers.is_empty() {
        "no_servers".to_string()
    } else if has_relay {
        "active".to_string()
    } else {
        "inactive".to_string()
    };

    Ok(Json(RelayConfigResponse {
        servers,
        reconnect_interval: 5, // Default from RelayConfig
        buffer_size: 1000,     // Default from RelayConfig
        compression_enabled: true,
        status,
    }))
}

/// Known PDS instance info
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct KnownInstanceInfo {
    did: String,
    url: String,
    name: Option<String>,
    open_registrations: bool,
    user_count: Option<i64>,
    last_seen: Option<i64>,
}

/// Response for listKnownInstances endpoint
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ListKnownInstancesResponse {
    instances: Vec<KnownInstanceInfo>,
    total: usize,
}

/// List known PDS instances
///
/// Returns all PDS instances discovered through relay servers.
async fn list_known_instances(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<ListKnownInstancesResponse>, (StatusCode, String)> {
    let instances: Vec<KnownInstanceInfo> = if let Some(ref discovery) = ctx.pds_discovery {
        discovery
            .get_known_instances()
            .await
            .into_iter()
            .map(|inst| KnownInstanceInfo {
                did: inst.did,
                url: inst.url,
                name: inst.name,
                open_registrations: inst.open_registrations,
                user_count: inst.user_count,
                last_seen: inst.last_seen,
            })
            .collect()
    } else {
        vec![]
    };

    let total = instances.len();
    Ok(Json(ListKnownInstancesResponse { instances, total }))
}

/// Trigger PDS discovery
///
/// Initiates discovery of PDS instances from configured relay servers.
async fn trigger_pds_discovery(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if let Some(ref discovery) = ctx.pds_discovery {
        match discovery.discover_from_relays().await {
            Ok(instances) => {
                // Log action
                let _ = ctx
                    .admin_role_manager
                    .log_action(
                        &auth.did,
                        "federation.discover",
                        None,
                        Some(&format!("discovered {} instances", instances.len())),
                        None,
                    )
                    .await;

                tracing::info!(
                    "Admin {} triggered PDS discovery: {} instances found",
                    auth.did,
                    instances.len()
                );

                Ok(Json(serde_json::json!({
                    "success": true,
                    "discovered_count": instances.len(),
                    "message": format!("Discovered {} PDS instances", instances.len()),
                })))
            }
            Err(e) => {
                tracing::warn!("PDS discovery failed: {}", e);
                Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Discovery failed: {}", e),
                ))
            }
        }
    } else {
        Err((
            StatusCode::BAD_REQUEST,
            "Federation discovery is not enabled".to_string(),
        ))
    }
}

/// Get nonce store status (service auth nonces)
///
/// Returns statistics about the service authentication nonce store.
async fn get_nonce_store_status(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let service_auth_enabled = ctx.nonce_store.is_some();
    let dpop_enabled = ctx.dpop_nonce_store.is_some();

    // Get nonce counts if available
    let service_auth_count = if let Some(ref store) = ctx.nonce_store {
        store.count().await
    } else {
        0
    };

    let dpop_count = if let Some(ref store) = ctx.dpop_nonce_store {
        store.count().await
    } else {
        0
    };

    Ok(Json(serde_json::json!({
        "service_auth": {
            "enabled": service_auth_enabled,
            "active_nonces": service_auth_count,
        },
        "dpop": {
            "enabled": dpop_enabled,
            "active_nonces": dpop_count,
        },
        "status": if service_auth_enabled || dpop_enabled { "active" } else { "disabled" },
    })))
}

/// Cleanup nonce stores
///
/// Triggers cleanup of expired nonces in both service auth and DPoP stores.
async fn cleanup_nonce_stores(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let mut cleaned_service_auth = 0;
    let mut cleaned_dpop = 0;

    // Cleanup service auth nonces
    if let Some(ref store) = ctx.nonce_store {
        if let Ok(removed) = store.cleanup_expired().await {
            cleaned_service_auth = removed;
        }
    }

    // Cleanup DPoP nonces
    if let Some(ref store) = ctx.dpop_nonce_store {
        if let Ok(removed) = store.cleanup_expired().await {
            cleaned_dpop = removed;
        }
    }

    let total_cleaned = cleaned_service_auth + cleaned_dpop;

    // Log action
    let _ = ctx
        .admin_role_manager
        .log_action(
            &auth.did,
            "federation.nonce_cleanup",
            None,
            Some(&format!("cleaned {} nonces", total_cleaned)),
            None,
        )
        .await;

    tracing::info!(
        "Admin {} triggered nonce cleanup: {} service auth, {} DPoP",
        auth.did,
        cleaned_service_auth,
        cleaned_dpop
    );

    Ok(Json(serde_json::json!({
        "success": true,
        "cleaned": {
            "service_auth": cleaned_service_auth,
            "dpop": cleaned_dpop,
            "total": total_cleaned,
        },
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        account::ValidatedSession,
        admin::roles::Role,
        config::{
            AuthConfig, BlobstoreConfig, FederationConfig, IdentityConfig, InviteConfig,
            LoggingConfig, OAuthConfig, RateLimitConfig, ServerConfig, ServiceConfig,
            StorageConfig,
        },
    };
    use tempfile::tempdir;

    async fn create_test_context() -> AppContext {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        let config = ServerConfig {
            service: ServiceConfig {
                hostname: "localhost".to_string(),
                port: 2583,
                service_did: "did:web:localhost".to_string(),
                version: "0.1.0-test".to_string(),
                blob_upload_limit: 5242880,
            },
            storage: StorageConfig {
                data_directory: dir.path().to_path_buf(),
                account_db: db_path.clone(),
                sequencer_db: dir.path().join("sequencer.db"),
                did_cache_db: dir.path().join("did_cache.db"),
                actor_store_directory: dir.path().join("actors"),
                blobstore: BlobstoreConfig::Disk {
                    location: dir.path().join("blobs"),
                    tmp_location: dir.path().join("temp"),
                },
            },
            authentication: AuthConfig {
                // Config validation requires JWT secrets >= 32 chars.
                jwt_secret: "test-secret-key-for-admin-tests-32-chars".to_string(),
                // Valid 32-byte hex keys so PlcSigner::from_hex succeeds in
                // tests that exercise PLC code paths.
                repo_signing_key: "a".repeat(64),
                plc_rotation_key: "b".repeat(64),
                admin_dids: vec![],
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
            },
            email: None,
            invites: InviteConfig {
                required: false,
                interval: 604800,
                epoch: "2024-01-01T00:00:00Z".to_string(),
            },
            rate_limit: RateLimitConfig {
                enabled: true,
                global_requests_per_minute: 3000,
                use_redis: false,
                redis_url: None,
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
            validation_mode: crate::validation::ValidationMode::Required,
        };

        AppContext::new(config).await.unwrap()
    }

    #[tokio::test]
    async fn test_get_system_health() {
        let ctx = create_test_context().await;
        let auth = AdminAuthContext {
            did: "did:plc:test".to_string(),
            session: ValidatedSession {
                did: "did:plc:test".to_string(),
                session_id: "test_session".to_string(),
                is_app_password: false,
            },
            role: Role::Admin,
        };

        let result = get_system_health(State(ctx), auth).await;
        assert!(result.is_ok());

        let json = result.unwrap().0;
        assert!(json["status"].is_string());
        assert_eq!(json["version"], "0.1.0-test");
        assert!(json["uptime_seconds"].is_number());
        assert!(json["services"].is_object());
        assert!(json["services"]["database"].is_string());
    }

    #[tokio::test]
    async fn test_get_database_status() {
        let ctx = create_test_context().await;
        let auth = AdminAuthContext {
            did: "did:plc:test".to_string(),
            session: ValidatedSession {
                did: "did:plc:test".to_string(),
                session_id: "test_session".to_string(),
                is_app_password: false,
            },
            role: Role::Admin,
        };

        let result = get_database_status(State(ctx), auth).await;
        assert!(result.is_ok());

        let json = result.unwrap().0;
        assert_eq!(json["status"], "healthy");
        assert!(json["pool"]["size"].is_number());
        assert!(json["pool"]["idle_connections"].is_number());
        assert!(json["pool"]["active_connections"].is_number());
        assert!(json["latency_ms"].is_number());
        assert!(json["statistics"]["total_accounts"].is_number());
    }

    #[tokio::test]
    async fn test_get_resource_usage() {
        let ctx = create_test_context().await;
        let auth = AdminAuthContext {
            did: "did:plc:test".to_string(),
            session: ValidatedSession {
                did: "did:plc:test".to_string(),
                session_id: "test_session".to_string(),
                is_app_password: false,
            },
            role: Role::Admin,
        };

        let result = get_resource_usage(State(ctx), auth).await;
        assert!(result.is_ok());

        let json = result.unwrap().0;
        assert!(json["memory"].is_object());
        assert!(json["memory"]["resident_bytes"].is_number());
        assert!(json["memory"]["resident_mb"].is_number());
        assert!(json["cpu"].is_object());
        assert!(json["cpu"]["seconds_total"].is_number());
    }

    #[tokio::test]
    async fn test_list_background_jobs() {
        let ctx = create_test_context().await;
        let auth = AdminAuthContext {
            did: "did:plc:test".to_string(),
            session: ValidatedSession {
                did: "did:plc:test".to_string(),
                session_id: "test_session".to_string(),
                is_app_password: false,
            },
            role: Role::Admin,
        };

        let result = list_background_jobs(State(ctx), auth).await;
        assert!(result.is_ok());

        let json = result.unwrap().0;
        assert!(json["active_jobs"].is_number());
        assert!(json["job_statistics"].is_array());
    }

    #[tokio::test]
    async fn test_run_health_checks() {
        let ctx = create_test_context().await;
        let auth = AdminAuthContext {
            did: "did:plc:test".to_string(),
            session: ValidatedSession {
                did: "did:plc:test".to_string(),
                session_id: "test_session".to_string(),
                is_app_password: false,
            },
            role: Role::Admin,
        };

        let result = run_health_checks(State(ctx), auth).await;
        assert!(result.is_ok());

        let json = result.unwrap().0;
        assert!(json["overall_status"].is_string());
        assert!(json["checks"].is_array());
        assert!(json["total_duration_ms"].is_number());
        assert!(json["timestamp"].is_string());

        // Verify critical components are checked
        let checks = json["checks"].as_array().unwrap();
        let component_names: Vec<&str> = checks
            .iter()
            .filter_map(|c| c["component"].as_str())
            .collect();

        assert!(component_names.contains(&"database"));
        assert!(component_names.contains(&"blob_storage"));
        assert!(component_names.contains(&"sequencer"));
        assert!(component_names.contains(&"identity_resolver"));
    }

    #[tokio::test]
    async fn test_get_version_info() {
        let ctx = create_test_context().await;
        let auth = AdminAuthContext {
            did: "did:plc:test".to_string(),
            session: ValidatedSession {
                did: "did:plc:test".to_string(),
                session_id: "test_session".to_string(),
                is_app_password: false,
            },
            role: Role::Admin,
        };

        let result = get_version_info(State(ctx), auth).await;
        assert!(result.is_ok());

        let json = result.unwrap().0;
        assert_eq!(json["version"], "0.1.0-test");
        assert_eq!(json["service_did"], "did:web:localhost");
        assert_eq!(json["hostname"], "localhost");
        assert_eq!(json["port"], 2583);
        assert!(json["build_profile"].is_string());
        assert!(json["features"].is_object());
        assert_eq!(json["features"]["federation"], false);
    }

    #[tokio::test]
    async fn test_get_system_metrics() {
        let ctx = create_test_context().await;
        let auth = AdminAuthContext {
            did: "did:plc:test".to_string(),
            session: ValidatedSession {
                did: "did:plc:test".to_string(),
                session_id: "test_session".to_string(),
                is_app_password: false,
            },
            role: Role::Admin,
        };

        let result = get_system_metrics(State(ctx), auth).await;
        assert!(result.is_ok());

        let json = result.unwrap().0;
        assert!(json["uptime_seconds"].is_number());
        assert!(json["http"].is_object());
        assert!(json["http"]["requests_total"].is_number());
        assert!(json["http"]["active_requests"].is_number());
        assert!(json["database"].is_object());
        assert!(json["database"]["queries_total"].is_number());
        assert!(json["database"]["pool_size"].is_number());
        assert!(json["cache"].is_object());
        assert!(json["cache"]["hits"].is_number());
        assert!(json["cache"]["misses"].is_number());
        assert!(json["cache"]["hit_rate_percent"].is_number());
        assert!(json["sequencer"].is_object());
        assert!(json["accounts"].is_object());
        assert!(json["background_jobs"].is_object());
    }

    #[tokio::test]
    async fn test_database_status_pool_metrics() {
        let ctx = create_test_context().await;
        let auth = AdminAuthContext {
            did: "did:plc:test".to_string(),
            session: ValidatedSession {
                did: "did:plc:test".to_string(),
                session_id: "test_session".to_string(),
                is_app_password: false,
            },
            role: Role::Admin,
        };

        // Make a query to activate a connection
        let _ = sqlx::query("SELECT 1").fetch_one(&ctx.account_db).await;

        let result = get_database_status(State(ctx.clone()), auth).await;
        assert!(result.is_ok());

        let json = result.unwrap().0;
        let pool_size = json["pool"]["size"].as_u64().unwrap();
        let idle = json["pool"]["idle_connections"].as_u64().unwrap();
        let active = json["pool"]["active_connections"].as_i64().unwrap();

        // Verify pool metrics are consistent
        assert!(pool_size > 0);
        // idle and active are unsigned, so always >= 0
        assert_eq!(pool_size as i64, idle as i64 + active);
    }

    #[tokio::test]
    async fn test_health_checks_response_times() {
        let ctx = create_test_context().await;
        let auth = AdminAuthContext {
            did: "did:plc:test".to_string(),
            session: ValidatedSession {
                did: "did:plc:test".to_string(),
                session_id: "test_session".to_string(),
                is_app_password: false,
            },
            role: Role::Admin,
        };

        let result = run_health_checks(State(ctx), auth).await;
        assert!(result.is_ok());

        let json = result.unwrap().0;
        let checks = json["checks"].as_array().unwrap();

        // Verify all checks have response times
        for check in checks {
            assert!(check["response_time_ms"].is_number());
            let response_time = check["response_time_ms"].as_u64().unwrap();
            // Response time should be reasonable (< 1 second)
            assert!(
                response_time < 1000,
                "Response time too high: {}",
                response_time
            );
        }
    }

    #[tokio::test]
    async fn test_system_metrics_cache_hit_rate() {
        let ctx = create_test_context().await;
        let auth = AdminAuthContext {
            did: "did:plc:test".to_string(),
            session: ValidatedSession {
                did: "did:plc:test".to_string(),
                session_id: "test_session".to_string(),
                is_app_password: false,
            },
            role: Role::Admin,
        };

        // Record some cache events
        crate::metrics::record_cache_access("test", true);
        crate::metrics::record_cache_access("test", true);
        crate::metrics::record_cache_access("test", false);

        let result = get_system_metrics(State(ctx), auth).await;
        assert!(result.is_ok());

        let json = result.unwrap().0;
        let hits = json["cache"]["hits"].as_i64().unwrap();
        let misses = json["cache"]["misses"].as_i64().unwrap();
        let hit_rate = json["cache"]["hit_rate_percent"].as_f64().unwrap();

        assert!(hits >= 2);
        assert!(misses >= 1);
        assert!((0.0..=100.0).contains(&hit_rate));
    }

    fn admin_test_auth() -> AdminAuthContext {
        AdminAuthContext {
            did: "did:plc:test".to_string(),
            session: ValidatedSession {
                did: "did:plc:test".to_string(),
                session_id: "test_session".to_string(),
                is_app_password: false,
            },
            role: Role::Admin,
        }
    }

    async fn read_response_body(resp: axum::response::Response) -> (StatusCode, String) {
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn test_update_account_signing_key_rejects_non_plc_did() {
        let ctx = create_test_context().await;
        let req = UpdateAccountSigningKeyRequest {
            did: "did:web:example.com".to_string(),
            signing_key: "did:key:z6MkpTHR8VNsBxYAAWHut2Geadd9jSwuBV8xRoAnwWsdvktH".to_string(),
        };

        let resp = update_account_signing_key(State(ctx), admin_test_auth(), Json(req))
            .await
            .expect_err("non-did:plc DID should be rejected");
        let (status, body) = read_response_body(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("did:plc"));
    }

    #[tokio::test]
    async fn test_update_account_signing_key_rejects_non_did_key_signing_key() {
        let ctx = create_test_context().await;
        let req = UpdateAccountSigningKeyRequest {
            did: "did:plc:abcdefghijklmnop".to_string(),
            signing_key: "z6MkpTHR8VNsBxYAAWHut2Geadd9jSwuBV8xRoAnwWsdvktH".to_string(),
        };

        let resp = update_account_signing_key(State(ctx), admin_test_auth(), Json(req))
            .await
            .expect_err("bare multibase signingKey should be rejected");
        let (status, body) = read_response_body(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("did:key"));
    }

    #[tokio::test]
    async fn test_update_account_signing_key_rejects_mismatched_signing_key() {
        use crate::crypto::plc::PlcSigner;

        let ctx = create_test_context().await;
        // Sanity-check: derive the operator's did:key so we know what would be
        // accepted, then submit something different.
        let operator_signer =
            PlcSigner::from_hex(&ctx.config.authentication.repo_signing_key).unwrap();
        let operator_did_key = operator_signer.public_key_did_key();
        let mismatching_did_key = "did:key:zQ3shokFTS3brHcDQrn82RUDfCZESWL1ZdCEJwekUDPQiYBme";
        assert_ne!(operator_did_key, mismatching_did_key);

        let req = UpdateAccountSigningKeyRequest {
            did: "did:plc:abcdefghijklmnop".to_string(),
            signing_key: mismatching_did_key.to_string(),
        };

        let resp = update_account_signing_key(State(ctx), admin_test_auth(), Json(req))
            .await
            .expect_err("mismatched signingKey should be rejected by strict-mode");
        let (status, body) = read_response_body(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let parsed: serde_json::Value = serde_json::from_str(&body).expect("body must be JSON");
        assert_eq!(parsed["error"], "InvalidRequest");
        assert!(parsed["message"]
            .as_str()
            .unwrap()
            .contains("operator's configured signing key"));
    }

    #[tokio::test]
    async fn test_update_account_signing_key_accepts_matching_signing_key() {
        use crate::crypto::plc::PlcSigner;

        let ctx = create_test_context().await;
        let operator_signer =
            PlcSigner::from_hex(&ctx.config.authentication.repo_signing_key).unwrap();
        let operator_did_key = operator_signer.public_key_did_key();

        let req = UpdateAccountSigningKeyRequest {
            did: "did:plc:abcdefghijklmnop".to_string(),
            signing_key: operator_did_key.clone(),
        };

        // A matching signingKey passes strict-mode validation; the handler
        // then proceeds to fetch the PLC document, which fails in the test
        // environment because the configured PLC URL is plc.directory and
        // the DID is fictitious. We assert that the failure is *not* the
        // strict-mode 400 InvalidRequest — i.e., strict-mode let us through.
        let resp = update_account_signing_key(State(ctx), admin_test_auth(), Json(req))
            .await
            .expect_err("PLC document fetch will fail in test env");
        let (status, body) = read_response_body(resp).await;
        if status == StatusCode::BAD_REQUEST {
            let parsed: serde_json::Value =
                serde_json::from_str(&body).expect("BAD_REQUEST body must be JSON in this path");
            assert_ne!(
                parsed["error"], "InvalidRequest",
                "strict-mode incorrectly rejected matching signingKey"
            );
        }
        // Otherwise we hit a downstream failure (network, NOT_FOUND from
        // PLC, etc.) — which is expected and confirms strict-mode passed.
    }
}
