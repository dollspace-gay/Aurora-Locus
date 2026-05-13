//! Moderator-tier read endpoints under `tools.aurora.moderator.*`.
//!
//! Implements Phase 3.3 (chainlink #100) and Phase 3.4 (chainlink
//! #101) per the [design doc](../../docs/AURORA_DESIGN.md) §4.3.1:
//!
//! - `queryEvents` — paginated query of moderation events
//! - `getEvent` — single event by ID
//! - `queryStatuses` — paginated query of subject statuses
//! - `getSubjectContext` — comprehensive view of single subject
//! - `getSubjectHistory` — chronological action history for subject
//! - `listAppeals` — paginated appeals query (3.4)
//! - `getAppeal` — single appeal by ID with timeline (3.4)
//!
//! All endpoints share rich-context helpers (`resolve_handles`,
//! `fetch_action_summaries`) that batch resolution per response page
//! rather than N+1 per item.
//!
//! Auth: Moderator+ via `AdminAuthContext` (matches Phase 2.3 ops
//! convention). The namespace scope-check middleware also gates
//! `tools.aurora.moderator.*` to `atproto:admin.moderation` per
//! Phase 2.2 substrate, so OAuth-authed callers without the right
//! scope get rejected before reaching the handler.

use crate::{
    admin::defs::{
        AuroraAdminError, CursorPosition, PaginatedResponse, PaginationParams, Subject,
        SubjectType,
    },
    auth::AdminAuthContext,
    error::PdsError,
    AppContext,
};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::collections::{HashMap, HashSet};

// ===========================================================================
// Response types — rich-context views
// ===========================================================================

/// A moderation event with rich resolved context (handles + metadata).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventWithContext {
    pub id: i64,
    pub event_type: String,
    pub actor_did: String,
    pub actor_handle: Option<String>,
    pub subject: Option<Subject>,
    pub subject_handle: Option<String>,
    pub details: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

/// A status row (account_moderation entry) with rich resolved context.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusWithContext {
    pub id: i64,
    pub did: String,
    pub handle: Option<String>,
    pub action: String,
    pub reason: String,
    pub moderated_by: String,
    pub moderated_by_handle: Option<String>,
    pub moderated_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub reversed: bool,
    pub reversed_at: Option<DateTime<Utc>>,
    pub report_id: Option<i64>,
}

/// Comprehensive view of a single subject — current status, recent
/// action history, related reports, related appeals.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubjectContextResponse {
    pub subject: Subject,
    pub primary_did: Option<String>,
    pub handle: Option<String>,
    pub current_status: Option<CurrentStatus>,
    pub recent_actions: Vec<StatusWithContext>,
    pub related_reports: Vec<RelatedReport>,
    pub related_appeals: Vec<RelatedAppeal>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentStatus {
    pub takedown_ref: Option<String>,
    pub deactivated_at: Option<DateTime<Utc>>,
    pub active_action: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelatedReport {
    pub id: i64,
    pub reason_type: String,
    pub reason: Option<String>,
    pub reported_by: String,
    pub reported_at: DateTime<Utc>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelatedAppeal {
    pub id: i64,
    pub appellant_did: String,
    pub appellant_handle: Option<String>,
    pub status: String,
    pub submitted_at: DateTime<Utc>,
}

// ===========================================================================
// Rich-context helpers — batch resolution
// ===========================================================================

/// Batch handle resolution. One SQL query per call regardless of
/// input size; returns a map suitable for `.get(did).cloned()`
/// lookups while constructing response items.
///
/// DIDs not present in the actor table simply don't appear in the
/// returned map — callers should treat missing entries as `None`.
pub(crate) async fn resolve_handles(
    ctx: &AppContext,
    dids: &[String],
) -> Result<HashMap<String, String>, PdsError> {
    if dids.is_empty() {
        return Ok(HashMap::new());
    }
    // Deduplicate — common case is many events sharing the same actor.
    let unique: HashSet<&str> = dids.iter().map(|s| s.as_str()).collect();
    // Build "($1, $2, ..., $N)" placeholder list.
    let placeholders: Vec<String> = (1..=unique.len()).map(|i| format!("${}", i)).collect();
    let sql = format!(
        "SELECT did, handle FROM actor WHERE did IN ({})",
        placeholders.join(", ")
    );
    let mut q = sqlx::query(&sql);
    for did in &unique {
        q = q.bind(*did);
    }
    let rows = q.fetch_all(&ctx.account_db).await?;
    let mut out = HashMap::with_capacity(rows.len());
    for row in rows {
        let did: String = row.try_get("did")?;
        let handle: String = row.try_get("handle")?;
        out.insert(did, handle);
    }
    Ok(out)
}

// ===========================================================================
// Cursor + pagination helpers
// ===========================================================================

/// Parse RFC3339 timestamp from sqlx column. Mirrors the cycle's
/// established parse_timestamp pattern (per Phase 3 cycle convention,
/// duplicated rather than centralized).
fn parse_ts(s: &str) -> Result<DateTime<Utc>, PdsError> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| PdsError::Internal(format!("Invalid timestamp: {}", e)))
}

fn parse_ts_opt(s: Option<String>) -> Result<Option<DateTime<Utc>>, PdsError> {
    match s {
        None => Ok(None),
        Some(s) => parse_ts(&s).map(Some),
    }
}

// ===========================================================================
// 5.2.1 — queryEvents
// ===========================================================================

/// Query parameters for `tools.aurora.moderator.queryEvents`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryEventsParams {
    /// Filter by event type (e.g. `account_takedown`).
    #[serde(default)]
    pub event_type: Option<String>,
    /// Filter by actor DID.
    #[serde(default)]
    pub actor: Option<String>,
    /// Filter by subject DID.
    #[serde(default)]
    pub subject_did: Option<String>,
    /// Lower bound on `created_at` (inclusive), RFC3339.
    #[serde(default)]
    pub after: Option<String>,
    /// Upper bound on `created_at` (inclusive), RFC3339.
    #[serde(default)]
    pub before: Option<String>,
    #[serde(flatten)]
    pub pagination: PaginationParams,
}

pub async fn query_events(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(params): Query<QueryEventsParams>,
) -> Result<Json<PaginatedResponse<EventWithContext>>, (StatusCode, Json<serde_json::Value>)> {
    let limit = params.pagination.effective_limit() as i64;
    let cursor = params.pagination.decode_cursor().map_err(|_| {
        let e = AuroraAdminError::OutdatedCursor;
        (e.http_status(), Json(serde_json::json!({"error": e.code()})))
    })?;

    // Build the dynamic query. We collect WHERE clauses + binds then
    // glue them together — keeps placeholder numbering correct.
    let mut clauses: Vec<&'static str> = Vec::new();
    let mut binds: Vec<String> = Vec::new();

    if let Some(t) = &params.event_type {
        clauses.push("event_type = ?");
        binds.push(t.clone());
    }
    if let Some(a) = &params.actor {
        clauses.push("actor_did = ?");
        binds.push(a.clone());
    }
    if let Some(s) = &params.subject_did {
        clauses.push("subject_did = ?");
        binds.push(s.clone());
    }
    if let Some(a) = &params.after {
        clauses.push("created_at >= ?");
        binds.push(a.clone());
    }
    if let Some(b) = &params.before {
        clauses.push("created_at <= ?");
        binds.push(b.clone());
    }
    // Cursor: rows older than the cursor's (created_at, id) tuple.
    if let Some(c) = &cursor {
        clauses.push("(created_at < ? OR (created_at = ? AND id < ?))");
        binds.push(c.after_created.to_rfc3339());
        binds.push(c.after_created.to_rfc3339());
    }

    // Convert ?-style placeholders to $N for Postgres compatibility
    // (Phase 5.0 portability lesson). The clauses above are all
    // ?-style by convention; rebuild with $N.
    let clauses_pg = renumber_placeholders(&clauses, &binds, cursor.is_some());

    let where_sql = if clauses_pg.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses_pg.join(" AND "))
    };
    let limit_idx = binds.len() + if cursor.is_some() { 2 } else { 1 };
    let sql = format!(
        "SELECT id, event_type, actor_did, subject_did, subject_uri, subject_cid, \
                details, created_at \
         FROM moderation_event{} \
         ORDER BY created_at DESC, id DESC \
         LIMIT ${}",
        where_sql, limit_idx
    );

    let mut q = sqlx::query(&sql);
    for b in &binds {
        q = q.bind(b);
    }
    if let Some(c) = &cursor {
        q = q.bind(c.after_id);
    }
    q = q.bind(limit + 1);

    let rows = q.fetch_all(&ctx.account_db).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Internal", "message": e.to_string()})),
        )
    })?;

    // Detect whether there's another page.
    let has_more = rows.len() as i64 > limit;
    let page_rows: Vec<_> = rows.into_iter().take(limit as usize).collect();

    // Collect distinct DIDs for batch handle resolution.
    let mut dids = Vec::new();
    for row in &page_rows {
        if let Ok(d) = row.try_get::<String, _>("actor_did") {
            dids.push(d);
        }
        if let Ok(Some(d)) = row.try_get::<Option<String>, _>("subject_did") {
            dids.push(d);
        }
    }
    let handles = resolve_handles(&ctx, &dids).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Internal", "message": e.to_string()})),
        )
    })?;

    // Build response items.
    let mut items = Vec::with_capacity(page_rows.len());
    let mut last_created_at = None;
    let mut last_id = None;
    for row in page_rows {
        let id: i64 = row.try_get("id").map_err(internal)?;
        let event_type: String = row.try_get("event_type").map_err(internal)?;
        let actor_did: String = row.try_get("actor_did").map_err(internal)?;
        let subject_did: Option<String> =
            row.try_get("subject_did").ok().flatten();
        let subject_uri: Option<String> =
            row.try_get("subject_uri").ok().flatten();
        let subject_cid: Option<String> =
            row.try_get("subject_cid").ok().flatten();
        let details_str: String = row.try_get("details").map_err(internal)?;
        let created_at = parse_ts(&row.try_get::<String, _>("created_at").map_err(internal)?)
            .map_err(internal_pds)?;

        let subject = Subject::from_columns(
            subject_did.as_deref(),
            subject_uri.as_deref(),
            subject_cid.as_deref(),
        );
        let actor_handle = handles.get(&actor_did).cloned();
        let subject_handle = subject_did.as_deref().and_then(|d| handles.get(d).cloned());

        last_created_at = Some(created_at);
        last_id = Some(id);

        items.push(EventWithContext {
            id,
            event_type,
            actor_did,
            actor_handle,
            subject,
            subject_handle,
            details: serde_json::from_str(&details_str)
                .unwrap_or(serde_json::Value::String(details_str)),
            created_at,
        });
    }

    let next_cursor = if has_more {
        match (last_created_at, last_id) {
            (Some(t), Some(i)) => Some(
                CursorPosition {
                    after_created: t,
                    after_id: i,
                }
                .encode(),
            ),
            _ => None,
        }
    } else {
        None
    };

    Ok(Json(PaginatedResponse {
        items,
        cursor: next_cursor,
    }))
}

/// Helper: convert ?-style placeholders to $N for Postgres. The
/// `cursor_present` bool tells us whether the cursor's two binds
/// (created_at appears twice) need to consume two placeholder
/// numbers in the cursor clause.
fn renumber_placeholders(
    clauses: &[&'static str],
    binds: &[String],
    cursor_present: bool,
) -> Vec<String> {
    let _ = binds;
    let _ = cursor_present;
    let mut idx = 1usize;
    clauses
        .iter()
        .map(|clause| {
            let mut out = String::with_capacity(clause.len() + 8);
            for c in clause.chars() {
                if c == '?' {
                    out.push_str(&format!("${}", idx));
                    idx += 1;
                } else {
                    out.push(c);
                }
            }
            out
        })
        .collect()
}

fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({"error": "Internal", "message": e.to_string()})),
    )
}
fn internal_pds(e: PdsError) -> (StatusCode, Json<serde_json::Value>) {
    internal(e)
}

// ===========================================================================
// 5.2.2 — getEvent
// ===========================================================================

#[derive(Debug, Deserialize)]
pub struct GetEventParams {
    pub id: i64,
}

pub async fn get_event(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(params): Query<GetEventParams>,
) -> Result<Json<EventWithContext>, (StatusCode, Json<serde_json::Value>)> {
    let row = sqlx::query(
        "SELECT id, event_type, actor_did, subject_did, subject_uri, subject_cid, \
                details, created_at \
         FROM moderation_event WHERE id = $1",
    )
    .bind(params.id)
    .fetch_optional(&ctx.account_db)
    .await
    .map_err(internal)?;

    let row = row.ok_or_else(|| -> (StatusCode, Json<serde_json::Value>) {
        AuroraAdminError::SubjectNotFound.into()
    })?;

    let id: i64 = row.try_get("id").map_err(internal)?;
    let event_type: String = row.try_get("event_type").map_err(internal)?;
    let actor_did: String = row.try_get("actor_did").map_err(internal)?;
    let subject_did: Option<String> = row.try_get("subject_did").ok().flatten();
    let subject_uri: Option<String> = row.try_get("subject_uri").ok().flatten();
    let subject_cid: Option<String> = row.try_get("subject_cid").ok().flatten();
    let details_str: String = row.try_get("details").map_err(internal)?;
    let created_at =
        parse_ts(&row.try_get::<String, _>("created_at").map_err(internal)?).map_err(internal_pds)?;

    let mut dids = vec![actor_did.clone()];
    if let Some(d) = &subject_did {
        dids.push(d.clone());
    }
    let handles = resolve_handles(&ctx, &dids).await.map_err(internal_pds)?;
    let actor_handle = handles.get(&actor_did).cloned();
    let subject_handle = subject_did.as_deref().and_then(|d| handles.get(d).cloned());
    let subject = Subject::from_columns(
        subject_did.as_deref(),
        subject_uri.as_deref(),
        subject_cid.as_deref(),
    );

    Ok(Json(EventWithContext {
        id,
        event_type,
        actor_did,
        actor_handle,
        subject,
        subject_handle,
        details: serde_json::from_str(&details_str)
            .unwrap_or(serde_json::Value::String(details_str)),
        created_at,
    }))
}

// ===========================================================================
// 5.2.3 — queryStatuses
// ===========================================================================

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryStatusesParams {
    /// Filter by subject DID.
    #[serde(default)]
    pub did: Option<String>,
    /// Filter by subject type — Account|Record|Blob. Currently only
    /// Account is meaningful (account_moderation table is per-DID);
    /// Record/Blob filters are accepted but yield empty results until
    /// per-record/per-blob status surfaces ship.
    #[serde(default)]
    pub subject_type: Option<SubjectType>,
    /// Filter by action type (e.g. `takedown`, `suspend`).
    #[serde(default)]
    pub action: Option<String>,
    /// Include reversed actions (default: true — historical view).
    #[serde(default)]
    pub include_reversed: Option<bool>,
    #[serde(flatten)]
    pub pagination: PaginationParams,
}

pub async fn query_statuses(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(params): Query<QueryStatusesParams>,
) -> Result<Json<PaginatedResponse<StatusWithContext>>, (StatusCode, Json<serde_json::Value>)> {
    // Record/Blob filters: account_moderation only stores per-DID
    // actions; return empty quickly rather than scanning.
    if let Some(SubjectType::Record | SubjectType::Blob) = params.subject_type {
        return Ok(Json(PaginatedResponse {
            items: Vec::new(),
            cursor: None,
        }));
    }

    let limit = params.pagination.effective_limit() as i64;
    let cursor = params.pagination.decode_cursor().map_err(|_| {
        let e = AuroraAdminError::OutdatedCursor;
        (e.http_status(), Json(serde_json::json!({"error": e.code()})))
    })?;

    let mut clauses: Vec<&'static str> = Vec::new();
    let mut binds: Vec<String> = Vec::new();

    if let Some(d) = &params.did {
        clauses.push("did = ?");
        binds.push(d.clone());
    }
    if let Some(a) = &params.action {
        clauses.push("action = ?");
        binds.push(a.clone());
    }
    if !params.include_reversed.unwrap_or(true) {
        clauses.push("NOT reversed");
    }
    if let Some(c) = &cursor {
        clauses.push("(moderated_at < ? OR (moderated_at = ? AND id < ?))");
        binds.push(c.after_created.to_rfc3339());
        binds.push(c.after_created.to_rfc3339());
    }

    let clauses_pg = renumber_placeholders(&clauses, &binds, cursor.is_some());
    let where_sql = if clauses_pg.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses_pg.join(" AND "))
    };
    let limit_idx = binds.len() + if cursor.is_some() { 2 } else { 1 };
    let sql = format!(
        "SELECT id, did, action, reason, moderated_by, moderated_at, expires_at, \
                reversed, reversed_at, report_id \
         FROM account_moderation{} \
         ORDER BY moderated_at DESC, id DESC \
         LIMIT ${}",
        where_sql, limit_idx
    );

    let mut q = sqlx::query(&sql);
    for b in &binds {
        q = q.bind(b);
    }
    if let Some(c) = &cursor {
        q = q.bind(c.after_id);
    }
    q = q.bind(limit + 1);

    let rows = q.fetch_all(&ctx.account_db).await.map_err(internal)?;
    let has_more = rows.len() as i64 > limit;
    let page_rows: Vec<_> = rows.into_iter().take(limit as usize).collect();

    let mut dids = Vec::new();
    for row in &page_rows {
        if let Ok(d) = row.try_get::<String, _>("did") {
            dids.push(d);
        }
        if let Ok(d) = row.try_get::<String, _>("moderated_by") {
            dids.push(d);
        }
    }
    let handles = resolve_handles(&ctx, &dids).await.map_err(internal_pds)?;

    let mut items = Vec::with_capacity(page_rows.len());
    let mut last_at = None;
    let mut last_id = None;
    for row in page_rows {
        let id: i64 = row.try_get("id").map_err(internal)?;
        let did: String = row.try_get("did").map_err(internal)?;
        let action: String = row.try_get("action").map_err(internal)?;
        let reason: String = row.try_get("reason").map_err(internal)?;
        let moderated_by: String = row.try_get("moderated_by").map_err(internal)?;
        let moderated_at = parse_ts(
            &row.try_get::<String, _>("moderated_at").map_err(internal)?,
        )
        .map_err(internal_pds)?;
        let expires_at =
            parse_ts_opt(row.try_get::<Option<String>, _>("expires_at").ok().flatten())
                .map_err(internal_pds)?;
        let reversed = crate::db::read_bool(&row, "reversed").map_err(internal)?;
        let reversed_at =
            parse_ts_opt(row.try_get::<Option<String>, _>("reversed_at").ok().flatten())
                .map_err(internal_pds)?;
        let report_id: Option<i64> = row.try_get("report_id").ok().flatten();

        last_at = Some(moderated_at);
        last_id = Some(id);

        let handle = handles.get(&did).cloned();
        let moderated_by_handle = handles.get(&moderated_by).cloned();
        items.push(StatusWithContext {
            id,
            did,
            handle,
            action,
            reason,
            moderated_by,
            moderated_by_handle,
            moderated_at,
            expires_at,
            reversed,
            reversed_at,
            report_id,
        });
    }

    let next_cursor = if has_more {
        match (last_at, last_id) {
            (Some(t), Some(i)) => Some(
                CursorPosition {
                    after_created: t,
                    after_id: i,
                }
                .encode(),
            ),
            _ => None,
        }
    } else {
        None
    };

    Ok(Json(PaginatedResponse {
        items,
        cursor: next_cursor,
    }))
}

// ===========================================================================
// 5.2.4 — getSubjectContext
// ===========================================================================

#[derive(Debug, Deserialize)]
pub struct GetSubjectContextParams {
    pub did: String,
}

pub async fn get_subject_context(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(params): Query<GetSubjectContextParams>,
) -> Result<Json<SubjectContextResponse>, (StatusCode, Json<serde_json::Value>)> {
    let did = params.did.clone();

    // Current actor row (handle, takedown_ref, deactivated_at).
    let actor_row = sqlx::query(
        "SELECT handle, takedown_ref, deactivated_at FROM actor WHERE did = $1",
    )
    .bind(&did)
    .fetch_optional(&ctx.account_db)
    .await
    .map_err(internal)?;

    let actor_row = actor_row.ok_or_else(|| -> (StatusCode, Json<serde_json::Value>) {
        AuroraAdminError::SubjectNotFound.into()
    })?;
    let handle: Option<String> = actor_row.try_get("handle").ok();
    let takedown_ref: Option<String> = actor_row.try_get("takedown_ref").ok().flatten();
    let deactivated_at = parse_ts_opt(
        actor_row
            .try_get::<Option<String>, _>("deactivated_at")
            .ok()
            .flatten(),
    )
    .map_err(internal_pds)?;

    // Active (not-reversed) moderation action, if any.
    let active_action_row = sqlx::query(
        "SELECT action FROM account_moderation \
         WHERE did = $1 AND NOT reversed \
         ORDER BY moderated_at DESC LIMIT 1",
    )
    .bind(&did)
    .fetch_optional(&ctx.account_db)
    .await
    .map_err(internal)?;
    let active_action: Option<String> =
        active_action_row.and_then(|r| r.try_get("action").ok());

    // Recent moderation actions (last 50).
    let action_rows = sqlx::query(
        "SELECT id, did, action, reason, moderated_by, moderated_at, expires_at, \
                reversed, reversed_at, report_id \
         FROM account_moderation WHERE did = $1 \
         ORDER BY moderated_at DESC LIMIT 50",
    )
    .bind(&did)
    .fetch_all(&ctx.account_db)
    .await
    .map_err(internal)?;

    let mut all_dids = vec![did.clone()];
    for r in &action_rows {
        if let Ok(d) = r.try_get::<String, _>("moderated_by") {
            all_dids.push(d);
        }
    }

    // Recent reports against this subject.
    let report_rows = sqlx::query(
        "SELECT id, reason_type, reason, reported_by, reported_at, status \
         FROM report WHERE subject_did = $1 \
         ORDER BY reported_at DESC LIMIT 50",
    )
    .bind(&did)
    .fetch_all(&ctx.account_db)
    .await
    .map_err(internal)?;

    // Recent appeals from this DID.
    let appeal_rows = sqlx::query(
        "SELECT id, appellant_did, status, submitted_at \
         FROM appeal WHERE appellant_did = $1 \
         ORDER BY submitted_at DESC LIMIT 50",
    )
    .bind(&did)
    .fetch_all(&ctx.account_db)
    .await
    .map_err(internal)?;
    for r in &appeal_rows {
        if let Ok(d) = r.try_get::<String, _>("appellant_did") {
            all_dids.push(d);
        }
    }

    let handles = resolve_handles(&ctx, &all_dids).await.map_err(internal_pds)?;

    // Map action rows to StatusWithContext.
    let mut recent_actions = Vec::with_capacity(action_rows.len());
    for row in action_rows {
        let id: i64 = row.try_get("id").map_err(internal)?;
        let action: String = row.try_get("action").map_err(internal)?;
        let reason: String = row.try_get("reason").map_err(internal)?;
        let moderated_by: String = row.try_get("moderated_by").map_err(internal)?;
        let moderated_at =
            parse_ts(&row.try_get::<String, _>("moderated_at").map_err(internal)?)
                .map_err(internal_pds)?;
        let expires_at =
            parse_ts_opt(row.try_get::<Option<String>, _>("expires_at").ok().flatten())
                .map_err(internal_pds)?;
        let reversed = crate::db::read_bool(&row, "reversed").map_err(internal)?;
        let reversed_at =
            parse_ts_opt(row.try_get::<Option<String>, _>("reversed_at").ok().flatten())
                .map_err(internal_pds)?;
        let report_id: Option<i64> = row.try_get("report_id").ok().flatten();
        let moderated_by_handle = handles.get(&moderated_by).cloned();
        recent_actions.push(StatusWithContext {
            id,
            did: did.clone(),
            handle: handle.clone(),
            action,
            reason,
            moderated_by,
            moderated_by_handle,
            moderated_at,
            expires_at,
            reversed,
            reversed_at,
            report_id,
        });
    }

    let mut related_reports = Vec::with_capacity(report_rows.len());
    for row in report_rows {
        let id: i64 = row.try_get("id").map_err(internal)?;
        let reason_type: String = row.try_get("reason_type").map_err(internal)?;
        let reason: Option<String> = row.try_get("reason").ok().flatten();
        let reported_by: String = row.try_get("reported_by").map_err(internal)?;
        let reported_at =
            parse_ts(&row.try_get::<String, _>("reported_at").map_err(internal)?)
                .map_err(internal_pds)?;
        let status: String = row.try_get("status").map_err(internal)?;
        related_reports.push(RelatedReport {
            id,
            reason_type,
            reason,
            reported_by,
            reported_at,
            status,
        });
    }

    let mut related_appeals = Vec::with_capacity(appeal_rows.len());
    for row in appeal_rows {
        let id: i64 = row.try_get("id").map_err(internal)?;
        let appellant_did: String = row.try_get("appellant_did").map_err(internal)?;
        let status: String = row.try_get("status").map_err(internal)?;
        let submitted_at =
            parse_ts(&row.try_get::<String, _>("submitted_at").map_err(internal)?)
                .map_err(internal_pds)?;
        let appellant_handle = handles.get(&appellant_did).cloned();
        related_appeals.push(RelatedAppeal {
            id,
            appellant_did,
            appellant_handle,
            status,
            submitted_at,
        });
    }

    Ok(Json(SubjectContextResponse {
        subject: Subject::Repo {
            did: did.clone(),
        },
        primary_did: Some(did),
        handle,
        current_status: Some(CurrentStatus {
            takedown_ref,
            deactivated_at,
            active_action,
        }),
        recent_actions,
        related_reports,
        related_appeals,
    }))
}

// ===========================================================================
// 5.2.5 — getSubjectHistory
// ===========================================================================

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetSubjectHistoryParams {
    pub did: String,
    /// Filter by action type.
    #[serde(default)]
    pub action: Option<String>,
    /// Direction: `asc` (oldest-first) or `desc` (newest-first, default).
    #[serde(default)]
    pub direction: Option<String>,
    #[serde(flatten)]
    pub pagination: PaginationParams,
}

pub async fn get_subject_history(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(params): Query<GetSubjectHistoryParams>,
) -> Result<Json<PaginatedResponse<StatusWithContext>>, (StatusCode, Json<serde_json::Value>)> {
    let limit = params.pagination.effective_limit() as i64;
    let cursor = params.pagination.decode_cursor().map_err(|_| {
        let e = AuroraAdminError::OutdatedCursor;
        (e.http_status(), Json(serde_json::json!({"error": e.code()})))
    })?;
    let asc = matches!(params.direction.as_deref(), Some("asc"));

    let mut clauses: Vec<&'static str> = vec!["did = ?"];
    let mut binds: Vec<String> = vec![params.did.clone()];
    if let Some(a) = &params.action {
        clauses.push("action = ?");
        binds.push(a.clone());
    }
    if let Some(c) = &cursor {
        if asc {
            clauses
                .push("(moderated_at > ? OR (moderated_at = ? AND id > ?))");
        } else {
            clauses
                .push("(moderated_at < ? OR (moderated_at = ? AND id < ?))");
        }
        binds.push(c.after_created.to_rfc3339());
        binds.push(c.after_created.to_rfc3339());
    }

    let clauses_pg = renumber_placeholders(&clauses, &binds, cursor.is_some());
    let where_sql = format!(" WHERE {}", clauses_pg.join(" AND "));
    let order = if asc { "ASC" } else { "DESC" };
    let limit_idx = binds.len() + if cursor.is_some() { 2 } else { 1 };
    let sql = format!(
        "SELECT id, did, action, reason, moderated_by, moderated_at, expires_at, \
                reversed, reversed_at, report_id \
         FROM account_moderation{} \
         ORDER BY moderated_at {order}, id {order} \
         LIMIT ${}",
        where_sql, limit_idx
    );

    let mut q = sqlx::query(&sql);
    for b in &binds {
        q = q.bind(b);
    }
    if let Some(c) = &cursor {
        q = q.bind(c.after_id);
    }
    q = q.bind(limit + 1);

    let rows = q.fetch_all(&ctx.account_db).await.map_err(internal)?;
    let has_more = rows.len() as i64 > limit;
    let page_rows: Vec<_> = rows.into_iter().take(limit as usize).collect();

    let mut handle_dids = vec![params.did.clone()];
    for row in &page_rows {
        if let Ok(d) = row.try_get::<String, _>("moderated_by") {
            handle_dids.push(d);
        }
    }
    let handles = resolve_handles(&ctx, &handle_dids).await.map_err(internal_pds)?;
    let subject_handle = handles.get(&params.did).cloned();

    let mut items = Vec::with_capacity(page_rows.len());
    let mut last_at = None;
    let mut last_id = None;
    for row in page_rows {
        let id: i64 = row.try_get("id").map_err(internal)?;
        let did: String = row.try_get("did").map_err(internal)?;
        let action: String = row.try_get("action").map_err(internal)?;
        let reason: String = row.try_get("reason").map_err(internal)?;
        let moderated_by: String = row.try_get("moderated_by").map_err(internal)?;
        let moderated_at = parse_ts(
            &row.try_get::<String, _>("moderated_at").map_err(internal)?,
        )
        .map_err(internal_pds)?;
        let expires_at =
            parse_ts_opt(row.try_get::<Option<String>, _>("expires_at").ok().flatten())
                .map_err(internal_pds)?;
        let reversed = crate::db::read_bool(&row, "reversed").map_err(internal)?;
        let reversed_at =
            parse_ts_opt(row.try_get::<Option<String>, _>("reversed_at").ok().flatten())
                .map_err(internal_pds)?;
        let report_id: Option<i64> = row.try_get("report_id").ok().flatten();

        last_at = Some(moderated_at);
        last_id = Some(id);

        items.push(StatusWithContext {
            id,
            did,
            handle: subject_handle.clone(),
            action,
            reason,
            moderated_by: moderated_by.clone(),
            moderated_by_handle: handles.get(&moderated_by).cloned(),
            moderated_at,
            expires_at,
            reversed,
            reversed_at,
            report_id,
        });
    }

    let next_cursor = if has_more {
        match (last_at, last_id) {
            (Some(t), Some(i)) => Some(
                CursorPosition {
                    after_created: t,
                    after_id: i,
                }
                .encode(),
            ),
            _ => None,
        }
    } else {
        None
    };

    Ok(Json(PaginatedResponse {
        items,
        cursor: next_cursor,
    }))
}

// ===========================================================================
// 5.2.6 / 5.2.7 — Appeals reads (Phase 3.4 / chainlink #101)
// ===========================================================================
//
// Two endpoints (listAppeals + getAppeal) reusing 3.3's foundation
// types and rich-context patterns. Subject derivation: appeals
// reference one of moderation_id, report_id, or quarantine_id; we
// resolve the underlying subject DID/URI to construct a Subject.
// blob_quarantine carries cid only (no DID), so quarantine appeals
// fall back to a Repo subject keyed on the appellant.

/// Wire-format appeal status. Mirrors the `appeal.status` column
/// vocabulary (snake_case) directly so the serialized JSON matches
/// what's stored. Distinct from `crate::admin::appeals::AppealStatus`
/// (which uses `rename_all = "lowercase"` and disagrees with its own
/// `as_str()` form).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiAppealStatus {
    Pending,
    UnderReview,
    Approved,
    Denied,
    Escalated,
}

impl ApiAppealStatus {
    pub fn as_db_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::UnderReview => "under_review",
            Self::Approved => "approved",
            Self::Denied => "denied",
            Self::Escalated => "escalated",
        }
    }

    pub fn from_db_str(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "under_review" => Some(Self::UnderReview),
            "approved" => Some(Self::Approved),
            "denied" => Some(Self::Denied),
            "escalated" => Some(Self::Escalated),
            _ => None,
        }
    }
}

/// Brief description of the action being appealed. Embedded in
/// AppealView so list consumers don't need a follow-up query to
/// understand what each appeal is about.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OriginalActionSummary {
    /// One of `"moderation"`, `"report"`, `"quarantine"`.
    pub kind: &'static str,
    pub id: i64,
    /// Short human-readable summary (e.g. `"takedown: spam"` for a
    /// moderation action, `"open report: harassment"` for a report).
    pub summary: String,
}

/// Appeal resolution. Present only when the appeal has been reviewed
/// (status != Pending|UnderReview).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppealResolution {
    pub reviewed_by: String,
    pub reviewed_by_handle: Option<String>,
    pub reviewed_at: DateTime<Utc>,
    pub decision: Option<String>,
    pub notes: Option<String>,
}

/// Paginated appeal list item.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppealView {
    pub id: i64,
    pub status: ApiAppealStatus,
    pub submitter_did: String,
    pub submitter_handle: Option<String>,
    pub subject: Option<Subject>,
    pub reason: String,
    pub details: Option<String>,
    pub submitted_at: DateTime<Utc>,
    pub original_action_summary: Option<OriginalActionSummary>,
    pub resolution: Option<AppealResolution>,
}

/// Single timeline entry on an appeal (lifecycle event).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppealTimelineEntry {
    /// `"submitted"` or `"reviewed"`.
    pub kind: &'static str,
    pub at: DateTime<Utc>,
    pub by_did: String,
    pub by_handle: Option<String>,
    pub note: Option<String>,
}

/// Detailed appeal view returned by `getAppeal`. Same fields as
/// `AppealView` plus a chronological timeline of the appeal's
/// lifecycle.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppealDetail {
    #[serde(flatten)]
    pub view: AppealView,
    pub timeline: Vec<AppealTimelineEntry>,
}

/// Batch-fetch action summaries for the given moderation/report/
/// quarantine IDs. Returns three maps `id -> (Option<subject>,
/// summary)`. Mirrors `resolve_handles`'s shape — one SQL query per
/// table per call regardless of input size.
pub(crate) async fn fetch_action_summaries(
    ctx: &AppContext,
    moderation_ids: &[i64],
    report_ids: &[i64],
    quarantine_ids: &[i64],
) -> Result<
    (
        HashMap<i64, (Option<Subject>, OriginalActionSummary)>,
        HashMap<i64, (Option<Subject>, OriginalActionSummary)>,
        HashMap<i64, (Option<Subject>, OriginalActionSummary)>,
    ),
    PdsError,
> {
    let mut moderations = HashMap::new();
    let mut reports = HashMap::new();
    let mut quarantines = HashMap::new();

    if !moderation_ids.is_empty() {
        let unique: HashSet<i64> = moderation_ids.iter().copied().collect();
        let placeholders: Vec<String> =
            (1..=unique.len()).map(|i| format!("${}", i)).collect();
        let sql = format!(
            "SELECT id, did, action, reason FROM account_moderation WHERE id IN ({})",
            placeholders.join(", ")
        );
        let mut q = sqlx::query(&sql);
        for id in &unique {
            q = q.bind(*id);
        }
        let rows = q.fetch_all(&ctx.account_db).await?;
        for row in rows {
            let id: i64 = row.try_get("id")?;
            let did: String = row.try_get("did")?;
            let action: String = row.try_get("action")?;
            let reason: String = row.try_get("reason")?;
            let summary = OriginalActionSummary {
                kind: "moderation",
                id,
                summary: format!("{}: {}", action, reason),
            };
            moderations.insert(id, (Some(Subject::Repo { did }), summary));
        }
    }

    if !report_ids.is_empty() {
        let unique: HashSet<i64> = report_ids.iter().copied().collect();
        let placeholders: Vec<String> =
            (1..=unique.len()).map(|i| format!("${}", i)).collect();
        let sql = format!(
            "SELECT id, subject_did, subject_uri, subject_cid, reason_type, reason, status \
             FROM report WHERE id IN ({})",
            placeholders.join(", ")
        );
        let mut q = sqlx::query(&sql);
        for id in &unique {
            q = q.bind(*id);
        }
        let rows = q.fetch_all(&ctx.account_db).await?;
        for row in rows {
            let id: i64 = row.try_get("id")?;
            let subject_did: Option<String> = row.try_get("subject_did").ok().flatten();
            let subject_uri: Option<String> = row.try_get("subject_uri").ok().flatten();
            let subject_cid: Option<String> = row.try_get("subject_cid").ok().flatten();
            let reason_type: String = row.try_get("reason_type")?;
            let status: String = row.try_get("status")?;
            let reason: Option<String> = row.try_get("reason").ok().flatten();
            let summary_text = match reason {
                Some(r) if !r.is_empty() => format!("{} report ({}): {}", status, reason_type, r),
                _ => format!("{} report ({})", status, reason_type),
            };
            let subject = Subject::from_columns(
                subject_did.as_deref(),
                subject_uri.as_deref(),
                subject_cid.as_deref(),
            );
            reports.insert(
                id,
                (
                    subject,
                    OriginalActionSummary {
                        kind: "report",
                        id,
                        summary: summary_text,
                    },
                ),
            );
        }
    }

    if !quarantine_ids.is_empty() {
        let unique: HashSet<i64> = quarantine_ids.iter().copied().collect();
        let placeholders: Vec<String> =
            (1..=unique.len()).map(|i| format!("${}", i)).collect();
        let sql = format!(
            "SELECT id, cid, reason FROM blob_quarantine WHERE id IN ({})",
            placeholders.join(", ")
        );
        let mut q = sqlx::query(&sql);
        for id in &unique {
            q = q.bind(*id);
        }
        let rows = q.fetch_all(&ctx.account_db).await?;
        for row in rows {
            let id: i64 = row.try_get("id")?;
            let cid: String = row.try_get("cid")?;
            let reason: String = row.try_get("reason")?;
            // blob_quarantine has no DID column — Subject can't be
            // populated from this row alone, so the appeal handler
            // falls back to Repo{appellant_did}. The summary still
            // carries the cid so callers know which blob it was.
            quarantines.insert(
                id,
                (
                    None,
                    OriginalActionSummary {
                        kind: "quarantine",
                        id,
                        summary: format!("blob quarantine ({}): {}", cid, reason),
                    },
                ),
            );
        }
    }

    Ok((moderations, reports, quarantines))
}

/// Query parameters for `tools.aurora.moderator.listAppeals`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListAppealsParams {
    #[serde(default)]
    pub status: Option<ApiAppealStatus>,
    /// Filter by appellant DID.
    #[serde(default)]
    pub appellant: Option<String>,
    /// Filter by reviewer DID (matches `reviewed_by`).
    #[serde(default)]
    pub reviewer: Option<String>,
    /// Lower bound on `submitted_at` (inclusive), RFC3339.
    #[serde(default)]
    pub submitted_after: Option<String>,
    /// Upper bound on `submitted_at` (inclusive), RFC3339.
    #[serde(default)]
    pub submitted_before: Option<String>,
    #[serde(flatten)]
    pub pagination: PaginationParams,
}

pub async fn list_appeals(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(params): Query<ListAppealsParams>,
) -> Result<Json<PaginatedResponse<AppealView>>, (StatusCode, Json<serde_json::Value>)> {
    let limit = params.pagination.effective_limit() as i64;
    let cursor = params.pagination.decode_cursor().map_err(|_| {
        let e = AuroraAdminError::OutdatedCursor;
        (e.http_status(), Json(serde_json::json!({"error": e.code()})))
    })?;

    let mut clauses: Vec<&'static str> = Vec::new();
    let mut binds: Vec<String> = Vec::new();

    if let Some(s) = &params.status {
        clauses.push("status = ?");
        binds.push(s.as_db_str().to_string());
    }
    if let Some(a) = &params.appellant {
        clauses.push("appellant_did = ?");
        binds.push(a.clone());
    }
    if let Some(r) = &params.reviewer {
        clauses.push("reviewed_by = ?");
        binds.push(r.clone());
    }
    if let Some(a) = &params.submitted_after {
        clauses.push("submitted_at >= ?");
        binds.push(a.clone());
    }
    if let Some(b) = &params.submitted_before {
        clauses.push("submitted_at <= ?");
        binds.push(b.clone());
    }
    if let Some(c) = &cursor {
        clauses.push("(submitted_at < ? OR (submitted_at = ? AND id < ?))");
        binds.push(c.after_created.to_rfc3339());
        binds.push(c.after_created.to_rfc3339());
    }

    let clauses_pg = renumber_placeholders(&clauses, &binds, cursor.is_some());
    let where_sql = if clauses_pg.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses_pg.join(" AND "))
    };
    let limit_idx = binds.len() + if cursor.is_some() { 2 } else { 1 };
    let sql = format!(
        "SELECT id, moderation_id, report_id, quarantine_id, appellant_did, reason, details, \
                submitted_at, status, reviewed_by, reviewed_at, decision, notes \
         FROM appeal{} \
         ORDER BY submitted_at DESC, id DESC \
         LIMIT ${}",
        where_sql, limit_idx
    );

    let mut q = sqlx::query(&sql);
    for b in &binds {
        q = q.bind(b);
    }
    if let Some(c) = &cursor {
        q = q.bind(c.after_id);
    }
    q = q.bind(limit + 1);

    let rows = q.fetch_all(&ctx.account_db).await.map_err(internal)?;
    let has_more = rows.len() as i64 > limit;
    let page_rows: Vec<_> = rows.into_iter().take(limit as usize).collect();

    // Collect IDs for batch action-summary lookup.
    let mut mod_ids = Vec::new();
    let mut rep_ids = Vec::new();
    let mut qua_ids = Vec::new();
    let mut handle_dids = Vec::new();
    for row in &page_rows {
        if let Ok(Some(i)) = row.try_get::<Option<i64>, _>("moderation_id") {
            mod_ids.push(i);
        }
        if let Ok(Some(i)) = row.try_get::<Option<i64>, _>("report_id") {
            rep_ids.push(i);
        }
        if let Ok(Some(i)) = row.try_get::<Option<i64>, _>("quarantine_id") {
            qua_ids.push(i);
        }
        if let Ok(d) = row.try_get::<String, _>("appellant_did") {
            handle_dids.push(d);
        }
        if let Ok(Some(d)) = row.try_get::<Option<String>, _>("reviewed_by") {
            handle_dids.push(d);
        }
    }
    let (moderations, reports, quarantines) =
        fetch_action_summaries(&ctx, &mod_ids, &rep_ids, &qua_ids)
            .await
            .map_err(internal_pds)?;
    // Add subject DIDs that came from the moderation/report lookups
    // so handle resolution covers them too.
    for (subj, _) in moderations.values().chain(reports.values()) {
        if let Some(s) = subj {
            if let Some(d) = s.primary_did() {
                handle_dids.push(d.to_string());
            }
        }
    }
    let handles = resolve_handles(&ctx, &handle_dids)
        .await
        .map_err(internal_pds)?;

    let mut items = Vec::with_capacity(page_rows.len());
    let mut last_at = None;
    let mut last_id = None;
    for row in page_rows {
        let view = build_appeal_view(&row, &moderations, &reports, &quarantines, &handles)
            .map_err(internal_pds)?;
        last_at = Some(view.submitted_at);
        last_id = Some(view.id);
        items.push(view);
    }

    let next_cursor = if has_more {
        match (last_at, last_id) {
            (Some(t), Some(i)) => Some(
                CursorPosition {
                    after_created: t,
                    after_id: i,
                }
                .encode(),
            ),
            _ => None,
        }
    } else {
        None
    };

    Ok(Json(PaginatedResponse {
        items,
        cursor: next_cursor,
    }))
}

/// Extract one `AppealView` from a row + the pre-fetched lookup maps.
/// Pulled out to keep `list_appeals` and `get_appeal` from
/// duplicating the column-extraction dance.
fn build_appeal_view(
    row: &sqlx::any::AnyRow,
    moderations: &HashMap<i64, (Option<Subject>, OriginalActionSummary)>,
    reports: &HashMap<i64, (Option<Subject>, OriginalActionSummary)>,
    quarantines: &HashMap<i64, (Option<Subject>, OriginalActionSummary)>,
    handles: &HashMap<String, String>,
) -> Result<AppealView, PdsError> {
    let id: i64 = row.try_get("id")?;
    let moderation_id: Option<i64> = row.try_get("moderation_id").ok().flatten();
    let report_id: Option<i64> = row.try_get("report_id").ok().flatten();
    let quarantine_id: Option<i64> = row.try_get("quarantine_id").ok().flatten();
    let appellant_did: String = row.try_get("appellant_did")?;
    let reason: String = row.try_get("reason")?;
    let details: Option<String> = row.try_get("details").ok().flatten();
    let submitted_at = parse_ts(&row.try_get::<String, _>("submitted_at")?)?;
    let status_str: String = row.try_get("status")?;
    let status = ApiAppealStatus::from_db_str(&status_str).ok_or_else(|| {
        PdsError::Internal(format!("Unknown appeal status in db: {}", status_str))
    })?;
    let reviewed_by: Option<String> = row.try_get("reviewed_by").ok().flatten();
    let reviewed_at_str: Option<String> = row.try_get("reviewed_at").ok().flatten();
    let reviewed_at = parse_ts_opt(reviewed_at_str)?;
    let decision: Option<String> = row.try_get("decision").ok().flatten();
    let notes: Option<String> = row.try_get("notes").ok().flatten();

    // Pick whichever reference is populated (moderation > report >
    // quarantine in priority). Use the lookup maps for both subject
    // and summary; fall back to Repo{appellant_did} if nothing
    // resolves.
    let (subject, summary) = if let Some(mid) = moderation_id {
        moderations
            .get(&mid)
            .cloned()
            .map(|(s, sm)| (s, Some(sm)))
            .unwrap_or((None, None))
    } else if let Some(rid) = report_id {
        reports
            .get(&rid)
            .cloned()
            .map(|(s, sm)| (s, Some(sm)))
            .unwrap_or((None, None))
    } else if let Some(qid) = quarantine_id {
        quarantines
            .get(&qid)
            .cloned()
            .map(|(s, sm)| (s, Some(sm)))
            .unwrap_or((None, None))
    } else {
        (None, None)
    };
    let subject = subject.or(Some(Subject::Repo {
        did: appellant_did.clone(),
    }));

    let submitter_handle = handles.get(&appellant_did).cloned();
    let resolution = match (reviewed_by.clone(), reviewed_at) {
        (Some(by), Some(at)) => Some(AppealResolution {
            reviewed_by_handle: handles.get(&by).cloned(),
            reviewed_by: by,
            reviewed_at: at,
            decision: decision.clone(),
            notes: notes.clone(),
        }),
        _ => None,
    };

    Ok(AppealView {
        id,
        status,
        submitter_did: appellant_did,
        submitter_handle,
        subject,
        reason,
        details,
        submitted_at,
        original_action_summary: summary,
        resolution,
    })
}

#[derive(Debug, Deserialize)]
pub struct GetAppealParams {
    pub id: i64,
}

pub async fn get_appeal(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(params): Query<GetAppealParams>,
) -> Result<Json<AppealDetail>, (StatusCode, Json<serde_json::Value>)> {
    let row = sqlx::query(
        "SELECT id, moderation_id, report_id, quarantine_id, appellant_did, reason, details, \
                submitted_at, status, reviewed_by, reviewed_at, decision, notes \
         FROM appeal WHERE id = $1",
    )
    .bind(params.id)
    .fetch_optional(&ctx.account_db)
    .await
    .map_err(internal)?;

    let row = row.ok_or_else(|| -> (StatusCode, Json<serde_json::Value>) {
        AuroraAdminError::AppealNotFound.into()
    })?;

    let moderation_id: Option<i64> = row.try_get("moderation_id").ok().flatten();
    let report_id: Option<i64> = row.try_get("report_id").ok().flatten();
    let quarantine_id: Option<i64> = row.try_get("quarantine_id").ok().flatten();

    let mod_ids: Vec<i64> = moderation_id.into_iter().collect();
    let rep_ids: Vec<i64> = report_id.into_iter().collect();
    let qua_ids: Vec<i64> = quarantine_id.into_iter().collect();
    let (moderations, reports, quarantines) =
        fetch_action_summaries(&ctx, &mod_ids, &rep_ids, &qua_ids)
            .await
            .map_err(internal_pds)?;

    let mut handle_dids = Vec::new();
    if let Ok(d) = row.try_get::<String, _>("appellant_did") {
        handle_dids.push(d);
    }
    if let Ok(Some(d)) = row.try_get::<Option<String>, _>("reviewed_by") {
        handle_dids.push(d);
    }
    for (subj, _) in moderations.values().chain(reports.values()) {
        if let Some(s) = subj {
            if let Some(d) = s.primary_did() {
                handle_dids.push(d.to_string());
            }
        }
    }
    let handles = resolve_handles(&ctx, &handle_dids)
        .await
        .map_err(internal_pds)?;

    let view = build_appeal_view(&row, &moderations, &reports, &quarantines, &handles)
        .map_err(internal_pds)?;

    // Build timeline. The appeal table stores at most two lifecycle
    // events directly: submission + review. Future event-history
    // sources (e.g. moderation_event entries with appeal_submit /
    // appeal_review types from Phase 3.5 onward) can extend this
    // without a wire-format break.
    let mut timeline = vec![AppealTimelineEntry {
        kind: "submitted",
        at: view.submitted_at,
        by_did: view.submitter_did.clone(),
        by_handle: view.submitter_handle.clone(),
        note: Some(view.reason.clone()),
    }];
    if let Some(res) = &view.resolution {
        timeline.push(AppealTimelineEntry {
            kind: "reviewed",
            at: res.reviewed_at,
            by_did: res.reviewed_by.clone(),
            by_handle: res.reviewed_by_handle.clone(),
            note: res
                .decision
                .clone()
                .or_else(|| res.notes.clone())
                .or_else(|| Some(format!("status -> {:?}", view.status))),
        });
    }

    Ok(Json(AppealDetail { view, timeline }))
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin::roles::Role;
    use crate::account::ValidatedSession;

    /// Test fixture mirroring src/api/admin.rs::admin_test_auth().
    fn moderator_test_auth() -> AdminAuthContext {
        AdminAuthContext {
            did: "did:plc:moderator".to_string(),
            session: ValidatedSession {
                did: "did:plc:moderator".to_string(),
                session_id: "test_session".to_string(),
                is_app_password: false,
            },
            role: Role::Moderator,
        }
    }

    /// Reuse src/api/admin.rs's create_test_context indirectly via
    /// the AppContext constructor pattern. We can't call the
    /// admin.rs helper directly (it's private), so we build a thin
    /// equivalent here. Phase 3.4+ work could share a fixture.
    async fn create_test_context() -> AppContext {
        // Minimal fixture: SQLite in tempdir, migrations applied.
        // Mirrors create_test_context in src/api/admin.rs in shape,
        // simplified for what these tests need.
        use crate::config::*;
        use std::path::PathBuf;
        use tempfile::tempdir;
        let dir = tempdir().unwrap().keep();
        let db_path = dir.join("test.db");
        let config = ServerConfig {
            service: ServiceConfig {
                hostname: "localhost".to_string(),
                port: 2583,
                service_did: "did:web:localhost".to_string(),
                version: "0.1.0-test".to_string(),
                blob_upload_limit: 5_242_880,
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
                jwt_secret: "test-secret-key-aurora-moderator-test-32".to_string(),
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
            validation_mode: PathBuf::from("required").into_os_string().to_string_lossy().parse().unwrap_or(crate::validation::ValidationMode::Required),
            distributed_state_mode: Default::default(),
            maintenance_pool: Default::default(),
            gc_sweep: Default::default(),
        };
        AppContext::new(
            config,
            std::sync::Arc::new(crate::api::registry::RouteRegistry::default()),
        )
        .await
        .unwrap()
    }

    /// Insert an actor + a moderation_event for a clean test fixture.
    async fn seed_event(
        ctx: &AppContext,
        did: &str,
        handle: &str,
        event_type: &str,
        actor_did: &str,
    ) {
        // Ensure actor row exists for handle resolution.
        sqlx::query("INSERT INTO actor (did, handle, created_at) VALUES ($1, $2, $3)")
            .bind(did)
            .bind(handle)
            .bind(chrono::Utc::now().to_rfc3339())
            .execute(&ctx.account_db)
            .await
            .ok();
        sqlx::query("INSERT INTO actor (did, handle, created_at) VALUES ($1, $2, $3)")
            .bind(actor_did)
            .bind(format!("{}-actor", handle))
            .bind(chrono::Utc::now().to_rfc3339())
            .execute(&ctx.account_db)
            .await
            .ok();
        sqlx::query(
            "INSERT INTO moderation_event \
             (event_type, actor_did, subject_did, details, created_at) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(event_type)
        .bind(actor_did)
        .bind(did)
        .bind(r#"{"note":"test"}"#)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&ctx.account_db)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn query_events_returns_with_handle_resolution() {
        let ctx = create_test_context().await;
        seed_event(
            &ctx,
            "did:plc:subject",
            "subject.test",
            "AccountTakedown",
            "did:plc:adminx",
        )
        .await;

        let resp = query_events(
            State(ctx),
            moderator_test_auth(),
            Query(QueryEventsParams {
                event_type: None,
                actor: None,
                subject_did: None,
                after: None,
                before: None,
                pagination: PaginationParams::default(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(resp.items.len(), 1);
        assert_eq!(resp.items[0].event_type, "AccountTakedown");
        assert_eq!(
            resp.items[0].subject_handle.as_deref(),
            Some("subject.test")
        );
        assert_eq!(
            resp.items[0].actor_handle.as_deref(),
            Some("subject.test-actor")
        );
    }

    #[tokio::test]
    async fn query_events_filters_by_event_type() {
        let ctx = create_test_context().await;
        seed_event(&ctx, "did:plc:s1", "s1.test", "AccountTakedown", "did:plc:a").await;
        seed_event(&ctx, "did:plc:s2", "s2.test", "AccountWarn", "did:plc:b").await;

        let resp = query_events(
            State(ctx),
            moderator_test_auth(),
            Query(QueryEventsParams {
                event_type: Some("AccountTakedown".to_string()),
                actor: None,
                subject_did: None,
                after: None,
                before: None,
                pagination: PaginationParams::default(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(resp.items.len(), 1);
        assert_eq!(resp.items[0].event_type, "AccountTakedown");
    }

    #[tokio::test]
    async fn query_events_paginates_with_cursor() {
        let ctx = create_test_context().await;
        for i in 0..5 {
            seed_event(
                &ctx,
                &format!("did:plc:s{}", i),
                &format!("s{}.test", i),
                "AccountWarn",
                "did:plc:a",
            )
            .await;
            // Tiny sleep so each event has a distinct timestamp.
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let page1 = query_events(
            State(ctx.clone()),
            moderator_test_auth(),
            Query(QueryEventsParams {
                event_type: None,
                actor: None,
                subject_did: None,
                after: None,
                before: None,
                pagination: PaginationParams {
                    cursor: None,
                    limit: Some(2),
                },
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(page1.items.len(), 2);
        assert!(page1.cursor.is_some(), "expected cursor for next page");

        let page2 = query_events(
            State(ctx),
            moderator_test_auth(),
            Query(QueryEventsParams {
                event_type: None,
                actor: None,
                subject_did: None,
                after: None,
                before: None,
                pagination: PaginationParams {
                    cursor: page1.cursor,
                    limit: Some(2),
                },
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(page2.items.len(), 2);
        // No overlap with page 1.
        let p1_ids: Vec<i64> = page1.items.iter().map(|e| e.id).collect();
        let p2_ids: Vec<i64> = page2.items.iter().map(|e| e.id).collect();
        for id in &p2_ids {
            assert!(!p1_ids.contains(id), "page2 should not overlap page1");
        }
    }

    #[tokio::test]
    async fn get_event_returns_404_for_missing() {
        let ctx = create_test_context().await;
        let err = get_event(
            State(ctx),
            moderator_test_auth(),
            Query(GetEventParams { id: 99999 }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn get_event_returns_event_with_context() {
        let ctx = create_test_context().await;
        seed_event(
            &ctx,
            "did:plc:sub",
            "sub.test",
            "AccountTakedown",
            "did:plc:adm",
        )
        .await;
        // Find the inserted event id.
        let id: i64 =
            sqlx::query_scalar("SELECT id FROM moderation_event ORDER BY id DESC LIMIT 1")
                .fetch_one(&ctx.account_db)
                .await
                .unwrap();
        let resp = get_event(
            State(ctx),
            moderator_test_auth(),
            Query(GetEventParams { id }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(resp.event_type, "AccountTakedown");
        assert_eq!(resp.subject_handle.as_deref(), Some("sub.test"));
    }

    #[tokio::test]
    async fn query_statuses_record_subject_returns_empty() {
        // SubjectType::Record short-circuits to empty since
        // account_moderation is per-DID.
        let ctx = create_test_context().await;
        let resp = query_statuses(
            State(ctx),
            moderator_test_auth(),
            Query(QueryStatusesParams {
                did: None,
                subject_type: Some(SubjectType::Record),
                action: None,
                include_reversed: None,
                pagination: PaginationParams::default(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert!(resp.items.is_empty());
        assert!(resp.cursor.is_none());
    }

    #[tokio::test]
    async fn get_subject_context_404_for_missing_did() {
        let ctx = create_test_context().await;
        let err = get_subject_context(
            State(ctx),
            moderator_test_auth(),
            Query(GetSubjectContextParams {
                did: "did:plc:nonexistent".to_string(),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn get_subject_context_returns_handle_for_known_subject() {
        let ctx = create_test_context().await;
        sqlx::query("INSERT INTO actor (did, handle, created_at) VALUES ($1, $2, $3)")
            .bind("did:plc:knownsubj")
            .bind("known.test")
            .bind(chrono::Utc::now().to_rfc3339())
            .execute(&ctx.account_db)
            .await
            .unwrap();
        let resp = get_subject_context(
            State(ctx),
            moderator_test_auth(),
            Query(GetSubjectContextParams {
                did: "did:plc:knownsubj".to_string(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(resp.handle.as_deref(), Some("known.test"));
        assert!(resp.recent_actions.is_empty());
        assert!(resp.related_reports.is_empty());
        assert!(resp.related_appeals.is_empty());
    }

    #[tokio::test]
    async fn get_subject_history_returns_empty_for_unknown_did() {
        let ctx = create_test_context().await;
        let resp = get_subject_history(
            State(ctx),
            moderator_test_auth(),
            Query(GetSubjectHistoryParams {
                did: "did:plc:noone".to_string(),
                action: None,
                direction: None,
                pagination: PaginationParams::default(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert!(resp.items.is_empty());
    }

    #[tokio::test]
    async fn resolve_handles_returns_known_dids() {
        let ctx = create_test_context().await;
        sqlx::query("INSERT INTO actor (did, handle, created_at) VALUES ($1, $2, $3)")
            .bind("did:plc:rh1")
            .bind("rh1.test")
            .bind(chrono::Utc::now().to_rfc3339())
            .execute(&ctx.account_db)
            .await
            .unwrap();
        sqlx::query("INSERT INTO actor (did, handle, created_at) VALUES ($1, $2, $3)")
            .bind("did:plc:rh2")
            .bind("rh2.test")
            .bind(chrono::Utc::now().to_rfc3339())
            .execute(&ctx.account_db)
            .await
            .unwrap();
        let h = resolve_handles(
            &ctx,
            &[
                "did:plc:rh1".to_string(),
                "did:plc:rh2".to_string(),
                "did:plc:absent".to_string(),
            ],
        )
        .await
        .unwrap();
        assert_eq!(h.get("did:plc:rh1").map(|s| s.as_str()), Some("rh1.test"));
        assert_eq!(h.get("did:plc:rh2").map(|s| s.as_str()), Some("rh2.test"));
        assert!(!h.contains_key("did:plc:absent"));
    }

    #[tokio::test]
    async fn resolve_handles_empty_input_returns_empty_map() {
        let ctx = create_test_context().await;
        let h = resolve_handles(&ctx, &[]).await.unwrap();
        assert!(h.is_empty());
    }

    // -----------------------------------------------------------------
    // Phase 3.4 — appeals reads (chainlink #101)
    // -----------------------------------------------------------------

    /// Insert one appeal row. `moderation_id` may be supplied to wire
    /// a backing moderation action; pass `None` for a standalone
    /// appeal that falls back to Repo{appellant_did}.
    #[allow(clippy::too_many_arguments)]
    async fn seed_appeal(
        ctx: &AppContext,
        appellant_did: &str,
        appellant_handle: Option<&str>,
        moderation_id: Option<i64>,
        report_id: Option<i64>,
        reason: &str,
        status: &str,
        reviewed_by: Option<&str>,
        offset_secs: i64,
    ) -> i64 {
        if let Some(h) = appellant_handle {
            sqlx::query("INSERT INTO actor (did, handle, created_at) VALUES ($1, $2, $3)")
                .bind(appellant_did)
                .bind(h)
                .bind(chrono::Utc::now().to_rfc3339())
                .execute(&ctx.account_db)
                .await
                .ok();
        }
        // Stagger submitted_at so cursor pagination has distinct
        // timestamps even when seeding in a tight loop.
        let when = (chrono::Utc::now() - chrono::Duration::seconds(offset_secs)).to_rfc3339();
        let reviewed_at = if reviewed_by.is_some() {
            Some(chrono::Utc::now().to_rfc3339())
        } else {
            None
        };
        sqlx::query(
            "INSERT INTO appeal (moderation_id, report_id, quarantine_id, appellant_did, \
                                 reason, details, submitted_at, status, reviewed_by, \
                                 reviewed_at, decision, notes) \
             VALUES ($1, $2, NULL, $3, $4, NULL, $5, $6, $7, $8, NULL, NULL) \
             RETURNING id",
        )
        .bind(moderation_id)
        .bind(report_id)
        .bind(appellant_did)
        .bind(reason)
        .bind(when)
        .bind(status)
        .bind(reviewed_by)
        .bind(reviewed_at)
        .fetch_one(&ctx.account_db)
        .await
        .unwrap()
        .try_get::<i64, _>(0)
        .unwrap()
    }

    /// Insert one account_moderation row, return its id.
    async fn seed_moderation(
        ctx: &AppContext,
        did: &str,
        action: &str,
        reason: &str,
        moderated_by: &str,
    ) -> i64 {
        sqlx::query(
            "INSERT INTO account_moderation (did, action, reason, moderated_by, moderated_at, \
                                              reversed) \
             VALUES ($1, $2, $3, $4, $5, 0) \
             RETURNING id",
        )
        .bind(did)
        .bind(action)
        .bind(reason)
        .bind(moderated_by)
        .bind(chrono::Utc::now().to_rfc3339())
        .fetch_one(&ctx.account_db)
        .await
        .unwrap()
        .try_get::<i64, _>(0)
        .unwrap()
    }

    #[tokio::test]
    async fn list_appeals_returns_with_handle_and_subject() {
        let ctx = create_test_context().await;
        let mod_id = seed_moderation(
            &ctx,
            "did:plc:victim",
            "takedown",
            "spam",
            "did:plc:adminx",
        )
        .await;
        seed_appeal(
            &ctx,
            "did:plc:victim",
            Some("victim.test"),
            Some(mod_id),
            None,
            "false positive",
            "pending",
            None,
            0,
        )
        .await;

        let resp = list_appeals(
            State(ctx),
            moderator_test_auth(),
            Query(ListAppealsParams {
                status: None,
                appellant: None,
                reviewer: None,
                submitted_after: None,
                submitted_before: None,
                pagination: PaginationParams::default(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(resp.items.len(), 1);
        let a = &resp.items[0];
        assert_eq!(a.status, ApiAppealStatus::Pending);
        assert_eq!(a.submitter_handle.as_deref(), Some("victim.test"));
        // Subject derived from moderation -> Repo{victim DID}.
        assert!(matches!(&a.subject, Some(Subject::Repo { did }) if did == "did:plc:victim"));
        let summary = a.original_action_summary.as_ref().unwrap();
        assert_eq!(summary.kind, "moderation");
        assert!(summary.summary.contains("takedown"));
        assert!(a.resolution.is_none());
    }

    #[tokio::test]
    async fn list_appeals_filters_by_status() {
        let ctx = create_test_context().await;
        seed_appeal(
            &ctx,
            "did:plc:a1",
            Some("a1.test"),
            None,
            None,
            "first",
            "pending",
            None,
            0,
        )
        .await;
        seed_appeal(
            &ctx,
            "did:plc:a2",
            Some("a2.test"),
            None,
            None,
            "second",
            "approved",
            Some("did:plc:adminx"),
            5,
        )
        .await;

        let resp = list_appeals(
            State(ctx),
            moderator_test_auth(),
            Query(ListAppealsParams {
                status: Some(ApiAppealStatus::Approved),
                appellant: None,
                reviewer: None,
                submitted_after: None,
                submitted_before: None,
                pagination: PaginationParams::default(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(resp.items.len(), 1);
        assert_eq!(resp.items[0].status, ApiAppealStatus::Approved);
        assert!(resp.items[0].resolution.is_some());
    }

    #[tokio::test]
    async fn list_appeals_paginates_with_cursor() {
        let ctx = create_test_context().await;
        for i in 0..5 {
            seed_appeal(
                &ctx,
                &format!("did:plc:a{}", i),
                Some(&format!("a{}.test", i)),
                None,
                None,
                &format!("appeal {}", i),
                "pending",
                None,
                i as i64, // distinct submitted_at per row
            )
            .await;
        }

        let page1 = list_appeals(
            State(ctx.clone()),
            moderator_test_auth(),
            Query(ListAppealsParams {
                status: None,
                appellant: None,
                reviewer: None,
                submitted_after: None,
                submitted_before: None,
                pagination: PaginationParams {
                    cursor: None,
                    limit: Some(2),
                },
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(page1.items.len(), 2);
        assert!(page1.cursor.is_some(), "expected cursor for next page");

        let page2 = list_appeals(
            State(ctx),
            moderator_test_auth(),
            Query(ListAppealsParams {
                status: None,
                appellant: None,
                reviewer: None,
                submitted_after: None,
                submitted_before: None,
                pagination: PaginationParams {
                    cursor: page1.cursor,
                    limit: Some(2),
                },
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(page2.items.len(), 2);
        let p1: Vec<i64> = page1.items.iter().map(|a| a.id).collect();
        let p2: Vec<i64> = page2.items.iter().map(|a| a.id).collect();
        for id in &p2 {
            assert!(!p1.contains(id), "page2 must not overlap page1");
        }
    }

    #[tokio::test]
    async fn list_appeals_filters_by_appellant_and_date_range() {
        let ctx = create_test_context().await;
        seed_appeal(
            &ctx,
            "did:plc:keep",
            Some("keep.test"),
            None,
            None,
            "kept",
            "pending",
            None,
            0,
        )
        .await;
        seed_appeal(
            &ctx,
            "did:plc:other",
            Some("other.test"),
            None,
            None,
            "filtered",
            "pending",
            None,
            10,
        )
        .await;

        // Appellant filter: keeps only the matching DID.
        let resp = list_appeals(
            State(ctx.clone()),
            moderator_test_auth(),
            Query(ListAppealsParams {
                status: None,
                appellant: Some("did:plc:keep".to_string()),
                reviewer: None,
                submitted_after: None,
                submitted_before: None,
                pagination: PaginationParams::default(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(resp.items.len(), 1);
        assert_eq!(resp.items[0].submitter_did, "did:plc:keep");

        // Date range: only the recent appeal (within last 5 seconds).
        let cutoff = (chrono::Utc::now() - chrono::Duration::seconds(5)).to_rfc3339();
        let resp = list_appeals(
            State(ctx),
            moderator_test_auth(),
            Query(ListAppealsParams {
                status: None,
                appellant: None,
                reviewer: None,
                submitted_after: Some(cutoff),
                submitted_before: None,
                pagination: PaginationParams::default(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(resp.items.len(), 1);
        assert_eq!(resp.items[0].submitter_did, "did:plc:keep");
    }

    #[tokio::test]
    async fn get_appeal_returns_404_for_missing() {
        let ctx = create_test_context().await;
        let err = get_appeal(
            State(ctx),
            moderator_test_auth(),
            Query(GetAppealParams { id: 99999 }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::NOT_FOUND);
        // Error envelope carries the AppealNotFound code (distinct from
        // SubjectNotFound which getEvent uses).
        let body = err.1.0;
        assert_eq!(body["error"], "AppealNotFound");
    }

    #[tokio::test]
    async fn get_appeal_returns_full_detail_with_timeline() {
        let ctx = create_test_context().await;
        let mod_id = seed_moderation(
            &ctx,
            "did:plc:user",
            "suspend",
            "ToS violation",
            "did:plc:adm1",
        )
        .await;
        // Reviewer needs an actor row so handle resolution populates
        // resolution.reviewedByHandle.
        sqlx::query("INSERT INTO actor (did, handle, created_at) VALUES ($1, $2, $3)")
            .bind("did:plc:adm1")
            .bind("admin1.test")
            .bind(chrono::Utc::now().to_rfc3339())
            .execute(&ctx.account_db)
            .await
            .unwrap();
        let appeal_id = seed_appeal(
            &ctx,
            "did:plc:user",
            Some("user.test"),
            Some(mod_id),
            None,
            "I did not violate",
            "approved",
            Some("did:plc:adm1"),
            0,
        )
        .await;

        let resp = get_appeal(
            State(ctx),
            moderator_test_auth(),
            Query(GetAppealParams { id: appeal_id }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(resp.view.id, appeal_id);
        assert_eq!(resp.view.status, ApiAppealStatus::Approved);
        assert_eq!(resp.view.submitter_handle.as_deref(), Some("user.test"));
        let res = resp.view.resolution.as_ref().unwrap();
        assert_eq!(res.reviewed_by, "did:plc:adm1");
        assert_eq!(res.reviewed_by_handle.as_deref(), Some("admin1.test"));
        // Timeline: submission + review.
        assert_eq!(resp.timeline.len(), 2);
        assert_eq!(resp.timeline[0].kind, "submitted");
        assert_eq!(resp.timeline[1].kind, "reviewed");
        assert_eq!(resp.timeline[0].by_did, "did:plc:user");
        assert_eq!(resp.timeline[1].by_did, "did:plc:adm1");
    }

    #[tokio::test]
    async fn get_appeal_pending_has_single_timeline_entry() {
        // Pending appeals have no review event yet — timeline is just
        // the submission.
        let ctx = create_test_context().await;
        let appeal_id = seed_appeal(
            &ctx,
            "did:plc:pend",
            Some("pend.test"),
            None,
            None,
            "still waiting",
            "pending",
            None,
            0,
        )
        .await;

        let resp = get_appeal(
            State(ctx),
            moderator_test_auth(),
            Query(GetAppealParams { id: appeal_id }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(resp.timeline.len(), 1);
        assert_eq!(resp.timeline[0].kind, "submitted");
        assert!(resp.view.resolution.is_none());
        // No backing moderation/report — falls back to Repo{appellant}.
        assert!(matches!(&resp.view.subject, Some(Subject::Repo { did }) if did == "did:plc:pend"));
        assert!(resp.view.original_action_summary.is_none());
    }

    #[tokio::test]
    async fn api_appeal_status_serializes_snake_case() {
        // Wire format guard: response JSON must use snake_case so it
        // matches the stored db vocabulary one-to-one.
        let json = serde_json::to_string(&ApiAppealStatus::UnderReview).unwrap();
        assert_eq!(json, "\"under_review\"");
        let parsed: ApiAppealStatus = serde_json::from_str("\"under_review\"").unwrap();
        assert_eq!(parsed, ApiAppealStatus::UnderReview);
    }
}
