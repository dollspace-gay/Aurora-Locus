/// Admin API Endpoints
/// Implements com.atproto.admin.* endpoints for server administration
use crate::{
    admin::InviteCode,
    auth::AdminAuthContext,
    AppContext,
};
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
        .route("/xrpc/com.atproto.admin.updateSubjectStatus", post(update_subject_status))
        // Invite codes
        .route("/xrpc/com.atproto.admin.createInviteCode", post(create_invite_code))
        .route("/xrpc/com.atproto.admin.getInviteCodes", get(get_invite_codes))
        .route("/xrpc/com.atproto.admin.listInviteCodes", get(list_invite_codes))
        .route("/xrpc/com.atproto.admin.disableInviteCode", post(disable_invite_code))
        // Role management
        .route("/xrpc/com.atproto.admin.grantRole", post(grant_role))
        .route("/xrpc/com.atproto.admin.revokeRole", post(revoke_role))
        .route("/xrpc/com.atproto.admin.listRoles", get(list_roles))
        // Account moderation
        .route("/xrpc/com.atproto.admin.takedownAccount", post(takedown_account))
        .route("/xrpc/com.atproto.admin.suspendAccount", post(suspend_account))
        .route("/xrpc/com.atproto.admin.restoreAccount", post(restore_account))
        .route("/xrpc/com.atproto.admin.getModerationHistory", get(get_moderation_history))
        .route("/xrpc/com.atproto.admin.getModerationQueue", get(get_moderation_queue))
        // Labels
        .route("/xrpc/com.atproto.admin.applyLabel", post(apply_label))
        .route("/xrpc/com.atproto.admin.removeLabel", post(remove_label))
        // Reports
        .route("/xrpc/com.atproto.admin.submitReport", post(submit_report))
        .route("/xrpc/com.atproto.admin.updateReportStatus", post(update_report_status))
        .route("/xrpc/com.atproto.admin.listReports", get(list_reports))
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
        .create_invite(&auth.did, uses, expires_in, req.note.clone(), req.for_account.clone())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Log the action
    let _ = ctx.admin_role_manager
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
    limit: Option<i64>,
    #[serde(default)]
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

    Ok(Json(ListInviteCodesResponse { codes, cursor: None }))
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

    let active_sessions: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM session WHERE expires_at > datetime('now')"
    )
    .fetch_one(&ctx.account_db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let pending_reports: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM report WHERE status = 'open'"
    )
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

    let users: Vec<serde_json::Value> = if let Some(cursor) = params.cursor {
        sqlx::query_as::<_, (String, String, Option<String>, String, String)>(
            "SELECT did, handle, email, created_at, status FROM account WHERE did > ? ORDER BY did LIMIT ?"
        )
        .bind(cursor)
        .bind(limit)
        .fetch_all(&ctx.account_db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    } else {
        sqlx::query_as::<_, (String, String, Option<String>, String, String)>(
            "SELECT did, handle, email, created_at, status FROM account ORDER BY did LIMIT ?"
        )
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

    let cursor = users.last().and_then(|u| u.get("did")).and_then(|d| d.as_str());

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
    let role = Role::from_str(&req.role)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    // Grant role
    let admin_role = ctx.admin_role_manager
        .grant_role(&req.did, role, &auth.did, None)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Log action
    let _ = ctx.admin_role_manager
        .log_action(&auth.did, "role.grant", Some(&req.did), Some(&req.role), None)
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
    let _ = ctx.admin_role_manager
        .log_action(&auth.did, "role.revoke", Some(&req.did), req.reason.as_deref(), None)
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
        let role_record = ctx.admin_role_manager
            .get_role(&did)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        Ok(Json(serde_json::json!({
            "did": did,
            "role": role_record,
        })))
    } else {
        // List all active role assignments
        let assignments = ctx.admin_role_manager
            .list_active_roles()
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        Ok(Json(serde_json::json!({
            "roles": assignments,
        })))
    }
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
    use crate::admin::moderation::ModerationAction;

    // Apply takedown action
    let record = ctx.moderation_manager
        .apply_action(
            &req.did,
            ModerationAction::Takedown,
            &req.reason,
            &auth.did,
            None,
            None,
            req.notes.clone(),
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Log action
    let _ = ctx.admin_role_manager
        .log_action(&auth.did, "account.takedown", Some(&req.did), Some(&req.reason), None)
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
    use crate::admin::moderation::ModerationAction;

    let expires_in = req.duration_days.map(Duration::days);

    // Apply suspension
    let record = ctx.moderation_manager
        .apply_action(
            &req.did,
            ModerationAction::Suspend,
            &req.reason,
            &auth.did,
            expires_in,
            None,
            req.notes.clone(),
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Log action
    let _ = ctx.admin_role_manager
        .log_action(&auth.did, "account.suspend", Some(&req.did), Some(&req.reason), None)
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
    let _ = ctx.admin_role_manager
        .log_action(&auth.did, "account.restore", Some(&req.did), Some(&req.reason), None)
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
    let history = ctx.moderation_manager
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

    let label = ctx.label_manager
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
    let _ = ctx.admin_role_manager
        .log_action(&auth.did, "label.apply", None, Some(&req.val), Some(&req.uri))
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
    let label = ctx.label_manager
        .remove_label(
            &req.uri,
            req.cid.as_deref(),
            &req.val,
            &auth.did,
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Log action
    let _ = ctx.admin_role_manager
        .log_action(&auth.did, "label.remove", None, Some(&req.val), Some(&req.uri))
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
    let reason_type = ReportReason::from_str(&req.reason_type)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    // Submit report
    let report = ctx.report_manager
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
    let status = ReportStatus::from_str(&req.status)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    // Update status
    ctx.report_manager
        .update_status(
            req.report_id,
            status,
            &auth.did,
            req.resolution.as_deref(),
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Log action
    let _ = ctx.admin_role_manager
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
        Some(ReportStatus::from_str(&status_str)
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?)
    } else {
        None
    };

    // List reports
    let reports = ctx.report_manager
        .list_reports(status_filter, query.limit)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "reports": reports,
    })))
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
    let account = ctx.account_manager
        .get_account(&query.did)
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, format!("Account not found: {}", e)))?;

    Ok(Json(serde_json::json!({
        "did": account.did,
        "handle": account.handle,
        "email": account.email,
        "created_at": account.created_at,
        "email_confirmed": account.email_confirmed,
        "takedown": account.taken_down,
    })))
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
    use crate::admin::moderation::ModerationAction;

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
        return Err((StatusCode::BAD_REQUEST, "Invalid subject format".to_string()));
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
        .apply_action(
            &did,
            action,
            &reason,
            &auth.did,
            duration,
            None,
            None,
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "success": true,
        "did": did,
        "action": req.action,
    })))
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
    let reports = ctx.report_manager
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
