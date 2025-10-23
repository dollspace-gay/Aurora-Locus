/// Admin API Endpoints
/// Implements com.atproto.admin.* endpoints for server administration
use crate::{
    admin::{
        InviteCode, Label, ModerationAction, ModerationRecord, Report, ReportReason,
        ReportStatus, Role,
    },
    auth::AdminAuthContext,
    error::PdsResult,
    require_admin_role,
    AppContext,
};
use axum::{
    extract::{Query, State},
    routing::{get, post},
    Json, Router,
};
use chrono::Duration;
use serde::Deserialize;

/// Build admin API routes
pub fn routes() -> Router<AppContext> {
    Router::new()
        // Role management (SuperAdmin only)
        .route("/xrpc/com.atproto.admin.grantRole", post(grant_role))
        .route("/xrpc/com.atproto.admin.revokeRole", post(revoke_role))
        .route("/xrpc/com.atproto.admin.listRoles", get(list_roles))
        // Account moderation
        .route("/xrpc/com.atproto.admin.takedownAccount", post(takedown_account))
        .route("/xrpc/com.atproto.admin.suspendAccount", post(suspend_account))
        .route("/xrpc/com.atproto.admin.restoreAccount", post(restore_account))
        .route(
            "/xrpc/com.atproto.admin.getModerationHistory",
            get(get_moderation_history),
        )
        // Labels
        .route("/xrpc/com.atproto.admin.applyLabel", post(apply_label))
        .route("/xrpc/com.atproto.admin.removeLabel", post(remove_label))
        // Invite codes
        .route("/xrpc/com.atproto.admin.createInviteCode", post(create_invite_code))
        .route("/xrpc/com.atproto.admin.disableInviteCode", post(disable_invite_code))
        .route("/xrpc/com.atproto.admin.listInviteCodes", get(list_invite_codes))
        // Reports
        .route("/xrpc/com.atproto.admin.submitReport", post(submit_report))
        .route("/xrpc/com.atproto.admin.updateReportStatus", post(update_report_status))
        .route("/xrpc/com.atproto.admin.listReports", get(list_reports))
}

// ========== Role Management ==========

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GrantRoleRequest {
    did: String,
    role: String,
    notes: Option<String>,
}

async fn grant_role(
    State(ctx): State<AppContext>,
    admin: AdminAuthContext,
    Json(req): Json<GrantRoleRequest>,
) -> PdsResult<Json<serde_json::Value>> {
    require_admin_role!(admin, Role::SuperAdmin);

    let role = Role::from_str(&req.role)?;
    let admin_role = ctx
        .admin_role_manager
        .grant_role(&req.did, role, &admin.did, req.notes)
        .await?;

    ctx.admin_role_manager
        .log_action(&admin.did, "role.grant", Some(&req.did), None, None)
        .await?;

    Ok(Json(serde_json::json!({
        "did": admin_role.did,
        "role": admin_role.role.as_str(),
        "grantedAt": admin_role.granted_at,
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RevokeRoleRequest {
    did: String,
    reason: Option<String>,
}

async fn revoke_role(
    State(ctx): State<AppContext>,
    admin: AdminAuthContext,
    Json(req): Json<RevokeRoleRequest>,
) -> PdsResult<Json<()>> {
    require_admin_role!(admin, Role::SuperAdmin);

    ctx.admin_role_manager
        .revoke_role(&req.did, &admin.did, req.reason)
        .await?;

    ctx.admin_role_manager
        .log_action(&admin.did, "role.revoke", Some(&req.did), None, None)
        .await?;

    Ok(Json(()))
}

async fn list_roles(
    State(ctx): State<AppContext>,
    admin: AdminAuthContext,
) -> PdsResult<Json<serde_json::Value>> {
    require_admin_role!(admin, Role::Admin);

    let roles = ctx.admin_role_manager.list_active_roles().await?;

    Ok(Json(serde_json::json!({ "roles": roles })))
}

// ========== Account Moderation ==========

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TakedownAccountRequest {
    did: String,
    reason: String,
    notes: Option<String>,
}

async fn takedown_account(
    State(ctx): State<AppContext>,
    admin: AdminAuthContext,
    Json(req): Json<TakedownAccountRequest>,
) -> PdsResult<Json<ModerationRecord>> {
    require_admin_role!(admin, Role::Admin);

    let record = ctx
        .moderation_manager
        .apply_action(
            &req.did,
            ModerationAction::Takedown,
            &req.reason,
            &admin.did,
            None,
            None,
            req.notes,
        )
        .await?;

    ctx.admin_role_manager
        .log_action(
            &admin.did,
            "account.takedown",
            Some(&req.did),
            Some(&req.reason),
            None,
        )
        .await?;

    Ok(Json(record))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SuspendAccountRequest {
    did: String,
    reason: String,
    duration_days: Option<i64>,
    notes: Option<String>,
}

async fn suspend_account(
    State(ctx): State<AppContext>,
    admin: AdminAuthContext,
    Json(req): Json<SuspendAccountRequest>,
) -> PdsResult<Json<ModerationRecord>> {
    require_admin_role!(admin, Role::Admin);

    let expires_in = req.duration_days.map(Duration::days);

    let record = ctx
        .moderation_manager
        .apply_action(
            &req.did,
            ModerationAction::Suspend,
            &req.reason,
            &admin.did,
            expires_in,
            None,
            req.notes,
        )
        .await?;

    ctx.admin_role_manager
        .log_action(
            &admin.did,
            "account.suspend",
            Some(&req.did),
            Some(&req.reason),
            None,
        )
        .await?;

    Ok(Json(record))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RestoreAccountRequest {
    moderation_id: i64,
    reason: String,
}

async fn restore_account(
    State(ctx): State<AppContext>,
    admin: AdminAuthContext,
    Json(req): Json<RestoreAccountRequest>,
) -> PdsResult<Json<()>> {
    require_admin_role!(admin, Role::Admin);

    ctx.moderation_manager
        .reverse_action(req.moderation_id, &admin.did, &req.reason)
        .await?;

    ctx.admin_role_manager
        .log_action(&admin.did, "account.restore", None, Some(&req.reason), None)
        .await?;

    Ok(Json(()))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetModerationHistoryQuery {
    did: String,
}

async fn get_moderation_history(
    State(ctx): State<AppContext>,
    _admin: AdminAuthContext,
    Query(params): Query<GetModerationHistoryQuery>,
) -> PdsResult<Json<serde_json::Value>> {
    let history = ctx.moderation_manager.get_history(&params.did).await?;

    Ok(Json(serde_json::json!({ "history": history })))
}

// ========== Labels ==========

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApplyLabelRequest {
    uri: String,
    cid: Option<String>,
    val: String,
    expires_days: Option<i64>,
}

async fn apply_label(
    State(ctx): State<AppContext>,
    admin: AdminAuthContext,
    Json(req): Json<ApplyLabelRequest>,
) -> PdsResult<Json<Label>> {
    require_admin_role!(admin, Role::Moderator);

    let expires_in = req.expires_days.map(Duration::days);

    let label = ctx
        .label_manager
        .apply_label(&req.uri, req.cid.as_deref(), &req.val, &admin.did, expires_in)
        .await?;

    ctx.admin_role_manager
        .log_action(&admin.did, "label.apply", None, Some(&req.uri), None)
        .await?;

    Ok(Json(label))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoveLabelRequest {
    uri: String,
    cid: Option<String>,
    val: String,
}

async fn remove_label(
    State(ctx): State<AppContext>,
    admin: AdminAuthContext,
    Json(req): Json<RemoveLabelRequest>,
) -> PdsResult<Json<Label>> {
    require_admin_role!(admin, Role::Moderator);

    let label = ctx
        .label_manager
        .remove_label(&req.uri, req.cid.as_deref(), &req.val, &admin.did)
        .await?;

    ctx.admin_role_manager
        .log_action(&admin.did, "label.remove", None, Some(&req.uri), None)
        .await?;

    Ok(Json(label))
}

// ========== Invite Codes ==========

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateInviteCodeRequest {
    uses: Option<i32>,
    expires_days: Option<i64>,
    note: Option<String>,
    for_account: Option<String>,
}

async fn create_invite_code(
    State(ctx): State<AppContext>,
    admin: AdminAuthContext,
    Json(req): Json<CreateInviteCodeRequest>,
) -> PdsResult<Json<InviteCode>> {
    require_admin_role!(admin, Role::Admin);

    let uses = req.uses.unwrap_or(1);
    let expires_in = req.expires_days.map(Duration::days);

    let code = ctx
        .invite_manager
        .create_invite(&admin.did, uses, expires_in, req.note, req.for_account)
        .await?;

    ctx.admin_role_manager
        .log_action(&admin.did, "invite.create", None, Some(&code.code), None)
        .await?;

    Ok(Json(code))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DisableInviteCodeRequest {
    code: String,
}

async fn disable_invite_code(
    State(ctx): State<AppContext>,
    admin: AdminAuthContext,
    Json(req): Json<DisableInviteCodeRequest>,
) -> PdsResult<Json<()>> {
    require_admin_role!(admin, Role::Admin);

    ctx.invite_manager.disable_code(&req.code).await?;

    ctx.admin_role_manager
        .log_action(&admin.did, "invite.disable", None, Some(&req.code), None)
        .await?;

    Ok(Json(()))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListInviteCodesQuery {
    include_disabled: Option<bool>,
}

async fn list_invite_codes(
    State(ctx): State<AppContext>,
    _admin: AdminAuthContext,
    Query(params): Query<ListInviteCodesQuery>,
) -> PdsResult<Json<serde_json::Value>> {
    let include_disabled = params.include_disabled.unwrap_or(false);
    let codes = ctx.invite_manager.list_codes(include_disabled).await?;

    Ok(Json(serde_json::json!({ "codes": codes })))
}

// ========== Reports ==========

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubmitReportRequest {
    subject_did: Option<String>,
    subject_uri: Option<String>,
    subject_cid: Option<String>,
    reason_type: String,
    reason: Option<String>,
}

async fn submit_report(
    State(ctx): State<AppContext>,
    auth: crate::auth::AuthContext,
    Json(req): Json<SubmitReportRequest>,
) -> PdsResult<Json<Report>> {
    let reason_type = ReportReason::from_str(&req.reason_type)?;

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
        .await?;

    Ok(Json(report))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateReportStatusRequest {
    report_id: i64,
    status: String,
    resolution: Option<String>,
}

async fn update_report_status(
    State(ctx): State<AppContext>,
    admin: AdminAuthContext,
    Json(req): Json<UpdateReportStatusRequest>,
) -> PdsResult<Json<()>> {
    require_admin_role!(admin, Role::Moderator);

    let status = ReportStatus::from_str(&req.status)?;

    ctx.report_manager
        .update_status(req.report_id, status, &admin.did, req.resolution.as_deref())
        .await?;

    ctx.admin_role_manager
        .log_action(
            &admin.did,
            "report.update",
            None,
            Some(&format!("Report #{}", req.report_id)),
            None,
        )
        .await?;

    Ok(Json(()))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListReportsQuery {
    status: Option<String>,
    limit: Option<i64>,
}

async fn list_reports(
    State(ctx): State<AppContext>,
    _admin: AdminAuthContext,
    Query(params): Query<ListReportsQuery>,
) -> PdsResult<Json<serde_json::Value>> {
    let status = if let Some(status_str) = params.status {
        Some(ReportStatus::from_str(&status_str)?)
    } else {
        None
    };

    let reports = ctx.report_manager.list_reports(status, params.limit).await?;

    Ok(Json(serde_json::json!({ "reports": reports })))
}
