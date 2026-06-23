//! §5.5.4 Phase A — configurable default action on report intake.
//!
//! When a report lands, substrate applies the operator-configured
//! default action immediately (§2.1). v0.9 Phase A ships the §2
//! default-action surface only — the Pipeline A (§6.9) steps for §3
//! auto-label, §4 routing, and §5 escalation arrive in later phases.
//!
//! The consumer ([`apply_report_default`]) is invoked by the report-
//! intake handlers (`com.atproto.moderation.createReport` and the admin
//! `submitReport`) after the report row is written. It is **full-tier
//! gated** (§2.7 / §6.3): in `reduced`/`disabled` moderation mode it is
//! a no-op. Substrate-emitted audit entries carry `actor_did =
//! "did:system"` and a `source` discriminator (§6.1).
//!
//! **Local-idiom translations** (recorded per the design's §6.1/§6.7
//! delegation of audit-chain-layer encoding to implementation):
//!
//! - *Subject = report ID* (§6.1): Aurora-Locus's audit `Subject` is a
//!   typed content/account reference (Repo/Record/Blob), not a free
//!   integer. The report-decision entry (`moderation_default_applied`)
//!   therefore carries the report's **content** subject and the report
//!   id in the tamper-evident `payload` (`{"report_id": N, ...}`), which
//!   links the entry to both the content and the report row.
//! - *Lazy stale scan* (§2.5): rather than a dedicated background timer
//!   (which `config.rs` warns against adding silently), the stale-hold
//!   sweep piggybacks on report intake — a write path naturally rate-
//!   limited by report volume. [`scan_stale_hide_pending`] is also
//!   callable directly.

use crate::admin::audit_chain::{self, AppendEntryParams, AppendChainGuard};
use crate::admin::defs::Subject;
use crate::admin::labels::LabelManager;
use crate::admin::reports::{Report, ReportReason};
use crate::api::aurora_admin::{
    resolve_runtime_setting, MODERATION_DEFAULTS_CATEGORY_MAP_KEY,
    MODERATION_DEFAULTS_REPORT_ACTION_KEY, MODERATION_DEFAULTS_STALE_DAYS_KEY,
    MODERATION_MODE_KEY, RECOVERY_MODE_ENV,
};
use crate::error::{PdsError, PdsResult};
use crate::AppContext;
use chrono::{DateTime, Utc};
use sqlx::Row as _;

/// Reserved substrate-applied label for the hide-pending-review
/// primitive (§2.6). Part of the `tools.aurora.ops.moderation.<purpose>`
/// reserved namespace for all substrate-applied moderation labels.
pub const HIDE_PENDING_LABEL: &str = "tools.aurora.ops.moderation.hide-pending";

/// Substrate-identity DID for substrate-initiated actions (§6.1).
pub(crate) const SYSTEM_DID: &str = "did:system";

// Audit action names (§6.1 registry — the three Phase A emits).
const ACTION_DEFAULT_APPLIED: &str = "moderation_default_applied";
const ACTION_DEFAULT_EXPIRED: &str = "moderation_default_expired";
const ACTION_AUTO_LABEL_APPLIED: &str = "moderation_auto_label_applied";

// Audit source discriminators (§6.1).
const SOURCE_DEFAULT_ACTION: &str = "default_action";
const SOURCE_STALE_EXPIRATION: &str = "stale_expiration";

/// Upper bound on a single lazy stale-sweep so the piggybacked scan
/// stays cheap. If a sweep hits the cap, the remainder is swept on the
/// next intake — logged, never silently dropped.
const STALE_SWEEP_LIMIT: i64 = 100;

/// The effective per-report default action after resolving the
/// `report-action` setting (and, for `auto-resolve-by-category`, the
/// per-category map). The category-map values are themselves only
/// `acknowledge` | `hide-pending-review`, so this two-variant enum is
/// the full resolved space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EffectiveAction {
    Acknowledge,
    HidePending,
}

/// Resolve the effective action for a report given the configured
/// `report-action` setting + category map.
///
/// `auto-resolve-by-category` consults the map for the report's
/// category; an unmapped category (or a malformed map) falls back to
/// `acknowledge` — the safe, non-content-affecting default. This is
/// also where the §2.2 "≥1 entry when auto-resolve-by-category"
/// constraint degrades gracefully: a misconfigured empty map applies
/// `acknowledge` rather than erroring at intake (the per-key
/// `validate_runtime_value` cannot enforce a cross-key invariant).
fn effective_action(
    report_action: &str,
    category_map: &serde_json::Value,
    reason: ReportReason,
) -> EffectiveAction {
    match report_action {
        "hide-pending-review" => EffectiveAction::HidePending,
        "auto-resolve-by-category" => {
            match category_map.get(reason.as_str()).and_then(|v| v.as_str()) {
                Some("hide-pending-review") => EffectiveAction::HidePending,
                _ => EffectiveAction::Acknowledge,
            }
        }
        // "acknowledge" or any unexpected value → acknowledge.
        _ => EffectiveAction::Acknowledge,
    }
}

/// Whether configurable defaults apply right now: `full` moderation
/// tier only (§2.7 / §6.3). Recovery mode forces `full` (mirrors
/// `get_runtime_setting`'s recovery override).
pub(crate) async fn defaults_active(ctx: &AppContext) -> bool {
    let recovery = std::env::var(RECOVERY_MODE_ENV)
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);
    if recovery {
        return true;
    }
    resolve_runtime_setting(ctx, MODERATION_MODE_KEY)
        .await
        .as_str()
        .map(|s| s == "full")
        .unwrap_or(false)
}

/// Build the content [`Subject`] for a report plus the label target
/// (uri, cid) the hide-pending primitive applies to. A record report
/// yields a `Record` subject (label keyed on the record URI); an
/// account-only report yields a `Repo` subject (label keyed on the
/// account DID, account-level). `submit_report` guarantees at least one
/// of did/uri is present.
fn report_subject(report: &Report) -> PdsResult<(Subject, String, Option<String>)> {
    if let Some(uri) = report.subject_uri.as_deref() {
        let cid = report.subject_cid.clone().unwrap_or_default();
        Ok((
            Subject::Record {
                uri: uri.to_string(),
                cid,
            },
            uri.to_string(),
            report.subject_cid.clone(),
        ))
    } else if let Some(did) = report.subject_did.as_deref() {
        Ok((
            Subject::Repo {
                did: did.to_string(),
            },
            did.to_string(),
            None,
        ))
    } else {
        Err(PdsError::Internal(
            "report has neither subject_uri nor subject_did".to_string(),
        ))
    }
}

/// `did:web:<hostname>` — the labeling authority / chain server DID,
/// matching the manual label-apply convention in `api::admin`.
pub(crate) fn server_did(ctx: &AppContext) -> String {
    format!("did:web:{}", ctx.config.service.hostname)
}

/// Apply the configured default action to a freshly-submitted report
/// (Pipeline A, §2). No-op outside `full` tier. Best-effort: a failure
/// here does not roll back the already-committed report — the operator
/// can still action the item manually — but it is surfaced to the
/// caller for logging.
pub async fn apply_report_default(ctx: &AppContext, report: &Report) -> PdsResult<()> {
    if !defaults_active(ctx).await {
        return Ok(());
    }

    // Lazy stale-hold sweep (§2.5) on the intake write-path. Best-effort:
    // a sweep failure must not block applying the new default.
    if let Err(e) = scan_stale_hide_pending(ctx).await {
        tracing::warn!(error = %e, "stale hide-pending sweep failed during report intake");
    }

    let report_action = resolve_runtime_setting(ctx, MODERATION_DEFAULTS_REPORT_ACTION_KEY).await;
    let report_action = report_action.as_str().unwrap_or("acknowledge");
    let category_map = resolve_runtime_setting(ctx, MODERATION_DEFAULTS_CATEGORY_MAP_KEY).await;
    let action = effective_action(report_action, &category_map, report.reason_type);

    let (subject, label_uri, label_cid) = report_subject(report)?;
    let backend = ctx.config.database.backend;

    match action {
        EffectiveAction::Acknowledge => {
            // Policy-decision entry only (§2.4): no content-affecting
            // mechanical consequence.
            let payload = serde_json::json!({
                "report_id": report.id,
                "action": "acknowledge",
            });
            let rationale = format!("default action acknowledge for report {}", report.id);
            audit_chain::insert_chain_entry_pool(
                &ctx.account_db,
                backend,
                AppendEntryParams {
                    actor_did: SYSTEM_DID,
                    source: SOURCE_DEFAULT_ACTION,
                    payload: Some(payload),
                    action: ACTION_DEFAULT_APPLIED,
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
        EffectiveAction::HidePending => {
            // §2.4: two entries — the policy decision + the mechanical
            // label consequence — plus the label apply itself, all in
            // ONE transaction (§6.1 single-transaction ordering). Hold
            // the chain guard across the commit per LB-1.
            let server = server_did(ctx);
            let _chain_guard = AppendChainGuard::acquire().await;
            let mut tx = ctx.account_db.begin().await?;

            let applied = LabelManager::apply_label_in_tx(
                &mut tx,
                &server,
                &label_uri,
                label_cid.as_deref(),
                HIDE_PENDING_LABEL,
                SYSTEM_DID,
                None,
                // §3.8 provenance: substrate default-action label.
                "default_action",
                None,
            )
            .await?;

            let decision_payload = serde_json::json!({
                "report_id": report.id,
                "action": "hide-pending-review",
            });
            let decision_rationale =
                format!("default action hide-pending-review for report {}", report.id);
            audit_chain::insert_chain_entry(
                &mut tx,
                backend,
                AppendEntryParams {
                    actor_did: SYSTEM_DID,
                    source: SOURCE_DEFAULT_ACTION,
                    payload: Some(decision_payload),
                    action: ACTION_DEFAULT_APPLIED,
                    subject: Some(&subject),
                    rationale: &decision_rationale,
                    snapshot_id: None,
                    event_id: None,
                    cascade_subjects: &[],
                    cascade_snapshot_ids: &[],
                },
            )
            .await?;

            // applied = true on a fresh issue, false if the label was
            // already active per dedup (§2.4).
            let label_payload = serde_json::json!({ "applied": applied.issued });
            let label_rationale = format!(
                "hide-pending label {} for report {}",
                HIDE_PENDING_LABEL, report.id
            );
            audit_chain::insert_chain_entry(
                &mut tx,
                backend,
                AppendEntryParams {
                    actor_did: SYSTEM_DID,
                    source: SOURCE_DEFAULT_ACTION,
                    payload: Some(label_payload),
                    action: ACTION_AUTO_LABEL_APPLIED,
                    subject: Some(&subject),
                    rationale: &label_rationale,
                    snapshot_id: None,
                    event_id: None,
                    cascade_subjects: &[],
                    cascade_snapshot_ids: &[],
                },
            )
            .await?;

            tx.commit().await?;
        }
    }

    Ok(())
}

/// Build a content [`Subject`] from a stored label's `uri`/`cid`. An
/// `at://` uri is a `Record`; a `did:` uri is account-level (`Repo`).
fn subject_from_label(uri: &str, cid: Option<String>) -> Subject {
    if uri.starts_with("at://") {
        Subject::Record {
            uri: uri.to_string(),
            cid: cid.unwrap_or_default(),
        }
    } else {
        Subject::Repo {
            did: uri.to_string(),
        }
    }
}

/// Lazily expire stale hide-pending-review holds (§2.5): remove active
/// `tools.aurora.ops.moderation.hide-pending` labels older than the
/// configured `hide-pending-review-stale-days`, emitting a
/// `moderation_default_expired` audit entry (source = `stale_expiration`)
/// per removal. Returns the number expired. Bounded by
/// [`STALE_SWEEP_LIMIT`] per call; a capped sweep logs the remainder.
pub async fn scan_stale_hide_pending(ctx: &AppContext) -> PdsResult<usize> {
    let stale_days = resolve_runtime_setting(ctx, MODERATION_DEFAULTS_STALE_DAYS_KEY)
        .await
        .as_i64()
        .unwrap_or(90)
        .clamp(1, 365);
    let cutoff = Utc::now() - chrono::Duration::days(stale_days);
    let backend = ctx.config.database.backend;
    let server = server_did(ctx);

    // Active (not later-negated) hide-pending labels. created_at is
    // RFC-3339 TEXT; parse + compare in Rust to stay backend-portable.
    let rows = sqlx::query(
        r#"
        SELECT l.id, l.uri, l.cid, l.created_at
        FROM label l
        WHERE l.val = $1 AND l.neg = FALSE
          AND NOT EXISTS (
            SELECT 1 FROM label n
            WHERE n.uri = l.uri AND n.val = l.val AND n.neg = TRUE AND n.id > l.id
          )
        ORDER BY l.id ASC
        LIMIT $2
        "#,
    )
    .bind(HIDE_PENDING_LABEL)
    .bind(STALE_SWEEP_LIMIT)
    .fetch_all(&ctx.account_db)
    .await?;

    let hit_cap = rows.len() as i64 == STALE_SWEEP_LIMIT;
    let mut expired = 0usize;
    for row in rows {
        let created_at_s: String = row.try_get("created_at")?;
        let created_at = match DateTime::parse_from_rfc3339(&created_at_s) {
            Ok(dt) => dt.with_timezone(&Utc),
            Err(_) => continue, // unparseable timestamp: skip, don't abort the sweep
        };
        if created_at >= cutoff {
            continue; // not yet stale
        }
        let uri: String = row.try_get("uri")?;
        let cid: Option<String> = row.try_get("cid").ok().flatten();
        let subject = subject_from_label(&uri, cid.clone());

        // Removal (negative label) + audit entry atomically.
        let _chain_guard = AppendChainGuard::acquire().await;
        let mut tx = ctx.account_db.begin().await?;
        LabelManager::remove_label_in_tx(
            &mut tx,
            &server,
            &uri,
            cid.as_deref(),
            HIDE_PENDING_LABEL,
            SYSTEM_DID,
        )
        .await?;
        let payload = serde_json::json!({
            "label": HIDE_PENDING_LABEL,
            "stale_days": stale_days,
        });
        let rationale = format!(
            "stale hide-pending hold expired after {} days",
            stale_days
        );
        audit_chain::insert_chain_entry(
            &mut tx,
            backend,
            AppendEntryParams {
                actor_did: SYSTEM_DID,
                source: SOURCE_STALE_EXPIRATION,
                payload: Some(payload),
                action: ACTION_DEFAULT_EXPIRED,
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
        expired += 1;
    }

    if hit_cap {
        tracing::info!(
            limit = STALE_SWEEP_LIMIT,
            "stale hide-pending sweep hit its per-call cap; remainder expires on the next intake"
        );
    }
    Ok(expired)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::*;
    use tempfile::tempdir;

    // --- pure effective-action resolution -------------------------------

    fn map(json: serde_json::Value) -> serde_json::Value {
        json
    }

    #[test]
    fn acknowledge_setting_always_acknowledges() {
        let m = map(serde_json::json!({"spam": "hide-pending-review"}));
        assert_eq!(
            effective_action("acknowledge", &m, ReportReason::Spam),
            EffectiveAction::Acknowledge
        );
    }

    #[test]
    fn hide_pending_setting_always_hides() {
        let m = map(serde_json::json!({}));
        assert_eq!(
            effective_action("hide-pending-review", &m, ReportReason::Other),
            EffectiveAction::HidePending
        );
    }

    #[test]
    fn by_category_uses_the_map_per_category() {
        let m = map(serde_json::json!({"spam": "hide-pending-review", "rude": "acknowledge"}));
        assert_eq!(
            effective_action("auto-resolve-by-category", &m, ReportReason::Spam),
            EffectiveAction::HidePending
        );
        assert_eq!(
            effective_action("auto-resolve-by-category", &m, ReportReason::Rude),
            EffectiveAction::Acknowledge
        );
    }

    #[test]
    fn by_category_unmapped_falls_back_to_acknowledge() {
        let m = map(serde_json::json!({"spam": "hide-pending-review"}));
        // 'sexual' is unmapped → acknowledge.
        assert_eq!(
            effective_action("auto-resolve-by-category", &m, ReportReason::Sexual),
            EffectiveAction::Acknowledge
        );
        // empty map → acknowledge (the §2.2 misconfiguration safety net).
        let empty = map(serde_json::json!({}));
        assert_eq!(
            effective_action("auto-resolve-by-category", &empty, ReportReason::Spam),
            EffectiveAction::Acknowledge
        );
    }

    #[test]
    fn unknown_setting_value_falls_back_to_acknowledge() {
        let m = map(serde_json::json!({}));
        assert_eq!(
            effective_action("garbage", &m, ReportReason::Spam),
            EffectiveAction::Acknowledge
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
                jwt_secret: "test-secret-key-aurora-mod-defaults-32x".to_string(),
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
                auto_stream_events: false,
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

    /// Directly seed a runtime setting (JSON-encoded value), mirroring how
    /// `set_runtime_setting` persists rows, so `resolve_runtime_setting`
    /// reads it back.
    async fn set_setting(ctx: &AppContext, key: &str, value: serde_json::Value) {
        let encoded = serde_json::to_string(&value).unwrap();
        sqlx::query(
            "INSERT INTO runtime_settings (key, value, last_modified, last_modified_by) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(key)
        .bind(&encoded)
        .bind(Utc::now().to_rfc3339())
        .bind("did:web:localhost")
        .execute(&ctx.account_db)
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
                Some("test report"),
                "did:plc:reporter",
            )
            .await
            .unwrap()
    }

    async fn audit_rows(ctx: &AppContext) -> Vec<(String, String, Option<String>)> {
        let rows = sqlx::query("SELECT action, source, payload FROM audit_chain_entry ORDER BY sequence ASC")
            .fetch_all(&ctx.account_db)
            .await
            .unwrap();
        rows.iter()
            .map(|r| {
                (
                    r.try_get::<String, _>("action").unwrap(),
                    r.try_get::<String, _>("source").unwrap(),
                    r.try_get::<Option<String>, _>("payload").ok().flatten(),
                )
            })
            .collect()
    }

    #[tokio::test]
    async fn reduced_tier_is_a_no_op() {
        let ctx = create_test_context().await;
        set_setting(&ctx, MODERATION_MODE_KEY, serde_json::json!("reduced")).await;
        set_setting(
            &ctx,
            MODERATION_DEFAULTS_REPORT_ACTION_KEY,
            serde_json::json!("hide-pending-review"),
        )
        .await;
        let report = submit(&ctx, ReportReason::Spam).await;
        apply_report_default(&ctx, &report).await.unwrap();
        assert!(audit_rows(&ctx).await.is_empty(), "reduced tier emits nothing");
    }

    #[tokio::test]
    async fn acknowledge_emits_single_default_applied() {
        let ctx = create_test_context().await;
        // moderation-mode defaults to "full"; report-action defaults to acknowledge.
        let report = submit(&ctx, ReportReason::Spam).await;
        apply_report_default(&ctx, &report).await.unwrap();
        let rows = audit_rows(&ctx).await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, ACTION_DEFAULT_APPLIED);
        assert_eq!(rows[0].1, SOURCE_DEFAULT_ACTION);
        assert!(rows[0].2.as_deref().unwrap().contains("\"report_id\""));
    }

    #[tokio::test]
    async fn hide_pending_applies_label_and_two_entries() {
        let ctx = create_test_context().await;
        set_setting(
            &ctx,
            MODERATION_DEFAULTS_REPORT_ACTION_KEY,
            serde_json::json!("hide-pending-review"),
        )
        .await;
        let report = submit(&ctx, ReportReason::Spam).await;
        apply_report_default(&ctx, &report).await.unwrap();

        let rows = audit_rows(&ctx).await;
        assert_eq!(rows.len(), 2, "default_applied + auto_label_applied");
        assert_eq!(rows[0].0, ACTION_DEFAULT_APPLIED);
        assert_eq!(rows[1].0, ACTION_AUTO_LABEL_APPLIED);
        assert!(
            rows[1].2.as_deref().unwrap().contains("\"applied\":true"),
            "fresh label issue is applied=true"
        );
        // The hide-pending label is active.
        let cnt: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM label WHERE val = $1 AND neg = FALSE",
        )
        .bind(HIDE_PENDING_LABEL)
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert_eq!(cnt, 1);
    }

    #[tokio::test]
    async fn re_report_dedups_to_applied_false() {
        let ctx = create_test_context().await;
        set_setting(
            &ctx,
            MODERATION_DEFAULTS_REPORT_ACTION_KEY,
            serde_json::json!("hide-pending-review"),
        )
        .await;
        let r1 = submit(&ctx, ReportReason::Spam).await;
        apply_report_default(&ctx, &r1).await.unwrap();
        let r2 = submit(&ctx, ReportReason::Spam).await;
        apply_report_default(&ctx, &r2).await.unwrap();

        let rows = audit_rows(&ctx).await;
        // 2 entries per apply = 4 total; the last auto_label_applied is applied=false.
        assert_eq!(rows.len(), 4);
        let last_label = rows
            .iter()
            .rfind(|r| r.0 == ACTION_AUTO_LABEL_APPLIED)
            .unwrap();
        assert!(
            last_label.2.as_deref().unwrap().contains("\"applied\":false"),
            "second apply dedups → applied=false"
        );
        // Still exactly one active label row (no duplicate insert).
        let cnt: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM label WHERE val = $1 AND neg = FALSE",
        )
        .bind(HIDE_PENDING_LABEL)
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert_eq!(cnt, 1);
    }

    #[tokio::test]
    async fn stale_scan_expires_old_holds() {
        let ctx = create_test_context().await;
        let server = server_did(&ctx);
        // Seed a hide-pending label dated well past the default 90-day window.
        let old = (Utc::now() - chrono::Duration::days(120)).to_rfc3339();
        sqlx::query(
            "INSERT INTO label (uri, cid, val, neg, src, created_at, created_by) \
             VALUES ($1, NULL, $2, FALSE, $3, $4, $5)",
        )
        .bind("at://did:plc:victim/app.bsky.feed.post/old")
        .bind(HIDE_PENDING_LABEL)
        .bind(&server)
        .bind(&old)
        .bind(SYSTEM_DID)
        .execute(&ctx.account_db)
        .await
        .unwrap();

        let expired = scan_stale_hide_pending(&ctx).await.unwrap();
        assert_eq!(expired, 1);

        // A negation row now exists, and an expiry audit entry was emitted.
        let neg: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM label WHERE val = $1 AND neg = TRUE",
        )
        .bind(HIDE_PENDING_LABEL)
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert_eq!(neg, 1);
        let rows = audit_rows(&ctx).await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, ACTION_DEFAULT_EXPIRED);
        assert_eq!(rows[0].1, SOURCE_STALE_EXPIRATION);
    }

    #[tokio::test]
    async fn fresh_holds_are_not_expired() {
        let ctx = create_test_context().await;
        set_setting(
            &ctx,
            MODERATION_DEFAULTS_REPORT_ACTION_KEY,
            serde_json::json!("hide-pending-review"),
        )
        .await;
        let report = submit(&ctx, ReportReason::Spam).await;
        apply_report_default(&ctx, &report).await.unwrap();
        // A second sweep (the intake already ran one) finds nothing stale.
        let expired = scan_stale_hide_pending(&ctx).await.unwrap();
        assert_eq!(expired, 0);
    }
}
