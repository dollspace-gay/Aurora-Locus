//! §5.5.4 Phase C — auto-label rules (§3).
//!
//! Operator-defined rules that auto-apply labels when substrate-observable
//! conditions are met, across three event pipelines (§6.9):
//!
//! - **Pipeline A** (report intake): `report-count` triggers — N reports of
//!   category X against a subject within window Y.
//! - **Pipeline B** (operator moderation action via `emit_event`):
//!   `operator-action` triggers — N operator actions of type X against a
//!   subject within window Y. The repeat-offender mechanism (§3.3).
//! - **Pipeline C** (post creation): `account-age-activity` triggers — new
//!   accounts (< N days) with > M posts.
//!
//! Full-tier gated (§6.3, reuses Phase A's `defaults_active`). Rule CRUD is
//! SuperAdmin-gated; rules soft-delete; capped at 100 active.
//!
//! **Local-idiom translations** (memory #18, recorded per Nova's Decisions):
//! - Pipeline B hooks synchronously at `emit_event` (the unified operator-
//!   moderation surface), scoped to its 16 moderation action_types — not a
//!   generic audit-chain-append hook (AL has no central one). Pipeline B's
//!   window count matches `audit_chain_entry.subject_did` (account-level
//!   operator actions; the indexed path).
//! - Pipeline C counts posts against the author's own per-actor record store
//!   (AL has no global records table); account-age via `actor.created_at`.
//! - Rule-lifecycle audits emit `source = "manual"` (AL's `source` column is
//!   NOT NULL and operator actions use `manual`; the design's NULL doesn't
//!   apply). Subject = `None` with the rule id in the payload (AL's audit
//!   `Subject` is content/account-typed, not a free rule id — same pattern
//!   as Phase A's report-id translation).

use crate::admin::audit_chain::{self, AppendEntryParams};
use crate::admin::defs::Subject;
use crate::admin::labels::LabelManager;
use crate::admin::reports::Report;
use crate::api::moderation_defaults::{defaults_active, server_did, SYSTEM_DID};
use crate::error::PdsResult;
use crate::AppContext;
use chrono::{DateTime, Duration, Utc};
use sqlx::Row as _;
use uuid::Uuid;

/// §3.5 active-rule cap.
const MAX_ACTIVE_RULES: i64 = 100;

// Audit action names (§3.5 / §6.1 — Phase C).
const ACTION_RULE_CREATED: &str = "moderation_auto_label_rule_created";
const ACTION_RULE_EDITED: &str = "moderation_auto_label_rule_edited";
const ACTION_RULE_DELETED: &str = "moderation_auto_label_rule_deleted";
const ACTION_AUTO_LABEL_APPLIED: &str = "moderation_auto_label_applied";

/// §3.8 label provenance for rule-applied labels.
const SOURCE_AUTO_LABEL_RULE: &str = "auto_label_rule";

/// The 16 `emit_event` moderation action_types (`action_kind_str`) — the
/// validation vocabulary for `operator-action` triggers (Decision 1 scope).
pub(crate) const OPERATOR_ACTION_TYPES: &[&str] = &[
    "TakedownAccount",
    "SuspendAccount",
    "RestoreAccount",
    "DeleteAccount",
    "ApplyLabel",
    "RemoveLabel",
    "TakedownRecord",
    "QuarantineBlob",
    "RestoreBlob",
    "DeleteBlob",
    "ResolveReport",
    "DismissReport",
    "ResolveAppeal",
    "EscalateAppeal",
    "SendEmail",
    "UpdateSubjectStatus",
];

const TRIGGER_TYPES: &[&str] = &["report-count", "operator-action", "account-age-activity"];
const SUBJECT_SCOPES: &[&str] = &["post", "account", "both"];
pub(crate) const REPORT_CATEGORIES: &[&str] =
    &["spam", "violation", "misleading", "sexual", "rude", "other"];

/// Build a content [`Subject`] from a report's columns (record → `Record`,
/// account-only → `Repo`). Shared with Phase D's escalation-audit subject
/// normalization. Returns `None` when the report carries no subject.
pub(crate) fn report_subject(report: &Report) -> Option<Subject> {
    if let Some(uri) = report.subject_uri.as_deref() {
        Some(Subject::Record {
            uri: uri.to_string(),
            cid: report.subject_cid.clone().unwrap_or_default(),
        })
    } else {
        report
            .subject_did
            .as_deref()
            .map(|d| Subject::Repo { did: d.to_string() })
    }
}

// =====================================================================
// Subject normalization (§5.9 — Pipeline B; Phase D reuses)
// =====================================================================

/// Extract the authority DID from an `at://did/...` URI.
fn extract_did_from_uri(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("at://")?;
    let did = rest.split('/').next()?;
    if did.starts_with("did:") {
        Some(did.to_string())
    } else {
        None
    }
}

/// Resolve a moderation `Subject` to the account DID it concerns (§5.9).
/// `Repo`/`Blob` carry the DID directly; `Record` resolves via its at-URI.
/// Returns `None` for unresolvable shapes. Phase D reuses this verbatim.
pub fn normalize_subject_value(subject: &Subject) -> Option<String> {
    match subject {
        Subject::Repo { did } => Some(did.clone()),
        Subject::Blob { did, .. } => Some(did.clone()),
        Subject::Record { uri, .. } => extract_did_from_uri(uri),
    }
}

// NOTE: the audit-entry-taking `normalize_subject(&AuditEntry)` wrapper from
// the design's §5.9 pseudo-code is intentionally NOT shipped here — Pipeline B
// has the `Subject` in hand at the `emit_event` hook and calls
// `normalize_subject_value` directly, so the wrapper would be dead code (the
// lib/bin dead-code tax forbids shipping unused pub fns). Phase D adds the
// wrapper alongside its first consumer; `normalize_subject_value` is the
// shared primitive both phases use.

// =====================================================================
// Rule model + store
// =====================================================================

/// An auto-label rule row (§3.5).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoLabelRule {
    pub id: String,
    pub trigger_type: String,
    pub trigger_params: serde_json::Value,
    pub label_value: String,
    pub subject_scope: String,
    pub enabled: bool,
    pub created_at: String,
    pub created_by_did: String,
    pub last_modified_at: String,
    pub last_modified_by_did: String,
    pub rationale: Option<String>,
    pub deleted_at: Option<String>,
}

fn rule_from_row(row: &sqlx::any::AnyRow) -> PdsResult<AutoLabelRule> {
    let params_str: String = row.try_get("trigger_params")?;
    Ok(AutoLabelRule {
        id: row.try_get("id")?,
        trigger_type: row.try_get("trigger_type")?,
        trigger_params: serde_json::from_str(&params_str)
            .unwrap_or(serde_json::Value::Null),
        label_value: row.try_get("label_value")?,
        subject_scope: row.try_get("subject_scope")?,
        enabled: row.try_get::<i64, _>("enabled").unwrap_or(0) != 0,
        created_at: row.try_get("created_at")?,
        created_by_did: row.try_get("created_by_did")?,
        last_modified_at: row.try_get("last_modified_at")?,
        last_modified_by_did: row.try_get("last_modified_by_did")?,
        rationale: row.try_get("rationale").ok().flatten(),
        deleted_at: row.try_get("deleted_at").ok().flatten(),
    })
}

/// Validate per-trigger-type params (§3.4). Returns an error string for the
/// 400 response.
pub fn validate_trigger_params(
    trigger_type: &str,
    params: &serde_json::Value,
) -> Result<(), String> {
    let int = |k: &str| -> Result<i64, String> {
        params
            .get(k)
            .and_then(|v| v.as_i64())
            .ok_or_else(|| format!("missing or non-integer param '{}'", k))
    };
    let positive = |k: &str| -> Result<i64, String> {
        let v = int(k)?;
        if v > 0 {
            Ok(v)
        } else {
            Err(format!("param '{}' must be > 0", k))
        }
    };
    let window = |k: &str| -> Result<(), String> {
        let v = positive(k)?;
        if v <= 365 {
            Ok(())
        } else {
            Err(format!("param '{}' must be ≤ 365", k))
        }
    };
    match trigger_type {
        "report-count" => {
            let cat = params
                .get("category")
                .and_then(|v| v.as_str())
                .ok_or("missing 'category'")?;
            if !REPORT_CATEGORIES.contains(&cat) {
                return Err(format!("category '{}' not in the report vocabulary", cat));
            }
            positive("threshold")?;
            window("window_days")?;
        }
        "operator-action" => {
            let at = params
                .get("action_type")
                .and_then(|v| v.as_str())
                .ok_or("missing 'action_type'")?;
            if !OPERATOR_ACTION_TYPES.contains(&at) {
                return Err(format!("action_type '{}' not a moderation action", at));
            }
            positive("threshold")?;
            window("window_days")?;
        }
        "account-age-activity" => {
            window("max_age_days")?;
            positive("min_posts")?;
        }
        other => return Err(format!("unknown trigger type '{}'", other)),
    }
    Ok(())
}

/// Load active (non-deleted, enabled) rules of a given trigger type.
async fn load_active_rules_of_type(
    ctx: &AppContext,
    trigger_type: &str,
) -> PdsResult<Vec<AutoLabelRule>> {
    let rows = sqlx::query(
        "SELECT * FROM moderation_auto_label_rule \
         WHERE deleted_at IS NULL AND enabled <> 0 AND trigger_type = $1",
    )
    .bind(trigger_type)
    .fetch_all(&ctx.account_db)
    .await?;
    rows.iter().map(rule_from_row).collect()
}

// =====================================================================
// Rule firing + §3.8 dedup-aware audit
// =====================================================================

/// Apply a rule's label to a target uri and emit `moderation_auto_label_applied`
/// per the §3.8 dedup table. Skips entirely when the SAME rule already applied
/// the SAME active label (dedup row 1: no apply, no audit).
async fn apply_and_audit(
    ctx: &AppContext,
    rule: &AutoLabelRule,
    label_uri: &str,
    label_cid: Option<&str>,
) -> PdsResult<()> {
    let server = server_did(ctx);
    let backend = ctx.config.database.backend;
    let _guard = audit_chain::AppendChainGuard::acquire().await;
    let mut tx = ctx.account_db.begin().await?;
    let app = LabelManager::apply_label_in_tx(
        &mut tx,
        &server,
        label_uri,
        label_cid,
        &rule.label_value,
        SYSTEM_DID,
        None,
        SOURCE_AUTO_LABEL_RULE,
        Some(&rule.id),
    )
    .await?;

    // §3.8 row 1: same rule, same subject, same label already active →
    // no-op, no audit. (apply_label_in_tx already skipped the insert.)
    if !app.issued && app.existing_rule_id.as_deref() == Some(rule.id.as_str()) {
        tx.commit().await?;
        return Ok(());
    }

    let subject = subject_for_uri(label_uri, label_cid);
    let payload = serde_json::json!({
        "applied": app.issued,
        "rule_id": rule.id,
        "existing_rule_id": app.existing_rule_id,
        "existing_source": app.existing_source,
    });
    let rationale = format!("auto-label rule {} applied {}", rule.id, rule.label_value);
    audit_chain::insert_chain_entry(
        &mut tx,
        backend,
        AppendEntryParams {
            actor_did: SYSTEM_DID,
            source: SOURCE_AUTO_LABEL_RULE,
            payload: Some(payload),
            action: ACTION_AUTO_LABEL_APPLIED,
            subject: Some(&subject),
            rationale: &rationale,
            snapshot_id: None,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Build the audit `Subject` for a label target uri (account DID → Repo;
/// at-uri → Record).
fn subject_for_uri(uri: &str, cid: Option<&str>) -> Subject {
    if uri.starts_with("at://") {
        Subject::Record {
            uri: uri.to_string(),
            cid: cid.unwrap_or("").to_string(),
        }
    } else {
        Subject::Repo {
            did: uri.to_string(),
        }
    }
}

/// Fire a rule against a resolved (account_did, optional post_uri), applying
/// the label per the rule's subject_scope (§3.2).
async fn fire_rule(
    ctx: &AppContext,
    rule: &AutoLabelRule,
    account_did: &str,
    post_uri: Option<&str>,
) -> PdsResult<()> {
    let label_account = matches!(rule.subject_scope.as_str(), "account" | "both");
    let label_post = matches!(rule.subject_scope.as_str(), "post" | "both");
    if label_account {
        apply_and_audit(ctx, rule, account_did, None).await?;
    }
    if label_post {
        if let Some(uri) = post_uri {
            apply_and_audit(ctx, rule, uri, None).await?;
        }
    }
    Ok(())
}

// =====================================================================
// Pipeline consumers
// =====================================================================

fn parse_window_cutoff(params: &serde_json::Value, key: &str) -> DateTime<Utc> {
    let days = params.get(key).and_then(|v| v.as_i64()).unwrap_or(0);
    Utc::now() - Duration::days(days)
}

/// Pipeline A (§6.9) — report-count triggers at report intake.
pub async fn evaluate_pipeline_a(ctx: &AppContext, report: &Report) -> PdsResult<()> {
    if !defaults_active(ctx).await {
        return Ok(());
    }
    let account_did = match report.subject_did.as_deref() {
        Some(d) => d.to_string(),
        None => match report.subject_uri.as_deref().and_then(extract_did_from_uri) {
            Some(d) => d,
            None => return Ok(()),
        },
    };
    let rules = load_active_rules_of_type(ctx, "report-count").await?;
    for rule in &rules {
        let p = &rule.trigger_params;
        let category = p.get("category").and_then(|v| v.as_str()).unwrap_or("");
        if category != report.reason_type.as_str() {
            continue;
        }
        let threshold = p.get("threshold").and_then(|v| v.as_i64()).unwrap_or(i64::MAX);
        let cutoff = parse_window_cutoff(p, "window_days").to_rfc3339();
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM report \
             WHERE subject_did = $1 AND reason_type = $2 AND reported_at > $3",
        )
        .bind(&account_did)
        .bind(category)
        .bind(&cutoff)
        .fetch_one(&ctx.account_db)
        .await?;
        if count >= threshold {
            fire_rule(ctx, rule, &account_did, report.subject_uri.as_deref()).await?;
        }
    }
    Ok(())
}

/// Pipeline B (§6.9) — operator-action triggers, hooked at `emit_event`
/// post-commit. `subject` is the moderation action's subject; `action_type`
/// is its `action_kind_str`; `actor_did` is the operator (already known to be
/// non-`did:system` since substrate never calls emit_event).
pub async fn evaluate_pipeline_b(
    ctx: &AppContext,
    subject: &Subject,
    action_type: &str,
    actor_did: &str,
) -> PdsResult<()> {
    if !defaults_active(ctx).await {
        return Ok(());
    }
    if actor_did == SYSTEM_DID {
        return Ok(());
    }
    let account_did = match normalize_subject_value(subject) {
        Some(d) => d,
        None => return Ok(()),
    };
    let rules = load_active_rules_of_type(ctx, "operator-action").await?;
    for rule in &rules {
        let p = &rule.trigger_params;
        let at = p.get("action_type").and_then(|v| v.as_str()).unwrap_or("");
        if at != action_type {
            continue;
        }
        let threshold = p.get("threshold").and_then(|v| v.as_i64()).unwrap_or(i64::MAX);
        let cutoff = parse_window_cutoff(p, "window_days").to_rfc3339();
        // Account-level operator actions (subject_did), excluding substrate
        // entries. (Record-subject actions aggregate by account via the URI
        // are out of the indexed path — documented Phase C scope.)
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_chain_entry \
             WHERE subject_did = $1 AND action = $2 AND created_at > $3 \
               AND actor_did <> $4",
        )
        .bind(&account_did)
        .bind(action_type)
        .bind(&cutoff)
        .bind(SYSTEM_DID)
        .fetch_one(&ctx.account_db)
        .await?;
        if count >= threshold {
            fire_rule(ctx, rule, &account_did, None).await?;
        }
    }
    Ok(())
}

/// Pipeline C (§6.9) — account-age-activity triggers at post creation. Counts
/// the author's posts against their own per-actor store; age via
/// `actor.created_at`.
pub async fn evaluate_pipeline_c(
    ctx: &AppContext,
    author_did: &str,
    post_uri: &str,
) -> PdsResult<()> {
    if !defaults_active(ctx).await {
        return Ok(());
    }
    let created_at_s: Option<String> =
        sqlx::query_scalar("SELECT created_at FROM actor WHERE did = $1")
            .bind(author_did)
            .fetch_optional(&ctx.account_db)
            .await?;
    let age_days = match created_at_s.and_then(|s| DateTime::parse_from_rfc3339(&s).ok()) {
        Some(created) => (Utc::now() - created.with_timezone(&Utc)).num_days(),
        None => return Ok(()),
    };
    let rules = load_active_rules_of_type(ctx, "account-age-activity").await?;
    if rules.is_empty() {
        return Ok(());
    }
    let post_count = ctx
        .actor_store
        .count_records(author_did, "app.bsky.feed.post")
        .await
        .unwrap_or(0);
    for rule in &rules {
        let p = &rule.trigger_params;
        let max_age = p.get("max_age_days").and_then(|v| v.as_i64()).unwrap_or(0);
        let min_posts = p.get("min_posts").and_then(|v| v.as_i64()).unwrap_or(i64::MAX);
        if age_days >= max_age {
            continue; // account too old
        }
        if post_count >= min_posts {
            fire_rule(ctx, rule, author_did, Some(post_uri)).await?;
        }
    }
    Ok(())
}

// =====================================================================
// Rule-lifecycle audit
// =====================================================================

/// Emit a rule-lifecycle audit (created/edited/deleted). Operator-DID actor;
/// source `manual` (AL's non-substrate operator convention — the schema's
/// `source` is NOT NULL); rule id in the payload (no content subject).
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
            source: "manual",
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

// =====================================================================
// CRUD store operations (handlers in api::admin wire auth + these)
// =====================================================================

/// Create a rule (§3.5). Validates params, enforces the 100-active cap
/// atomically, emits `moderation_auto_label_rule_created`. Returns the rule.
#[allow(clippy::too_many_arguments)]
pub async fn create_rule(
    ctx: &AppContext,
    operator_did: &str,
    trigger_type: &str,
    trigger_params: &serde_json::Value,
    label_value: &str,
    subject_scope: &str,
    enabled: bool,
    rationale: Option<&str>,
) -> Result<AutoLabelRule, (u16, String)> {
    if !TRIGGER_TYPES.contains(&trigger_type) {
        return Err((400, format!("unknown trigger type '{}'", trigger_type)));
    }
    if !SUBJECT_SCOPES.contains(&subject_scope) {
        return Err((400, format!("unknown subject scope '{}'", subject_scope)));
    }
    validate_trigger_params(trigger_type, trigger_params).map_err(|e| (400, e))?;

    let mut tx = ctx.account_db.begin().await.map_err(internal)?;
    // Count active + insert in one tx (the SQLite/Postgres write-lock + the
    // count check bound the cap; the design's serializable retry is a no-op
    // under AL's existing single-flight write path).
    let active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM moderation_auto_label_rule WHERE deleted_at IS NULL",
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(internal)?;
    if active >= MAX_ACTIVE_RULES {
        return Err((400, format!("active auto-label rule limit ({}) reached", MAX_ACTIVE_RULES)));
    }
    let id = Uuid::new_v4().simple().to_string();
    let now = Utc::now().to_rfc3339();
    let params_str = serde_json::to_string(trigger_params).map_err(|e| (500, e.to_string()))?;
    sqlx::query(
        "INSERT INTO moderation_auto_label_rule \
         (id, trigger_type, trigger_params, label_value, subject_scope, enabled, \
          created_at, created_by_did, last_modified_at, last_modified_by_did, rationale, deleted_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, NULL)",
    )
    .bind(&id)
    .bind(trigger_type)
    .bind(&params_str)
    .bind(label_value)
    .bind(subject_scope)
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

    emit_rule_lifecycle(ctx, ACTION_RULE_CREATED, &id, operator_did, rationale.unwrap_or("auto-label rule created"))
        .await
        .map_err(internal)?;

    Ok(AutoLabelRule {
        id,
        trigger_type: trigger_type.to_string(),
        trigger_params: trigger_params.clone(),
        label_value: label_value.to_string(),
        subject_scope: subject_scope.to_string(),
        enabled,
        created_at: now.clone(),
        created_by_did: operator_did.to_string(),
        last_modified_at: now,
        last_modified_by_did: operator_did.to_string(),
        rationale: rationale.map(String::from),
        deleted_at: None,
    })
}

/// Edit a rule (§3.5). Re-validates params; emits `_edited`.
#[allow(clippy::too_many_arguments)]
pub async fn edit_rule(
    ctx: &AppContext,
    operator_did: &str,
    id: &str,
    trigger_type: &str,
    trigger_params: &serde_json::Value,
    label_value: &str,
    subject_scope: &str,
    enabled: bool,
    rationale: Option<&str>,
) -> Result<(), (u16, String)> {
    if !TRIGGER_TYPES.contains(&trigger_type) {
        return Err((400, format!("unknown trigger type '{}'", trigger_type)));
    }
    if !SUBJECT_SCOPES.contains(&subject_scope) {
        return Err((400, format!("unknown subject scope '{}'", subject_scope)));
    }
    validate_trigger_params(trigger_type, trigger_params).map_err(|e| (400, e))?;
    let now = Utc::now().to_rfc3339();
    let params_str = serde_json::to_string(trigger_params).map_err(|e| (500, e.to_string()))?;
    let res = sqlx::query(
        "UPDATE moderation_auto_label_rule SET trigger_type = $1, trigger_params = $2, \
         label_value = $3, subject_scope = $4, enabled = $5, last_modified_at = $6, \
         last_modified_by_did = $7, rationale = $8 WHERE id = $9 AND deleted_at IS NULL",
    )
    .bind(trigger_type)
    .bind(&params_str)
    .bind(label_value)
    .bind(subject_scope)
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
    emit_rule_lifecycle(ctx, ACTION_RULE_EDITED, id, operator_did, rationale.unwrap_or("auto-label rule edited"))
        .await
        .map_err(internal)?;
    Ok(())
}

/// Soft-delete a rule (§3.5). Sets `deleted_at`; emits `_deleted`.
pub async fn delete_rule(
    ctx: &AppContext,
    operator_did: &str,
    id: &str,
) -> Result<(), (u16, String)> {
    let now = Utc::now().to_rfc3339();
    let res = sqlx::query(
        "UPDATE moderation_auto_label_rule SET deleted_at = $1 WHERE id = $2 AND deleted_at IS NULL",
    )
    .bind(&now)
    .bind(id)
    .execute(&ctx.account_db)
    .await
    .map_err(internal)?;
    if res.rows_affected() == 0 {
        return Err((404, format!("rule {} not found", id)));
    }
    emit_rule_lifecycle(ctx, ACTION_RULE_DELETED, id, operator_did, "auto-label rule deleted")
        .await
        .map_err(internal)?;
    Ok(())
}

/// List rules (§3.5). `include_deleted` surfaces soft-deleted rows.
pub async fn list_rules(
    ctx: &AppContext,
    include_deleted: bool,
) -> Result<Vec<AutoLabelRule>, (u16, String)> {
    let sql = if include_deleted {
        "SELECT * FROM moderation_auto_label_rule ORDER BY created_at DESC"
    } else {
        "SELECT * FROM moderation_auto_label_rule WHERE deleted_at IS NULL ORDER BY created_at DESC"
    };
    let rows = sqlx::query(sql)
        .fetch_all(&ctx.account_db)
        .await
        .map_err(internal)?;
    rows.iter()
        .map(|r| rule_from_row(r).map_err(internal))
        .collect()
}

fn internal<E: std::fmt::Display>(e: E) -> (u16, String) {
    (500, e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin::reports::ReportReason;
    use crate::config::*;
    use tempfile::tempdir;

    // --- pure validation + normalization --------------------------------

    #[test]
    fn validate_report_count() {
        assert!(validate_trigger_params(
            "report-count",
            &serde_json::json!({"category": "spam", "threshold": 3, "window_days": 7})
        )
        .is_ok());
        // bad category
        assert!(validate_trigger_params(
            "report-count",
            &serde_json::json!({"category": "harassment", "threshold": 3, "window_days": 7})
        )
        .is_err());
        // threshold <= 0
        assert!(validate_trigger_params(
            "report-count",
            &serde_json::json!({"category": "spam", "threshold": 0, "window_days": 7})
        )
        .is_err());
        // window > 365
        assert!(validate_trigger_params(
            "report-count",
            &serde_json::json!({"category": "spam", "threshold": 3, "window_days": 400})
        )
        .is_err());
    }

    #[test]
    fn validate_operator_action_and_age() {
        assert!(validate_trigger_params(
            "operator-action",
            &serde_json::json!({"action_type": "TakedownAccount", "threshold": 2, "window_days": 30})
        )
        .is_ok());
        // action_type not a moderation action
        assert!(validate_trigger_params(
            "operator-action",
            &serde_json::json!({"action_type": "role.grant", "threshold": 2, "window_days": 30})
        )
        .is_err());
        assert!(validate_trigger_params(
            "account-age-activity",
            &serde_json::json!({"max_age_days": 7, "min_posts": 50})
        )
        .is_ok());
        assert!(validate_trigger_params(
            "account-age-activity",
            &serde_json::json!({"max_age_days": 7, "min_posts": 0})
        )
        .is_err());
    }

    #[test]
    fn normalize_subject_value_maps_variants() {
        assert_eq!(
            normalize_subject_value(&Subject::Repo { did: "did:plc:a".into() }).as_deref(),
            Some("did:plc:a")
        );
        assert_eq!(
            normalize_subject_value(&Subject::Record {
                uri: "at://did:plc:b/app.bsky.feed.post/x".into(),
                cid: "c".into()
            })
            .as_deref(),
            Some("did:plc:b")
        );
        assert_eq!(
            normalize_subject_value(&Subject::Record {
                uri: "https://not-an-at-uri".into(),
                cid: "c".into()
            }),
            None
        );
    }

    // --- integration over a real AppContext -----------------------------

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
                jwt_secret: "test-secret-key-aurora-auto-label-rule-x".to_string(),
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
            },
            identity: IdentityConfig {
                did_plc_url: "https://plc.directory".to_string(),
                service_handle_domains: vec![".localhost".to_string()],
                did_cache_stale_ttl: 3600,
                did_cache_max_ttl: 86400,
                recovery_did_key: None,
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
                buckets_retention_days: 7,
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
        AppContext::new(
            config,
            std::sync::Arc::new(crate::api::registry::RouteRegistry::default()),
        )
        .await
        .unwrap()
    }

    async fn mk_rule(ctx: &AppContext, tt: &str, params: serde_json::Value, label: &str, scope: &str) -> AutoLabelRule {
        create_rule(ctx, "did:plc:super", tt, &params, label, scope, true, Some("t"))
            .await
            .unwrap()
    }

    async fn submit_report(ctx: &AppContext, subject_did: &str, reason: ReportReason) {
        ctx.report_manager
            .submit_report(Some(subject_did), None, None, reason, Some("r"), "did:plc:reporter")
            .await
            .unwrap();
    }

    async fn audit_actions(ctx: &AppContext) -> Vec<(String, String)> {
        let rows = sqlx::query("SELECT action, source FROM audit_chain_entry ORDER BY sequence ASC")
            .fetch_all(&ctx.account_db)
            .await
            .unwrap();
        rows.iter()
            .map(|r| {
                (
                    r.try_get::<String, _>("action").unwrap(),
                    r.try_get::<String, _>("source").unwrap(),
                )
            })
            .collect()
    }

    #[tokio::test]
    async fn crud_create_list_edit_delete_with_lifecycle_audits() {
        let ctx = create_test_context().await;
        let r = mk_rule(
            &ctx,
            "report-count",
            serde_json::json!({"category": "spam", "threshold": 3, "window_days": 7}),
            "tools.aurora.ops.moderation.spam-repeat",
            "account",
        )
        .await;
        // created audit emitted (operator-DID, source=manual).
        let acts = audit_actions(&ctx).await;
        assert!(acts.iter().any(|(a, s)| a == ACTION_RULE_CREATED && s == "manual"));
        // list shows it; edit; delete; list excludes by default.
        assert_eq!(list_rules(&ctx, false).await.unwrap().len(), 1);
        edit_rule(
            &ctx,
            "did:plc:super",
            &r.id,
            "report-count",
            &serde_json::json!({"category": "spam", "threshold": 5, "window_days": 14}),
            "tools.aurora.ops.moderation.spam-repeat",
            "account",
            false,
            Some("tune"),
        )
        .await
        .unwrap();
        delete_rule(&ctx, "did:plc:super", &r.id).await.unwrap();
        assert_eq!(list_rules(&ctx, false).await.unwrap().len(), 0);
        assert_eq!(list_rules(&ctx, true).await.unwrap().len(), 1);
        let acts = audit_actions(&ctx).await;
        assert!(acts.iter().any(|(a, _)| a == ACTION_RULE_EDITED));
        assert!(acts.iter().any(|(a, _)| a == ACTION_RULE_DELETED));
    }

    #[tokio::test]
    async fn rule_cap_enforced() {
        let ctx = create_test_context().await;
        // Seed 100 active rules directly, then the 101st via create_rule fails.
        for i in 0..MAX_ACTIVE_RULES {
            sqlx::query(
                "INSERT INTO moderation_auto_label_rule (id, trigger_type, trigger_params, \
                 label_value, subject_scope, enabled, created_at, created_by_did, \
                 last_modified_at, last_modified_by_did) \
                 VALUES ($1, 'report-count', '{}', 'l', 'account', 1, 'now', 'op', 'now', 'op')",
            )
            .bind(format!("seed-{}", i))
            .execute(&ctx.account_db)
            .await
            .unwrap();
        }
        let err = create_rule(
            &ctx,
            "did:plc:super",
            "report-count",
            &serde_json::json!({"category": "spam", "threshold": 1, "window_days": 1}),
            "l",
            "account",
            true,
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, 400);
        assert!(err.1.contains("limit"));
    }

    #[tokio::test]
    async fn pipeline_a_fires_at_threshold() {
        let ctx = create_test_context().await;
        mk_rule(
            &ctx,
            "report-count",
            serde_json::json!({"category": "spam", "threshold": 2, "window_days": 30}),
            "tools.aurora.ops.moderation.spam-repeat",
            "account",
        )
        .await;
        // Two spam reports for the same subject; the 2nd intake meets threshold.
        submit_report(&ctx, "did:plc:victim", ReportReason::Spam).await;
        let r2 = ctx
            .report_manager
            .submit_report(Some("did:plc:victim"), None, None, ReportReason::Spam, Some("r"), "did:plc:reporter")
            .await
            .unwrap();
        evaluate_pipeline_a(&ctx, &r2).await.unwrap();
        let acts = audit_actions(&ctx).await;
        assert!(
            acts.iter().any(|(a, s)| a == ACTION_AUTO_LABEL_APPLIED && s == SOURCE_AUTO_LABEL_RULE),
            "rule fired an auto-label"
        );
        // Label is active with rule provenance.
        let src: Option<String> = sqlx::query_scalar(
            "SELECT source FROM label WHERE uri='did:plc:victim' AND val='tools.aurora.ops.moderation.spam-repeat'",
        )
        .fetch_optional(&ctx.account_db)
        .await
        .unwrap()
        .flatten();
        assert_eq!(src.as_deref(), Some("auto_label_rule"));
    }

    #[tokio::test]
    async fn pipeline_a_below_threshold_does_not_fire() {
        let ctx = create_test_context().await;
        mk_rule(
            &ctx,
            "report-count",
            serde_json::json!({"category": "spam", "threshold": 5, "window_days": 30}),
            "l",
            "account",
        )
        .await;
        let r = ctx
            .report_manager
            .submit_report(Some("did:plc:victim"), None, None, ReportReason::Spam, Some("r"), "did:plc:reporter")
            .await
            .unwrap();
        evaluate_pipeline_a(&ctx, &r).await.unwrap();
        let acts = audit_actions(&ctx).await;
        assert!(!acts.iter().any(|(a, _)| a == ACTION_AUTO_LABEL_APPLIED));
    }

    #[tokio::test]
    async fn pipeline_b_fires_on_operator_action_skips_system() {
        let ctx = create_test_context().await;
        mk_rule(
            &ctx,
            "operator-action",
            serde_json::json!({"action_type": "TakedownAccount", "threshold": 1, "window_days": 30}),
            "tools.aurora.ops.moderation.repeat-offender",
            "account",
        )
        .await;
        // Seed one prior operator TakedownAccount audit on the subject.
        let subject = Subject::Repo { did: "did:plc:victim".into() };
        audit_chain::insert_chain_entry_pool(
            &ctx.account_db,
            ctx.config.database.backend,
            AppendEntryParams {
                actor_did: "did:plc:moderator",
                source: "manual",
                payload: None,
                action: "TakedownAccount",
                subject: Some(&subject),
                rationale: "prior",
                snapshot_id: None,
                event_id: None,
                cascade_subjects: &[],
                cascade_snapshot_ids: &[],
            },
        )
        .await
        .unwrap();
        // did:system actor → skipped (no fire).
        evaluate_pipeline_b(&ctx, &subject, "TakedownAccount", SYSTEM_DID).await.unwrap();
        assert!(!audit_actions(&ctx).await.iter().any(|(a, _)| a == ACTION_AUTO_LABEL_APPLIED));
        // operator actor → fires (count=1 ≥ threshold).
        evaluate_pipeline_b(&ctx, &subject, "TakedownAccount", "did:plc:moderator").await.unwrap();
        assert!(audit_actions(&ctx).await.iter().any(|(a, s)| a == ACTION_AUTO_LABEL_APPLIED && s == SOURCE_AUTO_LABEL_RULE));
    }

    #[tokio::test]
    async fn dedup_same_rule_skips_audit() {
        let ctx = create_test_context().await;
        let rule = mk_rule(
            &ctx,
            "report-count",
            serde_json::json!({"category": "spam", "threshold": 1, "window_days": 30}),
            "tools.aurora.ops.moderation.x",
            "account",
        )
        .await;
        // First fire → applied=true + audit.
        fire_rule(&ctx, &rule, "did:plc:victim", None).await.unwrap();
        let before = audit_actions(&ctx).await.iter().filter(|(a, _)| a == ACTION_AUTO_LABEL_APPLIED).count();
        assert_eq!(before, 1);
        // Same rule, same subject, same label already active → no apply, no audit (§3.8 row 1).
        fire_rule(&ctx, &rule, "did:plc:victim", None).await.unwrap();
        let after = audit_actions(&ctx).await.iter().filter(|(a, _)| a == ACTION_AUTO_LABEL_APPLIED).count();
        assert_eq!(after, 1, "same-rule re-fire emits no new audit");
    }

    #[tokio::test]
    async fn dedup_different_source_emits_applied_false() {
        let ctx = create_test_context().await;
        // Pre-existing manual label on the subject for the rule's label value.
        let mut tx = ctx.account_db.begin().await.unwrap();
        LabelManager::apply_label_in_tx(
            &mut tx,
            "did:web:localhost",
            "did:plc:victim",
            None,
            "tools.aurora.ops.moderation.x",
            "did:plc:mod",
            None,
            "manual",
            None,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();

        let rule = mk_rule(
            &ctx,
            "report-count",
            serde_json::json!({"category": "spam", "threshold": 1, "window_days": 30}),
            "tools.aurora.ops.moderation.x",
            "account",
        )
        .await;
        fire_rule(&ctx, &rule, "did:plc:victim", None).await.unwrap();
        // The auto-label audit emits applied=false, existing_source=manual.
        let payload: Option<String> = sqlx::query_scalar(
            "SELECT payload FROM audit_chain_entry WHERE action = $1 ORDER BY sequence DESC LIMIT 1",
        )
        .bind(ACTION_AUTO_LABEL_APPLIED)
        .fetch_optional(&ctx.account_db)
        .await
        .unwrap()
        .flatten();
        let p = payload.unwrap();
        assert!(p.contains("\"applied\":false"));
        assert!(p.contains("\"existing_source\":\"manual\""));
    }

    #[tokio::test]
    async fn tier_gate_blocks_pipelines() {
        let ctx = create_test_context().await;
        sqlx::query("INSERT INTO runtime_settings (key, value, last_modified, last_modified_by) VALUES ('moderation-mode', '\"reduced\"', 'now', 'op')")
            .execute(&ctx.account_db).await.unwrap();
        mk_rule(
            &ctx,
            "report-count",
            serde_json::json!({"category": "spam", "threshold": 1, "window_days": 30}),
            "l",
            "account",
        )
        .await;
        let r = ctx
            .report_manager
            .submit_report(Some("did:plc:victim"), None, None, ReportReason::Spam, Some("r"), "did:plc:reporter")
            .await
            .unwrap();
        evaluate_pipeline_a(&ctx, &r).await.unwrap();
        assert!(!audit_actions(&ctx).await.iter().any(|(a, _)| a == ACTION_AUTO_LABEL_APPLIED));
    }
}
