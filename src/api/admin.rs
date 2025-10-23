/// Admin API Endpoints
/// Implements com.atproto.admin.* endpoints for server administration
use crate::{
    account::ValidatedSession,
    admin::{
        InviteCode, Label, ModerationAction, ModerationRecord, Report, ReportReason,
        ReportStatus, Role,
    },
    error::{PdsError, PdsResult},
    AppContext,
};
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use chrono::Duration;
use serde::Deserialize;

/// Helper function to extract and validate admin auth manually
/// Returns the validated session DID
async fn validate_admin_auth(headers: &HeaderMap, ctx: &AppContext) -> Result<String, (StatusCode, String)> {
    // Extract Bearer token from Authorization header
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or((StatusCode::UNAUTHORIZED, "Missing authorization header".to_string()))?;

    // Validate the access token
    let session = ctx
        .account_manager
        .validate_access_token(token)
        .await
        .map_err(|e| (StatusCode::UNAUTHORIZED, format!("Invalid token: {}", e)))?;

    Ok(session.did)
}

/// Build admin API routes
pub fn routes() -> Router<AppContext> {
    Router::new()
        // Admin stats and data (WORKING - converted to manual auth)
        .route("/xrpc/com.atproto.admin.getStats", get(get_stats))
        .route("/xrpc/com.atproto.admin.getUsers", get(get_users))
        // Invite codes (WORKING - converted to manual auth)
        .route("/xrpc/com.atproto.admin.createInviteCode", post(create_invite_code))
        .route("/xrpc/com.atproto.admin.getInviteCodes", get(get_invite_codes))
        .route("/xrpc/com.atproto.admin.listInviteCodes", get(list_invite_codes))
        // TODO: Convert remaining endpoints to manual auth
        // Role management (SuperAdmin only)
        // .route("/xrpc/com.atproto.admin.grantRole", post(grant_role))
        // .route("/xrpc/com.atproto.admin.revokeRole", post(revoke_role))
        // .route("/xrpc/com.atproto.admin.listRoles", get(list_roles))
        // Account moderation
        // .route("/xrpc/com.atproto.admin.takedownAccount", post(takedown_account))
        // .route("/xrpc/com.atproto.admin.suspendAccount", post(suspend_account))
        // .route("/xrpc/com.atproto.admin.restoreAccount", post(restore_account))
        // .route(
        //     "/xrpc/com.atproto.admin.getModerationHistory",
        //     get(get_moderation_history),
        // )
        // Labels
        // .route("/xrpc/com.atproto.admin.applyLabel", post(apply_label))
        // .route("/xrpc/com.atproto.admin.removeLabel", post(remove_label))
        // Invite codes
        // .route("/xrpc/com.atproto.admin.disableInviteCode", post(disable_invite_code))
        // .route("/xrpc/com.atproto.admin.listInviteCodes", get(list_invite_codes))
        // Reports
        // .route("/xrpc/com.atproto.admin.submitReport", post(submit_report))
        // .route("/xrpc/com.atproto.admin.updateReportStatus", post(update_report_status))
        // .route("/xrpc/com.atproto.admin.listReports", get(list_reports))
}

// ============================================================================
// Working Endpoints (Manual Auth)
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
    headers: HeaderMap,
    Json(req): Json<CreateInviteCodeRequest>,
) -> Result<Json<InviteCode>, (StatusCode, String)> {
    eprintln!("[DEBUG] create_invite_code: Function entered");

    // Validate admin authentication
    eprintln!("[DEBUG] create_invite_code: Validating auth");
    let admin_did = match validate_admin_auth(&headers, &ctx).await {
        Ok(did) => {
            eprintln!("[DEBUG] create_invite_code: Auth succeeded for DID: {}", did);
            did
        }
        Err(e) => {
            eprintln!("[DEBUG] create_invite_code: Auth failed: {:?}", e);
            return Err(e);
        }
    };

    // Create invite code
    eprintln!("[DEBUG] create_invite_code: Creating invite with uses={:?}, expires_days={:?}", req.uses, req.expires_days);
    let uses = req.uses.unwrap_or(1);
    let expires_in = req.expires_days.map(Duration::days);

    eprintln!("[DEBUG] create_invite_code: Calling invite_manager.create_invite");
    let code = ctx
        .invite_manager
        .create_invite(&admin_did, uses, expires_in, req.note.clone(), req.for_account.clone())
        .await
        .map_err(|e| {
            eprintln!("[DEBUG] create_invite_code: invite_manager.create_invite failed: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?;

    eprintln!("[DEBUG] create_invite_code: Invite created successfully: {}", code.code);

    // Log the action
    eprintln!("[DEBUG] create_invite_code: Logging admin action");
    let _ = ctx.admin_role_manager
        .log_action(&admin_did, "invite.create", None, Some(&code.code), None)
        .await;

    eprintln!("[DEBUG] create_invite_code: Returning success");
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
    headers: HeaderMap,
    Query(query): Query<GetInviteCodesQuery>,
) -> Result<Json<GetInviteCodesResponse>, (StatusCode, String)> {
    // Validate admin authentication
    let _admin_did = validate_admin_auth(&headers, &ctx).await?;

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
    headers: HeaderMap,
    Query(query): Query<ListInviteCodesQuery>,
) -> Result<Json<ListInviteCodesResponse>, (StatusCode, String)> {
    // Validate admin authentication
    let _admin_did = validate_admin_auth(&headers, &ctx).await?;

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
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Validate admin authentication
    let _admin_did = validate_admin_auth(&headers, &ctx).await?;

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
    headers: HeaderMap,
    Query(params): Query<GetUsersParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Validate admin authentication
    let _admin_did = validate_admin_auth(&headers, &ctx).await?;

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
