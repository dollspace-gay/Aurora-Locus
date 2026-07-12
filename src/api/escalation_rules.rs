//! §5.5.4 Phase D — escalation rules (§5).
//!
//! Operator-defined rules that auto-escalate queue items (a report's
//! `status` → `escalated`, `assignment_source` → `escalation`) on severity
//! signals, across Pipelines A (report intake) and B (operator moderation
//! action via `emit_event`). Full-tier gated; SuperAdmin CRUD; rules
//! soft-delete; capped at 100 active. The ONLY path into `escalated` in v0.9
//! is rule firing (no manual escalation, §5.1 HM-CC).
//!
//! Centerpieces: the §5.9 tightened skip-guard + §5.6 audit-chain lookup,
//! both built on the `caused_state_change` discriminator in the audit
//! `payload`, plus per-item row-level serialization and the consumed-row
//! de-escalation mechanism.
//!
//! **Local-idiom translations** (memory #18, recorded per Nova's Decisions):
//! - **D1 row-lock:** backend-conditional — Postgres `SELECT … FOR UPDATE
//!   NOWAIT` on the report row inside the mutation tx; SQLite the
//!   `escalation_eval_lock` advisory row (INSERT-conflict = contention). The
//!   table ships unconditionally.
//! - **D2 JSON filtering:** `caused_state_change`/`item_id` are filtered
//!   Rust-side after an index-narrowed SQL fetch — JSON-in-WHERE is not a
//!   portable `sqlx::Any` pattern.
//! - **D3 audit subject:** escalation runtime audits set `subject =
//!   Repo{ normalize_subject_value(report.subject) }` (the account DID, so
//!   `subject_did` is index-narrowable) + `item_id` (report id) in `payload`.
//!   Reports that don't normalize to an account are skipped (can't be tracked).
//! - Audit `source`: `escalation` for `_triggered`/`_cleared`/`_reassigned`/
//!   `reviewer_assigned`; `manual` for `_eval_skipped` + rule-lifecycle (the
//!   `source` column is NOT NULL; the design's NULL doesn't apply).

use crate::admin::audit_chain::{self, AppendEntryParams, AppendChainGuard};
use crate::admin::defs::Subject;
use crate::admin::reports::Report;
use crate::admin::roles::Role;
use crate::api::auto_label_rules::{
    normalize_subject_value, report_subject, OPERATOR_ACTION_TYPES, REPORT_CATEGORIES,
};
use crate::api::aurora_admin::{
    cas_runtime_setting, read_runtime_row_value, resolve_runtime_setting,
    MODERATION_ESCALATION_SUPERADMIN_CURSOR_KEY, MODERATION_REVIEWER_MODE_KEY,
};
use crate::api::moderation_defaults::{defaults_active, SYSTEM_DID};
use crate::config::DatabaseBackend;
use crate::error::PdsResult;
use crate::AppContext;
use chrono::{Duration, Utc};
use sqlx::Row as _;
use uuid::Uuid;

const MAX_ACTIVE_RULES: i64 = 100;
const CAS_MAX_RETRIES: usize = 3;
const TERMINAL_STATUS: &str = "resolved";

// Audit action names (§5.10 / §6.1).
const ACTION_RULE_CREATED: &str = "moderation_escalation_rule_created";
const ACTION_RULE_EDITED: &str = "moderation_escalation_rule_edited";
const ACTION_RULE_DELETED: &str = "moderation_escalation_rule_deleted";
const ACTION_TRIGGERED: &str = "moderation_escalation_triggered";
const ACTION_CLEARED: &str = "moderation_escalation_cleared";
const ACTION_EVAL_SKIPPED: &str = "moderation_escalation_eval_skipped";
const ACTION_REASSIGNED: &str = "moderation_escalation_reassigned";
const ACTION_REVIEWER_ASSIGNED: &str = "moderation_reviewer_assigned";

// Audit sources (§6.1 enum).
const SOURCE_ESCALATION: &str = "escalation";
const SOURCE_MANUAL: &str = "manual";

const TRIGGER_TYPES: &[&str] = &["report-count", "operator-action", "category-match"];
const ACTION_TYPES: &[&str] = &["mark", "reassign-to-superadmin"];

// =====================================================================
// Rule model + validation + CRUD (mirrors §3 auto-label-rule shape)
// =====================================================================

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EscalationRule {
    pub id: String,
    pub trigger_type: String,
    pub trigger_params: serde_json::Value,
    pub action_type: String,
    pub enabled: bool,
    pub created_at: String,
    pub created_by_did: String,
    pub last_modified_at: String,
    pub last_modified_by_did: String,
    pub rationale: Option<String>,
    pub deleted_at: Option<String>,
}

fn rule_from_row(row: &sqlx::any::AnyRow) -> PdsResult<EscalationRule> {
    let params_str: String = row.try_get("trigger_params")?;
    Ok(EscalationRule {
        id: row.try_get("id")?,
        trigger_type: row.try_get("trigger_type")?,
        trigger_params: serde_json::from_str(&params_str).unwrap_or(serde_json::Value::Null),
        action_type: row.try_get("action_type")?,
        enabled: row.try_get::<i64, _>("enabled").unwrap_or(0) != 0,
        created_at: row.try_get("created_at")?,
        created_by_did: row.try_get("created_by_did")?,
        last_modified_at: row.try_get("last_modified_at")?,
        last_modified_by_did: row.try_get("last_modified_by_did")?,
        rationale: row.try_get("rationale").ok().flatten(),
        deleted_at: row.try_get("deleted_at").ok().flatten(),
    })
}

/// Validate per-trigger-type params (§5.7). `report-count`/`operator-action`
/// reuse the §3.4 shapes; `category-match` needs only a valid category.
pub fn validate_trigger_params(trigger_type: &str, params: &serde_json::Value) -> Result<(), String> {
    let positive = |k: &str| -> Result<i64, String> {
        params
            .get(k)
            .and_then(|v| v.as_i64())
            .filter(|v| *v > 0)
            .ok_or_else(|| format!("param '{}' must be a positive integer", k))
    };
    let window = |k: &str| -> Result<(), String> {
        let v = positive(k)?;
        if v <= 365 { Ok(()) } else { Err(format!("param '{}' must be ≤ 365", k)) }
    };
    let category = |key: &str| -> Result<(), String> {
        let c = params.get(key).and_then(|v| v.as_str()).ok_or(format!("missing '{}'", key))?;
        if REPORT_CATEGORIES.contains(&c) { Ok(()) } else { Err(format!("category '{}' invalid", c)) }
    };
    match trigger_type {
        "report-count" => {
            category("category")?;
            positive("threshold")?;
            window("window_days")?;
        }
        "operator-action" => {
            let at = params.get("action_type").and_then(|v| v.as_str()).ok_or("missing 'action_type'")?;
            if !OPERATOR_ACTION_TYPES.contains(&at) {
                return Err(format!("action_type '{}' not a moderation action", at));
            }
            positive("threshold")?;
            window("window_days")?;
        }
        "category-match" => category("category")?,
        other => return Err(format!("unknown trigger type '{}'", other)),
    }
    Ok(())
}

async fn load_active_rules(ctx: &AppContext, trigger_types: &[&str]) -> PdsResult<Vec<EscalationRule>> {
    let rows = sqlx::query(
        "SELECT * FROM moderation_escalation_rule WHERE deleted_at IS NULL AND enabled <> 0",
    )
    .fetch_all(&ctx.account_db)
    .await?;
    let mut out = Vec::new();
    for r in &rows {
        let rule = rule_from_row(r)?;
        if trigger_types.contains(&rule.trigger_type.as_str()) {
            out.push(rule);
        }
    }
    Ok(out)
}

fn internal<E: std::fmt::Display>(e: E) -> (u16, String) {
    (500, e.to_string())
}

#[allow(clippy::too_many_arguments)]
pub async fn create_rule(
    ctx: &AppContext,
    operator_did: &str,
    trigger_type: &str,
    trigger_params: &serde_json::Value,
    action_type: &str,
    enabled: bool,
    rationale: Option<&str>,
) -> Result<EscalationRule, (u16, String)> {
    if !TRIGGER_TYPES.contains(&trigger_type) {
        return Err((400, format!("unknown trigger type '{}'", trigger_type)));
    }
    if !ACTION_TYPES.contains(&action_type) {
        return Err((400, format!("unknown action type '{}'", action_type)));
    }
    validate_trigger_params(trigger_type, trigger_params).map_err(|e| (400, e))?;

    let mut tx = ctx.account_db.begin().await.map_err(internal)?;
    let active: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM moderation_escalation_rule WHERE deleted_at IS NULL")
            .fetch_one(&mut *tx)
            .await
            .map_err(internal)?;
    if active >= MAX_ACTIVE_RULES {
        return Err((400, format!("active escalation rule limit ({}) reached", MAX_ACTIVE_RULES)));
    }
    let id = Uuid::new_v4().simple().to_string();
    let now = Utc::now().to_rfc3339();
    let params_str = serde_json::to_string(trigger_params).map_err(|e| (500, e.to_string()))?;
    sqlx::query(
        "INSERT INTO moderation_escalation_rule \
         (id, trigger_type, trigger_params, action_type, enabled, created_at, created_by_did, \
          last_modified_at, last_modified_by_did, rationale, deleted_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, NULL)",
    )
    .bind(&id)
    .bind(trigger_type)
    .bind(&params_str)
    .bind(action_type)
    .bind(if enabled { 1_i64 } else { 0 })
    .bind(&now)
    .bind(operator_did)
    .bind(&now)
    .bind(operator_did)
    .bind(rationale)
    .execute(&mut *tx)
    .await
    .map_err(internal)?;
    tx.commit().await.map_err(internal)?;

    emit_rule_lifecycle(ctx, ACTION_RULE_CREATED, &id, operator_did, rationale.unwrap_or("escalation rule created"))
        .await
        .map_err(internal)?;

    Ok(EscalationRule {
        id,
        trigger_type: trigger_type.to_string(),
        trigger_params: trigger_params.clone(),
        action_type: action_type.to_string(),
        enabled,
        created_at: now.clone(),
        created_by_did: operator_did.to_string(),
        last_modified_at: now,
        last_modified_by_did: operator_did.to_string(),
        rationale: rationale.map(String::from),
        deleted_at: None,
    })
}

#[allow(clippy::too_many_arguments)]
pub async fn edit_rule(
    ctx: &AppContext,
    operator_did: &str,
    id: &str,
    trigger_type: &str,
    trigger_params: &serde_json::Value,
    action_type: &str,
    enabled: bool,
    rationale: Option<&str>,
) -> Result<(), (u16, String)> {
    if !TRIGGER_TYPES.contains(&trigger_type) {
        return Err((400, format!("unknown trigger type '{}'", trigger_type)));
    }
    if !ACTION_TYPES.contains(&action_type) {
        return Err((400, format!("unknown action type '{}'", action_type)));
    }
    validate_trigger_params(trigger_type, trigger_params).map_err(|e| (400, e))?;
    let now = Utc::now().to_rfc3339();
    let params_str = serde_json::to_string(trigger_params).map_err(|e| (500, e.to_string()))?;
    let res = sqlx::query(
        "UPDATE moderation_escalation_rule SET trigger_type = $1, trigger_params = $2, \
         action_type = $3, enabled = $4, last_modified_at = $5, last_modified_by_did = $6, \
         rationale = $7 WHERE id = $8 AND deleted_at IS NULL",
    )
    .bind(trigger_type)
    .bind(&params_str)
    .bind(action_type)
    .bind(if enabled { 1_i64 } else { 0 })
    .bind(&now)
    .bind(operator_did)
    .bind(rationale)
    .bind(id)
    .execute(&ctx.account_db)
    .await
    .map_err(internal)?;
    if res.rows_affected() == 0 {
        return Err((404, format!("rule {} not found", id)));
    }
    emit_rule_lifecycle(ctx, ACTION_RULE_EDITED, id, operator_did, rationale.unwrap_or("escalation rule edited"))
        .await
        .map_err(internal)?;
    Ok(())
}

pub async fn delete_rule(ctx: &AppContext, operator_did: &str, id: &str) -> Result<(), (u16, String)> {
    let now = Utc::now().to_rfc3339();
    let res = sqlx::query(
        "UPDATE moderation_escalation_rule SET deleted_at = $1 WHERE id = $2 AND deleted_at IS NULL",
    )
    .bind(&now)
    .bind(id)
    .execute(&ctx.account_db)
    .await
    .map_err(internal)?;
    if res.rows_affected() == 0 {
        return Err((404, format!("rule {} not found", id)));
    }
    emit_rule_lifecycle(ctx, ACTION_RULE_DELETED, id, operator_did, "escalation rule deleted")
        .await
        .map_err(internal)?;
    Ok(())
}

pub async fn list_rules(ctx: &AppContext, include_deleted: bool) -> Result<Vec<EscalationRule>, (u16, String)> {
    let sql = if include_deleted {
        "SELECT * FROM moderation_escalation_rule ORDER BY created_at DESC"
    } else {
        "SELECT * FROM moderation_escalation_rule WHERE deleted_at IS NULL ORDER BY created_at DESC"
    };
    let rows = sqlx::query(sql).fetch_all(&ctx.account_db).await.map_err(internal)?;
    rows.iter().map(|r| rule_from_row(r).map_err(internal)).collect()
}

// =====================================================================
// Audit emitters
// =====================================================================

async fn emit_rule_lifecycle(
    ctx: &AppContext,
    action: &str,
    rule_id: &str,
    operator_did: &str,
    rationale: &str,
) -> PdsResult<i64> {
    audit_chain::insert_chain_entry_pool(
        &ctx.account_db,
        ctx.config.database.backend,
        AppendEntryParams {
            actor_did: operator_did,
            source: SOURCE_MANUAL,
            payload: Some(serde_json::json!({ "rule_id": rule_id })),
            action,
            subject: None,
            rationale,
            snapshot_id: None,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn emit_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    backend: DatabaseBackend,
    actor: &str,
    action: &str,
    source: &str,
    subject: &Subject,
    payload: serde_json::Value,
    rationale: &str,
) -> PdsResult<()> {
    audit_chain::insert_chain_entry(
        tx,
        backend,
        AppendEntryParams {
            actor_did: actor,
            source,
            payload: Some(payload),
            action,
            subject: Some(subject),
            rationale,
            snapshot_id: None,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await?;
    Ok(())
}

// =====================================================================
// Episode detection (§5.9 skip-guard / §5.6 lookup) — Rust-side (D2/D3)
// =====================================================================

/// Resolve the originating rule id of the item's CURRENTLY-ACTIVE escalation
/// episode, or `None` if no active episode (D2/D3). Index-narrowed by the
/// account's `subject_did`, then Rust-filtered by `item_id` and
/// `caused_state_change`: walking newest-first, the first relevant event for
/// this item decides — a `_cleared` means the episode is closed (`None`); a
/// `_triggered` with `caused_state_change=true` means active (its `rule_id`).
async fn active_episode_rule(
    ctx: &AppContext,
    account_did: &str,
    item_id: &str,
) -> PdsResult<Option<String>> {
    let rows = sqlx::query(
        "SELECT action, payload FROM audit_chain_entry \
         WHERE subject_did = $1 AND action IN ($2, $3) \
         ORDER BY created_at DESC, sequence DESC LIMIT 200",
    )
    .bind(account_did)
    .bind(ACTION_TRIGGERED)
    .bind(ACTION_CLEARED)
    .fetch_all(&ctx.account_db)
    .await?;
    for row in &rows {
        let action: String = row.try_get("action")?;
        let payload_s: Option<String> = row.try_get("payload").ok().flatten();
        let p: serde_json::Value = payload_s
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(serde_json::Value::Null);
        if p.get("item_id").and_then(|v| v.as_str()) != Some(item_id) {
            continue;
        }
        if action == ACTION_CLEARED {
            return Ok(None); // most recent relevant event closed the episode
        }
        if action == ACTION_TRIGGERED
            && p.get("caused_state_change").and_then(|v| v.as_bool()) == Some(true)
        {
            return Ok(p.get("rule_id").and_then(|v| v.as_str()).map(String::from));
        }
    }
    Ok(None)
}

// =====================================================================
// Consumed-row mechanism (§5.6)
// =====================================================================

async fn is_consumed(ctx: &AppContext, rule_id: &str, item_id: &str) -> PdsResult<bool> {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM moderation_escalation_consumed WHERE rule_id = $1 AND item_id = $2",
    )
    .bind(rule_id)
    .bind(item_id)
    .fetch_one(&ctx.account_db)
    .await?;
    Ok(n > 0)
}

// =====================================================================
// Row-level serialization (§5.9 / D1)
// =====================================================================

/// Acquire the per-item escalation-eval lock inside the mutation tx. Postgres:
/// `SELECT … FOR UPDATE NOWAIT` (lock-unavailable → `Ok(false)`). SQLite: the
/// `escalation_eval_lock` advisory row (unique-key conflict → `Ok(false)`).
async fn acquire_eval_lock_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    backend: DatabaseBackend,
    item_id: &str,
) -> bool {
    match backend {
        DatabaseBackend::Postgres => sqlx::query("SELECT id FROM report WHERE id = $1 FOR UPDATE NOWAIT")
            .bind(item_id.parse::<i64>().unwrap_or(-1))
            .fetch_optional(&mut **tx)
            .await
            .is_ok(),
        DatabaseBackend::Sqlite => {
            let key = format!("escalation-eval-lock:{}", item_id);
            sqlx::query("INSERT INTO escalation_eval_lock (lock_key, acquired_at) VALUES ($1, $2)")
                .bind(&key)
                .bind(Utc::now().to_rfc3339())
                .execute(&mut **tx)
                .await
                .is_ok()
        }
    }
}

/// Release the SQLite advisory lock row before commit (Postgres FOR UPDATE
/// releases on commit — no-op).
async fn release_eval_lock_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    backend: DatabaseBackend,
    item_id: &str,
) -> PdsResult<()> {
    if let DatabaseBackend::Sqlite = backend {
        let key = format!("escalation-eval-lock:{}", item_id);
        sqlx::query("DELETE FROM escalation_eval_lock WHERE lock_key = $1")
            .bind(&key)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

// =====================================================================
// SuperAdmin cursor (§5.4) — reassign-to-superadmin
// =====================================================================

/// All active SuperAdmins, deduped + DID-sorted (§5.4 pool).
async fn superadmin_pool(ctx: &AppContext) -> PdsResult<Vec<String>> {
    let roles = ctx.admin_role_manager.list_active_roles().await?;
    let mut dids: Vec<String> = roles
        .into_iter()
        .filter(|r| r.role.can_act_as(Role::SuperAdmin))
        .map(|r| r.did)
        .collect();
    dids.sort();
    dids.dedup();
    Ok(dids)
}

/// CAS-advance the escalation SuperAdmin cursor over `modulo`, returning the
/// assignee index. Bounded retry; best-effort proceed (§5.4 / Phase B CAS).
async fn cas_advance_superadmin_cursor(ctx: &AppContext, modulo: usize) -> usize {
    if modulo == 0 {
        return 0;
    }
    let m = modulo as u64;
    let key = MODERATION_ESCALATION_SUPERADMIN_CURSOR_KEY;
    for _ in 0..CAS_MAX_RETRIES {
        let raw = read_runtime_row_value(ctx, key).await.unwrap_or_else(|| "0".to_string());
        let current = serde_json::from_str::<serde_json::Value>(&raw)
            .ok()
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let assignee = (current % m) as usize;
        let next = ((current + 1) % m).to_string();
        if cas_runtime_setting(ctx, key, &raw, &next, SYSTEM_DID).await.unwrap_or(false) {
            return assignee;
        }
    }
    let raw = read_runtime_row_value(ctx, key).await.unwrap_or_else(|| "0".to_string());
    (serde_json::from_str::<serde_json::Value>(&raw).ok().and_then(|v| v.as_u64()).unwrap_or(0) % m)
        as usize
}

// =====================================================================
// Apply an escalation rule to one queue item (§5.4 + §5.9)
// =====================================================================

/// `(status, account_did)` for an open report id, or `None` if absent.
async fn report_state(ctx: &AppContext, item_id: &str) -> PdsResult<Option<(String, Option<String>)>> {
    let row = sqlx::query("SELECT status, subject_did, subject_uri FROM report WHERE id = $1")
        .bind(item_id.parse::<i64>().unwrap_or(-1))
        .fetch_optional(&ctx.account_db)
        .await?;
    Ok(row.map(|r| {
        let status: String = r.try_get("status").unwrap_or_default();
        let did: Option<String> = r.try_get("subject_did").ok().flatten();
        let uri: Option<String> = r.try_get("subject_uri").ok().flatten();
        let account = did.or_else(|| {
            uri.and_then(|u| normalize_subject_value(&Subject::Record { uri: u, cid: String::new() }))
        });
        (status, account)
    }))
}

/// Apply one escalation rule to one queue item, with row-level serialization,
/// skip-if-consumed, and the tightened skip-guard. `triggering_event_id` is
/// the report id (Pipeline A) or audit-entry id (Pipeline B); `pipeline` ∈ A|B.
async fn apply_escalation(
    ctx: &AppContext,
    rule: &EscalationRule,
    item_id: &str,
    account_did: &str,
    triggering_event_id: &str,
    pipeline: &str,
) -> PdsResult<()> {
    // §5.9 skip-if-consumed → no fire, no audit.
    if is_consumed(ctx, &rule.id, item_id).await? {
        return Ok(());
    }
    let backend = ctx.config.database.backend;
    let subject = Subject::Repo { did: account_did.to_string() };

    // reassign-to-superadmin resolves its assignee up-front (independent CAS).
    let reassign_to: Option<String> = if rule.action_type == "reassign-to-superadmin" {
        let pool = superadmin_pool(ctx).await?;
        if pool.is_empty() {
            None
        } else {
            let idx = cas_advance_superadmin_cursor(ctx, pool.len()).await;
            pool.get(idx).cloned()
        }
    } else {
        None
    };

    for attempt in 0..CAS_MAX_RETRIES {
        let _guard = AppendChainGuard::acquire().await;
        let mut tx = ctx.account_db.begin().await?;
        if !acquire_eval_lock_in_tx(&mut tx, backend, item_id).await {
            let _ = tx.rollback().await;
            drop(_guard);
            // Bounded backoff before retry (skip on the last attempt).
            if attempt + 1 < CAS_MAX_RETRIES {
                tokio::time::sleep(std::time::Duration::from_millis(10 << attempt)).await;
            }
            continue;
        }

        let active = active_episode_rule(ctx, account_did, item_id).await?;
        let status = report_state(ctx, item_id)
            .await?
            .map(|(s, _)| s)
            .unwrap_or_default();

        if active.is_some() && status == "escalated" {
            // §5.9 skip-guard fire: audit for per-rule visibility, no state change.
            emit_in_tx(
                &mut tx,
                backend,
                SYSTEM_DID,
                ACTION_TRIGGERED,
                SOURCE_ESCALATION,
                &subject,
                serde_json::json!({
                    "rule_id": rule.id,
                    "caused_state_change": false,
                    "triggering_event_id": triggering_event_id,
                    "pipeline": pipeline,
                    "item_id": item_id,
                }),
                "escalation skip-guard fire (active episode)",
            )
            .await?;
        } else {
            // Originating fire: state change.
            if let Some(sa) = &reassign_to {
                sqlx::query(
                    "UPDATE report SET status = 'escalated', assignment_source = 'escalation', \
                     assigned_operator_did = $1 WHERE id = $2",
                )
                .bind(sa)
                .bind(item_id.parse::<i64>().unwrap_or(-1))
                .execute(&mut *tx)
                .await?;
            } else {
                sqlx::query(
                    "UPDATE report SET status = 'escalated', assignment_source = 'escalation' WHERE id = $1",
                )
                .bind(item_id.parse::<i64>().unwrap_or(-1))
                .execute(&mut *tx)
                .await?;
            }
            emit_in_tx(
                &mut tx,
                backend,
                SYSTEM_DID,
                ACTION_TRIGGERED,
                SOURCE_ESCALATION,
                &subject,
                serde_json::json!({
                    "rule_id": rule.id,
                    "caused_state_change": true,
                    "triggering_event_id": triggering_event_id,
                    "pipeline": pipeline,
                    "item_id": item_id,
                }),
                "escalation originating fire",
            )
            .await?;
            // The assignment-change audit (§5.4): mark → reviewer_assigned;
            // reassign → escalation_reassigned. Both source=escalation.
            if let Some(sa) = &reassign_to {
                emit_in_tx(
                    &mut tx,
                    backend,
                    SYSTEM_DID,
                    ACTION_REASSIGNED,
                    SOURCE_ESCALATION,
                    &subject,
                    serde_json::json!({ "rule_id": rule.id, "item_id": item_id, "assigned_operator_did": sa }),
                    "escalation reassigned to SuperAdmin",
                )
                .await?;
            } else {
                emit_in_tx(
                    &mut tx,
                    backend,
                    SYSTEM_DID,
                    ACTION_REVIEWER_ASSIGNED,
                    SOURCE_ESCALATION,
                    &subject,
                    serde_json::json!({ "rule_id": rule.id, "item_id": item_id }),
                    "escalation mark",
                )
                .await?;
            }
        }

        release_eval_lock_in_tx(&mut tx, backend, item_id).await?;
        tx.commit().await?;
        drop(_guard);
        return Ok(());
    }

    // §5.9 contention exhaustion → eval_skipped, abort (no re-evaluation).
    emit_eval_skipped(ctx, &subject, item_id, triggering_event_id, pipeline).await?;
    Ok(())
}

async fn emit_eval_skipped(
    ctx: &AppContext,
    subject: &Subject,
    item_id: &str,
    triggering_event_id: &str,
    pipeline: &str,
) -> PdsResult<()> {
    audit_chain::insert_chain_entry_pool(
        &ctx.account_db,
        ctx.config.database.backend,
        AppendEntryParams {
            actor_did: SYSTEM_DID,
            source: SOURCE_MANUAL,
            payload: Some(serde_json::json!({
                "reason": "concurrent_evaluation_in_progress",
                "triggering_event_id": triggering_event_id,
                "pipeline": pipeline,
                "item_id": item_id,
            })),
            action: ACTION_EVAL_SKIPPED,
            subject: Some(subject),
            rationale: "escalation evaluation skipped (contention)",
            snapshot_id: None,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await?;
    Ok(())
}

// =====================================================================
// Pipeline consumers
// =====================================================================

fn window_cutoff(params: &serde_json::Value, key: &str) -> String {
    let days = params.get(key).and_then(|v| v.as_i64()).unwrap_or(0);
    (Utc::now() - Duration::days(days)).to_rfc3339()
}

/// Pipeline A (§6.9) — report-count + category-match escalation at intake.
pub async fn evaluate_pipeline_a(ctx: &AppContext, report: &Report) -> PdsResult<()> {
    if !defaults_active(ctx).await {
        return Ok(());
    }
    let account = match report_subject(report).as_ref().and_then(normalize_subject_value) {
        Some(a) => a,
        None => return Ok(()), // can't track an unresolvable subject (D3 graceful None)
    };
    let item_id = report.id.to_string();
    let rules = load_active_rules(ctx, &["report-count", "category-match"]).await?;
    for rule in &rules {
        let p = &rule.trigger_params;
        let matched = match rule.trigger_type.as_str() {
            "category-match" => {
                p.get("category").and_then(|v| v.as_str()) == Some(report.reason_type.as_str())
            }
            "report-count" => {
                let category = p.get("category").and_then(|v| v.as_str()).unwrap_or("");
                if category != report.reason_type.as_str() {
                    false
                } else {
                    let threshold = p.get("threshold").and_then(|v| v.as_i64()).unwrap_or(i64::MAX);
                    let cutoff = window_cutoff(p, "window_days");
                    let count: i64 = sqlx::query_scalar(
                        "SELECT COUNT(*) FROM report WHERE subject_did = $1 AND reason_type = $2 AND reported_at > $3",
                    )
                    .bind(&account)
                    .bind(category)
                    .bind(&cutoff)
                    .fetch_one(&ctx.account_db)
                    .await?;
                    count >= threshold
                }
            }
            _ => false,
        };
        if matched {
            apply_escalation(ctx, rule, &item_id, &account, &item_id, "A").await?;
        }
    }
    Ok(())
}

/// Pipeline B (§6.9) — operator-action escalation at `emit_event` post-commit.
/// Resolves the account's open queue items and evaluates each.
pub async fn evaluate_pipeline_b(
    ctx: &AppContext,
    subject: &Subject,
    action_type: &str,
    actor_did: &str,
) -> PdsResult<()> {
    if !defaults_active(ctx).await || actor_did == SYSTEM_DID {
        return Ok(());
    }
    let account = match normalize_subject_value(subject) {
        Some(a) => a,
        None => return Ok(()),
    };
    let rules = load_active_rules(ctx, &["operator-action"]).await?;
    if rules.is_empty() {
        return Ok(());
    }
    // §5.9 Pipeline B subject→queue-item resolution: the account's open reports.
    let item_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM report WHERE subject_did = $1 AND status <> $2",
    )
    .bind(&account)
    .bind(TERMINAL_STATUS)
    .fetch_all(&ctx.account_db)
    .await?;
    for rule in &rules {
        let at = rule.trigger_params.get("action_type").and_then(|v| v.as_str()).unwrap_or("");
        if at != action_type {
            continue;
        }
        let threshold = rule.trigger_params.get("threshold").and_then(|v| v.as_i64()).unwrap_or(i64::MAX);
        let cutoff = window_cutoff(&rule.trigger_params, "window_days");
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_chain_entry \
             WHERE subject_did = $1 AND action = $2 AND created_at > $3 AND actor_did <> $4",
        )
        .bind(&account)
        .bind(action_type)
        .bind(&cutoff)
        .bind(SYSTEM_DID)
        .fetch_one(&ctx.account_db)
        .await?;
        if count < threshold {
            continue;
        }
        for id in &item_ids {
            let item_id = id.to_string();
            apply_escalation(ctx, rule, &item_id, &account, action_type, "B").await?;
        }
    }
    Ok(())
}

// =====================================================================
// De-escalation (§5.6)
// =====================================================================

/// `clearEscalation` (§5.6): SuperAdmin de-escalates an item. Resolves the
/// originating rule (audit-chain lookup), transitions status → acknowledged
/// per the current §4 mode, conditionally records the consumed row, and emits
/// `moderation_escalation_cleared`.
pub async fn clear_escalation(
    ctx: &AppContext,
    item_id: &str,
    rationale: &str,
    operator_did: &str,
) -> Result<(), (u16, String)> {
    let (status, account) = report_state(ctx, item_id)
        .await
        .map_err(internal)?
        .ok_or((404, format!("item {} not found", item_id)))?;
    if status != "escalated" {
        return Err((400, "item is not escalated".to_string()));
    }
    let account = account.ok_or((400, "item subject does not resolve to an account".to_string()))?;
    let rule_id = active_episode_rule(ctx, &account, item_id).await.map_err(internal)?;

    let mode = resolve_runtime_setting(ctx, MODERATION_REVIEWER_MODE_KEY).await;
    let mode = mode.as_str().unwrap_or("manual").to_string();
    let backend = ctx.config.database.backend;
    let subject = Subject::Repo { did: account.clone() };

    let _guard = AppendChainGuard::acquire().await;
    let mut tx = ctx.account_db.begin().await.map_err(internal)?;
    if mode == "manual" {
        // Preserve the assignee; just exit the escalation episode.
        sqlx::query("UPDATE report SET status = 'acknowledged', assignment_source = 'manual_override' WHERE id = $1")
            .bind(item_id.parse::<i64>().unwrap_or(-1))
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
    } else {
        // Clear the lock; re-routing happens post-commit per the current mode.
        sqlx::query("UPDATE report SET status = 'acknowledged', assignment_source = NULL, assigned_operator_did = NULL WHERE id = $1")
            .bind(item_id.parse::<i64>().unwrap_or(-1))
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
    }
    // Conditional consumed-row insert (skip on audit-lookup-failure: rule_id None).
    if let Some(rid) = &rule_id {
        sqlx::query(
            "INSERT INTO moderation_escalation_consumed (rule_id, item_id, deescalated_at, deescalated_by) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(rid)
        .bind(item_id)
        .bind(Utc::now().to_rfc3339())
        .bind(operator_did)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
    }
    emit_in_tx(
        &mut tx,
        backend,
        operator_did,
        ACTION_CLEARED,
        SOURCE_ESCALATION,
        &subject,
        serde_json::json!({ "rationale": rationale, "rule_id": rule_id, "item_id": item_id }),
        rationale,
    )
    .await
    .map_err(internal)?;
    tx.commit().await.map_err(internal)?;
    drop(_guard);

    // Non-manual modes: re-route per the current §4 mode (sets source='auto').
    if mode != "manual" {
        if let Ok(Some(report)) = ctx.report_manager.get_report(item_id.parse::<i64>().unwrap_or(-1)).await {
            let _ = crate::api::reviewer_assignment::assign_reviewer_on_intake(ctx, &report).await;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin::reports::ReportReason;
    use crate::config::*;
    use tempfile::tempdir;

    #[test]
    fn validation_covers_three_triggers_and_actions() {
        assert!(validate_trigger_params("category-match", &serde_json::json!({"category": "spam"})).is_ok());
        assert!(validate_trigger_params("category-match", &serde_json::json!({"category": "x"})).is_err());
        assert!(validate_trigger_params(
            "report-count",
            &serde_json::json!({"category": "spam", "threshold": 3, "window_days": 7})
        )
        .is_ok());
        assert!(validate_trigger_params(
            "operator-action",
            &serde_json::json!({"action_type": "role.grant", "threshold": 1, "window_days": 7})
        )
        .is_err());
        assert!(ACTION_TYPES.contains(&"mark"));
        assert!(ACTION_TYPES.contains(&"reassign-to-superadmin"));
        assert!(!ACTION_TYPES.contains(&"bogus"));
    }

    async fn create_test_context() -> AppContext {
        let dir = tempdir().unwrap().keep();
        let db_path = dir.join("test.db");
        let config = ServerConfig {
            service: ServiceConfig {
                hostname: "localhost".to_string(),
                port: 2583,
                service_did: "did:web:localhost".to_string(),
                version: "0.1.0-test".to_string(),
                blob_upload_limit: 5_242_880,
                public_url: None,
                max_blob_fetch_size: 50_000_000,
                blob_fetch_timeout_seconds: 30,
                blob_fetch_max_retries: 3,
                accepting_imports: true,
                max_import_size: None,
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
                jwt_secret: "test-secret-key-aurora-escalation-rule-x".to_string(),
                repo_signing_key: "a".repeat(64),
                plc_rotation_key: "b".repeat(64),
                password_login_enabled: false,
                oauth: OAuthConfig {
                    client_id: "http://localhost:3000/client-metadata.json".to_string(),
                    redirect_uri: "http://localhost:3000/oauth/callback".to_string(),
                    pds_url: "https://bsky.social".to_string(),
                },
                jwt_sunset_date: "Sat, 31 Dec 2024 23:59:59 GMT".to_string(),
                oauth_migration_guide_url: "https://docs.atproto.com/guides/oauth-migration".to_string(),
            },
            identity: IdentityConfig {
                did_plc_url: "https://plc.directory".to_string(),
                service_handle_domains: vec![".localhost".to_string()],
                did_cache_stale_ttl: 3600,
                did_cache_max_ttl: 86400,
                recovery_did_key: None,
            },
            email: None,
            invites: InviteConfig { required: false, interval: 604800, epoch: "2024-01-01T00:00:00Z".to_string() },
            rate_limit: RateLimitConfig { enabled: false, global_requests_per_minute: 3000, exempt_admin_assets: true, buckets_retention_days: 7 },
            logging: LoggingConfig { level: "info".to_string() },
            federation: FederationConfig {
                enabled: false,
                relay_urls: vec![],
                appview_url: None,
                firehose_enabled: false,
                crawl_enabled: false,
                public_url: Some("http://localhost:2583".to_string()),
                peer_pds: vec![],
            },
            validation_mode: crate::validation::ValidationMode::Required,
            distributed_state_mode: Default::default(),
            maintenance_pool: Default::default(),
            gc_sweep: Default::default(),
            bind_audit_orphan_marker: Default::default(),
            blob_metadata: Default::default(),
            entryway: None,
            lexicon: crate::config::LexiconConfig::default(),
            kryphocron: crate::config::KryphocronConfig::default(),
        };
        AppContext::new(config, std::sync::Arc::new(crate::api::registry::RouteRegistry::default()))
            .await
            .unwrap()
    }

    async fn mk_rule(ctx: &AppContext, tt: &str, params: serde_json::Value, action: &str) -> EscalationRule {
        create_rule(ctx, "did:plc:super", tt, &params, action, true, Some("t")).await.unwrap()
    }

    async fn submit(ctx: &AppContext, reason: ReportReason) -> Report {
        ctx.report_manager
            .submit_report(Some("did:plc:victim"), None, None, reason, Some("r"), "did:plc:reporter")
            .await
            .unwrap()
    }

    async fn status_of(ctx: &AppContext, id: i64) -> (String, Option<String>, Option<String>) {
        let r = sqlx::query("SELECT status, assignment_source, assigned_operator_did FROM report WHERE id = $1")
            .bind(id).fetch_one(&ctx.account_db).await.unwrap();
        (r.try_get("status").unwrap(), r.try_get("assignment_source").ok().flatten(), r.try_get("assigned_operator_did").ok().flatten())
    }

    async fn audit_actions(ctx: &AppContext) -> Vec<(String, String, Option<String>)> {
        let rows = sqlx::query("SELECT action, source, payload FROM audit_chain_entry ORDER BY sequence ASC")
            .fetch_all(&ctx.account_db).await.unwrap();
        rows.iter().map(|r| (
            r.try_get::<String, _>("action").unwrap(),
            r.try_get::<String, _>("source").unwrap(),
            r.try_get::<Option<String>, _>("payload").ok().flatten(),
        )).collect()
    }

    #[tokio::test]
    async fn crud_with_lifecycle_audits_and_cap() {
        let ctx = create_test_context().await;
        let r = mk_rule(&ctx, "category-match", serde_json::json!({"category": "spam"}), "mark").await;
        assert!(audit_actions(&ctx).await.iter().any(|(a, s, _)| a == ACTION_RULE_CREATED && s == "manual"));
        assert_eq!(list_rules(&ctx, false).await.unwrap().len(), 1);
        edit_rule(&ctx, "did:plc:super", &r.id, "category-match", &serde_json::json!({"category": "rude"}), "mark", false, Some("e")).await.unwrap();
        delete_rule(&ctx, "did:plc:super", &r.id).await.unwrap();
        assert_eq!(list_rules(&ctx, false).await.unwrap().len(), 0);
        assert_eq!(list_rules(&ctx, true).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn pipeline_a_category_match_escalates_and_audits() {
        let ctx = create_test_context().await;
        mk_rule(&ctx, "category-match", serde_json::json!({"category": "spam"}), "mark").await;
        let report = submit(&ctx, ReportReason::Spam).await;
        evaluate_pipeline_a(&ctx, &report).await.unwrap();
        let (status, src, _) = status_of(&ctx, report.id).await;
        assert_eq!(status, "escalated");
        assert_eq!(src.as_deref(), Some("escalation"));
        let acts = audit_actions(&ctx).await;
        // _triggered caused_state_change=true + reviewer_assigned (source=escalation).
        assert!(acts.iter().any(|(a, s, p)| a == ACTION_TRIGGERED && s == "escalation"
            && p.as_deref().unwrap().contains("\"caused_state_change\":true")));
        assert!(acts.iter().any(|(a, s, _)| a == ACTION_REVIEWER_ASSIGNED && s == "escalation"));
    }

    #[tokio::test]
    async fn reassign_to_superadmin_sets_assignee() {
        let ctx = create_test_context().await;
        ctx.admin_role_manager.grant_role("did:plc:sa", Role::SuperAdmin, "did:plc:boot", None).await.unwrap();
        mk_rule(&ctx, "category-match", serde_json::json!({"category": "spam"}), "reassign-to-superadmin").await;
        let report = submit(&ctx, ReportReason::Spam).await;
        evaluate_pipeline_a(&ctx, &report).await.unwrap();
        let (status, src, assignee) = status_of(&ctx, report.id).await;
        assert_eq!(status, "escalated");
        assert_eq!(src.as_deref(), Some("escalation"));
        assert_eq!(assignee.as_deref(), Some("did:plc:sa"));
        assert!(audit_actions(&ctx).await.iter().any(|(a, s, _)| a == ACTION_REASSIGNED && s == "escalation"));
    }

    #[tokio::test]
    async fn skip_guard_does_not_double_escalate() {
        let ctx = create_test_context().await;
        mk_rule(&ctx, "category-match", serde_json::json!({"category": "spam"}), "mark").await;
        let r1 = submit(&ctx, ReportReason::Spam).await;
        evaluate_pipeline_a(&ctx, &r1).await.unwrap();
        // Re-evaluating the SAME item (already escalated, active episode) →
        // skip-guard fire, no second state change. (Pipeline B re-evaluates
        // open items repeatedly; here we re-run Pipeline A on the same report.)
        evaluate_pipeline_a(&ctx, &r1).await.unwrap();
        // Exactly one originating (csc=true) + one skip-guard (csc=false).
        let acts = audit_actions(&ctx).await;
        let csc_true = acts.iter().filter(|(a, _, p)| a == ACTION_TRIGGERED && p.as_deref().unwrap().contains("\"caused_state_change\":true")).count();
        let csc_false = acts.iter().filter(|(a, _, p)| a == ACTION_TRIGGERED && p.as_deref().unwrap().contains("\"caused_state_change\":false")).count();
        assert_eq!(csc_true, 1, "one originating fire");
        assert_eq!(csc_false, 1, "one skip-guard fire");
    }

    #[tokio::test]
    async fn clear_escalation_manual_mode_and_consumed() {
        let ctx = create_test_context().await;
        let rule = mk_rule(&ctx, "category-match", serde_json::json!({"category": "spam"}), "mark").await;
        let report = submit(&ctx, ReportReason::Spam).await;
        evaluate_pipeline_a(&ctx, &report).await.unwrap();
        assert_eq!(status_of(&ctx, report.id).await.0, "escalated");

        // Default mode is "manual" → assignee preserved, status acknowledged.
        clear_escalation(&ctx, &report.id.to_string(), "appeal granted", "did:plc:sa").await.unwrap();
        let (status, src, _) = status_of(&ctx, report.id).await;
        assert_eq!(status, "acknowledged");
        assert_eq!(src.as_deref(), Some("manual_override"));
        // _cleared emitted + consumed row recorded for the originating rule.
        assert!(audit_actions(&ctx).await.iter().any(|(a, s, _)| a == ACTION_CLEARED && s == "escalation"));
        assert!(is_consumed(&ctx, &rule.id, &report.id.to_string()).await.unwrap());

        // Re-report → the consumed rule does NOT re-fire.
        let r2 = ctx.report_manager
            .submit_report(Some("did:plc:victim"), None, None, ReportReason::Spam, Some("r"), "did:plc:reporter")
            .await.unwrap();
        evaluate_pipeline_a(&ctx, &r2).await.unwrap();
        // status stays acknowledged (no new escalation from the consumed rule).
        assert_eq!(status_of(&ctx, report.id).await.0, "acknowledged");
    }

    #[tokio::test]
    async fn clear_escalation_rejects_non_escalated() {
        let ctx = create_test_context().await;
        let report = submit(&ctx, ReportReason::Spam).await;
        let err = clear_escalation(&ctx, &report.id.to_string(), "x", "did:plc:sa").await.unwrap_err();
        assert_eq!(err.0, 400);
    }

    #[tokio::test]
    async fn pipeline_b_operator_action_escalates_open_item() {
        let ctx = create_test_context().await;
        mk_rule(&ctx, "operator-action", serde_json::json!({"action_type": "TakedownAccount", "threshold": 1, "window_days": 30}), "mark").await;
        // An open report for the account is the queue item to escalate.
        let report = submit(&ctx, ReportReason::Spam).await;
        let subject = Subject::Repo { did: "did:plc:victim".into() };
        // Seed a prior operator TakedownAccount audit for the account.
        audit_chain::insert_chain_entry_pool(&ctx.account_db, ctx.config.database.backend, AppendEntryParams {
            actor_did: "did:plc:mod", source: "manual", payload: None, action: "TakedownAccount",
            subject: Some(&subject), rationale: "p", snapshot_id: None, event_id: None,
            cascade_subjects: &[], cascade_snapshot_ids: &[],
        }).await.unwrap();
        evaluate_pipeline_b(&ctx, &subject, "TakedownAccount", "did:plc:mod").await.unwrap();
        assert_eq!(status_of(&ctx, report.id).await.0, "escalated");
    }

    #[tokio::test]
    async fn contention_emits_eval_skipped() {
        let ctx = create_test_context().await;
        mk_rule(&ctx, "category-match", serde_json::json!({"category": "spam"}), "mark").await;
        let report = submit(&ctx, ReportReason::Spam).await;
        // Pre-seed a committed advisory lock for this item → every acquire conflicts.
        sqlx::query("INSERT INTO escalation_eval_lock (lock_key, acquired_at) VALUES ($1, $2)")
            .bind(format!("escalation-eval-lock:{}", report.id))
            .bind(Utc::now().to_rfc3339())
            .execute(&ctx.account_db).await.unwrap();
        evaluate_pipeline_a(&ctx, &report).await.unwrap();
        // No escalation; an _eval_skipped audit with the contention reason.
        assert_eq!(status_of(&ctx, report.id).await.0, "open");
        assert!(audit_actions(&ctx).await.iter().any(|(a, _, p)|
            a == ACTION_EVAL_SKIPPED && p.as_deref().unwrap().contains("concurrent_evaluation_in_progress")));
    }

    // §5.5.4 Phase E (§6.2) — reversibility: soft-deleting a rule stops its
    // go-forward behavior; existing audit history is preserved.
    #[tokio::test]
    async fn reversibility_deleting_rule_stops_escalation_keeps_history() {
        let ctx = create_test_context().await;
        let rule = mk_rule(&ctx, "category-match", serde_json::json!({"category": "spam"}), "mark").await;
        let r1 = submit(&ctx, ReportReason::Spam).await;
        evaluate_pipeline_a(&ctx, &r1).await.unwrap();
        assert_eq!(status_of(&ctx, r1.id).await.0, "escalated");
        let triggered_before = audit_actions(&ctx).await.iter().filter(|(a, _, _)| a == ACTION_TRIGGERED).count();
        assert!(triggered_before >= 1);

        // Rollback: soft-delete the rule.
        delete_rule(&ctx, "did:plc:super", &rule.id).await.unwrap();

        // Re-exercise with a fresh account → no escalation.
        let r2 = ctx.report_manager
            .submit_report(Some("did:plc:other"), None, None, ReportReason::Spam, Some("r"), "did:plc:reporter")
            .await.unwrap();
        evaluate_pipeline_a(&ctx, &r2).await.unwrap();
        assert_eq!(status_of(&ctx, r2.id).await.0, "open", "deleted rule does not fire");
        // History preserved: no new _triggered, the original still present.
        let triggered_after = audit_actions(&ctx).await.iter().filter(|(a, _, _)| a == ACTION_TRIGGERED).count();
        assert_eq!(triggered_after, triggered_before, "audit history preserved, no new fires");
    }

    #[tokio::test]
    async fn tier_gate_blocks_escalation() {
        let ctx = create_test_context().await;
        sqlx::query("INSERT INTO runtime_settings (key, value, last_modified, last_modified_by) VALUES ('moderation-mode', '\"reduced\"', 'now', 'op')")
            .execute(&ctx.account_db).await.unwrap();
        mk_rule(&ctx, "category-match", serde_json::json!({"category": "spam"}), "mark").await;
        let report = submit(&ctx, ReportReason::Spam).await;
        evaluate_pipeline_a(&ctx, &report).await.unwrap();
        assert_eq!(status_of(&ctx, report.id).await.0, "open");
    }
}
