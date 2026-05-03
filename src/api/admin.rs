/// Admin API Endpoints
/// Implements com.atproto.admin.* endpoints for server administration
use crate::{
    admin::InviteCode,
    auth::AdminAuthContext,
    error::{PdsError, PdsResult},
    AppContext,
};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use chrono::Duration;
use serde::{Deserialize, Serialize};

/// Build admin API routes.
///
/// Two namespaces are mounted here:
///
/// - `com.atproto.admin.*` — moderation/admin-tier endpoints. After
///   Phase 2.4 (chainlink #85) this surface is exactly the
///   bsky-PDS-2025-Q1 parity baseline plus the parity gaps closed in
///   Phase 1; operator/infrastructure endpoints have been removed.
///
/// - `tools.aurora.ops.*` — operator/infrastructure tier (chainlink #84).
///   30 relocated endpoints from the legacy admin namespace plus 2
///   net-new ones (`listAccounts`, `getInstanceMetrics`). Scope-checked
///   to `atproto:admin.server` via the namespace middleware (e9b66b9).
///
/// `listRecentEvents` intentionally stays at `com.atproto.admin.*` —
/// moderation-flavored stream review, not operator infrastructure. It
/// will likely move under `tools.aurora.moderator.*` when admin/mod
/// Phase 3 lands.
pub fn routes() -> Router<AppContext> {
    Router::new()
        // ---- com.atproto.admin.* (moderation/admin tier) ----

        // Account read
        .route("/xrpc/com.atproto.admin.getUsers", get(get_users))
        // listAccounts here is the bsky-PDS-compat alias to getUsers; the
        // operator-flavored listAccounts (broader filters) lives at
        // /xrpc/tools.aurora.ops.listAccounts.
        .route("/xrpc/com.atproto.admin.listAccounts", get(get_users))
        .route("/xrpc/com.atproto.admin.getAccount", get(get_account))
        .route(
            "/xrpc/com.atproto.admin.searchAccounts",
            get(search_accounts),
        )
        .route(
            "/xrpc/com.atproto.admin.getAccountInfo",
            get(get_account_info),
        )
        .route(
            "/xrpc/com.atproto.admin.getAccountInfos",
            get(get_account_infos),
        )
        // Subject status (cross-cutting moderation surface)
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
            "/xrpc/com.atproto.admin.disableInviteCodes",
            post(disable_invite_codes),
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
        // grantRole and revokeRole relocated to tools.aurora.superadmin.*
        // in Phase 3.6 (chainlink #103). listRoles stays at the
        // moderation tier — moderators may legitimately need to see
        // who has what role without being SuperAdmin themselves.
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
        // Sequencer event review (moderation-flavored; ops controls live
        // at tools.aurora.ops.{getSequencerStatus,pauseSequencer,...}).
        .route(
            "/xrpc/com.atproto.admin.listRecentEvents",
            get(list_recent_events),
        )
        // ---- tools.aurora.* top-level (chainlink #99 / Phase 3.2) ----
        //
        // Capability probe — clients call this to discover which
        // Aurora extensions this instance supports without trial-
        // and-error against individual endpoints.
        .route(
            "/xrpc/tools.aurora.describeCapabilities",
            get(describe_capabilities),
        )
        // ---- tools.aurora.ops.* (operator / infrastructure tier) ----
        //
        // Stats and account-listing.
        .route("/xrpc/tools.aurora.ops.getStats", get(get_stats))
        .route(
            "/xrpc/tools.aurora.ops.listAccounts",
            get(ops_list_accounts),
        )
        .route(
            "/xrpc/tools.aurora.ops.getInstanceMetrics",
            get(ops_get_instance_metrics),
        )
        // Health, metrics, validation, nonce store.
        .route(
            "/xrpc/tools.aurora.ops.getValidationFailures",
            get(get_validation_failures),
        )
        .route(
            "/xrpc/tools.aurora.ops.getSystemHealth",
            get(get_system_health),
        )
        .route(
            "/xrpc/tools.aurora.ops.getDatabaseStatus",
            get(get_database_status),
        )
        .route(
            "/xrpc/tools.aurora.ops.getResourceUsage",
            get(get_resource_usage),
        )
        .route(
            "/xrpc/tools.aurora.ops.listBackgroundJobs",
            get(list_background_jobs),
        )
        .route(
            "/xrpc/tools.aurora.ops.runHealthChecks",
            get(run_health_checks),
        )
        .route(
            "/xrpc/tools.aurora.ops.getVersionInfo",
            get(get_version_info),
        )
        .route(
            "/xrpc/tools.aurora.ops.getSystemMetrics",
            get(get_system_metrics),
        )
        .route(
            "/xrpc/tools.aurora.ops.getNonceStoreStatus",
            get(get_nonce_store_status),
        )
        .route(
            "/xrpc/tools.aurora.ops.cleanupNonceStores",
            post(cleanup_nonce_stores),
        )
        // Blob storage.
        .route(
            "/xrpc/tools.aurora.ops.getBlobStatistics",
            get(get_blob_statistics),
        )
        .route("/xrpc/tools.aurora.ops.listBlobs", get(list_blobs))
        .route("/xrpc/tools.aurora.ops.deleteBlob", post(delete_blob))
        .route(
            "/xrpc/tools.aurora.ops.quarantineBlob",
            post(quarantine_blob),
        )
        .route("/xrpc/tools.aurora.ops.restoreBlob", post(restore_blob))
        .route("/xrpc/tools.aurora.ops.runBlobGC", post(run_blob_gc))
        .route(
            "/xrpc/tools.aurora.ops.getBlobQuotas",
            get(get_blob_quotas),
        )
        // Sequencer infrastructure.
        .route(
            "/xrpc/tools.aurora.ops.getSequencerStatus",
            get(get_sequencer_status),
        )
        .route(
            "/xrpc/tools.aurora.ops.pauseSequencer",
            post(pause_sequencer),
        )
        .route(
            "/xrpc/tools.aurora.ops.resumeSequencer",
            post(resume_sequencer),
        )
        .route(
            "/xrpc/tools.aurora.ops.resetSequencerCursor",
            post(reset_sequencer_cursor),
        )
        .route(
            "/xrpc/tools.aurora.ops.rebuildSequencer",
            post(rebuild_sequencer),
        )
        // Rate limiting.
        .route(
            "/xrpc/tools.aurora.ops.getRateLimitConfig",
            get(get_rate_limit_config),
        )
        .route(
            "/xrpc/tools.aurora.ops.getRateLimitStatus",
            get(get_rate_limit_status),
        )
        .route(
            "/xrpc/tools.aurora.ops.cleanupRateLimitState",
            post(cleanup_rate_limit_state),
        )
        // Federation / relay.
        .route(
            "/xrpc/tools.aurora.ops.getFederationStatus",
            get(get_federation_status),
        )
        .route(
            "/xrpc/tools.aurora.ops.getRelayConfig",
            get(get_relay_config),
        )
        .route(
            "/xrpc/tools.aurora.ops.listKnownInstances",
            get(list_known_instances),
        )
        .route(
            "/xrpc/tools.aurora.ops.triggerPdsDiscovery",
            post(trigger_pds_discovery),
        )
        // ---- tools.aurora.moderator.* (chainlink #100 / Phase 3.3) ----
        //
        // Moderator-tier read endpoints. Five queries with shared
        // rich-context infrastructure (resolve_handles, etc.) in
        // src/api/aurora_moderator.rs. Auth: AdminAuthContext
        // (Moderator+); namespace middleware also gates
        // tools.aurora.moderator.* to atproto:admin.moderation.
        .route(
            "/xrpc/tools.aurora.moderator.queryEvents",
            get(crate::api::aurora_moderator::query_events),
        )
        .route(
            "/xrpc/tools.aurora.moderator.getEvent",
            get(crate::api::aurora_moderator::get_event),
        )
        .route(
            "/xrpc/tools.aurora.moderator.queryStatuses",
            get(crate::api::aurora_moderator::query_statuses),
        )
        .route(
            "/xrpc/tools.aurora.moderator.getSubjectContext",
            get(crate::api::aurora_moderator::get_subject_context),
        )
        .route(
            "/xrpc/tools.aurora.moderator.getSubjectHistory",
            get(crate::api::aurora_moderator::get_subject_history),
        )
        // ---- tools.aurora.moderator.* appeals reads (chainlink #101 / Phase 3.4) ----
        //
        // Two endpoints reusing 3.3's foundation types and rich-context
        // helpers (resolve_handles + new fetch_action_summaries batch
        // lookup). Auth: same AdminAuthContext + namespace scope as
        // the other moderator-tier endpoints.
        .route(
            "/xrpc/tools.aurora.moderator.listAppeals",
            get(crate::api::aurora_moderator::list_appeals),
        )
        .route(
            "/xrpc/tools.aurora.moderator.getAppeal",
            get(crate::api::aurora_moderator::get_appeal),
        )
        // ---- tools.aurora.admin.* (chainlink #102 / Phase 3.5) ----
        //
        // Admin-tier action surface. emitEvent is the unified dispatch
        // for moderation actions per AURORA_ADMIN_UI_DESIGN.md §8.1;
        // per-action endpoints under com.atproto.admin.* stay live
        // for protocol-compatibility but the UI consumes emitEvent
        // exclusively post-3.5 (§9.2).
        //
        // Auth: AdminModeration scope (namespace middleware); within-
        // tier role checks happen at handler level (Moderator+ for
        // content actions, Admin+ for account-infrastructure actions).
        .route(
            "/xrpc/tools.aurora.admin.emitEvent",
            post(crate::api::aurora_admin::emit_event),
        )
        // ---- tools.aurora.superadmin.* (chainlink #103 / Phase 3.6) ----
        //
        // Role management relocated from com.atproto.admin.* per design
        // doc §5.4. SuperAdmin scope check is enforced at handler level
        // (auth.role.can_act_as(Role::SuperAdmin)) — the namespace
        // alone doesn't gate this; the handler does. Per pre-deployment
        // framing, no deprecation aliases — clean wire-break.
        .route(
            "/xrpc/tools.aurora.superadmin.grantRole",
            post(grant_role),
        )
        .route(
            "/xrpc/tools.aurora.superadmin.revokeRole",
            post(revoke_role),
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
    /// `recent` (default) or `usage` per the lexicon's knownValues.
    #[serde(default)]
    sort: Option<String>,
    /// Page size, 1-500, default 100 per the lexicon.
    #[serde(default)]
    limit: Option<i64>,
    /// Opaque cursor produced by a previous response.
    #[serde(default)]
    cursor: Option<String>,
}

#[derive(Debug, serde::Serialize)]
struct GetInviteCodesResponse {
    codes: Vec<InviteCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor: Option<String>,
}

/// Encode a typed cursor as base64url-no-pad JSON.
fn encode_invite_cursor(cursor: &crate::admin::invites::InviteCursor) -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let json = serde_json::to_vec(cursor).expect("cursor enum is JSON-serialisable");
    URL_SAFE_NO_PAD.encode(json)
}

/// Decode a base64url-no-pad cursor, returning a 400 with `InvalidRequest`
/// shape if the cursor is malformed or the decoded sort doesn't match the
/// request's sort.
fn decode_invite_cursor(
    raw: &str,
    expected_sort: crate::admin::invites::InviteSortKey,
) -> Result<crate::admin::invites::InviteCursor, (StatusCode, String)> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let json = URL_SAFE_NO_PAD
        .decode(raw)
        .map_err(|_| (StatusCode::BAD_REQUEST, "Malformed cursor".to_string()))?;
    let cursor: crate::admin::invites::InviteCursor = serde_json::from_slice(&json)
        .map_err(|_| (StatusCode::BAD_REQUEST, "Malformed cursor".to_string()))?;
    if cursor.sort_key() != expected_sort {
        return Err((
            StatusCode::BAD_REQUEST,
            "cursor was issued for a different sort ordering".to_string(),
        ));
    }
    Ok(cursor)
}

/// Build a paginated invite-code response from a `Vec<(InviteCode, i64)>`
/// returned by the manager. Trims to `limit` and emits a cursor if more
/// results were available.
fn paginated_invite_response(
    mut rows: Vec<(InviteCode, i64)>,
    sort: crate::admin::invites::InviteSortKey,
    limit: i64,
) -> (Vec<InviteCode>, Option<String>) {
    use crate::admin::invites::InviteCursor;
    let has_more = rows.len() as i64 > limit;
    rows.truncate(limit as usize);
    let next_cursor = if has_more {
        rows.last().map(|(code, use_count)| {
            let cur = match sort {
                crate::admin::invites::InviteSortKey::Recent => InviteCursor::Recent {
                    after_created_at: code.created_at.to_rfc3339(),
                    after_code: code.code.clone(),
                },
                crate::admin::invites::InviteSortKey::Usage => InviteCursor::Usage {
                    after_use_count: *use_count,
                    after_code: code.code.clone(),
                },
            };
            encode_invite_cursor(&cur)
        })
    } else {
        None
    };
    (rows.into_iter().map(|(c, _)| c).collect(), next_cursor)
}

/// Get an admin view of invite codes (lexicon `com.atproto.admin.getInviteCodes`).
///
/// Phase 1.10 (#65) wired up the lexicon's sort/limit/cursor parameters
/// and removed the legacy `includeDisabled` parameter. Disabled-only
/// filtering relocates to a `tools.aurora.ops.*` endpoint per the
/// assessment doc Phase 2.
async fn get_invite_codes(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(query): Query<GetInviteCodesQuery>,
) -> Result<Json<GetInviteCodesResponse>, (StatusCode, String)> {
    let sort = crate::admin::invites::InviteSortKey::from_param(query.sort.as_deref())
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let limit = query.limit.unwrap_or(100);
    if !(1..=500).contains(&limit) {
        return Err((
            StatusCode::BAD_REQUEST,
            "limit must be between 1 and 500".to_string(),
        ));
    }
    let cursor = match query.cursor.as_deref() {
        Some(raw) => Some(decode_invite_cursor(raw, sort)?),
        None => None,
    };

    let rows = ctx
        .invite_manager
        .list_codes_paginated(sort, cursor.as_ref(), limit + 1)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let (codes, cursor) = paginated_invite_response(rows, sort, limit);
    Ok(Json(GetInviteCodesResponse { codes, cursor }))
}

#[derive(Debug, Deserialize)]
struct ListInviteCodesQuery {
    #[serde(default)]
    sort: Option<String>,
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

/// List invite codes (Aurora-Locus surface paralleling `getInviteCodes`).
///
/// Phase 1.10 (#65) wired the limit/cursor params that were previously
/// accepted-and-ignored. Reuses `getInviteCodes`'s pagination machinery.
async fn list_invite_codes(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(query): Query<ListInviteCodesQuery>,
) -> Result<Json<ListInviteCodesResponse>, (StatusCode, String)> {
    let sort = crate::admin::invites::InviteSortKey::from_param(query.sort.as_deref())
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let limit = query.limit.unwrap_or(100);
    if !(1..=500).contains(&limit) {
        return Err((
            StatusCode::BAD_REQUEST,
            "limit must be between 1 and 500".to_string(),
        ));
    }
    let cursor = match query.cursor.as_deref() {
        Some(raw) => Some(decode_invite_cursor(raw, sort)?),
        None => None,
    };

    let rows = ctx
        .invite_manager
        .list_codes_paginated(sort, cursor.as_ref(), limit + 1)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let (codes, cursor) = paginated_invite_response(rows, sort, limit);
    Ok(Json(ListInviteCodesResponse { codes, cursor }))
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

    // SuperAdmin only — relocated to tools.aurora.superadmin.* in
    // Phase 3.6 (chainlink #103). Per design doc §5.4, role
    // management is structurally a SuperAdmin operation; the
    // namespace makes that boundary visible, this guard enforces it.
    if !auth.role.can_act_as(Role::SuperAdmin) {
        return Err((
            StatusCode::FORBIDDEN,
            format!(
                "grantRole requires SuperAdmin role; have {}",
                auth.role.as_str()
            ),
        ));
    }

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
    use crate::admin::roles::Role;

    // SuperAdmin only — same rationale as grant_role above
    // (chainlink #103 / Phase 3.6).
    if !auth.role.can_act_as(Role::SuperAdmin) {
        return Err((
            StatusCode::FORBIDDEN,
            format!(
                "revokeRole requires SuperAdmin role; have {}",
                auth.role.as_str()
            ),
        ));
    }

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

/// Resolve the spec's `account` (at-identifier) field and Aurora's legacy
/// `did` field down to a canonical DID. Established by Phase 1.7 (chainlink
/// #62) and reused across the deprecation-alias rollout.
///
/// Behavior:
/// - exactly-one validation: providing both or neither returns 400
/// - `account`: if DID-form, returned as-is; if handle-form, resolved via
///   the local actor table (no external DNS/.well-known resolution, which
///   would be wrong for admin operations on local users)
/// - `did` (legacy): DID-form only, retains the historical behavior
///
/// Note: spec for `disableAccountInvites` and `enableAccountInvites` declares
/// `account` as `format=did`, while `updateAccountEmail` declares it as
/// `format=at-identifier`. This helper uniformly accepts either form on the
/// `account` field — spec-compliant clients that only ever pass DID still
/// work; operators that pass handles to the invites endpoints get a more
/// permissive (non-rejecting) experience than strict spec.
async fn resolve_account_or_did(
    ctx: &AppContext,
    account: Option<&str>,
    did: Option<&str>,
) -> Result<String, (StatusCode, String)> {
    match (account, did) {
        (Some(_), Some(_)) => Err((
            StatusCode::BAD_REQUEST,
            "Provide exactly one of `account` or `did` (legacy)".to_string(),
        )),
        (None, None) => Err((
            StatusCode::BAD_REQUEST,
            "Missing required field: `account`".to_string(),
        )),
        (Some(at_id), None) => ctx
            .account_manager
            .resolve_at_identifier_to_did(at_id)
            .await
            .map_err(|e| {
                if matches!(e, PdsError::NotFound(_)) {
                    (
                        StatusCode::NOT_FOUND,
                        format!("Account not found for identifier: {}", at_id),
                    )
                } else {
                    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
                }
            }),
        (None, Some(did_str)) => {
            if !did_str.starts_with("did:") {
                return Err((StatusCode::BAD_REQUEST, "Invalid DID format".to_string()));
            }
            Ok(did_str.to_string())
        }
    }
}

#[derive(Deserialize)]
struct UpdateAccountEmailRequest {
    /// Account at-identifier (handle or DID) per the lexicon. Required if
    /// the legacy `did` field is not provided.
    #[serde(default)]
    account: Option<String>,
    /// DEPRECATED: legacy `did` field retained for back-compat. Use
    /// `account` instead. Continues to accept DID-form only. To be
    /// removed in a later minor version.
    #[serde(default)]
    did: Option<String>,
    /// New email address
    email: String,
}

/// Update account email address
async fn update_account_email(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Json(req): Json<UpdateAccountEmailRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let canonical_did =
        resolve_account_or_did(&ctx, req.account.as_deref(), req.did.as_deref()).await?;

    if !req.email.contains('@') || req.email.len() < 5 {
        return Err((StatusCode::BAD_REQUEST, "Invalid email format".to_string()));
    }

    ctx.account_manager
        .update_email(&canonical_did, &req.email)
        .await
        .map_err(|e| {
            if matches!(e, PdsError::NotFound(_)) {
                (
                    StatusCode::NOT_FOUND,
                    format!("Account not found: {}", canonical_did),
                )
            } else if matches!(e, PdsError::Validation(_)) {
                (StatusCode::CONFLICT, e.to_string())
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            }
        })?;

    Ok(StatusCode::OK)
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
) -> Result<StatusCode, (StatusCode, String)> {
    // Validate DID format
    if !req.did.starts_with("did:") {
        return Err((StatusCode::BAD_REQUEST, "Invalid DID format".to_string()));
    }

    // Validate handle format (basic check)
    if req.handle.is_empty() || req.handle.len() > 253 {
        return Err((StatusCode::BAD_REQUEST, "Invalid handle format".to_string()));
    }

    ctx.account_manager
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

    Ok(StatusCode::OK)
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
) -> Result<StatusCode, (StatusCode, String)> {
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

    Ok(StatusCode::OK)
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
) -> Result<StatusCode, (StatusCode, String)> {
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

    Ok(StatusCode::OK)
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

/// Per spec: `subject` is optional, `senderDid` is required. Aurora retains
/// a permissive extension allowing `senderDid` to be omitted (defaults to
/// the authenticated admin's DID). Spec-compliant callers passing both
/// fields work unchanged.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendEmailRequest {
    /// DID of the recipient (spec-required).
    recipient_did: String,
    /// Email body content (spec-required).
    content: String,
    /// Optional email subject. Phase 1.8 (#63) flipped this from required
    /// to optional to match the lexicon. When omitted, a placeholder
    /// subject is used at the SMTP layer.
    #[serde(default)]
    subject: Option<String>,
    /// Aurora-permissive extension: spec marks `senderDid` as required, but
    /// Aurora defaults to the authenticated admin's DID when omitted.
    /// Spec-compliant callers pass an explicit value.
    #[serde(default)]
    sender_did: Option<String>,
    /// Optional sender comment used for audit context (spec-optional).
    #[serde(default)]
    comment: Option<String>,
}

/// Send email response per ATProto spec
#[derive(Debug, serde::Serialize)]
struct SendEmailResponse {
    sent: bool,
}

/// Default subject line used when the spec-optional `subject` field is omitted.
/// `send_admin_email` needs a non-empty string for the SMTP `Subject:` header;
/// "(no subject)" matches the conventional MUA fallback.
const DEFAULT_EMPTY_SUBJECT: &str = "(no subject)";

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

    let effective_subject = req.subject.as_deref().unwrap_or(DEFAULT_EMPTY_SUBJECT);

    // Send the email
    ctx.mailer
        .send_admin_email(&to_email, effective_subject, &req.content)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to send email: {}", e),
            )
        })?;

    // Log the action. Aurora's permissive extension: when senderDid is
    // omitted, attribute the action to the authenticated admin.
    let sender = req.sender_did.as_deref().unwrap_or(&auth.did);
    let _ = ctx
        .admin_role_manager
        .log_action(
            sender,
            "email.send",
            Some(&req.recipient_did),
            req.comment.as_deref(),
            req.subject.as_deref(),
        )
        .await;

    tracing::info!(
        "Admin {} sent email to {} ({}): {}",
        auth.did,
        req.recipient_did,
        to_email,
        effective_subject
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
    /// DIDs to look up. Decoded from repeated `?dids=...&dids=...` query
    /// parameters via `axum_extra::extract::Query`. Phase 1.9 (#64) replaced
    /// the legacy comma-separated single-string encoding with the
    /// lexicon-conformant repeated-param form; behavior change is documented
    /// in the commit that introduced this struct.
    dids: Vec<String>,
}

/// Account info for batch responses (lexicon `com.atproto.admin.defs#accountView`).
///
/// `handle` is required per the lexicon. Phase 1.9 (#64) flipped it from
/// `Option<String>` to `String`; the underlying `actor.handle` column is
/// `NOT NULL` in the schema, so the backing data is always present.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountInfo {
    did: String,
    handle: String,
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
#[derive(Debug, serde::Serialize)]
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
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct InviteCodeUse {
    used_by: String,
    used_at: String,
}

/// Threat signature (for future anti-spam/abuse detection)
#[derive(Debug, serde::Serialize)]
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

/// Build an `AccountInfo` (lexicon `accountView`) for a single DID.
///
/// Shared helper for `get_account_info` (singular) and `get_account_infos`
/// (plural). Returns `PdsError::NotFound` when the account does not exist;
/// callers map that to 404 / `RepoNotFound` for the singular endpoint or to
/// silent skip for the plural endpoint.
///
/// Future shape fixes tracked in chainlink #64 (Phase 1.9 — getAccountInfos
/// param encoding + handle field) will land in this single helper and
/// propagate to both endpoints simultaneously.
async fn build_account_info(ctx: &AppContext, did: &str) -> PdsResult<AccountInfo> {
    let account = ctx.account_manager.get_account(did).await?;

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
            uses: vec![],
        });

    let account_invites = ctx
        .invite_manager
        .get_codes_created_by(did)
        .await
        .unwrap_or_default();

    let invites: Vec<InviteCodeInfo> = account_invites
        .into_iter()
        .map(|inv| InviteCodeInfo {
            code: inv.code.clone(),
            available: inv.available,
            disabled: inv.disabled,
            for_account: inv.for_account.clone().unwrap_or_default(),
            created_by: inv.created_by.clone(),
            created_at: inv.created_at.to_rfc3339(),
            uses: vec![],
        })
        .collect();

    Ok(AccountInfo {
        did: account.did.clone(),
        // `actor.handle` is NOT NULL in schema; the Option on ActorAccount is
        // Rust-side defensiveness. Default to empty string only as a
        // belt-and-suspenders fallback for a row that violates the invariant.
        handle: account.handle.clone().unwrap_or_default(),
        email: account.email.clone(),
        indexed_at: account.created_at.to_rfc3339(),
        email_confirmed_at: account.email_confirmed_at.map(|dt| dt.to_rfc3339()),
        invited_by,
        invites,
        invites_disabled: account.invites_disabled.unwrap_or(false),
        invite_note: None,
        deactivated_at: account.deactivated_at.map(|dt| dt.to_rfc3339()),
        threat_signatures: vec![],
    })
}

/// Get multiple account details in batch
///
/// Batch lookup of multiple account details by DIDs. Accepts repeated
/// `?dids=...&dids=...` query parameters per the lexicon. Returns information
/// for all found accounts (missing DIDs are silently skipped). Uses
/// `axum_extra::extract::Query` rather than the default `axum::extract::Query`
/// because the latter's `serde_urlencoded` backend collapses repeated keys
/// to the last value.
async fn get_account_infos(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    axum_extra::extract::Query(query): axum_extra::extract::Query<GetAccountInfosQuery>,
) -> Result<Json<GetAccountInfosResponse>, (StatusCode, String)> {
    if query.dids.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "No DIDs provided".to_string()));
    }

    // Limit batch size to prevent abuse
    const MAX_BATCH_SIZE: usize = 100;
    if query.dids.len() > MAX_BATCH_SIZE {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("Too many DIDs (max {})", MAX_BATCH_SIZE),
        ));
    }

    let mut infos = Vec::new();
    for did in &query.dids {
        if !did.starts_with("did:") {
            continue;
        }
        if let Ok(info) = build_account_info(&ctx, did).await {
            infos.push(info);
        }
    }

    Ok(Json(GetAccountInfosResponse { infos }))
}

#[derive(Deserialize)]
struct GetAccountInfoQuery {
    /// DID of the account to look up
    did: String,
}

#[derive(Deserialize)]
struct SearchAccountsQuery {
    /// Optional email to filter by (exact, case-insensitive)
    #[serde(default)]
    email: Option<String>,
    /// Pagination cursor (opaque to clients; server treats it as the
    /// last DID returned by the previous page)
    #[serde(default)]
    cursor: Option<String>,
    /// Page size, 1-100, default 50 per lexicon
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(serde::Serialize)]
struct SearchAccountsResponse {
    /// Required per lexicon — always present, possibly empty
    accounts: Vec<AccountInfo>,
    /// Present only when more pages remain
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor: Option<String>,
}

/// Search accounts by email with cursor pagination
/// (lexicon `com.atproto.admin.searchAccounts`).
///
/// Reuses the `build_account_info` helper with `get_account_info` and
/// `get_account_infos` so the `accountView` shape stays consistent across
/// all three endpoints. Cursor pagination uses the trailing DID as an
/// opaque cursor; the same scheme will be reused by Phase 1.10 (#65) when
/// it backfills pagination on `listAccounts` and `getInviteCodes`.
async fn search_accounts(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(query): Query<SearchAccountsQuery>,
) -> Result<Json<SearchAccountsResponse>, (StatusCode, String)> {
    // Lexicon: limit is integer 1-100, default 50.
    let limit = query.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err((
            StatusCode::BAD_REQUEST,
            "limit must be between 1 and 100".to_string(),
        ));
    }

    // Fetch limit+1 to detect whether more pages remain.
    let rows = ctx
        .account_manager
        .search_accounts(
            query.email.as_deref(),
            query.cursor.as_deref(),
            limit + 1,
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let has_more = rows.len() as i64 > limit;
    let page: Vec<_> = rows.into_iter().take(limit as usize).collect();
    let next_cursor = if has_more {
        page.last().map(|a| a.did.clone())
    } else {
        None
    };

    let mut accounts = Vec::with_capacity(page.len());
    for actor in &page {
        // Reuse the shared accountView builder. Errors here mean the
        // account row was deleted between the search and the per-DID
        // lookup — extremely rare, but skip rather than fail the page.
        if let Ok(info) = build_account_info(&ctx, &actor.did).await {
            accounts.push(info);
        }
    }

    Ok(Json(SearchAccountsResponse {
        accounts,
        cursor: next_cursor,
    }))
}

// ---- tools.aurora.describeCapabilities (chainlink #99 / Phase 3.2) ----
//
// Top-level capability probe. Static at compile time (open question
// §9.4 resolved as Option A): the response reflects what's
// structurally present in this build, not what's wired-and-ready at
// runtime. Future sub-phases (3.5 event-variants, 3.8 hash-chained-
// audit, 3.9 realtime-events) extend the static lists below as they
// land.
//
// Auth: AdminAuthContext (Moderator+) — matches Phase 2.3 ops
// convention. Capability advertisement is a privileged operation
// because it surfaces operational structure that could inform
// targeted attacks; we don't gate the wire format on an unauth
// probe.

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DescribeCapabilitiesResponse {
    families: serde_json::Value,
    extensions: Vec<CapabilityExtension>,
    implementation: &'static str,
    version: &'static str,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CapabilityExtension {
    name: &'static str,
    /// Optional structured value (e.g. `event-variants` carries the list of
    /// supported ModEvent variant names). Omitted when not applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<serde_json::Value>,
}

/// Endpoint names per Aurora namespace, as currently shipped. Updated
/// by future sub-phases as they land. Phase 3.2's snapshot reflects
/// the surface present at this commit; sub-phases 3.3-3.9 each add
/// their endpoint names to the appropriate family.
fn aurora_capability_families() -> serde_json::Value {
    serde_json::json!({
        "tools.aurora.ops": [
            "getStats",
            "listAccounts",
            "getInstanceMetrics",
            "getValidationFailures",
            "getSystemHealth",
            "getDatabaseStatus",
            "getResourceUsage",
            "listBackgroundJobs",
            "runHealthChecks",
            "getVersionInfo",
            "getSystemMetrics",
            "getNonceStoreStatus",
            "cleanupNonceStores",
            "getBlobStatistics",
            "listBlobs",
            "deleteBlob",
            "quarantineBlob",
            "restoreBlob",
            "runBlobGC",
            "getBlobQuotas",
            "getSequencerStatus",
            "pauseSequencer",
            "resumeSequencer",
            "resetSequencerCursor",
            "rebuildSequencer",
            "getRateLimitConfig",
            "getRateLimitStatus",
            "cleanupRateLimitState",
            "getFederationStatus",
            "getRelayConfig",
            "listKnownInstances",
            "triggerPdsDiscovery"
        ],
        // Phase 3.3 (chainlink #100) — moderator-tier reads.
        // Phase 3.4 (chainlink #101) — appeals reads (listAppeals, getAppeal).
        "tools.aurora.moderator": [
            "queryEvents",
            "getEvent",
            "queryStatuses",
            "getSubjectContext",
            "getSubjectHistory",
            "listAppeals",
            "getAppeal"
        ],
        // Phase 3.5 (chainlink #102) — emitEvent unified action surface.
        "tools.aurora.admin": [
            "emitEvent"
        ],
        // Phase 3.6 (chainlink #103) — role management relocated from
        // com.atproto.admin.{grantRole,revokeRole}.
        "tools.aurora.superadmin": [
            "grantRole",
            "revokeRole"
        ]
    })
}

/// Static extension list, reflecting what's structurally present in
/// this build. Extensions added incrementally by future sub-phases:
///   - "event-variants" (Phase 3.5: ModEvent variant names)
///   - "hash-chained-audit" (Phase 3.8: getAuditTrail verified flag)
///   - "realtime-events" (Phase 3.9: subscribeModEvents WebSocket)
fn aurora_capability_extensions() -> Vec<CapabilityExtension> {
    // Empty for Phase 3.2 — sub-phases 3.5/3.8/3.9 will append.
    Vec::new()
}

/// `tools.aurora.describeCapabilities` — top-level probe.
async fn describe_capabilities(
    State(_ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<DescribeCapabilitiesResponse>, (StatusCode, String)> {
    Ok(Json(DescribeCapabilitiesResponse {
        families: aurora_capability_families(),
        extensions: aurora_capability_extensions(),
        implementation: "aurora-locus",
        // Cargo.toml's package version. Bumped as part of release work.
        version: env!("CARGO_PKG_VERSION"),
    }))
}

// ---- tools.aurora.ops.listAccounts (chainlink #84 / Phase 2.3.7) ----

/// Query parameters for tools.aurora.ops.listAccounts.
///
/// Operator-facing account listing with broader filters than
/// com.atproto.admin.searchAccounts. See AccountManager::ops_list_accounts
/// for the full filter semantics.
#[derive(Deserialize)]
struct OpsListAccountsQuery {
    /// Lower bound for `actor.created_at` (inclusive), RFC3339.
    #[serde(rename = "signupDateFrom", default)]
    signup_date_from: Option<String>,
    /// Upper bound for `actor.created_at` (inclusive), RFC3339.
    #[serde(rename = "signupDateTo", default)]
    signup_date_to: Option<String>,
    /// Filter to accounts onboarded via an invite code created by this DID.
    #[serde(rename = "inviteSource", default)]
    invite_source: Option<String>,
    /// Status filter: `active` | `deactivated` | `takedown` | `suspended`.
    #[serde(default)]
    status: Option<String>,
    /// Pagination cursor: trailing DID from previous page (opaque to clients).
    #[serde(default)]
    cursor: Option<String>,
    /// Page size, 1-100, default 50.
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Debug, serde::Serialize)]
struct OpsListAccountsResponse {
    /// Required, possibly empty.
    accounts: Vec<AccountInfo>,
    /// Present only when more pages remain.
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor: Option<String>,
}

/// Operator-facing account listing.
///
/// Preserves Aurora-Locus's broader filtering capability beyond bsky-PDS's
/// `searchAccounts`. Filters on signup date range, invite source DID, and
/// status; cursor + limit pagination. Returns paginated `accountView[]`
/// using the same `build_account_info` helper as the other admin
/// account endpoints so the wire shape stays consistent.
async fn ops_list_accounts(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(query): Query<OpsListAccountsQuery>,
) -> Result<Json<OpsListAccountsResponse>, (StatusCode, String)> {
    // Validate limit.
    let limit = query.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err((
            StatusCode::BAD_REQUEST,
            "limit must be between 1 and 100".to_string(),
        ));
    }

    // Validate status enum if provided. Anything else is a client bug; reject
    // explicitly so callers don't quietly get unfiltered results.
    if let Some(s) = query.status.as_deref() {
        if !matches!(s, "active" | "deactivated" | "takedown" | "suspended") {
            return Err((
                StatusCode::BAD_REQUEST,
                format!(
                    "status must be one of: active, deactivated, takedown, suspended (got {})",
                    s
                ),
            ));
        }
    }

    // Validate dates as RFC3339 (failure here means client typo, not server
    // problem; reject upfront rather than letting the SQL string-compare
    // through).
    for (label, val) in [
        ("signupDateFrom", &query.signup_date_from),
        ("signupDateTo", &query.signup_date_to),
    ] {
        if let Some(s) = val {
            if chrono::DateTime::parse_from_rfc3339(s).is_err() {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!("{} must be RFC3339 datetime", label),
                ));
            }
        }
    }

    // Validate inviteSource is a DID-looking string.
    if let Some(d) = query.invite_source.as_deref() {
        if !d.starts_with("did:") {
            return Err((
                StatusCode::BAD_REQUEST,
                "inviteSource must be a DID identifier".to_string(),
            ));
        }
    }

    // Fetch limit+1 to detect more pages.
    let rows = ctx
        .account_manager
        .ops_list_accounts(
            query.signup_date_from.as_deref(),
            query.signup_date_to.as_deref(),
            query.invite_source.as_deref(),
            query.status.as_deref(),
            query.cursor.as_deref(),
            limit + 1,
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let has_more = rows.len() as i64 > limit;
    let page: Vec<_> = rows.into_iter().take(limit as usize).collect();
    let next_cursor = if has_more {
        page.last().map(|a| a.did.clone())
    } else {
        None
    };

    let mut accounts = Vec::with_capacity(page.len());
    for actor in &page {
        if let Ok(info) = build_account_info(&ctx, &actor.did).await {
            accounts.push(info);
        }
    }

    Ok(Json(OpsListAccountsResponse {
        accounts,
        cursor: next_cursor,
    }))
}

// ---- tools.aurora.ops.getInstanceMetrics (chainlink #84 / Phase 2.3.8) ----

/// Aggregated operator-flavored metrics for the instance.
///
/// Fields that aren't populated from existing instrumentation are omitted
/// rather than zero-filled, so absence is meaningful (e.g. no relay client
/// configured → federation_health.relay_connected is false, but the field
/// itself is always present; cpu_seconds_total may be None on platforms
/// where prometheus doesn't surface process-level counters).
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct OpsInstanceMetrics {
    system_health: OpsSystemHealth,
    resource_usage: OpsResourceUsage,
    account_growth: OpsAccountGrowth,
    federation_health: OpsFederationHealth,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct OpsSystemHealth {
    /// "healthy" if a SELECT 1 against the account DB succeeds.
    status: &'static str,
    version: String,
    uptime_seconds: f64,
    active_http_requests: i64,
    active_sessions: i64,
    active_background_jobs: i64,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct OpsResourceUsage {
    /// Process resident memory in bytes (None when prometheus collector
    /// hasn't surfaced this counter — uncommon).
    #[serde(skip_serializing_if = "Option::is_none")]
    memory_resident_bytes: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cpu_seconds_total: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    open_fds: Option<i64>,
    db_pool_size: u32,
    db_pool_idle_connections: u32,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct OpsAccountGrowth {
    signups_last_24h: i64,
    signups_last_7d: i64,
    signups_last_30d: i64,
    total_accounts: i64,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct OpsFederationHealth {
    federation_enabled: bool,
    relay_connected: bool,
    /// Known peer count from the federation registry; 0 when federation
    /// is disabled or the registry is empty.
    known_instances: i64,
}

/// Operator-facing aggregate metrics endpoint.
///
/// Aggregates from sources Aurora-Locus already tracks (metrics module,
/// prometheus gauges, db pool stats, simple SQL counts). No new
/// instrumentation is added here — fields that aren't tracked end up as
/// `None` (omitted) rather than zero-filled.
async fn ops_get_instance_metrics(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<OpsInstanceMetrics>, (StatusCode, String)> {
    use crate::metrics;

    // ---- System health ----
    let db_healthy = sqlx::query("SELECT 1")
        .fetch_one(&ctx.account_db)
        .await
        .is_ok();
    let system_health = OpsSystemHealth {
        status: if db_healthy { "healthy" } else { "unhealthy" },
        version: ctx.config.service.version.clone(),
        uptime_seconds: metrics::UPTIME_SECONDS.get(),
        active_http_requests: metrics::HTTP_REQUESTS_ACTIVE.get(),
        active_sessions: metrics::SESSIONS_ACTIVE.get(),
        active_background_jobs: metrics::BACKGROUND_JOBS_ACTIVE.get(),
    };

    // ---- Resource usage (prometheus process metrics) ----
    let metric_families = prometheus::gather();
    let mut memory_resident_bytes = None;
    let mut cpu_seconds_total = None;
    let mut open_fds = None;
    for mf in &metric_families {
        match mf.name() {
            "process_resident_memory_bytes" => {
                if let Some(m) = mf.get_metric().first() {
                    memory_resident_bytes = Some(m.get_gauge().value());
                }
            }
            "process_cpu_seconds_total" => {
                if let Some(m) = mf.get_metric().first() {
                    cpu_seconds_total = Some(m.get_counter().value());
                }
            }
            "process_open_fds" => {
                if let Some(m) = mf.get_metric().first() {
                    open_fds = Some(m.get_gauge().value() as i64);
                }
            }
            _ => {}
        }
    }
    let resource_usage = OpsResourceUsage {
        memory_resident_bytes,
        cpu_seconds_total,
        open_fds,
        db_pool_size: ctx.account_db.size(),
        db_pool_idle_connections: ctx.account_db.num_idle() as u32,
    };

    // ---- Account growth (windowed counts) ----
    let now = chrono::Utc::now();
    let cutoff_24h = (now - chrono::Duration::hours(24)).to_rfc3339();
    let cutoff_7d = (now - chrono::Duration::days(7)).to_rfc3339();
    let cutoff_30d = (now - chrono::Duration::days(30)).to_rfc3339();

    let signups_last_24h: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM actor WHERE created_at > ?",
    )
    .bind(&cutoff_24h)
    .fetch_one(&ctx.account_db)
    .await
    .unwrap_or(0);
    let signups_last_7d: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM actor WHERE created_at > ?",
    )
    .bind(&cutoff_7d)
    .fetch_one(&ctx.account_db)
    .await
    .unwrap_or(0);
    let signups_last_30d: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM actor WHERE created_at > ?",
    )
    .bind(&cutoff_30d)
    .fetch_one(&ctx.account_db)
    .await
    .unwrap_or(0);
    let total_accounts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM actor")
        .fetch_one(&ctx.account_db)
        .await
        .unwrap_or(0);

    let account_growth = OpsAccountGrowth {
        signups_last_24h,
        signups_last_7d,
        signups_last_30d,
        total_accounts,
    };

    // ---- Federation health ----
    // Known instances come from the in-memory pds_discovery registry
    // (not a SQL table). 0 when federation is disabled or discovery is
    // not configured.
    let known_instances = if let Some(ref discovery) = ctx.pds_discovery {
        discovery.get_known_instances().await.len() as i64
    } else {
        0
    };

    let federation_health = OpsFederationHealth {
        federation_enabled: ctx.config.federation.enabled,
        relay_connected: ctx.relay_client.is_some(),
        known_instances,
    };

    Ok(Json(OpsInstanceMetrics {
        system_health,
        resource_usage,
        account_growth,
        federation_health,
    }))
}

/// Get details about a single account (lexicon `com.atproto.admin.getAccountInfo`).
///
/// Thin wrapper around the same `build_account_info` helper used by
/// `get_account_infos`. The `accountView` shape is shared so future fixes
/// from chainlink #64 (Phase 1.9) propagate to both endpoints at once.
async fn get_account_info(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(query): Query<GetAccountInfoQuery>,
) -> Result<Json<AccountInfo>, axum::response::Response> {
    use axum::response::IntoResponse;

    if !query.did.starts_with("did:") {
        return Err((
            StatusCode::BAD_REQUEST,
            "did must be a DID identifier".to_string(),
        )
            .into_response());
    }

    match build_account_info(&ctx, &query.did).await {
        Ok(info) => Ok(Json(info)),
        Err(PdsError::NotFound(_)) => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "RepoNotFound",
                "message": format!("Account not found: {}", query.did),
            })),
        )
            .into_response()),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()),
    }
}

/// Polymorphic subject for `updateSubjectStatus` and `getSubjectStatus`.
///
/// Lexicon-conformant union of `com.atproto.admin.defs#repoRef`,
/// `com.atproto.repo.strongRef`, and `com.atproto.admin.defs#repoBlobRef`,
/// internally-tagged via the `$type` discriminator per the ATProto JSON
/// convention. Phase 1.6 (#61) introduced this; the existing `SubjectRef`
/// struct used by `getSubjectStatus`'s response is left in place since
/// changing its shape would touch separate scope.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "$type")]
// Variant names mirror the lexicon's union member names verbatim;
// clippy's enum-variant-names lint flags the shared `Ref` postfix but
// renaming would diverge from the spec namespace.
#[allow(clippy::enum_variant_names)]
enum SubjectUnion {
    #[serde(rename = "com.atproto.admin.defs#repoRef")]
    RepoRef { did: String },
    #[serde(
        rename = "com.atproto.repo.strongRef",
        rename_all = "camelCase"
    )]
    StrongRef { uri: String, cid: String },
    #[serde(
        rename = "com.atproto.admin.defs#repoBlobRef",
        rename_all = "camelCase"
    )]
    RepoBlobRef {
        did: String,
        cid: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        record_uri: Option<String>,
    },
}

/// Request shape for `com.atproto.admin.updateSubjectStatus` (Phase 1.6).
///
/// Replaces the legacy imperative `{subject: string, action, duration}`
/// shape with the spec-conformant declarative status-patch model. Both
/// `takedown` and `deactivated` are optional patches; restore is implicit
/// via `takedown: {applied: false}`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateSubjectStatusRequest {
    subject: SubjectUnion,
    #[serde(default)]
    takedown: Option<StatusAttr>,
    #[serde(default)]
    deactivated: Option<StatusAttr>,
}

/// Response shape for `com.atproto.admin.updateSubjectStatus`.
///
/// Per the lexicon: subject (required) plus an optional `takedown` echoed
/// back. The lexicon does *not* echo `deactivated` in the output — we
/// match the spec exactly.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateSubjectStatusResponse {
    subject: SubjectUnion,
    #[serde(skip_serializing_if = "Option::is_none")]
    takedown: Option<StatusAttr>,
}

/// Update the service-specific admin status of a subject (lexicon
/// `com.atproto.admin.updateSubjectStatus`).
///
/// Phase 1.6 (#61) replaced the imperative-action model with the
/// declarative status-patch model per spec. Subject dispatch:
/// - `repoRef`: account-level. Both `takedown` and `deactivated` patches
///   are honored, mapped to `account_manager` setters.
/// - `repoBlobRef`: blob-level. `takedown` is honored via `BlobQuarantine`;
///   `deactivated` is rejected (400 InvalidRequest) since it isn't
///   applicable to blobs.
/// - `strongRef`: record-level. `takedown` returns 501 (no setter exists
///   yet — tracked under a follow-up); `deactivated` is rejected (400
///   InvalidRequest) since records aren't a deactivable concept.
async fn update_subject_status(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<UpdateSubjectStatusRequest>,
) -> Result<Json<UpdateSubjectStatusResponse>, axum::response::Response> {
    use axum::response::IntoResponse;

    let UpdateSubjectStatusRequest {
        subject,
        takedown,
        deactivated,
    } = req;

    let response_takedown = match &subject {
        SubjectUnion::RepoRef { did } => apply_account_status(
            &ctx,
            &auth,
            did,
            takedown.as_ref(),
            deactivated.as_ref(),
        )
        .await
        .map_err(|(s, m)| (s, m).into_response())?,

        SubjectUnion::RepoBlobRef { did, cid, .. } => {
            // Blobs don't have a deactivation concept — reject so the
            // caller learns their patch wasn't silently dropped.
            if deactivated.is_some() {
                return Err(xrpc_invalid_request_error(
                    "deactivated patch is not applicable to blob subjects; \
                     only takedown applies to blobs",
                ));
            }
            apply_blob_status(&ctx, &auth, did, cid, takedown.as_ref()).await?
        }

        SubjectUnion::StrongRef { .. } => {
            // Same reasoning: records aren't deactivable as a concept.
            // Reject deactivated before falling through to the takedown
            // 501 so the caller gets a precise error.
            if deactivated.is_some() {
                return Err(xrpc_invalid_request_error(
                    "deactivated patch is not applicable to record subjects; \
                     only takedown applies to records",
                ));
            }
            return Err((
                StatusCode::NOT_IMPLEMENTED,
                "Record-level (strongRef) subject takedown is not yet implemented; \
                 the actor-store record.takedown_ref column exists but has no setter.",
            )
                .into_response());
        }
    };

    Ok(Json(UpdateSubjectStatusResponse {
        subject,
        takedown: response_takedown,
    }))
}

/// Build a structured XRPC `InvalidRequest` 400 response per the atproto
/// error convention `{"error": "InvalidRequest", "message": "..."}`.
fn xrpc_invalid_request_error(message: &str) -> axum::response::Response {
    use axum::response::IntoResponse;
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({
            "error": "InvalidRequest",
            "message": message,
        })),
    )
        .into_response()
}

/// Build a structured XRPC `BlobNotFound` 404 response.
fn xrpc_blob_not_found_error(cid: &str) -> axum::response::Response {
    use axum::response::IntoResponse;
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({
            "error": "BlobNotFound",
            "message": format!("Blob not found: cid={}", cid),
        })),
    )
        .into_response()
}

/// Apply takedown/deactivated patches to an account. Returns the post-patch
/// takedown status to echo back in the response.
async fn apply_account_status(
    ctx: &AppContext,
    auth: &AdminAuthContext,
    did: &str,
    takedown: Option<&StatusAttr>,
    deactivated: Option<&StatusAttr>,
) -> Result<Option<StatusAttr>, (StatusCode, String)> {
    if let Some(td) = takedown {
        if td.applied {
            // Use the caller-supplied `ref` if present, otherwise generate one
            // from the admin DID + timestamp so audit trails always have a key.
            // Auto-generated when caller omits `ref` so the audit trail
            // always has a key. Format puts timestamp first to keep the
            // DID's colons unambiguous to downstream parsers.
            let takedown_ref = td.ref_field.clone().unwrap_or_else(|| {
                format!("auto-{}-{}", chrono::Utc::now().timestamp(), auth.did)
            });
            ctx.account_manager
                .takedown_account(did, &takedown_ref)
                .await
                .map_err(map_account_err(did))?;
        } else {
            ctx.account_manager
                .activate_account(did)
                .await
                .map_err(map_account_err(did))?;
        }
        let _ = ctx
            .admin_role_manager
            .log_action(
                &auth.did,
                if td.applied {
                    "subject.takedown.apply"
                } else {
                    "subject.takedown.remove"
                },
                Some(did),
                td.ref_field.as_deref(),
                None,
            )
            .await;
    }

    if let Some(d) = deactivated {
        if d.applied {
            ctx.account_manager
                .deactivate_account(did)
                .await
                .map_err(map_account_err(did))?;
        } else {
            ctx.account_manager
                .reactivate_account(did)
                .await
                .map_err(map_account_err(did))?;
        }
        let _ = ctx
            .admin_role_manager
            .log_action(
                &auth.did,
                if d.applied {
                    "subject.deactivate.apply"
                } else {
                    "subject.deactivate.remove"
                },
                Some(did),
                d.ref_field.as_deref(),
                None,
            )
            .await;
    }

    // Read fresh state so the response reflects post-patch reality.
    let account = ctx
        .account_manager
        .get_account(did)
        .await
        .map_err(map_account_err(did))?;
    Ok(Some(StatusAttr {
        applied: account.takedown_ref.is_some(),
        ref_field: account.takedown_ref,
    }))
}

/// Apply a takedown patch to a blob via the existing quarantine machinery.
///
/// Verifies the blob exists in `BlobStore` before any quarantine action so
/// that operating on a non-existent CID returns 404 BlobNotFound rather
/// than silently no-op'ing through the idempotency path. Already-in-state
/// cases (already quarantined when applying, not quarantined when removing)
/// are treated as idempotent success.
async fn apply_blob_status(
    ctx: &AppContext,
    auth: &AdminAuthContext,
    did: &str,
    cid: &str,
    takedown: Option<&StatusAttr>,
) -> Result<Option<StatusAttr>, axum::response::Response> {
    use crate::blob_store::quarantine::{BlobQuarantine, QuarantineReason};
    use axum::response::IntoResponse;

    // Establish that the blob actually exists. `BlobStore::get_metadata`
    // returns Some(_) iff the blob is registered; missing → 404.
    let exists = ctx
        .blob_store
        .get_metadata(cid)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?
        .is_some();
    if !exists {
        return Err(xrpc_blob_not_found_error(cid));
    }

    let quarantine = BlobQuarantine::new(ctx.account_db.clone());

    if let Some(td) = takedown {
        if td.applied {
            // Already-quarantined → Conflict from the quarantine layer →
            // idempotent success since the desired post-state already obtains.
            match quarantine
                .quarantine_blob(
                    cid,
                    QuarantineReason::Other,
                    td.ref_field.as_deref(),
                    &auth.did,
                    None,
                )
                .await
            {
                Ok(_) | Err(PdsError::Conflict(_)) => {}
                Err(e) => {
                    return Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())
                }
            }
        } else {
            // Not-currently-quarantined → NotFound from `restore_blob` →
            // idempotent success (operator wanted "ensure not quarantined";
            // we already are).
            match quarantine.restore_blob(cid, &auth.did).await {
                Ok(_) | Err(PdsError::NotFound(_)) => {}
                Err(e) => {
                    return Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())
                }
            }
        }
        let _ = ctx
            .admin_role_manager
            .log_action(
                &auth.did,
                if td.applied {
                    "subject.takedown.apply"
                } else {
                    "subject.takedown.remove"
                },
                Some(did),
                Some(cid),
                None,
            )
            .await;
    }

    let is_taken_down = quarantine
        .is_quarantined(cid)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?;
    Ok(Some(StatusAttr {
        applied: is_taken_down,
        ref_field: takedown.and_then(|td| td.ref_field.clone()),
    }))
}

/// Map an account-manager error to an HTTP status, matching the pattern
/// established by other admin handlers (NotFound → 404, otherwise 500).
fn map_account_err(did: &str) -> impl Fn(PdsError) -> (StatusCode, String) + '_ {
    move |e| {
        if matches!(e, PdsError::NotFound(_)) {
            (
                StatusCode::NOT_FOUND,
                format!("Account not found: {}", did),
            )
        } else {
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        }
    }
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
/// (lexicon `com.atproto.admin.defs#statusAttr`).
///
/// Used on the request side for `updateSubjectStatus` (per Phase 1.6 / #61)
/// and on the response side for both `getSubjectStatus` and
/// `updateSubjectStatus`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatusAttr {
    applied: bool,
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
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
struct DisableInviteCodesRequest {
    /// Specific invite codes to disable. Missing codes are silently skipped.
    #[serde(default)]
    codes: Vec<String>,
    /// Account DIDs whose issued invite codes should all be disabled.
    /// Matches `invite_code.for_account` (the intended recipient).
    #[serde(default)]
    accounts: Vec<String>,
}

/// Disable a batch of invite codes and/or all codes issued for a set of
/// accounts (lexicon `com.atproto.admin.disableInviteCodes`).
///
/// Updates run in a single SQLite transaction so a moderator working through
/// a spam ring gets all-or-nothing semantics rather than a partial commit.
/// Empty `codes` and `accounts` is a successful no-op per the lexicon (both
/// fields are optional with no `required` array).
async fn disable_invite_codes(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Json(req): Json<DisableInviteCodesRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    ctx.invite_manager
        .disable_codes_batch(&req.codes, &req.accounts)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::OK)
}

#[derive(Deserialize)]
struct AccountInvitesRequest {
    /// Account at-identifier (handle or DID) per the lexicon. Required if
    /// the legacy `did` field is not provided.
    #[serde(default)]
    account: Option<String>,
    /// DEPRECATED: legacy `did` field retained for back-compat. Use
    /// `account` instead. Continues to accept DID-form only. To be
    /// removed in a later minor version.
    #[serde(default)]
    did: Option<String>,
    /// Optional reason for the invites change (per lexicon). Persisted to
    /// the admin audit log.
    #[serde(default)]
    note: Option<String>,
}

/// Enable invite code creation for an account
async fn enable_account_invites(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<AccountInvitesRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let canonical_did =
        resolve_account_or_did(&ctx, req.account.as_deref(), req.did.as_deref()).await?;

    ctx.account_manager
        .enable_account_invites(&canonical_did)
        .await
        .map_err(|e| {
            if matches!(e, PdsError::NotFound(_)) {
                (
                    StatusCode::NOT_FOUND,
                    format!("Account not found: {}", canonical_did),
                )
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            }
        })?;

    // Best-effort audit log entry; failure here shouldn't fail the operation.
    let _ = ctx
        .admin_role_manager
        .log_action(
            &auth.did,
            "account.invites.enable",
            Some(&canonical_did),
            req.note.as_deref(),
            None,
        )
        .await;

    Ok(StatusCode::OK)
}

/// Disable invite code creation for an account
async fn disable_account_invites(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<AccountInvitesRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let canonical_did =
        resolve_account_or_did(&ctx, req.account.as_deref(), req.did.as_deref()).await?;

    ctx.account_manager
        .disable_account_invites(&canonical_did)
        .await
        .map_err(|e| {
            if matches!(e, PdsError::NotFound(_)) {
                (
                    StatusCode::NOT_FOUND,
                    format!("Account not found: {}", canonical_did),
                )
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            }
        })?;

    let _ = ctx
        .admin_role_manager
        .log_action(
            &auth.did,
            "account.invites.disable",
            Some(&canonical_did),
            req.note.as_deref(),
            None,
        )
        .await;

    Ok(StatusCode::OK)
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
                created_at: chrono::DateTime::parse_from_rfc3339(
                        &row.try_get::<String, _>("created_at")
                            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
                    )
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Invalid timestamp: {}", e)))?,
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
    use std::sync::OnceLock;
    use tempfile::tempdir;

    /// Serialises the filesystem-heavy portion of test setup.
    ///
    /// Without this, `cargo test --lib` would race 25+ parallel `tempdir()`
    /// plus `SqlitePool::connect` plus migration runs against each other and
    /// produce sporadic `SQLITE_CANTOPEN` (code 14) errors on first runs,
    /// especially under WSL2's drvfs where this crate's primary checkout
    /// lives. Holding the lock through `AppContext::new` is cheap and the
    /// test bodies still execute in parallel once setup completes.
    /// Tracked under chainlink #68.
    fn fixture_setup_lock() -> &'static tokio::sync::Mutex<()> {
        static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
    }

    async fn create_test_context() -> AppContext {
        let _guard = fixture_setup_lock().lock().await;
        // `into_path()` leaks the TempDir so its Drop doesn't unlink the
        // directory while sqlx connections still hold it open. Under the
        // AnyPool default journal mode (DELETE), SQLite reports
        // SQLITE_READONLY_DBMOVED on the next write once the directory
        // entry is gone — WAL was previously masking this. The OS cleans
        // up /tmp on its own; this is test-only so leaking is fine.
        let dir = tempdir().unwrap().keep();
        let db_path = dir.join("test.db");

        let config = ServerConfig {
            service: ServiceConfig {
                hostname: "localhost".to_string(),
                port: 2583,
                service_did: "did:web:localhost".to_string(),
                version: "0.1.0-test".to_string(),
                blob_upload_limit: 5242880,
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

    /// SuperAdmin auth fixture for tests of tools.aurora.superadmin.*
    /// endpoints (Phase 3.6 / chainlink #103). Same shape as
    /// admin_test_auth, role bumped to SuperAdmin so the handler-level
    /// SuperAdmin gate passes.
    fn superadmin_test_auth() -> AdminAuthContext {
        AdminAuthContext {
            did: "did:plc:superadmin".to_string(),
            session: ValidatedSession {
                did: "did:plc:superadmin".to_string(),
                session_id: "test_session_superadmin".to_string(),
                is_app_password: false,
            },
            role: Role::SuperAdmin,
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

    #[tokio::test]
    async fn test_get_account_info_rejects_non_did_input() {
        let ctx = create_test_context().await;
        let query = GetAccountInfoQuery {
            did: "not-a-did".to_string(),
        };

        let result = get_account_info(State(ctx), admin_test_auth(), Query(query)).await;
        let resp = match result {
            Err(r) => r,
            Ok(_) => panic!("non-DID input should be rejected"),
        };
        let (status, body) = read_response_body(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("DID"));
    }

    #[tokio::test]
    async fn test_get_account_info_returns_repo_not_found_for_missing_account() {
        let ctx = create_test_context().await;
        let query = GetAccountInfoQuery {
            did: "did:plc:nonexistentaccount0000".to_string(),
        };

        let result = get_account_info(State(ctx), admin_test_auth(), Query(query)).await;
        let resp = match result {
            Err(r) => r,
            Ok(_) => panic!("missing account should return RepoNotFound"),
        };
        let (status, body) = read_response_body(resp).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let parsed: serde_json::Value = serde_json::from_str(&body).expect("body must be JSON");
        assert_eq!(parsed["error"], "RepoNotFound");
        assert!(parsed["message"]
            .as_str()
            .unwrap()
            .contains("did:plc:nonexistentaccount0000"));
    }

    #[tokio::test]
    async fn test_disable_invite_codes_empty_input_is_noop() {
        let ctx = create_test_context().await;
        let req = DisableInviteCodesRequest {
            codes: vec![],
            accounts: vec![],
        };

        let result = disable_invite_codes(State(ctx), admin_test_auth(), Json(req)).await;
        assert_eq!(result.unwrap(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_disable_invite_codes_disables_specific_codes_atomically() {
        let ctx = create_test_context().await;

        // Seed two invite codes via the existing manager API.
        let code_a = ctx
            .invite_manager
            .create_invite("did:plc:creator", 5, None, None, None)
            .await
            .unwrap();
        let code_b = ctx
            .invite_manager
            .create_invite("did:plc:creator", 5, None, None, None)
            .await
            .unwrap();
        assert!(!code_a.disabled && !code_b.disabled);

        let req = DisableInviteCodesRequest {
            codes: vec![code_a.code.clone(), code_b.code.clone()],
            accounts: vec![],
        };
        let result = disable_invite_codes(State(ctx.clone()), admin_test_auth(), Json(req)).await;
        assert_eq!(result.unwrap(), StatusCode::OK);

        // Verify both codes are disabled in the database.
        let a_after = ctx
            .invite_manager
            .get_code(&code_a.code)
            .await
            .unwrap()
            .unwrap();
        let b_after = ctx
            .invite_manager
            .get_code(&code_b.code)
            .await
            .unwrap()
            .unwrap();
        assert!(a_after.disabled);
        assert!(b_after.disabled);
    }

    #[tokio::test]
    async fn test_disable_invite_codes_disables_codes_by_account() {
        let ctx = create_test_context().await;
        let target_did = "did:plc:targetaccount";

        // One code issued *for* the target account, one not.
        let issued_for_target = ctx
            .invite_manager
            .create_invite(
                "did:plc:creator",
                5,
                None,
                None,
                Some(target_did.to_string()),
            )
            .await
            .unwrap();
        let unrelated = ctx
            .invite_manager
            .create_invite("did:plc:creator", 5, None, None, None)
            .await
            .unwrap();

        let req = DisableInviteCodesRequest {
            codes: vec![],
            accounts: vec![target_did.to_string()],
        };
        let result = disable_invite_codes(State(ctx.clone()), admin_test_auth(), Json(req)).await;
        assert_eq!(result.unwrap(), StatusCode::OK);

        let target_after = ctx
            .invite_manager
            .get_code(&issued_for_target.code)
            .await
            .unwrap()
            .unwrap();
        let unrelated_after = ctx
            .invite_manager
            .get_code(&unrelated.code)
            .await
            .unwrap()
            .unwrap();
        assert!(target_after.disabled);
        assert!(!unrelated_after.disabled);
    }

    #[tokio::test]
    async fn test_disable_invite_codes_silently_skips_missing_codes() {
        let ctx = create_test_context().await;

        // Submit codes that don't exist; should succeed (the codes are
        // vacuously disabled). Distinct from the singular endpoint, which
        // returns NotFound for unknown codes.
        let req = DisableInviteCodesRequest {
            codes: vec!["aurora-nonexistent-code-1".to_string()],
            accounts: vec!["did:plc:noaccount".to_string()],
        };
        let result = disable_invite_codes(State(ctx), admin_test_auth(), Json(req)).await;
        assert_eq!(result.unwrap(), StatusCode::OK);
    }

    /// Insert minimal actor+account rows directly into the test database.
    /// Bypasses `account_manager.create_account` which requires PLC
    /// registration over the network. Used only for endpoint tests that
    /// need real DB rows to query against.
    async fn seed_test_account(ctx: &AppContext, did: &str, handle: &str, email: Option<&str>) {
        let now = chrono::Utc::now();
        sqlx::query(
            "INSERT INTO actor (did, handle, created_at, takedown_ref, deactivated_at, delete_after)
             VALUES (?1, ?2, ?3, NULL, NULL, NULL)",
        )
        .bind(did)
        .bind(handle)
        .bind(now.to_rfc3339())
        .execute(&ctx.account_db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO account (did, email, password_hash, email_confirmed_at, invites_disabled)
             VALUES (?1, ?2, 'test-hash', NULL, 0)",
        )
        .bind(did)
        .bind(email)
        .execute(&ctx.account_db)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_search_accounts_rejects_out_of_range_limit() {
        let ctx = create_test_context().await;
        let result = search_accounts(
            State(ctx),
            admin_test_auth(),
            Query(SearchAccountsQuery {
                email: None,
                cursor: None,
                limit: Some(101),
            }),
        )
        .await;
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("limit > 100 should be rejected"),
        };
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1.contains("limit"));

        let ctx2 = create_test_context().await;
        let result2 = search_accounts(
            State(ctx2),
            admin_test_auth(),
            Query(SearchAccountsQuery {
                email: None,
                cursor: None,
                limit: Some(0),
            }),
        )
        .await;
        let err2 = match result2 {
            Err(e) => e,
            Ok(_) => panic!("limit < 1 should be rejected"),
        };
        assert_eq!(err2.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_search_accounts_empty_db_returns_empty_no_cursor() {
        let ctx = create_test_context().await;
        let resp = search_accounts(
            State(ctx),
            admin_test_auth(),
            Query(SearchAccountsQuery {
                email: None,
                cursor: None,
                limit: None,
            }),
        )
        .await
        .unwrap()
        .0;
        assert!(resp.accounts.is_empty());
        assert!(resp.cursor.is_none());
    }

    #[tokio::test]
    async fn test_search_accounts_filters_by_email_case_insensitive() {
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:aaaa", "alice.test", Some("Alice@Example.com")).await;
        seed_test_account(&ctx, "did:plc:bbbb", "bob.test", Some("bob@example.com")).await;
        seed_test_account(&ctx, "did:plc:cccc", "carol.test", None).await;

        // Case-insensitive match should find Alice regardless of casing.
        let resp = search_accounts(
            State(ctx.clone()),
            admin_test_auth(),
            Query(SearchAccountsQuery {
                email: Some("alice@example.com".to_string()),
                cursor: None,
                limit: None,
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(resp.accounts.len(), 1);
        assert_eq!(resp.accounts[0].did, "did:plc:aaaa");
        assert!(resp.cursor.is_none());

        // Non-matching email returns empty.
        let resp = search_accounts(
            State(ctx),
            admin_test_auth(),
            Query(SearchAccountsQuery {
                email: Some("nobody@example.com".to_string()),
                cursor: None,
                limit: None,
            }),
        )
        .await
        .unwrap()
        .0;
        assert!(resp.accounts.is_empty());
    }

    // ---- tools.aurora.ops.listAccounts (chainlink #84 / Phase 2.3.7) ----

    fn ops_list_query() -> OpsListAccountsQuery {
        OpsListAccountsQuery {
            signup_date_from: None,
            signup_date_to: None,
            invite_source: None,
            status: None,
            cursor: None,
            limit: None,
        }
    }

    #[tokio::test]
    async fn test_ops_list_accounts_empty_db_returns_empty() {
        let ctx = create_test_context().await;
        let resp = ops_list_accounts(State(ctx), admin_test_auth(), Query(ops_list_query()))
            .await
            .unwrap()
            .0;
        assert!(resp.accounts.is_empty());
        assert!(resp.cursor.is_none());
    }

    #[tokio::test]
    async fn test_ops_list_accounts_returns_all_when_no_filters() {
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:a1", "a1.test", Some("a@x")).await;
        seed_test_account(&ctx, "did:plc:b2", "b2.test", Some("b@x")).await;
        seed_test_account(&ctx, "did:plc:c3", "c3.test", None).await;

        let resp = ops_list_accounts(State(ctx), admin_test_auth(), Query(ops_list_query()))
            .await
            .unwrap()
            .0;
        assert_eq!(resp.accounts.len(), 3);
    }

    #[tokio::test]
    async fn test_ops_list_accounts_takedown_status_filter() {
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:active", "ok.test", None).await;
        seed_test_account(&ctx, "did:plc:downed", "down.test", None).await;
        ctx.account_manager
            .takedown_account("did:plc:downed", "ticket-1")
            .await
            .unwrap();

        let mut q = ops_list_query();
        q.status = Some("takedown".to_string());
        let resp = ops_list_accounts(State(ctx.clone()), admin_test_auth(), Query(q))
            .await
            .unwrap()
            .0;
        assert_eq!(resp.accounts.len(), 1);
        assert_eq!(resp.accounts[0].did, "did:plc:downed");

        let mut q = ops_list_query();
        q.status = Some("active".to_string());
        let resp = ops_list_accounts(State(ctx), admin_test_auth(), Query(q))
            .await
            .unwrap()
            .0;
        assert_eq!(resp.accounts.len(), 1);
        assert_eq!(resp.accounts[0].did, "did:plc:active");
    }

    #[tokio::test]
    async fn test_ops_list_accounts_signup_date_range_filters() {
        let ctx = create_test_context().await;
        // Seed accounts and override created_at directly in SQL for the test.
        seed_test_account(&ctx, "did:plc:old", "old.test", None).await;
        seed_test_account(&ctx, "did:plc:mid", "mid.test", None).await;
        seed_test_account(&ctx, "did:plc:new", "new.test", None).await;
        sqlx::query("UPDATE actor SET created_at = ? WHERE did = ?")
            .bind("2024-01-01T00:00:00+00:00")
            .bind("did:plc:old")
            .execute(&ctx.account_db)
            .await
            .unwrap();
        sqlx::query("UPDATE actor SET created_at = ? WHERE did = ?")
            .bind("2025-01-01T00:00:00+00:00")
            .bind("did:plc:mid")
            .execute(&ctx.account_db)
            .await
            .unwrap();
        sqlx::query("UPDATE actor SET created_at = ? WHERE did = ?")
            .bind("2026-01-01T00:00:00+00:00")
            .bind("did:plc:new")
            .execute(&ctx.account_db)
            .await
            .unwrap();

        // Window catches just the middle one.
        let mut q = ops_list_query();
        q.signup_date_from = Some("2024-06-01T00:00:00+00:00".to_string());
        q.signup_date_to = Some("2025-06-01T00:00:00+00:00".to_string());
        let resp = ops_list_accounts(State(ctx), admin_test_auth(), Query(q))
            .await
            .unwrap()
            .0;
        assert_eq!(resp.accounts.len(), 1);
        assert_eq!(resp.accounts[0].did, "did:plc:mid");
    }

    #[tokio::test]
    async fn test_ops_list_accounts_invite_source_filter() {
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:inviter", "inv.test", None).await;
        seed_test_account(&ctx, "did:plc:invited", "vee.test", None).await;
        seed_test_account(&ctx, "did:plc:other", "other.test", None).await;

        // Create an invite code by inviter and have invited use it.
        let code = ctx
            .invite_manager
            .create_invite("did:plc:inviter", 5, None, None, None)
            .await
            .unwrap();
        ctx.invite_manager
            .use_code(&code.code, "did:plc:invited")
            .await
            .unwrap();

        let mut q = ops_list_query();
        q.invite_source = Some("did:plc:inviter".to_string());
        let resp = ops_list_accounts(State(ctx), admin_test_auth(), Query(q))
            .await
            .unwrap()
            .0;
        assert_eq!(resp.accounts.len(), 1);
        assert_eq!(resp.accounts[0].did, "did:plc:invited");
    }

    #[tokio::test]
    async fn test_ops_list_accounts_paginates_with_cursor() {
        let ctx = create_test_context().await;
        for did in ["did:plc:a", "did:plc:b", "did:plc:c", "did:plc:d"] {
            seed_test_account(&ctx, did, &format!("{}.test", &did[8..]), None).await;
        }

        let mut q = ops_list_query();
        q.limit = Some(2);
        let page1 = ops_list_accounts(State(ctx.clone()), admin_test_auth(), Query(q))
            .await
            .unwrap()
            .0;
        assert_eq!(page1.accounts.len(), 2);
        let cursor = page1.cursor.clone().expect("cursor expected");

        let mut q = ops_list_query();
        q.limit = Some(2);
        q.cursor = Some(cursor);
        let page2 = ops_list_accounts(State(ctx), admin_test_auth(), Query(q))
            .await
            .unwrap()
            .0;
        assert_eq!(page2.accounts.len(), 2);
        // No overlap.
        let p1: Vec<_> = page1.accounts.iter().map(|a| a.did.as_str()).collect();
        let p2: Vec<_> = page2.accounts.iter().map(|a| a.did.as_str()).collect();
        for did in &p2 {
            assert!(!p1.contains(did), "page2 should not overlap page1");
        }
    }

    #[tokio::test]
    async fn test_ops_list_accounts_validation_limit_out_of_range() {
        let ctx = create_test_context().await;
        let mut q = ops_list_query();
        q.limit = Some(0);
        let err = ops_list_accounts(State(ctx.clone()), admin_test_auth(), Query(q))
            .await
            .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);

        let mut q = ops_list_query();
        q.limit = Some(101);
        let err = ops_list_accounts(State(ctx), admin_test_auth(), Query(q))
            .await
            .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_ops_list_accounts_validation_unknown_status() {
        let ctx = create_test_context().await;
        let mut q = ops_list_query();
        q.status = Some("on-fire".to_string());
        let err = ops_list_accounts(State(ctx), admin_test_auth(), Query(q))
            .await
            .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1.contains("status"));
    }

    #[tokio::test]
    async fn test_ops_list_accounts_validation_bad_date() {
        let ctx = create_test_context().await;
        let mut q = ops_list_query();
        q.signup_date_from = Some("yesterday".to_string());
        let err = ops_list_accounts(State(ctx), admin_test_auth(), Query(q))
            .await
            .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1.contains("RFC3339"));
    }

    #[tokio::test]
    async fn test_ops_list_accounts_validation_bad_invite_source() {
        let ctx = create_test_context().await;
        let mut q = ops_list_query();
        q.invite_source = Some("not-a-did".to_string());
        let err = ops_list_accounts(State(ctx), admin_test_auth(), Query(q))
            .await
            .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1.contains("DID"));
    }

    // ---- tools.aurora.ops.getInstanceMetrics (chainlink #84 / Phase 2.3.8) ----

    #[tokio::test]
    async fn test_ops_get_instance_metrics_empty_instance() {
        let ctx = create_test_context().await;
        let resp = ops_get_instance_metrics(State(ctx), admin_test_auth())
            .await
            .unwrap()
            .0;

        // System health: db is alive, version matches config.
        assert_eq!(resp.system_health.status, "healthy");
        assert_eq!(resp.system_health.version, "0.1.0-test");

        // Account growth: empty DB = zero counts everywhere.
        assert_eq!(resp.account_growth.total_accounts, 0);
        assert_eq!(resp.account_growth.signups_last_24h, 0);
        assert_eq!(resp.account_growth.signups_last_7d, 0);
        assert_eq!(resp.account_growth.signups_last_30d, 0);

        // Federation: disabled in test config.
        assert!(!resp.federation_health.federation_enabled);
        assert!(!resp.federation_health.relay_connected);
        assert_eq!(resp.federation_health.known_instances, 0);
    }

    #[tokio::test]
    async fn test_ops_get_instance_metrics_account_growth_window() {
        let ctx = create_test_context().await;
        // Three accounts, one in each of 24h / 7d-not-24h / older windows.
        seed_test_account(&ctx, "did:plc:fresh", "fresh.test", None).await;
        seed_test_account(&ctx, "did:plc:weekish", "weekish.test", None).await;
        seed_test_account(&ctx, "did:plc:ancient", "ancient.test", None).await;
        // Move "weekish" to ~3 days ago (in 7d window, not 24h).
        sqlx::query("UPDATE actor SET created_at = ? WHERE did = ?")
            .bind(
                (chrono::Utc::now() - chrono::Duration::days(3))
                    .to_rfc3339(),
            )
            .bind("did:plc:weekish")
            .execute(&ctx.account_db)
            .await
            .unwrap();
        // Move "ancient" to ~60 days ago (outside all windows).
        sqlx::query("UPDATE actor SET created_at = ? WHERE did = ?")
            .bind(
                (chrono::Utc::now() - chrono::Duration::days(60))
                    .to_rfc3339(),
            )
            .bind("did:plc:ancient")
            .execute(&ctx.account_db)
            .await
            .unwrap();

        let resp = ops_get_instance_metrics(State(ctx), admin_test_auth())
            .await
            .unwrap()
            .0;

        assert_eq!(resp.account_growth.total_accounts, 3);
        assert_eq!(resp.account_growth.signups_last_24h, 1, "fresh");
        assert_eq!(
            resp.account_growth.signups_last_7d, 2,
            "fresh + weekish"
        );
        assert_eq!(
            resp.account_growth.signups_last_30d, 2,
            "ancient is outside 30d"
        );
    }

    #[tokio::test]
    async fn test_ops_get_instance_metrics_resource_usage_db_pool() {
        let ctx = create_test_context().await;
        let resp = ops_get_instance_metrics(State(ctx), admin_test_auth())
            .await
            .unwrap()
            .0;

        // db_pool_size > 0 because at least one connection is open after the
        // SELECT 1 and the COUNT(*) queries above. Same for idle.
        assert!(resp.resource_usage.db_pool_size >= 1);
    }

    // ---- tools.aurora.describeCapabilities (chainlink #99 / Phase 3.2) ----

    #[tokio::test]
    async fn test_describe_capabilities_returns_expected_shape() {
        let ctx = create_test_context().await;
        let resp = describe_capabilities(State(ctx), admin_test_auth())
            .await
            .unwrap()
            .0;

        assert_eq!(resp.implementation, "aurora-locus");
        // version comes from CARGO_PKG_VERSION; non-empty is enough
        // to confirm the env! macro resolved.
        assert!(!resp.version.is_empty(), "version must be set");

        // Families object must include the four Aurora namespaces, each
        // a JSON array (possibly empty for namespaces that haven't
        // shipped endpoints yet).
        let families = resp.families.as_object().expect("families is object");
        for ns in [
            "tools.aurora.ops",
            "tools.aurora.moderator",
            "tools.aurora.admin",
            "tools.aurora.superadmin",
        ] {
            assert!(
                families.get(ns).map(|v| v.is_array()).unwrap_or(false),
                "missing or non-array family: {}",
                ns
            );
        }
    }

    #[tokio::test]
    async fn test_describe_capabilities_lists_phase_2_3_ops_endpoints() {
        // Sanity-check the static list against what's actually shipped
        // — every endpoint named here was registered in Phase 2.3
        // (chainlink #84). Future sub-phases extend the list; this
        // test guards against the static list silently drifting from
        // the route registrations.
        let ctx = create_test_context().await;
        let resp = describe_capabilities(State(ctx), admin_test_auth())
            .await
            .unwrap()
            .0;

        let ops = resp
            .families
            .get("tools.aurora.ops")
            .and_then(|v| v.as_array())
            .expect("ops family present");
        let ops_names: Vec<&str> =
            ops.iter().map(|v| v.as_str().unwrap()).collect();

        // Spot-check several Phase 2.3 endpoints across categories.
        for expected in [
            "getStats",
            "listAccounts",
            "getInstanceMetrics",
            "pauseSequencer",
            "getFederationStatus",
        ] {
            assert!(
                ops_names.contains(&expected),
                "Phase 2.3 endpoint {} missing from capability list",
                expected
            );
        }
    }

    // ---- tools.aurora.superadmin.{grant,revoke}Role (chainlink #103 / Phase 3.6) ----

    #[tokio::test]
    async fn test_grant_role_rejects_non_superadmin() {
        // Admin is not enough — role management is SuperAdmin-only post
        // Phase 3.6. Verifies the handler-level gate fires before any
        // role mutation reaches the database.
        let ctx = create_test_context().await;
        let req = GrantRoleRequest {
            did: "did:plc:nominee".to_string(),
            role: "moderator".to_string(),
        };
        let (status, body) = grant_role(State(ctx), admin_test_auth(), Json(req))
            .await
            .expect_err("Admin must not be allowed to grant roles");
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(
            body.contains("SuperAdmin"),
            "error message should reference SuperAdmin requirement, got: {}",
            body
        );
    }

    #[tokio::test]
    async fn test_revoke_role_rejects_non_superadmin() {
        let ctx = create_test_context().await;
        let req = RevokeRoleRequest {
            did: "did:plc:victim".to_string(),
            reason: None,
        };
        let (status, body) = revoke_role(State(ctx), admin_test_auth(), Json(req))
            .await
            .expect_err("Admin must not be allowed to revoke roles");
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body.contains("SuperAdmin"));
    }

    #[tokio::test]
    async fn test_grant_role_allowed_for_superadmin() {
        // Happy path: SuperAdmin grants a Moderator role; succeeds.
        let ctx = create_test_context().await;
        let req = GrantRoleRequest {
            did: "did:plc:newmod".to_string(),
            role: "moderator".to_string(),
        };
        let resp = grant_role(State(ctx.clone()), superadmin_test_auth(), Json(req))
            .await
            .expect("SuperAdmin should be allowed to grant roles");
        let json = resp.0;
        assert_eq!(
            json.get("did").and_then(|v| v.as_str()),
            Some("did:plc:newmod")
        );
        assert_eq!(
            json.get("role").and_then(|v| v.as_str()),
            Some("moderator")
        );
    }

    #[tokio::test]
    async fn test_describe_capabilities_advertises_superadmin_endpoints() {
        // Phase 3.6 adds grantRole + revokeRole to the superadmin
        // family. Catches accidental removal from the static list.
        let ctx = create_test_context().await;
        let resp = describe_capabilities(State(ctx), admin_test_auth())
            .await
            .unwrap()
            .0;
        let superadmin = resp
            .families
            .get("tools.aurora.superadmin")
            .and_then(|v| v.as_array())
            .expect("superadmin family present");
        let names: Vec<&str> =
            superadmin.iter().map(|v| v.as_str().unwrap()).collect();
        assert!(names.contains(&"grantRole"), "grantRole missing");
        assert!(names.contains(&"revokeRole"), "revokeRole missing");
    }

    #[tokio::test]
    async fn test_describe_capabilities_extensions_initially_empty() {
        // Phase 3.2 ships with no extensions; the list grows as
        // sub-phases 3.5/3.8/3.9 land. This test catches accidental
        // additions before their backing infrastructure is ready —
        // an extension advertised before the sub-phase lands would
        // mislead clients.
        let ctx = create_test_context().await;
        let resp = describe_capabilities(State(ctx), admin_test_auth())
            .await
            .unwrap()
            .0;
        assert!(
            resp.extensions.is_empty(),
            "extensions list should be empty until sub-phases 3.5/3.8/3.9 ship; \
             got {:?}",
            resp.extensions.iter().map(|e| e.name).collect::<Vec<_>>()
        );
    }

    // ---- Phase 1.7: account/did deprecation-alias rollout ---------------

    #[tokio::test]
    async fn test_resolve_helper_rejects_both_fields() {
        let ctx = create_test_context().await;
        let result = resolve_account_or_did(
            &ctx,
            Some("did:plc:foo"),
            Some("did:plc:foo"),
        )
        .await;
        let err = result.expect_err("both fields should be rejected");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1.contains("exactly one"));
    }

    #[tokio::test]
    async fn test_resolve_helper_rejects_neither_field() {
        let ctx = create_test_context().await;
        let result = resolve_account_or_did(&ctx, None, None).await;
        let err = result.expect_err("missing both should be rejected");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1.contains("Missing"));
    }

    #[tokio::test]
    async fn test_resolve_helper_did_form_account_returns_as_is() {
        let ctx = create_test_context().await;
        let did = resolve_account_or_did(&ctx, Some("did:plc:abcd"), None)
            .await
            .unwrap();
        assert_eq!(did, "did:plc:abcd");
    }

    #[tokio::test]
    async fn test_resolve_helper_handle_form_account_resolves_via_db() {
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:alice", "alice.test", Some("alice@x")).await;

        let did = resolve_account_or_did(&ctx, Some("alice.test"), None)
            .await
            .unwrap();
        assert_eq!(did, "did:plc:alice");
    }

    #[tokio::test]
    async fn test_resolve_helper_legacy_did_field_works() {
        let ctx = create_test_context().await;
        let did = resolve_account_or_did(&ctx, None, Some("did:plc:legacy"))
            .await
            .unwrap();
        assert_eq!(did, "did:plc:legacy");
    }

    #[tokio::test]
    async fn test_resolve_helper_legacy_did_field_rejects_handle_form() {
        let ctx = create_test_context().await;
        let result = resolve_account_or_did(&ctx, None, Some("not-a-did")).await;
        let err = result.expect_err("legacy did field should reject non-DID");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    /// Helper: returns whether `account.invites_disabled` is set for a DID.
    async fn account_invites_disabled(ctx: &AppContext, did: &str) -> bool {
        use sqlx::Row;
        let row: i64 = sqlx::query("SELECT invites_disabled FROM account WHERE did = ?")
            .bind(did)
            .fetch_one(&ctx.account_db)
            .await
            .unwrap()
            .get(0);
        row != 0
    }

    #[tokio::test]
    async fn test_disable_account_invites_with_account_field_did_form() {
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:foo", "foo.test", Some("f@x")).await;

        let req = AccountInvitesRequest {
            account: Some("did:plc:foo".to_string()),
            did: None,
            note: None,
        };
        let status = disable_account_invites(State(ctx.clone()), admin_test_auth(), Json(req))
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK);
        // Side-effect verification: the canonical DID's row had its flag
        // flipped (this also verifies the resolver pointed at the right row).
        assert!(account_invites_disabled(&ctx, "did:plc:foo").await);
    }

    #[tokio::test]
    async fn test_disable_account_invites_with_account_field_handle_form() {
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:bar", "bar.test", Some("b@x")).await;

        let req = AccountInvitesRequest {
            account: Some("bar.test".to_string()),
            did: None,
            note: None,
        };
        let status = disable_account_invites(State(ctx.clone()), admin_test_auth(), Json(req))
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK);
        // Verifies the resolver mapped handle "bar.test" to "did:plc:bar".
        assert!(account_invites_disabled(&ctx, "did:plc:bar").await);
    }

    #[tokio::test]
    async fn test_disable_account_invites_with_legacy_did_field() {
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:baz", "baz.test", None).await;

        let req = AccountInvitesRequest {
            account: None,
            did: Some("did:plc:baz".to_string()),
            note: None,
        };
        let status = disable_account_invites(State(ctx.clone()), admin_test_auth(), Json(req))
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK);
        assert!(account_invites_disabled(&ctx, "did:plc:baz").await);
    }

    #[tokio::test]
    async fn test_disable_account_invites_rejects_both_fields() {
        let ctx = create_test_context().await;
        let req = AccountInvitesRequest {
            account: Some("did:plc:x".to_string()),
            did: Some("did:plc:x".to_string()),
            note: None,
        };
        let err = disable_account_invites(State(ctx), admin_test_auth(), Json(req))
            .await
            .expect_err("both fields should be rejected");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_disable_account_invites_rejects_neither_field() {
        let ctx = create_test_context().await;
        let req = AccountInvitesRequest {
            account: None,
            did: None,
            note: None,
        };
        let err = disable_account_invites(State(ctx), admin_test_auth(), Json(req))
            .await
            .expect_err("missing both fields should be rejected");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_disable_account_invites_propagates_note_to_audit_log() {
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:noted", "noted.test", None).await;

        let req = AccountInvitesRequest {
            account: Some("did:plc:noted".to_string()),
            did: None,
            note: Some("Spam ring cleanup 2026-Q2".to_string()),
        };
        let _ = disable_account_invites(State(ctx.clone()), admin_test_auth(), Json(req))
            .await
            .unwrap();

        // Verify the audit log captured the note in the details column.
        let row: (String, String, Option<String>) = sqlx::query_as(
            "SELECT action, subject_did, details FROM admin_audit_log
             WHERE action = 'account.invites.disable' AND subject_did = ?",
        )
        .bind("did:plc:noted")
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert_eq!(row.0, "account.invites.disable");
        assert_eq!(row.1, "did:plc:noted");
        assert_eq!(row.2.as_deref(), Some("Spam ring cleanup 2026-Q2"));
    }

    #[tokio::test]
    async fn test_enable_account_invites_happy_path_uses_same_pattern() {
        // enableAccountInvites and disableAccountInvites share AccountInvitesRequest;
        // exercising one happy path here (in addition to the disable suite above)
        // confirms the symmetric handler registers and routes correctly.
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:enabled", "enabled.test", None).await;
        // Start in the disabled state so re-enabling is a real change.
        ctx.account_manager
            .disable_account_invites("did:plc:enabled")
            .await
            .unwrap();

        let req = AccountInvitesRequest {
            account: Some("enabled.test".to_string()),
            did: None,
            note: Some("Reinstated after appeal".to_string()),
        };
        let status = enable_account_invites(State(ctx.clone()), admin_test_auth(), Json(req))
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK);
        // Resolver mapped handle correctly and the underlying DB op flipped.
        assert!(!account_invites_disabled(&ctx, "did:plc:enabled").await);
    }

    /// Helper: read the current email column for a DID.
    async fn account_email(ctx: &AppContext, did: &str) -> Option<String> {
        use sqlx::Row;
        sqlx::query("SELECT email FROM account WHERE did = ?")
            .bind(did)
            .fetch_one(&ctx.account_db)
            .await
            .unwrap()
            .get(0)
    }

    #[tokio::test]
    async fn test_update_account_email_with_account_handle_form() {
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:emailtest", "email.test", Some("old@x.com")).await;

        let req = UpdateAccountEmailRequest {
            account: Some("email.test".to_string()),
            did: None,
            email: "new@example.com".to_string(),
        };
        let status = update_account_email(State(ctx.clone()), admin_test_auth(), Json(req))
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            account_email(&ctx, "did:plc:emailtest").await.as_deref(),
            Some("new@example.com")
        );
    }

    #[tokio::test]
    async fn test_update_account_email_with_legacy_did() {
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:legacyemail", "legacyemail.test", None).await;

        let req = UpdateAccountEmailRequest {
            account: None,
            did: Some("did:plc:legacyemail".to_string()),
            email: "back@compat.com".to_string(),
        };
        let status = update_account_email(State(ctx.clone()), admin_test_auth(), Json(req))
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            account_email(&ctx, "did:plc:legacyemail").await.as_deref(),
            Some("back@compat.com")
        );
    }

    // ---- Phase 1.8: sendEmail required-field flips ----------------------

    #[test]
    fn test_send_email_request_subject_is_optional() {
        // Spec says `subject` is optional. Aurora used to require it; verify
        // the deserializer now accepts a payload that omits subject.
        let json = serde_json::json!({
            "recipientDid": "did:plc:r",
            "content": "hello",
            "senderDid": "did:plc:s",
        });
        let req: SendEmailRequest = serde_json::from_value(json).unwrap();
        assert!(req.subject.is_none());
        assert_eq!(req.recipient_did, "did:plc:r");
        assert_eq!(req.sender_did.as_deref(), Some("did:plc:s"));
    }

    #[test]
    fn test_send_email_request_sender_did_remains_optional_aurora_extension() {
        // Spec says `senderDid` is required, but Aurora retains the
        // permissive extension allowing omission (defaults to authenticated
        // admin DID at handler time).
        let json = serde_json::json!({
            "recipientDid": "did:plc:r",
            "content": "hello",
        });
        let req: SendEmailRequest = serde_json::from_value(json).unwrap();
        assert!(req.subject.is_none());
        assert!(req.sender_did.is_none());
    }

    #[tokio::test]
    async fn test_send_email_subject_omitted_reaches_handler() {
        // With no subject and a missing recipient, we hit the account-lookup
        // error path. Reaching that path proves the request deserialized
        // (subject correctly optional) and the handler ran past the entry.
        let ctx = create_test_context().await;
        let req = SendEmailRequest {
            recipient_did: "did:plc:doesnotexist".to_string(),
            content: "ping".to_string(),
            subject: None,
            sender_did: Some("did:plc:admin".to_string()),
            comment: None,
        };
        let err = send_email(State(ctx), admin_test_auth(), Json(req))
            .await
            .expect_err("missing recipient should 404 — proves we reached the handler");
        assert_eq!(err.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_send_email_sender_did_omitted_defaults_to_admin() {
        // Same shape as above with senderDid omitted — should also reach
        // the handler (account-not-found 404), proving the Aurora-permissive
        // extension still deserializes.
        let ctx = create_test_context().await;
        let req = SendEmailRequest {
            recipient_did: "did:plc:doesnotexist".to_string(),
            content: "ping".to_string(),
            subject: Some("urgent".to_string()),
            sender_did: None,
            comment: None,
        };
        let err = send_email(State(ctx), admin_test_auth(), Json(req))
            .await
            .expect_err("missing recipient should 404");
        assert_eq!(err.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_send_email_subject_provided_back_compat() {
        // Existing callers still passing a subject get the same behavior.
        let ctx = create_test_context().await;
        let req = SendEmailRequest {
            recipient_did: "did:plc:doesnotexist".to_string(),
            content: "back compat".to_string(),
            subject: Some("Important".to_string()),
            sender_did: Some("did:plc:s".to_string()),
            comment: Some("ticket-1234".to_string()),
        };
        let err = send_email(State(ctx), admin_test_auth(), Json(req))
            .await
            .expect_err("missing recipient should 404");
        assert_eq!(err.0, StatusCode::NOT_FOUND);
    }

    // ---- Phase 1.6: updateSubjectStatus polymorphism ---------------------

    #[test]
    fn test_subject_union_repo_ref_round_trip() {
        let json = serde_json::json!({
            "$type": "com.atproto.admin.defs#repoRef",
            "did": "did:plc:abc"
        });
        let parsed: SubjectUnion = serde_json::from_value(json.clone()).unwrap();
        match &parsed {
            SubjectUnion::RepoRef { did } => assert_eq!(did, "did:plc:abc"),
            _ => panic!("expected RepoRef"),
        }
        let serialized = serde_json::to_value(&parsed).unwrap();
        assert_eq!(serialized, json);
    }

    #[test]
    fn test_subject_union_strong_ref_round_trip() {
        let json = serde_json::json!({
            "$type": "com.atproto.repo.strongRef",
            "uri": "at://did:plc:abc/app.bsky.feed.post/xyz",
            "cid": "bafyabc"
        });
        let parsed: SubjectUnion = serde_json::from_value(json.clone()).unwrap();
        match &parsed {
            SubjectUnion::StrongRef { uri, cid } => {
                assert_eq!(uri, "at://did:plc:abc/app.bsky.feed.post/xyz");
                assert_eq!(cid, "bafyabc");
            }
            _ => panic!("expected StrongRef"),
        }
        assert_eq!(serde_json::to_value(&parsed).unwrap(), json);
    }

    #[test]
    fn test_subject_union_repo_blob_ref_round_trip() {
        let json = serde_json::json!({
            "$type": "com.atproto.admin.defs#repoBlobRef",
            "did": "did:plc:abc",
            "cid": "bafyblob",
            "recordUri": "at://did:plc:abc/app.bsky.feed.post/xyz"
        });
        let parsed: SubjectUnion = serde_json::from_value(json.clone()).unwrap();
        match &parsed {
            SubjectUnion::RepoBlobRef {
                did,
                cid,
                record_uri,
            } => {
                assert_eq!(did, "did:plc:abc");
                assert_eq!(cid, "bafyblob");
                assert_eq!(record_uri.as_deref(), Some("at://did:plc:abc/app.bsky.feed.post/xyz"));
            }
            _ => panic!("expected RepoBlobRef"),
        }
        assert_eq!(serde_json::to_value(&parsed).unwrap(), json);
    }

    #[test]
    fn test_subject_union_rejects_missing_type_discriminator() {
        let json = serde_json::json!({"did": "did:plc:abc"});
        let result: Result<SubjectUnion, _> = serde_json::from_value(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_status_attr_round_trip() {
        let json = serde_json::json!({"applied": true, "ref": "ticket-1234"});
        let parsed: StatusAttr = serde_json::from_value(json).unwrap();
        assert!(parsed.applied);
        assert_eq!(parsed.ref_field.as_deref(), Some("ticket-1234"));

        let json_no_ref = serde_json::json!({"applied": false});
        let parsed: StatusAttr = serde_json::from_value(json_no_ref).unwrap();
        assert!(!parsed.applied);
        assert!(parsed.ref_field.is_none());
    }

    /// Helper: read takedown_ref off the actor table.
    async fn account_takedown_ref(ctx: &AppContext, did: &str) -> Option<String> {
        use sqlx::Row;
        sqlx::query("SELECT takedown_ref FROM actor WHERE did = ?")
            .bind(did)
            .fetch_one(&ctx.account_db)
            .await
            .unwrap()
            .get(0)
    }

    /// Helper: read deactivated_at off the actor table.
    async fn account_deactivated(ctx: &AppContext, did: &str) -> bool {
        use sqlx::Row;
        let row: Option<String> =
            sqlx::query("SELECT deactivated_at FROM actor WHERE did = ?")
                .bind(did)
                .fetch_one(&ctx.account_db)
                .await
                .unwrap()
                .get(0);
        row.is_some()
    }

    #[tokio::test]
    async fn test_update_subject_status_takedown_account() {
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:victim", "victim.test", None).await;

        let req = UpdateSubjectStatusRequest {
            subject: SubjectUnion::RepoRef {
                did: "did:plc:victim".to_string(),
            },
            takedown: Some(StatusAttr {
                applied: true,
                ref_field: Some("ticket-99".to_string()),
            }),
            deactivated: None,
        };
        let resp = update_subject_status(State(ctx.clone()), admin_test_auth(), Json(req))
            .await
            .unwrap()
            .0;
        // Echoes back the subject and the post-patch takedown state.
        match resp.subject {
            SubjectUnion::RepoRef { did } => assert_eq!(did, "did:plc:victim"),
            _ => panic!("expected RepoRef"),
        }
        let td = resp.takedown.expect("takedown should be echoed");
        assert!(td.applied);
        assert_eq!(td.ref_field.as_deref(), Some("ticket-99"));
        assert_eq!(
            account_takedown_ref(&ctx, "did:plc:victim").await.as_deref(),
            Some("ticket-99")
        );
    }

    #[tokio::test]
    async fn test_update_subject_status_restores_account_via_applied_false() {
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:revived", "revived.test", None).await;
        // Pre-takedown the account.
        ctx.account_manager
            .takedown_account("did:plc:revived", "ticket-old")
            .await
            .unwrap();
        assert!(account_takedown_ref(&ctx, "did:plc:revived").await.is_some());

        // Patch with applied=false -> implicit restore per spec.
        let req = UpdateSubjectStatusRequest {
            subject: SubjectUnion::RepoRef {
                did: "did:plc:revived".to_string(),
            },
            takedown: Some(StatusAttr {
                applied: false,
                ref_field: None,
            }),
            deactivated: None,
        };
        let resp = update_subject_status(State(ctx.clone()), admin_test_auth(), Json(req))
            .await
            .unwrap()
            .0;
        let td = resp.takedown.expect("takedown should be echoed");
        assert!(!td.applied);
        assert!(td.ref_field.is_none());
        assert!(account_takedown_ref(&ctx, "did:plc:revived").await.is_none());
    }

    #[tokio::test]
    async fn test_update_subject_status_deactivates_account() {
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:dorm", "dorm.test", None).await;

        let req = UpdateSubjectStatusRequest {
            subject: SubjectUnion::RepoRef {
                did: "did:plc:dorm".to_string(),
            },
            takedown: None,
            deactivated: Some(StatusAttr {
                applied: true,
                ref_field: None,
            }),
        };
        let _ = update_subject_status(State(ctx.clone()), admin_test_auth(), Json(req))
            .await
            .unwrap();
        assert!(account_deactivated(&ctx, "did:plc:dorm").await);
    }

    #[tokio::test]
    async fn test_update_subject_status_record_returns_501() {
        let ctx = create_test_context().await;
        let req = UpdateSubjectStatusRequest {
            subject: SubjectUnion::StrongRef {
                uri: "at://did:plc:foo/app.bsky.feed.post/xyz".to_string(),
                cid: "bafyabc".to_string(),
            },
            takedown: Some(StatusAttr {
                applied: true,
                ref_field: None,
            }),
            deactivated: None,
        };
        let resp = update_subject_status(State(ctx), admin_test_auth(), Json(req))
            .await
            .expect_err("strongRef should return 501 until record-level setter exists");
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
        assert!(body.contains("Record-level"));
    }

    /// Seed a blob row directly into `blob_metadata` so the existence
    /// check in `apply_blob_status` (`BlobStore::get_metadata`) finds it.
    /// Bypasses the upload path.
    async fn seed_test_blob(ctx: &AppContext, cid: &str, did: &str) {
        sqlx::query(
            "INSERT INTO blob_metadata (cid, mime_type, size, creator_did, created_at)
             VALUES (?, 'application/octet-stream', 0, ?, ?)",
        )
        .bind(cid)
        .bind(did)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&ctx.account_db)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_update_subject_status_takedown_blob() {
        let ctx = create_test_context().await;
        seed_test_blob(&ctx, "bafyblob01", "did:plc:owner").await;

        let req = UpdateSubjectStatusRequest {
            subject: SubjectUnion::RepoBlobRef {
                did: "did:plc:owner".to_string(),
                cid: "bafyblob01".to_string(),
                record_uri: None,
            },
            takedown: Some(StatusAttr {
                applied: true,
                ref_field: Some("legal-1".to_string()),
            }),
            deactivated: None,
        };
        let resp = update_subject_status(State(ctx.clone()), admin_test_auth(), Json(req))
            .await
            .unwrap()
            .0;
        let td = resp.takedown.expect("takedown should be echoed");
        assert!(td.applied);

        // Verify via the quarantine system directly.
        use crate::blob_store::quarantine::BlobQuarantine;
        let quarantine = BlobQuarantine::new(ctx.account_db.clone());
        assert!(quarantine.is_quarantined("bafyblob01").await.unwrap());
    }

    /// Helper: read body bytes and parse JSON for a Response error.
    async fn read_xrpc_error(resp: axum::response::Response) -> (StatusCode, serde_json::Value) {
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value =
            serde_json::from_slice(&bytes).expect("error body must be JSON");
        (status, json)
    }

    #[tokio::test]
    async fn test_update_subject_status_blob_deactivated_returns_400() {
        let ctx = create_test_context().await;
        seed_test_blob(&ctx, "bafyblob02", "did:plc:owner").await;

        let req = UpdateSubjectStatusRequest {
            subject: SubjectUnion::RepoBlobRef {
                did: "did:plc:owner".to_string(),
                cid: "bafyblob02".to_string(),
                record_uri: None,
            },
            takedown: None,
            deactivated: Some(StatusAttr {
                applied: true,
                ref_field: None,
            }),
        };
        let resp = update_subject_status(State(ctx), admin_test_auth(), Json(req))
            .await
            .expect_err("blob + deactivated should reject");
        let (status, body) = read_xrpc_error(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "InvalidRequest");
        assert!(body["message"]
            .as_str()
            .unwrap()
            .contains("not applicable to blob"));
    }

    #[tokio::test]
    async fn test_update_subject_status_record_deactivated_returns_400() {
        let ctx = create_test_context().await;
        let req = UpdateSubjectStatusRequest {
            subject: SubjectUnion::StrongRef {
                uri: "at://did:plc:foo/app.bsky.feed.post/xyz".to_string(),
                cid: "bafyabc".to_string(),
            },
            takedown: None,
            deactivated: Some(StatusAttr {
                applied: true,
                ref_field: None,
            }),
        };
        let resp = update_subject_status(State(ctx), admin_test_auth(), Json(req))
            .await
            .expect_err("record + deactivated should reject");
        let (status, body) = read_xrpc_error(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "InvalidRequest");
        assert!(body["message"]
            .as_str()
            .unwrap()
            .contains("not applicable to record"));
    }

    #[tokio::test]
    async fn test_update_subject_status_blob_not_found_returns_404() {
        let ctx = create_test_context().await;
        // Do not seed; the blob doesn't exist.

        let req = UpdateSubjectStatusRequest {
            subject: SubjectUnion::RepoBlobRef {
                did: "did:plc:owner".to_string(),
                cid: "bafynonexistent".to_string(),
                record_uri: None,
            },
            takedown: Some(StatusAttr {
                applied: true,
                ref_field: None,
            }),
            deactivated: None,
        };
        let resp = update_subject_status(State(ctx), admin_test_auth(), Json(req))
            .await
            .expect_err("non-existent blob should 404");
        let (status, body) = read_xrpc_error(resp).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"], "BlobNotFound");
        assert!(body["message"]
            .as_str()
            .unwrap()
            .contains("bafynonexistent"));
    }

    #[tokio::test]
    async fn test_update_subject_status_restore_non_quarantined_blob_idempotent() {
        let ctx = create_test_context().await;
        seed_test_blob(&ctx, "bafyblob03", "did:plc:owner").await;
        // Blob exists but is NOT quarantined; restore should succeed
        // (idempotent — desired post-state already obtains).

        let req = UpdateSubjectStatusRequest {
            subject: SubjectUnion::RepoBlobRef {
                did: "did:plc:owner".to_string(),
                cid: "bafyblob03".to_string(),
                record_uri: None,
            },
            takedown: Some(StatusAttr {
                applied: false,
                ref_field: None,
            }),
            deactivated: None,
        };
        let resp = update_subject_status(State(ctx), admin_test_auth(), Json(req))
            .await
            .unwrap()
            .0;
        let td = resp.takedown.expect("takedown should be echoed");
        assert!(!td.applied);
    }

    #[tokio::test]
    async fn test_update_subject_status_takedown_already_quarantined_idempotent() {
        let ctx = create_test_context().await;
        seed_test_blob(&ctx, "bafyblob04", "did:plc:owner").await;
        // Pre-quarantine.
        use crate::blob_store::quarantine::{BlobQuarantine, QuarantineReason};
        let quarantine = BlobQuarantine::new(ctx.account_db.clone());
        quarantine
            .quarantine_blob(
                "bafyblob04",
                QuarantineReason::Other,
                Some("first-takedown"),
                "did:plc:admin1",
                None,
            )
            .await
            .unwrap();

        // Repeat takedown — should succeed despite already-quarantined.
        let req = UpdateSubjectStatusRequest {
            subject: SubjectUnion::RepoBlobRef {
                did: "did:plc:owner".to_string(),
                cid: "bafyblob04".to_string(),
                record_uri: None,
            },
            takedown: Some(StatusAttr {
                applied: true,
                ref_field: Some("second-takedown".to_string()),
            }),
            deactivated: None,
        };
        let resp = update_subject_status(State(ctx), admin_test_auth(), Json(req))
            .await
            .unwrap()
            .0;
        let td = resp.takedown.expect("takedown should be echoed");
        assert!(td.applied);
    }

    #[tokio::test]
    async fn test_update_subject_status_rejects_malformed_subject() {
        // Subject without $type discriminator → serde rejects deserialization.
        let json = serde_json::json!({
            "subject": {"did": "did:plc:abc"},
            "takedown": {"applied": true}
        });
        let result: Result<UpdateSubjectStatusRequest, _> = serde_json::from_value(json);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_update_subject_status_rejects_status_attr_without_applied() {
        let json = serde_json::json!({
            "subject": {
                "$type": "com.atproto.admin.defs#repoRef",
                "did": "did:plc:abc"
            },
            "takedown": {"ref": "missing-applied-field"}
        });
        let result: Result<UpdateSubjectStatusRequest, _> = serde_json::from_value(json);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_update_subject_status_response_shape_matches_lexicon() {
        // Lexicon output: subject (required) + takedown (optional). No
        // deactivated. Verify the serialised JSON matches.
        let resp = UpdateSubjectStatusResponse {
            subject: SubjectUnion::RepoRef {
                did: "did:plc:abc".to_string(),
            },
            takedown: Some(StatusAttr {
                applied: true,
                ref_field: Some("ticket-1".to_string()),
            }),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["subject"]["$type"], "com.atproto.admin.defs#repoRef");
        assert_eq!(json["subject"]["did"], "did:plc:abc");
        assert_eq!(json["takedown"]["applied"], true);
        assert_eq!(json["takedown"]["ref"], "ticket-1");
        // No deactivated field per the lexicon's output schema.
        assert!(json.get("deactivated").is_none());

        // takedown can be omitted entirely when None.
        let resp_no_td = UpdateSubjectStatusResponse {
            subject: SubjectUnion::RepoRef {
                did: "did:plc:xyz".to_string(),
            },
            takedown: None,
        };
        let json = serde_json::to_value(&resp_no_td).unwrap();
        assert!(json.get("takedown").is_none());
    }

    // ---- Phase 1.10: invite-code pagination -------------------------------

    /// Helper that creates `n` invite codes with a small delay between each
    /// so they have distinct `created_at` timestamps. Returns codes in
    /// creation order (oldest first).
    async fn seed_invite_codes(ctx: &AppContext, n: usize) -> Vec<crate::admin::InviteCode> {
        let mut codes = Vec::with_capacity(n);
        for i in 0..n {
            let c = ctx
                .invite_manager
                .create_invite("did:plc:creator", 5, None, Some(format!("seed {i}")), None)
                .await
                .unwrap();
            codes.push(c);
            // SQLite chrono RFC3339 strings are millisecond-resolution; a
            // tiny sleep keeps timestamps strictly distinct so the cursor
            // tuple boundary doesn't get ambiguous in tests.
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        codes
    }

    #[tokio::test]
    async fn test_get_invite_codes_sort_recent_returns_newest_first() {
        let ctx = create_test_context().await;
        let seeded = seed_invite_codes(&ctx, 3).await;

        let resp = get_invite_codes(
            State(ctx),
            admin_test_auth(),
            Query(GetInviteCodesQuery {
                sort: Some("recent".to_string()),
                limit: None,
                cursor: None,
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(resp.codes.len(), 3);
        // Newest-first: reverse of seed order.
        assert_eq!(resp.codes[0].code, seeded[2].code);
        assert_eq!(resp.codes[1].code, seeded[1].code);
        assert_eq!(resp.codes[2].code, seeded[0].code);
        assert!(resp.cursor.is_none()); // 3 codes, no more pages
    }

    #[tokio::test]
    async fn test_get_invite_codes_sort_usage_orders_by_use_count() {
        let ctx = create_test_context().await;
        let seeded = seed_invite_codes(&ctx, 3).await;
        // Record uses: seeded[0] used twice, seeded[1] used once, seeded[2] zero.
        for (idx, count) in [2u32, 1, 0].iter().enumerate() {
            for _ in 0..*count {
                sqlx::query(
                    "INSERT INTO invite_code_use (code, used_by, used_at) VALUES (?, ?, ?)",
                )
                .bind(&seeded[idx].code)
                .bind("did:plc:user")
                .bind(chrono::Utc::now().to_rfc3339())
                .execute(&ctx.account_db)
                .await
                .unwrap();
            }
        }

        let resp = get_invite_codes(
            State(ctx),
            admin_test_auth(),
            Query(GetInviteCodesQuery {
                sort: Some("usage".to_string()),
                limit: None,
                cursor: None,
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(resp.codes.len(), 3);
        // Most-used first.
        assert_eq!(resp.codes[0].code, seeded[0].code); // 2 uses
        assert_eq!(resp.codes[1].code, seeded[1].code); // 1 use
        assert_eq!(resp.codes[2].code, seeded[2].code); // 0 uses
    }

    #[tokio::test]
    async fn test_get_invite_codes_paginates_with_cursor_recent() {
        let ctx = create_test_context().await;
        let seeded = seed_invite_codes(&ctx, 5).await;

        // Page 1 of 2 with a cursor.
        let page1 = get_invite_codes(
            State(ctx.clone()),
            admin_test_auth(),
            Query(GetInviteCodesQuery {
                sort: Some("recent".to_string()),
                limit: Some(2),
                cursor: None,
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(page1.codes.len(), 2);
        assert_eq!(page1.codes[0].code, seeded[4].code); // newest
        assert_eq!(page1.codes[1].code, seeded[3].code);
        let cursor1 = page1.cursor.expect("more results, cursor expected");

        // Page 2.
        let page2 = get_invite_codes(
            State(ctx.clone()),
            admin_test_auth(),
            Query(GetInviteCodesQuery {
                sort: Some("recent".to_string()),
                limit: Some(2),
                cursor: Some(cursor1),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(page2.codes.len(), 2);
        assert_eq!(page2.codes[0].code, seeded[2].code);
        assert_eq!(page2.codes[1].code, seeded[1].code);
        let cursor2 = page2.cursor.expect("one more page expected");

        // Page 3 finishes the set.
        let page3 = get_invite_codes(
            State(ctx),
            admin_test_auth(),
            Query(GetInviteCodesQuery {
                sort: Some("recent".to_string()),
                limit: Some(2),
                cursor: Some(cursor2),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(page3.codes.len(), 1);
        assert_eq!(page3.codes[0].code, seeded[0].code); // oldest
        assert!(page3.cursor.is_none());
    }

    #[tokio::test]
    async fn test_get_invite_codes_rejects_out_of_range_limit() {
        let ctx = create_test_context().await;
        let result = get_invite_codes(
            State(ctx.clone()),
            admin_test_auth(),
            Query(GetInviteCodesQuery {
                sort: None,
                limit: Some(0),
                cursor: None,
            }),
        )
        .await;
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("limit=0 should be rejected"),
        };
        assert_eq!(err.0, StatusCode::BAD_REQUEST);

        let result = get_invite_codes(
            State(ctx),
            admin_test_auth(),
            Query(GetInviteCodesQuery {
                sort: None,
                limit: Some(501),
                cursor: None,
            }),
        )
        .await;
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("limit=501 should be rejected"),
        };
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_get_invite_codes_rejects_invalid_sort() {
        let ctx = create_test_context().await;
        let result = get_invite_codes(
            State(ctx),
            admin_test_auth(),
            Query(GetInviteCodesQuery {
                sort: Some("alphabetical".to_string()),
                limit: None,
                cursor: None,
            }),
        )
        .await;
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("invalid sort should be rejected"),
        };
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1.contains("recent") && err.1.contains("usage"));
    }

    #[tokio::test]
    async fn test_get_invite_codes_rejects_cursor_with_mismatched_sort() {
        let ctx = create_test_context().await;
        let _ = seed_invite_codes(&ctx, 3).await;

        // Get a cursor for sort=recent.
        let page1 = get_invite_codes(
            State(ctx.clone()),
            admin_test_auth(),
            Query(GetInviteCodesQuery {
                sort: Some("recent".to_string()),
                limit: Some(1),
                cursor: None,
            }),
        )
        .await
        .unwrap()
        .0;
        let recent_cursor = page1.cursor.unwrap();

        // Replay the same cursor on sort=usage; should be rejected.
        let result = get_invite_codes(
            State(ctx),
            admin_test_auth(),
            Query(GetInviteCodesQuery {
                sort: Some("usage".to_string()),
                limit: Some(1),
                cursor: Some(recent_cursor),
            }),
        )
        .await;
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("mismatched sort+cursor should be rejected"),
        };
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_list_invite_codes_paginates_too() {
        let ctx = create_test_context().await;
        let _ = seed_invite_codes(&ctx, 3).await;

        let page1 = list_invite_codes(
            State(ctx.clone()),
            admin_test_auth(),
            Query(ListInviteCodesQuery {
                sort: None,
                limit: Some(2),
                cursor: None,
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(page1.codes.len(), 2);
        let cursor = page1.cursor.expect("more results expected");

        let page2 = list_invite_codes(
            State(ctx),
            admin_test_auth(),
            Query(ListInviteCodesQuery {
                sort: None,
                limit: Some(2),
                cursor: Some(cursor),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(page2.codes.len(), 1);
        assert!(page2.cursor.is_none());
    }

    // ---- Phase 1.9: getAccountInfos param encoding + handle field --------

    #[tokio::test]
    async fn test_get_account_infos_repeated_query_params_returns_both() {
        use axum_extra::extract::Query as ExtraQuery;

        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:one", "one.test", Some("one@x")).await;
        seed_test_account(&ctx, "did:plc:two", "two.test", Some("two@x")).await;

        // Simulate axum-extra's parsing of `?dids=did:plc:one&dids=did:plc:two`.
        let query = GetAccountInfosQuery {
            dids: vec!["did:plc:one".to_string(), "did:plc:two".to_string()],
        };
        let resp = get_account_infos(State(ctx), admin_test_auth(), ExtraQuery(query))
            .await
            .unwrap()
            .0;
        assert_eq!(resp.infos.len(), 2);
        // Order matches input order; handle is now required (String, not Option).
        assert_eq!(resp.infos[0].did, "did:plc:one");
        assert_eq!(resp.infos[0].handle, "one.test");
        assert_eq!(resp.infos[1].did, "did:plc:two");
        assert_eq!(resp.infos[1].handle, "two.test");
    }

    #[tokio::test]
    async fn test_get_account_infos_silently_skips_missing_dids() {
        use axum_extra::extract::Query as ExtraQuery;

        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:exists", "exists.test", Some("e@x")).await;

        let query = GetAccountInfosQuery {
            dids: vec![
                "did:plc:exists".to_string(),
                "did:plc:doesnotexist".to_string(),
                "not-a-did-at-all".to_string(),
            ],
        };
        let resp = get_account_infos(State(ctx), admin_test_auth(), ExtraQuery(query))
            .await
            .unwrap()
            .0;
        // Existing skip-on-error behavior preserved: only the present account
        // appears, and the malformed entry is filtered before lookup.
        assert_eq!(resp.infos.len(), 1);
        assert_eq!(resp.infos[0].did, "did:plc:exists");
    }

    #[tokio::test]
    async fn test_get_account_infos_empty_array_400() {
        use axum_extra::extract::Query as ExtraQuery;

        let ctx = create_test_context().await;
        let query = GetAccountInfosQuery { dids: vec![] };
        let result = get_account_infos(State(ctx), admin_test_auth(), ExtraQuery(query)).await;
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("empty dids array should return 400"),
        };
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn test_account_info_handle_is_required_in_response_shape() {
        // Verify the serialized accountView has handle as a string, not as
        // null or missing. Spec marks handle as required.
        let info = AccountInfo {
            did: "did:plc:foo".to_string(),
            handle: "foo.test".to_string(),
            email: None,
            indexed_at: "2026-01-01T00:00:00Z".to_string(),
            email_confirmed_at: None,
            invited_by: None,
            invites: vec![],
            invites_disabled: false,
            invite_note: None,
            deactivated_at: None,
            threat_signatures: vec![],
        };
        let json = serde_json::to_value(&info).unwrap();
        // Must be a string, not null and not missing.
        assert!(json.get("handle").unwrap().is_string());
        assert_eq!(json["handle"], "foo.test");
    }

    #[tokio::test]
    async fn test_get_account_info_singular_returns_required_handle() {
        // The shared AccountInfo struct propagates to the singular endpoint
        // too — handle is required there as well.
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:single", "single.test", Some("s@x")).await;

        let resp = get_account_info(
            State(ctx),
            admin_test_auth(),
            Query(GetAccountInfoQuery {
                did: "did:plc:single".to_string(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(resp.handle, "single.test");
    }

    #[tokio::test]
    async fn test_search_accounts_returns_required_handle() {
        // Same propagation check for the searchAccounts endpoint.
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:srch", "srch.test", Some("s@x")).await;

        let resp = search_accounts(
            State(ctx),
            admin_test_auth(),
            Query(SearchAccountsQuery {
                email: Some("s@x".to_string()),
                cursor: None,
                limit: None,
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(resp.accounts.len(), 1);
        assert_eq!(resp.accounts[0].handle, "srch.test");
    }

    #[tokio::test]
    async fn test_send_email_account_without_email_400() {
        // Seed a recipient with no email; verify the handler reaches the
        // mailer step and rejects with 400 once it discovers the account
        // has no address. Confirms subject defaulting doesn't blow up in
        // the path before the email-presence check.
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:noemail", "noemail.test", None).await;

        let req = SendEmailRequest {
            recipient_did: "did:plc:noemail".to_string(),
            content: "hi".to_string(),
            subject: None, // exercises DEFAULT_EMPTY_SUBJECT path
            sender_did: Some("did:plc:s".to_string()),
            comment: None,
        };
        let err = send_email(State(ctx), admin_test_auth(), Json(req))
            .await
            .expect_err("account without email should 400");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1.contains("no email"));
    }

    #[tokio::test]
    async fn test_update_account_email_rejects_both_fields() {
        let ctx = create_test_context().await;
        let req = UpdateAccountEmailRequest {
            account: Some("did:plc:x".to_string()),
            did: Some("did:plc:x".to_string()),
            email: "x@y.com".to_string(),
        };
        let err = update_account_email(State(ctx), admin_test_auth(), Json(req))
            .await
            .expect_err("both fields should be rejected");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_search_accounts_paginates_with_cursor() {
        let ctx = create_test_context().await;
        // Three accounts, ordered did:plc:a < did:plc:b < did:plc:c.
        seed_test_account(&ctx, "did:plc:a", "a.test", Some("a@x")).await;
        seed_test_account(&ctx, "did:plc:b", "b.test", Some("b@x")).await;
        seed_test_account(&ctx, "did:plc:c", "c.test", Some("c@x")).await;

        // Page size 2 → first page returns a, b with cursor = "did:plc:b".
        let page1 = search_accounts(
            State(ctx.clone()),
            admin_test_auth(),
            Query(SearchAccountsQuery {
                email: None,
                cursor: None,
                limit: Some(2),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(page1.accounts.len(), 2);
        assert_eq!(page1.accounts[0].did, "did:plc:a");
        assert_eq!(page1.accounts[1].did, "did:plc:b");
        assert_eq!(page1.cursor.as_deref(), Some("did:plc:b"));

        // Second page picks up after the cursor; returns c, no further cursor.
        let page2 = search_accounts(
            State(ctx),
            admin_test_auth(),
            Query(SearchAccountsQuery {
                email: None,
                cursor: page1.cursor,
                limit: Some(2),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(page2.accounts.len(), 1);
        assert_eq!(page2.accounts[0].did, "did:plc:c");
        assert!(page2.cursor.is_none());
    }
}
