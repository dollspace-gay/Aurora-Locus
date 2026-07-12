//! §5.5.4 Phase E — startup-trigger lexicon migration (§6.7 #1).
//!
//! The report-category vocabulary is the `ReportReason` enum. If a future
//! build changes it (adds/renames/removes a category), category-keyed config
//! (the §2.3 default-action map, the §4.3 reviewer-routing map) and
//! category-referencing rules (§3 report-count, §5 report-count/category-match)
//! can hold now-stale categories. On boot, substrate detects an enum change
//! (a stored hash vs. the current set) and migrates: prunes stale map keys,
//! flags stale rules (WITHOUT disabling them — operators decide), emits a
//! `moderation_lexicon_migration` diagnostic audit, and raises a banner.
//!
//! At v0.9 the enum is fixed, so this no-ops after recording the initial hash
//! — dormant forward-infrastructure. Runs once at boot, after AppContext::new
//! (audit-chain ready) and before serving (migrated state visible to the first
//! request).
//!
//! Local-idiom translations (memory #18): action name
//! `moderation_lexicon_migration` (the established prefix); banner content in a
//! runtime setting + per-operator localStorage dismissal (Phase B convention).

use crate::admin::audit_chain::{self, AppendEntryParams};
use crate::api::aurora_admin::{
    read_runtime_row_value, MODERATION_DEFAULTS_CATEGORY_MAP_KEY, MODERATION_LEXICON_BANNER_KEY,
    MODERATION_LEXICON_ENUM_HASH_KEY, MODERATION_REVIEWER_CATEGORY_MAP_KEY,
};
use crate::AppContext;
use chrono::Utc;
use sha2::{Digest, Sha256};
use sqlx::Row as _;

const ACTION_LEXICON_MIGRATION: &str = "moderation_lexicon_migration";
const SOURCE_SYSTEM_DIAGNOSTIC: &str = "system_diagnostic";
const SYSTEM_DID: &str = "did:system";

/// The current report-category vocabulary (the `ReportReason` six). Kept here
/// (not imported as the enum) so the migration is value-driven.
fn current_categories() -> Vec<&'static str> {
    vec!["spam", "violation", "misleading", "sexual", "rude", "other"]
}

/// Stable hash of the current category set (sorted, joined) — the change witness.
fn enum_hash() -> String {
    let mut cats = current_categories();
    cats.sort_unstable();
    let mut hasher = Sha256::new();
    hasher.update(cats.join(",").as_bytes());
    hex::encode(hasher.finalize())
}

async fn write_setting(ctx: &AppContext, key: &str, value: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM runtime_settings WHERE key = $1").bind(key).execute(&ctx.account_db).await?;
    sqlx::query(
        "INSERT INTO runtime_settings (key, value, last_modified, last_modified_by) VALUES ($1, $2, $3, $4)",
    )
    .bind(key)
    .bind(value)
    .bind(Utc::now().to_rfc3339())
    .bind(SYSTEM_DID)
    .execute(&ctx.account_db)
    .await?;
    Ok(())
}

/// Prune keys outside `valid` from a category-keyed JSON-object setting. Returns
/// the pruned keys.
async fn prune_category_map(ctx: &AppContext, key: &str, valid: &[&str]) -> Vec<String> {
    let raw = match read_runtime_row_value(ctx, key).await {
        Some(r) => r,
        None => return Vec::new(),
    };
    let mut obj: serde_json::Map<String, serde_json::Value> = match serde_json::from_str(&raw) {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    let stale: Vec<String> = obj.keys().filter(|k| !valid.contains(&k.as_str())).cloned().collect();
    if stale.is_empty() {
        return Vec::new();
    }
    for k in &stale {
        obj.remove(k);
    }
    let _ = write_setting(ctx, key, &serde_json::to_string(&serde_json::Value::Object(obj)).unwrap_or_else(|_| "{}".into())).await;
    stale
}

/// Flag rules whose category-bearing trigger params reference a category
/// outside `valid` (NOT disabled — surfaced for operator decision). Returns
/// the flagged rule ids.
async fn flag_stale_rules(ctx: &AppContext, table: &str, valid: &[&str]) -> Vec<String> {
    let rows = match sqlx::query(&format!(
        "SELECT id, trigger_type, trigger_params FROM {} WHERE deleted_at IS NULL",
        table
    ))
    .fetch_all(&ctx.account_db)
    .await
    {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let mut flagged = Vec::new();
    for row in &rows {
        let tt: String = row.try_get("trigger_type").unwrap_or_default();
        if tt != "report-count" && tt != "category-match" {
            continue;
        }
        let params_s: String = row.try_get("trigger_params").unwrap_or_default();
        let params: serde_json::Value = serde_json::from_str(&params_s).unwrap_or(serde_json::Value::Null);
        if let Some(cat) = params.get("category").and_then(|v| v.as_str()) {
            if !valid.contains(&cat) {
                if let Ok(id) = row.try_get::<String, _>("id") {
                    flagged.push(id);
                }
            }
        }
    }
    flagged
}

/// Run the boot-time lexicon migration. Idempotent: records the initial hash on
/// first boot (no migration), no-ops on an unchanged enum, migrates on change.
/// Best-effort — a failure here must never block boot.
pub async fn run_lexicon_migration(ctx: &AppContext) {
    let current = enum_hash();
    let stored = read_runtime_row_value(ctx, MODERATION_LEXICON_ENUM_HASH_KEY).await;

    match stored {
        None => {
            // First boot: record the baseline, nothing to migrate from.
            let _ = write_setting(ctx, MODERATION_LEXICON_ENUM_HASH_KEY, &current).await;
            return;
        }
        Some(s) if s == current => return, // unchanged
        Some(_) => {} // changed → migrate
    }

    let valid = current_categories();
    let mut pruned = prune_category_map(ctx, MODERATION_DEFAULTS_CATEGORY_MAP_KEY, &valid).await;
    pruned.extend(prune_category_map(ctx, MODERATION_REVIEWER_CATEGORY_MAP_KEY, &valid).await);
    let mut flagged = flag_stale_rules(ctx, "moderation_auto_label_rule", &valid).await;
    flagged.extend(flag_stale_rules(ctx, "moderation_escalation_rule", &valid).await);

    let migrated_at = Utc::now().to_rfc3339();
    let banner = serde_json::json!({
        "migratedAt": migrated_at,
        "prunedKeys": pruned,
        "flaggedRuleIds": flagged,
    });
    let _ = write_setting(ctx, MODERATION_LEXICON_BANNER_KEY, &serde_json::to_string(&banner).unwrap_or_else(|_| "{}".into())).await;

    // Diagnostic audit (best-effort).
    let _ = audit_chain::insert_chain_entry_pool(
        &ctx.account_db,
        ctx.config.database.backend,
        AppendEntryParams {
            actor_did: SYSTEM_DID,
            source: SOURCE_SYSTEM_DIAGNOSTIC,
            payload: Some(banner),
            action: ACTION_LEXICON_MIGRATION,
            subject: None,
            rationale: "report-category lexicon change migrated at boot",
            snapshot_id: None,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await;

    let _ = write_setting(ctx, MODERATION_LEXICON_ENUM_HASH_KEY, &current).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::*;
    use tempfile::tempdir;

    async fn ctx() -> AppContext {
        let dir = tempdir().unwrap().keep();
        let db_path = dir.join("test.db");
        let config = ServerConfig {
            service: ServiceConfig {
                hostname: "localhost".to_string(), port: 2583, service_did: "did:web:localhost".to_string(),
                version: "0.1.0-test".to_string(), blob_upload_limit: 5_242_880, public_url: None,
                max_blob_fetch_size: 50_000_000, blob_fetch_timeout_seconds: 30, blob_fetch_max_retries: 3,
                accepting_imports: true, max_import_size: None,
            },
            storage: StorageConfig {
                data_directory: dir.clone(), account_db: db_path.clone(), sequencer_db: dir.join("sequencer.db"),
                did_cache_db: dir.join("did_cache.db"), actor_store_directory: dir.join("actors"),
                blobstore: BlobstoreConfig::Disk { location: dir.join("blobs"), tmp_location: dir.join("temp") },
            },
            database: Default::default(),
            authentication: AuthConfig {
                jwt_secret: "test-secret-key-aurora-lexicon-migration-x".to_string(),
                repo_signing_key: "a".repeat(64), plc_rotation_key: "b".repeat(64),
                oauth: OAuthConfig { client_id: "http://localhost:3000/client-metadata.json".to_string(), redirect_uri: "http://localhost:3000/oauth/callback".to_string(), pds_url: "https://bsky.social".to_string() },
                jwt_sunset_date: "Sat, 31 Dec 2024 23:59:59 GMT".to_string(),
                oauth_migration_guide_url: "https://docs.atproto.com/guides/oauth-migration".to_string(),
                password_login_enabled: false,
                admin_totp_encryption_key_hex: None,
            },
            identity: IdentityConfig { did_plc_url: "https://plc.directory".to_string(), service_handle_domains: vec![".localhost".to_string()], did_cache_stale_ttl: 3600, did_cache_max_ttl: 86400, recovery_did_key: None },
            email: None,
            invites: InviteConfig { required: false, interval: 604800, epoch: "2024-01-01T00:00:00Z".to_string() },
            rate_limit: RateLimitConfig { enabled: false, global_requests_per_minute: 3000, exempt_admin_assets: true, buckets_retention_days: 7, trust_proxy: false },
            logging: LoggingConfig { level: "info".to_string() },
            federation: FederationConfig { enabled: false, relay_urls: vec![], appview_url: None, firehose_enabled: false, crawl_enabled: false, public_url: Some("http://localhost:2583".to_string()), peer_pds: vec![] },
            validation_mode: crate::validation::ValidationMode::Required,
            distributed_state_mode: Default::default(), maintenance_pool: Default::default(), gc_sweep: Default::default(),
            bind_audit_orphan_marker: Default::default(), blob_metadata: Default::default(), entryway: None,
            lexicon: crate::config::LexiconConfig::default(), kryphocron: crate::config::KryphocronConfig::default(),
        };
        AppContext::new(config, std::sync::Arc::new(crate::api::registry::RouteRegistry::default())).await.unwrap()
    }

    async fn lexicon_audit_count(ctx: &AppContext) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_chain_entry WHERE action = $1")
            .bind(ACTION_LEXICON_MIGRATION)
            .fetch_one(&ctx.account_db).await.unwrap()
    }

    #[tokio::test]
    async fn first_boot_records_hash_without_migrating() {
        let ctx = ctx().await;
        run_lexicon_migration(&ctx).await;
        // Baseline hash recorded; no migration audit.
        assert_eq!(read_runtime_row_value(&ctx, MODERATION_LEXICON_ENUM_HASH_KEY).await.as_deref(), Some(enum_hash().as_str()));
        assert_eq!(lexicon_audit_count(&ctx).await, 0);
        // A second boot with the unchanged enum is a no-op.
        run_lexicon_migration(&ctx).await;
        assert_eq!(lexicon_audit_count(&ctx).await, 0);
    }

    #[tokio::test]
    async fn changed_enum_prunes_flags_and_audits() {
        let ctx = ctx().await;
        // Simulate a prior enum (different hash) + stale config.
        write_setting(&ctx, MODERATION_LEXICON_ENUM_HASH_KEY, "stale-prior-hash").await.unwrap();
        write_setting(&ctx, MODERATION_DEFAULTS_CATEGORY_MAP_KEY,
            r#"{"spam":"hide-pending-review","harassment":"hide-pending-review"}"#).await.unwrap();
        // A report-count auto-label rule referencing the removed 'harassment' category.
        sqlx::query(
            "INSERT INTO moderation_auto_label_rule (id, trigger_type, trigger_params, label_value, subject_scope, enabled, created_at, created_by_did, last_modified_at, last_modified_by_did) \
             VALUES ('rule-x', 'report-count', '{\"category\":\"harassment\",\"threshold\":1,\"window_days\":1}', 'l', 'account', 1, 'now', 'op', 'now', 'op')")
            .execute(&ctx.account_db).await.unwrap();

        run_lexicon_migration(&ctx).await;

        // 'harassment' pruned from the map; 'spam' preserved.
        let map = read_runtime_row_value(&ctx, MODERATION_DEFAULTS_CATEGORY_MAP_KEY).await.unwrap();
        assert!(map.contains("spam") && !map.contains("harassment"));
        // The stale rule is flagged in the banner (NOT disabled).
        let banner = read_runtime_row_value(&ctx, MODERATION_LEXICON_BANNER_KEY).await.unwrap();
        assert!(banner.contains("rule-x") && banner.contains("harassment"));
        let still_enabled: i64 = sqlx::query_scalar("SELECT enabled FROM moderation_auto_label_rule WHERE id='rule-x'")
            .fetch_one(&ctx.account_db).await.unwrap();
        assert_eq!(still_enabled, 1, "stale rule flagged, not disabled");
        // Diagnostic audit emitted with system_diagnostic source.
        assert_eq!(lexicon_audit_count(&ctx).await, 1);
        let src: String = sqlx::query_scalar("SELECT source FROM audit_chain_entry WHERE action = $1")
            .bind(ACTION_LEXICON_MIGRATION).fetch_one(&ctx.account_db).await.unwrap();
        assert_eq!(src, "system_diagnostic");
        // Hash updated to current → next boot no-ops.
        assert_eq!(read_runtime_row_value(&ctx, MODERATION_LEXICON_ENUM_HASH_KEY).await.as_deref(), Some(enum_hash().as_str()));
    }
}
