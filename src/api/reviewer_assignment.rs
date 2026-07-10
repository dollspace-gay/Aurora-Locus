//! §5.5.4 Phase B — reviewer assignment (§4).
//!
//! At report intake (Pipeline A, §6.9 step 2), substrate assigns the new
//! queue item to an operator per the configured mode — `manual` (no-op),
//! `round-robin`, `load-balanced`, or `category-routed`. Full-tier gated
//! (§6.3); reuses Phase A's `defaults_active`. Assignment writes
//! `report.assigned_operator_did` + `assignment_source = 'auto'`.
//!
//! Cursor advance uses the substrate-general value-CAS primitive
//! (`cas_runtime_setting`) with the design's bounded-retry-max-3 then
//! best-effort-proceed wrapper (§4.7). The rotation/category/escalation
//! cursors are migration-seeded so CAS is a pure conditional UPDATE.
//!
//! Operator-removal handling runs the PRIMARY path (§4.7): the prune +
//! per-item reset + cursor reset + audit all land inside the existing
//! role-revocation transaction (`revoke_role` already hosts a cross-
//! subsystem `account_db` tx with the chain guard held across commit —
//! the #122 pattern). No cleanup-job table / state machine (the fallback
//! path is not needed here).
//!
//! Audit `source` per the design's §6.1 enum (the kickoff's `role_revocation`
//! is not in that enum): prune/reset entries use `operator_removal`; the
//! rollback entry uses `system_diagnostic`.

use crate::admin::audit_chain::{self, AppendEntryParams};
use crate::admin::defs::Subject;
use crate::admin::reports::Report;
use crate::api::aurora_admin::{
    cas_runtime_setting, read_runtime_row_value, resolve_runtime_setting,
    MODERATION_ESCALATION_SUPERADMIN_CURSOR_KEY, MODERATION_REVIEWER_CATEGORY_CURSORS_KEY,
    MODERATION_REVIEWER_CATEGORY_MAP_KEY, MODERATION_REVIEWER_MODE_KEY,
    MODERATION_REVIEWER_MODE_VERSION_KEY, MODERATION_REVIEWER_ROTATION_CURSOR_KEY,
};
use crate::api::moderation_defaults::{defaults_active, SYSTEM_DID};
use crate::config::DatabaseBackend;
use crate::error::PdsResult;
use crate::AppContext;
use chrono::Utc;
use sqlx::Row as _;

// Audit action names (§6.1 registry — Phase B Either-path subset).
const ACTION_ROUTING_PRUNED: &str = "moderation_routing_pruned";
const ACTION_ROUTING_PRUNED_BULK: &str = "moderation_routing_pruned_bulk";
const ACTION_REVOCATION_ROLLBACK: &str = "operator_revocation_rollback";

// Audit source discriminators (§6.1 enum — NOT the kickoff's role_revocation).
const SOURCE_OPERATOR_REMOVAL: &str = "operator_removal";
const SOURCE_SYSTEM_DIAGNOSTIC: &str = "system_diagnostic";

/// Bounded CAS retry budget (§4.7).
const CAS_MAX_RETRIES: usize = 3;

/// Status set treated as terminal for the §4.6 active-items count. Per
/// Phase B recon (Nova-confirmed): Aurora-Locus's `ReportStatus` is the
/// authoritative 4-state enum, so `resolved` is the only terminal state
/// (the design's `dismissed` does not exist here). Active = NOT resolved.
const TERMINAL_STATUS: &str = "resolved";

/// Enumerate the operator pool: all active `admin_roles` DIDs, deduplicated
/// and lexicographically sorted by DID (§4.7). Round-robin and load-balanced
/// index into this stable ordering.
async fn enumerate_operators(ctx: &AppContext) -> PdsResult<Vec<String>> {
    let roles = ctx.admin_role_manager.list_active_roles().await?;
    let mut dids: Vec<String> = roles.into_iter().map(|r| r.did).collect();
    dids.sort();
    dids.dedup();
    Ok(dids)
}

fn parse_u64_value(raw: &str) -> u64 {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
}

/// CAS-advance a scalar integer cursor with wraparound over `modulo`,
/// returning the assignee index (current % modulo) and storing
/// (current + 1) % modulo. Bounded retry, then best-effort proceed with the
/// most recent read (§4.7).
async fn cas_advance_cursor(ctx: &AppContext, key: &str, modulo: usize) -> usize {
    if modulo == 0 {
        return 0;
    }
    let m = modulo as u64;
    for _ in 0..CAS_MAX_RETRIES {
        let raw = read_runtime_row_value(ctx, key)
            .await
            .unwrap_or_else(|| "0".to_string());
        let current = parse_u64_value(&raw);
        let assignee = (current % m) as usize;
        let next = ((current + 1) % m).to_string();
        if cas_runtime_setting(ctx, key, &raw, &next, SYSTEM_DID)
            .await
            .unwrap_or(false)
        {
            return assignee;
        }
    }
    // Best-effort proceed: re-read, use the current value without advancing.
    let raw = read_runtime_row_value(ctx, key)
        .await
        .unwrap_or_else(|| "0".to_string());
    (parse_u64_value(&raw) % m) as usize
}

/// CAS-advance the per-category cursor inside the
/// `reviewer-category-rotation-cursors` JSON object. Same semantics as
/// [`cas_advance_cursor`] but read-modify-writes one category's slot in the
/// object (CAS witnesses the whole serialized object string).
async fn cas_advance_category_cursor(ctx: &AppContext, category: &str, modulo: usize) -> usize {
    if modulo == 0 {
        return 0;
    }
    let m = modulo as u64;
    let key = MODERATION_REVIEWER_CATEGORY_CURSORS_KEY;
    for _ in 0..CAS_MAX_RETRIES {
        let raw = read_runtime_row_value(ctx, key)
            .await
            .unwrap_or_else(|| "{}".to_string());
        let mut obj: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(&raw).unwrap_or_default();
        let current = obj.get(category).and_then(|v| v.as_u64()).unwrap_or(0);
        let assignee = (current % m) as usize;
        let next = (current + 1) % m;
        obj.insert(category.to_string(), serde_json::json!(next));
        let new_str = match serde_json::to_string(&serde_json::Value::Object(obj)) {
            Ok(s) => s,
            Err(_) => return assignee,
        };
        if cas_runtime_setting(ctx, key, &raw, &new_str, SYSTEM_DID)
            .await
            .unwrap_or(false)
        {
            return assignee;
        }
    }
    let raw = read_runtime_row_value(ctx, key)
        .await
        .unwrap_or_else(|| "{}".to_string());
    let obj: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&raw).unwrap_or_default();
    (obj.get(category).and_then(|v| v.as_u64()).unwrap_or(0) % m) as usize
}

/// Active-item count for an operator (§4.6): assigned to them and not in a
/// terminal status. Drives load-balanced selection.
async fn count_active_assigned(ctx: &AppContext, did: &str) -> PdsResult<i64> {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM report WHERE assigned_operator_did = $1 AND status <> $2",
    )
    .bind(did)
    .bind(TERMINAL_STATUS)
    .fetch_one(&ctx.account_db)
    .await?;
    Ok(n)
}

/// Pool of DIDs configured for a category in the routing map (§4.3).
fn category_pool(map: &serde_json::Value, category: &str) -> Vec<String> {
    map.get(category)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|d| d.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Assign a freshly-submitted report to a reviewer per the configured mode
/// (Pipeline A §4). No-op outside `full` tier or in `manual` mode, or when
/// the relevant operator pool is empty (columns stay NULL). Best-effort:
/// surfaced to the caller for logging, never rolls back the report.
pub async fn assign_reviewer_on_intake(ctx: &AppContext, report: &Report) -> PdsResult<()> {
    if !defaults_active(ctx).await {
        return Ok(());
    }
    let mode = resolve_runtime_setting(ctx, MODERATION_REVIEWER_MODE_KEY).await;
    let mode = mode.as_str().unwrap_or("manual");

    let assignee: Option<String> = match mode {
        "round-robin" => {
            let pool = enumerate_operators(ctx).await?;
            if pool.is_empty() {
                None
            } else {
                let idx = cas_advance_cursor(ctx, MODERATION_REVIEWER_ROTATION_CURSOR_KEY, pool.len())
                    .await;
                pool.get(idx).cloned()
            }
        }
        "load-balanced" => {
            let pool = enumerate_operators(ctx).await?;
            // pool is DID-sorted; replacing only on strictly-less count means
            // ties resolve to the lexicographically smallest DID.
            let mut best: Option<(String, i64)> = None;
            for did in &pool {
                let c = count_active_assigned(ctx, did).await?;
                match &best {
                    Some((_, bc)) if *bc <= c => {}
                    _ => best = Some((did.clone(), c)),
                }
            }
            best.map(|(d, _)| d)
        }
        "category-routed" => {
            let map = resolve_runtime_setting(ctx, MODERATION_REVIEWER_CATEGORY_MAP_KEY).await;
            let category = report.reason_type.as_str();
            let pool = category_pool(&map, category);
            if pool.is_empty() {
                None
            } else {
                let idx = cas_advance_category_cursor(ctx, category, pool.len()).await;
                pool.get(idx).cloned()
            }
        }
        // "manual" or any unexpected value → leave unassigned.
        _ => None,
    };

    if let Some(did) = assignee {
        sqlx::query(
            "UPDATE report SET assigned_operator_did = $1, assignment_source = 'auto' WHERE id = $2",
        )
        .bind(&did)
        .bind(report.id)
        .execute(&ctx.account_db)
        .await?;
    }
    Ok(())
}

/// Reset all DID-pool cursors to their seed values (§4.5 cursor invalidation
/// on operator-set change). Runs inside the operator-tier mutation
/// transaction (grant/revoke), single-step — no CAS, no contention with
/// concurrent assignment because the mutation tx serializes it. Uniform
/// across all three cursor keys (the escalation cursor pre-registered in
/// Phase B per §4.5/§2.6 forward-compat).
pub async fn reset_assignment_cursors_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    actor: &str,
) -> PdsResult<()> {
    let now = Utc::now().to_rfc3339();
    for (key, seed) in [
        (MODERATION_REVIEWER_ROTATION_CURSOR_KEY, "0"),
        (MODERATION_ESCALATION_SUPERADMIN_CURSOR_KEY, "0"),
        (MODERATION_REVIEWER_CATEGORY_CURSORS_KEY, "{}"),
    ] {
        sqlx::query(
            "UPDATE runtime_settings SET value = $1, last_modified = $2, last_modified_by = $3 \
             WHERE key = $4",
        )
        .bind(seed)
        .bind(&now)
        .bind(actor)
        .bind(key)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// §4.7 Either-path operator-removal cleanup, run INSIDE the role-revocation
/// transaction (primary path): prune the removed DID from the routing map,
/// reset its in-flight queue assignments, reset all cursors, and emit the
/// `operator_removal`-sourced audit entries — all atomic with the revocation.
/// Caller holds the chain guard (the revoke handler already does).
pub async fn handle_operator_removal_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    backend: DatabaseBackend,
    removed_did: &str,
    actor: &str,
) -> PdsResult<()> {
    let subject = Subject::Repo {
        did: removed_did.to_string(),
    };

    // 1. Prune the removed DID from the routing-category map (§4.7 step 1).
    let raw_map: Option<String> =
        sqlx::query("SELECT value FROM runtime_settings WHERE key = $1")
            .bind(MODERATION_REVIEWER_CATEGORY_MAP_KEY)
            .fetch_optional(&mut **tx)
            .await?
            .and_then(|r| r.try_get::<String, _>("value").ok());

    let mut pruned: Vec<(String, usize)> = Vec::new(); // (category, remaining_pool_size)
    if let Some(raw) = raw_map {
        if let Ok(mut obj) =
            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&raw)
        {
            let mut changed = false;
            for (cat, v) in obj.iter_mut() {
                if let Some(arr) = v.as_array_mut() {
                    let before = arr.len();
                    arr.retain(|d| d.as_str() != Some(removed_did));
                    if arr.len() != before {
                        changed = true;
                        pruned.push((cat.clone(), arr.len()));
                    }
                }
            }
            if changed {
                let new_str = serde_json::to_string(&serde_json::Value::Object(obj))
                    .unwrap_or_else(|_| "{}".to_string());
                sqlx::query(
                    "UPDATE runtime_settings SET value = $1, last_modified = $2, \
                     last_modified_by = $3 WHERE key = $4",
                )
                .bind(&new_str)
                .bind(Utc::now().to_rfc3339())
                .bind(actor)
                .bind(MODERATION_REVIEWER_CATEGORY_MAP_KEY)
                .execute(&mut **tx)
                .await?;
            }
        }
    }

    // 2. Reset this operator's in-flight (non-terminal) queue assignments
    //    (§4.7 step 2).
    let reset = sqlx::query(
        "UPDATE report SET assigned_operator_did = NULL, assignment_source = NULL \
         WHERE assigned_operator_did = $1 AND status <> $2",
    )
    .bind(removed_did)
    .bind(TERMINAL_STATUS)
    .execute(&mut **tx)
    .await?;
    let affected_queue = reset.rows_affected();

    // 3. Reset all DID-pool cursors (§4.7 step 4 / §4.5).
    reset_assignment_cursors_in_tx(tx, actor).await?;

    // 4a. One moderation_routing_pruned per affected category (§4.7 audit).
    for (category, remaining) in &pruned {
        let payload = serde_json::json!({
            "removed_did": removed_did,
            "category": category,
            "remaining_pool_size": remaining,
        });
        let rationale = format!("pruned {} from routing category {}", removed_did, category);
        audit_chain::insert_chain_entry(
            tx,
            backend,
            AppendEntryParams {
                actor_did: SYSTEM_DID,
                source: SOURCE_OPERATOR_REMOVAL,
                payload: Some(payload),
                action: ACTION_ROUTING_PRUNED,
                subject: Some(&subject),
                rationale: &rationale,
                snapshot_id: None,
                event_id: None,
                cascade_subjects: &[],
                cascade_snapshot_ids: &[],
            },
        )
        .await?;
    }

    // 4b. One moderation_routing_pruned_bulk for the removal (§4.7 audit).
    let bulk_payload = serde_json::json!({
        "removed_did": removed_did,
        "affected_queue_count": affected_queue,
        "affected_category_count": pruned.len(),
    });
    let bulk_rationale = format!("reviewer-routing cleanup for removed operator {}", removed_did);
    audit_chain::insert_chain_entry(
        tx,
        backend,
        AppendEntryParams {
            actor_did: SYSTEM_DID,
            source: SOURCE_OPERATOR_REMOVAL,
            payload: Some(bulk_payload),
            action: ACTION_ROUTING_PRUNED_BULK,
            subject: Some(&subject),
            rationale: &bulk_rationale,
            snapshot_id: None,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await?;

    Ok(())
}

/// Emit `operator_revocation_rollback` (§4.7 / §6.1) when the role-revocation
/// transaction rolled back. Runs in its OWN pool transaction — the diagnostic
/// must survive the very rollback it records (it cannot ride the rolled-back
/// tx). Best-effort; source = `system_diagnostic` per §6.1. Returns the audit
/// entry id on success.
pub async fn emit_revocation_rollback(
    ctx: &AppContext,
    removed_did: &str,
    rollback_reason: &str,
) -> PdsResult<i64> {
    let subject = Subject::Repo {
        did: removed_did.to_string(),
    };
    let payload = serde_json::json!({ "rollback_reason": rollback_reason });
    let rationale = format!("role-revocation rolled back for {}", removed_did);
    audit_chain::insert_chain_entry_pool(
        &ctx.account_db,
        ctx.config.database.backend,
        AppendEntryParams {
            actor_did: SYSTEM_DID,
            source: SOURCE_SYSTEM_DIAGNOSTIC,
            payload: Some(payload),
            action: ACTION_REVOCATION_ROLLBACK,
            subject: Some(&subject),
            rationale: &rationale,
            snapshot_id: None,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
}

/// Increment the mode-change version (§4.5) via the value-CAS, bounded
/// retry. Drives the per-operator mode-change banner dismissal key. Called
/// after a successful change to the reviewer-assignment-mode setting.
pub async fn bump_mode_version(ctx: &AppContext) -> PdsResult<()> {
    for _ in 0..CAS_MAX_RETRIES {
        let raw = read_runtime_row_value(ctx, MODERATION_REVIEWER_MODE_VERSION_KEY)
            .await
            .unwrap_or_else(|| "0".to_string());
        let next = (parse_u64_value(&raw) + 1).to_string();
        if cas_runtime_setting(
            ctx,
            MODERATION_REVIEWER_MODE_VERSION_KEY,
            &raw,
            &next,
            SYSTEM_DID,
        )
        .await?
        {
            return Ok(());
        }
    }
    // Best-effort: a missed bump only means a banner re-show is skipped once.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin::reports::{AssignmentScope, ReportReason};
    use crate::admin::roles::Role;
    use crate::config::*;
    use tempfile::tempdir;

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
                jwt_secret: "test-secret-key-aurora-reviewer-assign-x".to_string(),
                repo_signing_key: "a".repeat(64),
                plc_rotation_key: "b".repeat(64),
                password_login_enabled: false,
                admin_totp_encryption_key_hex: None,
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
                trust_proxy: false,
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

    async fn set_setting(ctx: &AppContext, key: &str, value: serde_json::Value) {
        // Settings rows for the operator-facing keys may not exist yet; use
        // delete-then-insert so the test can set them unconditionally.
        sqlx::query("DELETE FROM runtime_settings WHERE key = $1")
            .bind(key)
            .execute(&ctx.account_db)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO runtime_settings (key, value, last_modified, last_modified_by) \
             VALUES ($1, $2, $3, 'did:web:localhost')",
        )
        .bind(key)
        .bind(serde_json::to_string(&value).unwrap())
        .bind(Utc::now().to_rfc3339())
        .execute(&ctx.account_db)
        .await
        .unwrap();
    }

    async fn grant(ctx: &AppContext, did: &str) {
        ctx.admin_role_manager
            .grant_role(did, Role::Moderator, "did:web:localhost", None)
            .await
            .unwrap();
    }

    async fn submit(ctx: &AppContext, reason: ReportReason) -> Report {
        ctx.report_manager
            .submit_report(
                Some("did:plc:victim"),
                Some("at://did:plc:victim/app.bsky.feed.post/1"),
                Some("bafytestcid"),
                reason,
                Some("r"),
                "did:plc:reporter",
            )
            .await
            .unwrap()
    }

    async fn assignee_of(ctx: &AppContext, report_id: i64) -> (Option<String>, Option<String>) {
        let row = sqlx::query(
            "SELECT assigned_operator_did, assignment_source FROM report WHERE id = $1",
        )
        .bind(report_id)
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        (
            row.try_get("assigned_operator_did").ok().flatten(),
            row.try_get("assignment_source").ok().flatten(),
        )
    }

    #[tokio::test]
    async fn enumerate_operators_is_deduped_and_did_sorted() {
        let ctx = create_test_context().await;
        grant(&ctx, "did:plc:charlie").await;
        grant(&ctx, "did:plc:alice").await;
        grant(&ctx, "did:plc:bob").await;
        let pool = enumerate_operators(&ctx).await.unwrap();
        assert_eq!(pool, vec!["did:plc:alice", "did:plc:bob", "did:plc:charlie"]);
    }

    #[tokio::test]
    async fn manual_mode_leaves_unassigned() {
        let ctx = create_test_context().await;
        grant(&ctx, "did:plc:alice").await;
        // mode defaults to manual.
        let r = submit(&ctx, ReportReason::Spam).await;
        assign_reviewer_on_intake(&ctx, &r).await.unwrap();
        assert_eq!(assignee_of(&ctx, r.id).await, (None, None));
    }

    #[tokio::test]
    async fn reduced_tier_skips_assignment() {
        let ctx = create_test_context().await;
        grant(&ctx, "did:plc:alice").await;
        set_setting(&ctx, "moderation-mode", serde_json::json!("reduced")).await;
        set_setting(&ctx, MODERATION_REVIEWER_MODE_KEY, serde_json::json!("round-robin")).await;
        let r = submit(&ctx, ReportReason::Spam).await;
        assign_reviewer_on_intake(&ctx, &r).await.unwrap();
        assert_eq!(assignee_of(&ctx, r.id).await, (None, None));
    }

    #[tokio::test]
    async fn round_robin_rotates_and_advances_cursor() {
        let ctx = create_test_context().await;
        grant(&ctx, "did:plc:alice").await;
        grant(&ctx, "did:plc:bob").await;
        set_setting(&ctx, MODERATION_REVIEWER_MODE_KEY, serde_json::json!("round-robin")).await;
        // Pool sorted: [alice, bob]. Cursor starts 0.
        let r1 = submit(&ctx, ReportReason::Spam).await;
        assign_reviewer_on_intake(&ctx, &r1).await.unwrap();
        let r2 = submit(&ctx, ReportReason::Spam).await;
        assign_reviewer_on_intake(&ctx, &r2).await.unwrap();
        let r3 = submit(&ctx, ReportReason::Spam).await;
        assign_reviewer_on_intake(&ctx, &r3).await.unwrap();
        assert_eq!(assignee_of(&ctx, r1.id).await.0.as_deref(), Some("did:plc:alice"));
        assert_eq!(assignee_of(&ctx, r2.id).await.0.as_deref(), Some("did:plc:bob"));
        assert_eq!(assignee_of(&ctx, r3.id).await.0.as_deref(), Some("did:plc:alice"));
        // Source is 'auto'.
        assert_eq!(assignee_of(&ctx, r1.id).await.1.as_deref(), Some("auto"));
    }

    #[tokio::test]
    async fn round_robin_no_operators_is_noop() {
        let ctx = create_test_context().await;
        set_setting(&ctx, MODERATION_REVIEWER_MODE_KEY, serde_json::json!("round-robin")).await;
        let r = submit(&ctx, ReportReason::Spam).await;
        assign_reviewer_on_intake(&ctx, &r).await.unwrap();
        assert_eq!(assignee_of(&ctx, r.id).await, (None, None));
    }

    #[tokio::test]
    async fn load_balanced_picks_fewest_then_ties_by_did() {
        let ctx = create_test_context().await;
        grant(&ctx, "did:plc:alice").await;
        grant(&ctx, "did:plc:bob").await;
        set_setting(&ctx, MODERATION_REVIEWER_MODE_KEY, serde_json::json!("load-balanced")).await;
        // Pre-load bob with an active item so alice has fewer.
        let pre = submit(&ctx, ReportReason::Spam).await;
        sqlx::query("UPDATE report SET assigned_operator_did='did:plc:bob' WHERE id=$1")
            .bind(pre.id)
            .execute(&ctx.account_db)
            .await
            .unwrap();
        let r = submit(&ctx, ReportReason::Spam).await;
        assign_reviewer_on_intake(&ctx, &r).await.unwrap();
        assert_eq!(assignee_of(&ctx, r.id).await.0.as_deref(), Some("did:plc:alice"));
        // Now both have 1 each → next ties resolve to the lexicographically
        // smallest DID (alice).
        let r2 = submit(&ctx, ReportReason::Spam).await;
        assign_reviewer_on_intake(&ctx, &r2).await.unwrap();
        assert_eq!(assignee_of(&ctx, r2.id).await.0.as_deref(), Some("did:plc:alice"));
    }

    #[tokio::test]
    async fn category_routed_uses_pool_and_empty_is_noop() {
        let ctx = create_test_context().await;
        set_setting(&ctx, MODERATION_REVIEWER_MODE_KEY, serde_json::json!("category-routed")).await;
        set_setting(
            &ctx,
            MODERATION_REVIEWER_CATEGORY_MAP_KEY,
            serde_json::json!({"spam": ["did:plc:alice", "did:plc:bob"]}),
        )
        .await;
        // spam → routed
        let r1 = submit(&ctx, ReportReason::Spam).await;
        assign_reviewer_on_intake(&ctx, &r1).await.unwrap();
        assert_eq!(assignee_of(&ctx, r1.id).await.0.as_deref(), Some("did:plc:alice"));
        // rude → unmapped → unassigned
        let r2 = submit(&ctx, ReportReason::Rude).await;
        assign_reviewer_on_intake(&ctx, &r2).await.unwrap();
        assert_eq!(assignee_of(&ctx, r2.id).await, (None, None));
    }

    #[tokio::test]
    async fn cas_runtime_setting_wins_and_loses() {
        let ctx = create_test_context().await;
        // The rotation cursor row is migration-seeded at "0".
        let ok = cas_runtime_setting(
            &ctx,
            MODERATION_REVIEWER_ROTATION_CURSOR_KEY,
            "0",
            "1",
            SYSTEM_DID,
        )
        .await
        .unwrap();
        assert!(ok, "CAS on the seeded value wins");
        let stale = cas_runtime_setting(
            &ctx,
            MODERATION_REVIEWER_ROTATION_CURSOR_KEY,
            "0",
            "9",
            SYSTEM_DID,
        )
        .await
        .unwrap();
        assert!(!stale, "CAS with a stale expected value loses");
    }

    #[tokio::test]
    async fn queue_filter_scope() {
        let ctx = create_test_context().await;
        let a = submit(&ctx, ReportReason::Spam).await; // unassigned
        let b = submit(&ctx, ReportReason::Spam).await;
        sqlx::query("UPDATE report SET assigned_operator_did='did:plc:alice' WHERE id=$1")
            .bind(b.id)
            .execute(&ctx.account_db)
            .await
            .unwrap();
        let c = submit(&ctx, ReportReason::Spam).await;
        sqlx::query("UPDATE report SET assigned_operator_did='did:plc:bob' WHERE id=$1")
            .bind(c.id)
            .execute(&ctx.account_db)
            .await
            .unwrap();
        // alice sees her item + the unassigned one, not bob's.
        let alice = ctx
            .report_manager
            .list_reports_scoped(None, None, AssignmentScope::AssignedTo("did:plc:alice"))
            .await
            .unwrap();
        let ids: Vec<i64> = alice.iter().map(|r| r.id).collect();
        assert!(ids.contains(&a.id) && ids.contains(&b.id) && !ids.contains(&c.id));
        // SuperAdmin (All) sees everything.
        let all = ctx
            .report_manager
            .list_reports_scoped(None, None, AssignmentScope::All)
            .await
            .unwrap();
        assert_eq!(all.len(), 3);
    }

    #[tokio::test]
    async fn operator_removal_prunes_resets_and_audits() {
        let ctx = create_test_context().await;
        set_setting(
            &ctx,
            MODERATION_REVIEWER_CATEGORY_MAP_KEY,
            serde_json::json!({"spam": ["did:plc:alice", "did:plc:bob"], "rude": ["did:plc:alice"]}),
        )
        .await;
        // Two in-flight items assigned to alice (one resolved → preserved).
        let active = submit(&ctx, ReportReason::Spam).await;
        sqlx::query("UPDATE report SET assigned_operator_did='did:plc:alice', assignment_source='auto' WHERE id=$1")
            .bind(active.id).execute(&ctx.account_db).await.unwrap();
        let resolved = submit(&ctx, ReportReason::Spam).await;
        sqlx::query("UPDATE report SET assigned_operator_did='did:plc:alice', status='resolved' WHERE id=$1")
            .bind(resolved.id).execute(&ctx.account_db).await.unwrap();

        let _guard = audit_chain::AppendChainGuard::acquire().await;
        let mut tx = ctx.account_db.begin().await.unwrap();
        handle_operator_removal_in_tx(
            &mut tx,
            ctx.config.database.backend,
            "did:plc:alice",
            "did:web:localhost",
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        drop(_guard);

        // alice pruned from both category pools.
        let map = crate::api::aurora_admin::resolve_runtime_setting(
            &ctx,
            MODERATION_REVIEWER_CATEGORY_MAP_KEY,
        )
        .await;
        assert_eq!(map["spam"], serde_json::json!(["did:plc:bob"]));
        assert_eq!(map["rude"], serde_json::json!([]));
        // Active item reset; resolved item preserved.
        assert_eq!(assignee_of(&ctx, active.id).await, (None, None));
        assert_eq!(
            assignee_of(&ctx, resolved.id).await.0.as_deref(),
            Some("did:plc:alice")
        );
        // Audits: one _pruned per affected category (2) + one _pruned_bulk.
        let rows = sqlx::query("SELECT action, source FROM audit_chain_entry ORDER BY sequence ASC")
            .fetch_all(&ctx.account_db)
            .await
            .unwrap();
        let actions: Vec<String> = rows
            .iter()
            .map(|r| r.try_get::<String, _>("action").unwrap())
            .collect();
        assert_eq!(actions.iter().filter(|a| *a == ACTION_ROUTING_PRUNED).count(), 2);
        assert_eq!(
            actions.iter().filter(|a| *a == ACTION_ROUTING_PRUNED_BULK).count(),
            1
        );
        // All operator_removal-sourced.
        for r in &rows {
            assert_eq!(r.try_get::<String, _>("source").unwrap(), SOURCE_OPERATOR_REMOVAL);
        }
    }

    #[tokio::test]
    async fn cursor_reset_zeroes_all_three() {
        let ctx = create_test_context().await;
        // Dirty all three cursors.
        set_setting(&ctx, MODERATION_REVIEWER_ROTATION_CURSOR_KEY, serde_json::json!(5)).await;
        set_setting(&ctx, MODERATION_ESCALATION_SUPERADMIN_CURSOR_KEY, serde_json::json!(7)).await;
        set_setting(
            &ctx,
            MODERATION_REVIEWER_CATEGORY_CURSORS_KEY,
            serde_json::json!({"spam": 3}),
        )
        .await;
        let mut tx = ctx.account_db.begin().await.unwrap();
        reset_assignment_cursors_in_tx(&mut tx, "did:web:localhost")
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(
            read_runtime_row_value(&ctx, MODERATION_REVIEWER_ROTATION_CURSOR_KEY).await.as_deref(),
            Some("0")
        );
        assert_eq!(
            read_runtime_row_value(&ctx, MODERATION_ESCALATION_SUPERADMIN_CURSOR_KEY).await.as_deref(),
            Some("0")
        );
        assert_eq!(
            read_runtime_row_value(&ctx, MODERATION_REVIEWER_CATEGORY_CURSORS_KEY).await.as_deref(),
            Some("{}")
        );
    }

    #[tokio::test]
    async fn revocation_rollback_emits_system_diagnostic() {
        let ctx = create_test_context().await;
        emit_revocation_rollback(&ctx, "did:plc:alice", "cleanup failed")
            .await
            .unwrap();
        let row = sqlx::query("SELECT action, source, payload FROM audit_chain_entry ORDER BY sequence DESC LIMIT 1")
            .fetch_one(&ctx.account_db)
            .await
            .unwrap();
        assert_eq!(row.try_get::<String, _>("action").unwrap(), ACTION_REVOCATION_ROLLBACK);
        assert_eq!(row.try_get::<String, _>("source").unwrap(), SOURCE_SYSTEM_DIAGNOSTIC);
        assert!(row
            .try_get::<Option<String>, _>("payload")
            .ok()
            .flatten()
            .unwrap()
            .contains("rollback_reason"));
    }

    #[tokio::test]
    async fn mode_version_bumps() {
        let ctx = create_test_context().await;
        assert_eq!(
            read_runtime_row_value(&ctx, MODERATION_REVIEWER_MODE_VERSION_KEY).await.as_deref(),
            Some("0")
        );
        bump_mode_version(&ctx).await.unwrap();
        assert_eq!(
            read_runtime_row_value(&ctx, MODERATION_REVIEWER_MODE_VERSION_KEY).await.as_deref(),
            Some("1")
        );
    }
}
