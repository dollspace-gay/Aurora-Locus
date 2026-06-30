//! v0.9 Integration hooks Phase A — wired surface (#350).
//!
//! Declaration-without-execution: CRUD over the `moderation_integration_hook`
//! table + composite-load + audit. All declaration logic (the `Hook` type, URL
//! validation, netaddr SSRF predicates, event-class taxonomy) lives in the
//! structurally-firewalled `hooks-core` crate (Layer 1); this module is the
//! plumbing between XRPC/DB/audit and that core. There is NO execution sink
//! here — no HTTP call to a hook URL — and `EXECUTION_ENABLED` is a literal
//! `false` (Layer 5).
//!
//! Local-idiom translation (memory #18): the design-commit-17 raw
//! `BEGIN IMMEDIATE`/`SERIALIZABLE` count-check is implemented as the §5.5.4
//! Phase C plain-`sqlx::Any`-tx cap pattern (the audit-chain "BEGIN IMMEDIATE
//! limitation" — sqlx::Any is BEGIN-DEFERRED), and rule-lifecycle audits use
//! `source="manual"` (the §5.5.4 NOT-NULL convention).

use crate::admin::audit_chain::{self, AppendEntryParams};
use crate::AppContext;
use chrono::Utc;
use hooks_core::Hook;
use sqlx::Row as _;
use uuid::Uuid;

/// §7.5 Layer 5 — the execution-status honesty constant. Declaration-only in
/// v0.9; a future cycle flips this when execution actually ships. A test pins
/// it to `false` so the UI's "not yet executed" banner can never silently lie.
pub const EXECUTION_ENABLED: bool = false;

/// The event classes available for subscription (§2.2 drop-policy survivors).
/// Per Phase A recon all 8 v0.9 classes have substrate emission points, so the
/// available set is the full taxonomy. If a future class loses its emission it
/// is removed here; the composite-load surfaces this as the UI's source of
/// truth (design-commit 14).
pub const AVAILABLE_EVENT_CLASSES: &[&str] = hooks_core::V0_9_EVENT_CLASSES;

const ACTION_CREATED: &str = "moderation_integration_hook_created";
const ACTION_EDITED: &str = "moderation_integration_hook_edited";
const ACTION_DELETED: &str = "moderation_integration_hook_deleted";

fn err(code: u16, msg: impl Into<String>) -> (u16, String) {
    (code, msg.into())
}

fn hook_from_row(row: &sqlx::any::AnyRow) -> Result<Hook, (u16, String)> {
    let classes_s: String = row.try_get("event_classes").map_err(|e| err(500, e.to_string()))?;
    let event_classes: Vec<String> = serde_json::from_str(&classes_s).unwrap_or_default();
    Ok(Hook {
        id: row.try_get("id").map_err(|e| err(500, e.to_string()))?,
        name: row.try_get("name").map_err(|e| err(500, e.to_string()))?,
        url: row.try_get("url").map_err(|e| err(500, e.to_string()))?,
        event_classes,
        description: row.try_get("description").ok().flatten(),
        enabled: row.try_get::<i64, _>("enabled").unwrap_or(0) != 0,
        created_at: row.try_get("created_at").map_err(|e| err(500, e.to_string()))?,
        created_by_did: row.try_get("created_by_did").map_err(|e| err(500, e.to_string()))?,
        last_modified_at: row.try_get("last_modified_at").map_err(|e| err(500, e.to_string()))?,
        last_modified_by_did: row.try_get("last_modified_by_did").map_err(|e| err(500, e.to_string()))?,
        rationale: row.try_get("rationale").ok().flatten(),
        deleted_at: row.try_get("deleted_at").ok().flatten(),
    })
}

/// Validate name + URL + description + event-classes via hooks-core, returning
/// the normalized URL. A validation failure maps to a 400 with the stable code.
fn validate(
    name: &str,
    url: &str,
    event_classes: &[String],
    description: Option<&str>,
) -> Result<String, (u16, String)> {
    hooks_core::validate_name(name).map_err(|e| err(400, e.to_string()))?;
    hooks_core::validate_description(description).map_err(|e| err(400, e.to_string()))?;
    hooks_core::validate_event_classes(event_classes, AVAILABLE_EVENT_CLASSES)
        .map_err(|e| err(400, e.to_string()))?;
    hooks_core::validate_hook_url(url).map_err(|e| err(400, e.to_string()))
}

async fn emit_lifecycle(
    ctx: &AppContext,
    action: &str,
    operator_did: &str,
    payload: serde_json::Value,
    rationale: &str,
) -> Result<i64, (u16, String)> {
    audit_chain::insert_chain_entry_pool(
        &ctx.account_db,
        ctx.config.database.backend,
        AppendEntryParams {
            actor_did: operator_did,
            source: "manual",
            payload: Some(payload),
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
    .map_err(|e| err(500, e.to_string()))
}

/// Create a hook (§2.5). Validates, enforces the 50-active cap atomically
/// (count-check + INSERT in one tx — the §5.5.4 plain-tx pattern), emits
/// `moderation_integration_hook_created`. Returns the stored hook.
#[allow(clippy::too_many_arguments)]
pub async fn create_hook(
    ctx: &AppContext,
    operator_did: &str,
    name: &str,
    url: &str,
    event_classes: &[String],
    description: Option<&str>,
    enabled: bool,
    rationale: Option<&str>,
) -> Result<Hook, (u16, String)> {
    let normalized_url = validate(name, url, event_classes, description)?;

    let mut tx = ctx.account_db.begin().await.map_err(|e| err(500, e.to_string()))?;
    let active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM moderation_integration_hook WHERE deleted_at IS NULL",
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| err(500, e.to_string()))?;
    if active >= hooks_core::MAX_ACTIVE_HOOKS {
        return Err(err(400, format!("active integration-hook limit ({}) reached", hooks_core::MAX_ACTIVE_HOOKS)));
    }
    let id = Uuid::new_v4().simple().to_string();
    let now = Utc::now().to_rfc3339();
    let classes_json = serde_json::to_string(event_classes).unwrap_or_else(|_| "[]".into());
    sqlx::query(
        "INSERT INTO moderation_integration_hook \
         (id, name, url, event_classes, description, enabled, created_at, created_by_did, \
          last_modified_at, last_modified_by_did, rationale, deleted_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, NULL)",
    )
    .bind(&id)
    .bind(name)
    .bind(&normalized_url)
    .bind(&classes_json)
    .bind(description)
    .bind(if enabled { 1_i64 } else { 0 })
    .bind(&now)
    .bind(operator_did)
    .bind(&now)
    .bind(operator_did)
    .bind(rationale)
    .execute(&mut *tx)
    .await
    .map_err(|e| err(500, e.to_string()))?;
    tx.commit().await.map_err(|e| err(500, e.to_string()))?;

    emit_lifecycle(
        ctx,
        ACTION_CREATED,
        operator_did,
        serde_json::json!({ "hook_id": id, "name": name, "url": normalized_url }),
        rationale.unwrap_or("integration hook created"),
    )
    .await?;

    Ok(Hook {
        id,
        name: name.to_string(),
        url: normalized_url,
        event_classes: event_classes.to_vec(),
        description: description.map(String::from),
        enabled,
        created_at: now.clone(),
        created_by_did: operator_did.to_string(),
        last_modified_at: now,
        last_modified_by_did: operator_did.to_string(),
        rationale: rationale.map(String::from),
        deleted_at: None,
    })
}

/// Build the §6 change_summary array diffing an old hook against the new
/// values. Scalar fields → `{field, before, after}`; event_classes →
/// `{field, added, removed}` (set difference) per design-commit 25.
fn change_summary(
    old: &Hook,
    name: &str,
    url: &str,
    event_classes: &[String],
    description: Option<&str>,
    enabled: bool,
    rationale: Option<&str>,
) -> serde_json::Value {
    let mut changes = Vec::new();
    let mut scalar = |field: &str, before: serde_json::Value, after: serde_json::Value| {
        if before != after {
            changes.push(serde_json::json!({ "field": field, "before": before, "after": after }));
        }
    };
    scalar("name", old.name.clone().into(), name.into());
    scalar("url", old.url.clone().into(), url.into());
    scalar(
        "description",
        old.description.clone().map(Into::into).unwrap_or(serde_json::Value::Null),
        description.map(Into::into).unwrap_or(serde_json::Value::Null),
    );
    scalar("enabled", old.enabled.into(), enabled.into());
    scalar(
        "rationale",
        old.rationale.clone().map(Into::into).unwrap_or(serde_json::Value::Null),
        rationale.map(Into::into).unwrap_or(serde_json::Value::Null),
    );
    // event_classes: set difference.
    let new_set: std::collections::BTreeSet<&str> = event_classes.iter().map(|s| s.as_str()).collect();
    let old_set: std::collections::BTreeSet<&str> = old.event_classes.iter().map(|s| s.as_str()).collect();
    let added: Vec<&str> = new_set.difference(&old_set).copied().collect();
    let removed: Vec<&str> = old_set.difference(&new_set).copied().collect();
    if !added.is_empty() || !removed.is_empty() {
        changes.push(serde_json::json!({ "field": "event_classes", "added": added, "removed": removed }));
    }
    serde_json::Value::Array(changes)
}

/// Edit a hook (§4) with optimistic concurrency: `expected_last_modified_at`
/// must match the stored token or it's a 409 Conflict. Emits `_edited` with a
/// change_summary.
#[allow(clippy::too_many_arguments)]
pub async fn edit_hook(
    ctx: &AppContext,
    operator_did: &str,
    id: &str,
    expected_last_modified_at: &str,
    name: &str,
    url: &str,
    event_classes: &[String],
    description: Option<&str>,
    enabled: bool,
    rationale: Option<&str>,
) -> Result<(), (u16, String)> {
    let normalized_url = validate(name, url, event_classes, description)?;

    // Load current (active) hook for the concurrency check + diff.
    let row = sqlx::query("SELECT * FROM moderation_integration_hook WHERE id = $1 AND deleted_at IS NULL")
        .bind(id)
        .fetch_optional(&ctx.account_db)
        .await
        .map_err(|e| err(500, e.to_string()))?;
    let old = match row {
        Some(r) => hook_from_row(&r)?,
        None => return Err(err(404, format!("hook {} not found", id))),
    };
    if old.last_modified_at != expected_last_modified_at {
        return Err(err(409, "hook was modified by another operator; reload and retry".to_string()));
    }

    let now = Utc::now().to_rfc3339();
    let classes_json = serde_json::to_string(event_classes).unwrap_or_else(|_| "[]".into());
    let summary = change_summary(&old, name, &normalized_url, event_classes, description, enabled, rationale);
    // Guard the UPDATE on the unchanged token too (defends a race between the
    // read and the write).
    let res = sqlx::query(
        "UPDATE moderation_integration_hook SET name = $1, url = $2, event_classes = $3, \
         description = $4, enabled = $5, last_modified_at = $6, last_modified_by_did = $7, \
         rationale = $8 WHERE id = $9 AND deleted_at IS NULL AND last_modified_at = $10",
    )
    .bind(name)
    .bind(&normalized_url)
    .bind(&classes_json)
    .bind(description)
    .bind(if enabled { 1_i64 } else { 0 })
    .bind(&now)
    .bind(operator_did)
    .bind(rationale)
    .bind(id)
    .bind(expected_last_modified_at)
    .execute(&ctx.account_db)
    .await
    .map_err(|e| err(500, e.to_string()))?;
    if res.rows_affected() == 0 {
        return Err(err(409, "hook was modified concurrently; reload and retry".to_string()));
    }

    emit_lifecycle(
        ctx,
        ACTION_EDITED,
        operator_did,
        serde_json::json!({ "hook_id": id, "change_summary": summary }),
        rationale.unwrap_or("integration hook edited"),
    )
    .await?;
    Ok(())
}

/// Soft-delete a hook (§4). One-way (no restore, design-commit 21). Emits `_deleted`.
pub async fn delete_hook(ctx: &AppContext, operator_did: &str, id: &str) -> Result<(), (u16, String)> {
    let now = Utc::now().to_rfc3339();
    let res = sqlx::query(
        "UPDATE moderation_integration_hook SET deleted_at = $1 WHERE id = $2 AND deleted_at IS NULL",
    )
    .bind(&now)
    .bind(id)
    .execute(&ctx.account_db)
    .await
    .map_err(|e| err(500, e.to_string()))?;
    if res.rows_affected() == 0 {
        return Err(err(404, format!("hook {} not found", id)));
    }
    emit_lifecycle(
        ctx,
        ACTION_DELETED,
        operator_did,
        serde_json::json!({ "hook_id": id }),
        "integration hook deleted",
    )
    .await?;
    Ok(())
}

/// List hooks (§4). `include_deleted` surfaces soft-deleted rows.
pub async fn list_hooks(ctx: &AppContext, include_deleted: bool) -> Result<Vec<Hook>, (u16, String)> {
    let sql = if include_deleted {
        "SELECT * FROM moderation_integration_hook ORDER BY created_at DESC"
    } else {
        "SELECT * FROM moderation_integration_hook WHERE deleted_at IS NULL ORDER BY created_at DESC"
    };
    let rows = sqlx::query(sql)
        .fetch_all(&ctx.account_db)
        .await
        .map_err(|e| err(500, e.to_string()))?;
    rows.iter().map(hook_from_row).collect()
}

/// Composite-load (§4.1): hooks + the available event-class set (UI source of
/// truth) + the honest execution-status banner data.
pub async fn integration_hooks_state(ctx: &AppContext) -> Result<serde_json::Value, (u16, String)> {
    let hooks = list_hooks(ctx, false).await?;
    Ok(serde_json::json!({
        "hooks": hooks,
        "availableEventClasses": AVAILABLE_EVENT_CLASSES,
        "executionStatus": {
            "enabled": EXECUTION_ENABLED,
            "message": "Hooks are declared here but not yet executed; execution ships in a future cycle.",
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::*;
    use tempfile::tempdir;

    // --- Tripwire tests (no DB) -----------------------------------------

    /// Layer 5 (§7.5): the execution-status honesty constant must be false in
    /// v0.9 — the UI's "not yet executed" banner can't silently lie.
    #[test]
    #[allow(clippy::assertions_on_constants)] // pinning the const value IS the test
    fn layer5_execution_disabled() {
        assert!(!EXECUTION_ENABLED, "v0.9 ships declaration-without-execution");
    }

    /// Layer 1 (design addendum §3.2): hooks-core's Cargo.toml declares NO
    /// HTTP-client-exposing crate as a direct dependency. PR-reviewable at the
    /// manifest level; pinned here so a future contributor adding `reqwest`
    /// (etc.) to the firewalled crate breaks the build.
    #[test]
    fn layer1_hooks_core_declares_no_http_client() {
        let manifest = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/libs/hooks-core/Cargo.toml"
        ))
        .expect("hooks-core Cargo.toml readable");
        // Only inspect the [dependencies] section (skip the prose in the
        // package description, which legitimately names these crates).
        let deps = manifest
            .split("[dependencies]")
            .nth(1)
            .expect("hooks-core has a [dependencies] section");
        for banned in [
            "reqwest", "hyper", "surf", "isahc", "ureq", "awc", "attohttpc",
            "http-client", "proto-blue", "kryphocron",
        ] {
            // A dependency declaration is `<name> = …` or `<name>.<key> = …`
            // at line start within the deps section.
            for line in deps.lines() {
                let t = line.trim_start();
                assert!(
                    !(t.starts_with(&format!("{} ", banned))
                        || t.starts_with(&format!("{}=", banned))
                        || t.starts_with(&format!("{}.", banned))),
                    "hooks-core must not declare HTTP-client/atproto dep `{}` (Layer 1)",
                    banned
                );
            }
        }
    }

    // --- DB integration -------------------------------------------------

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
                jwt_secret: "test-secret-key-aurora-integration-hooks".to_string(),
                repo_signing_key: "a".repeat(64), plc_rotation_key: "b".repeat(64),
                oauth: OAuthConfig { client_id: "http://localhost:3000/client-metadata.json".to_string(), redirect_uri: "http://localhost:3000/oauth/callback".to_string(), pds_url: "https://bsky.social".to_string() },
                jwt_sunset_date: "Sat, 31 Dec 2024 23:59:59 GMT".to_string(),
                oauth_migration_guide_url: "https://docs.atproto.com/guides/oauth-migration".to_string(),
            },
            identity: IdentityConfig { did_plc_url: "https://plc.directory".to_string(), service_handle_domains: vec![".localhost".to_string()], did_cache_stale_ttl: 3600, did_cache_max_ttl: 86400, recovery_did_key: None },
            email: None,
            invites: InviteConfig { required: false, interval: 604800, epoch: "2024-01-01T00:00:00Z".to_string() },
            rate_limit: RateLimitConfig { enabled: false, global_requests_per_minute: 3000, exempt_admin_assets: true, buckets_retention_days: 7 },
            logging: LoggingConfig { level: "info".to_string() },
            federation: FederationConfig { enabled: false, relay_urls: vec![], appview_url: None, firehose_enabled: false, crawl_enabled: false, public_url: Some("http://localhost:2583".to_string()), peer_pds: vec![] },
            validation_mode: crate::validation::ValidationMode::Required,
            distributed_state_mode: Default::default(), maintenance_pool: Default::default(), gc_sweep: Default::default(),
            bind_audit_orphan_marker: Default::default(), blob_metadata: Default::default(), entryway: None,
            lexicon: crate::config::LexiconConfig::default(), kryphocron: crate::config::KryphocronConfig::default(),
        };
        AppContext::new(config, std::sync::Arc::new(crate::api::registry::RouteRegistry::default())).await.unwrap()
    }

    async fn mk(ctx: &AppContext, name: &str, url: &str) -> Result<Hook, (u16, String)> {
        create_hook(ctx, "did:plc:super", name, url, &["account.created".to_string()], Some("d"), true, Some("r")).await
    }

    async fn audit_count(ctx: &AppContext, action: &str) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_chain_entry WHERE action = $1")
            .bind(action).fetch_one(&ctx.account_db).await.unwrap()
    }

    #[tokio::test]
    async fn create_list_and_audit() {
        let ctx = ctx().await;
        let h = mk(&ctx, "my-hook", "https://hooks.example.com/x#frag").await.unwrap();
        assert_eq!(h.url, "https://hooks.example.com/x", "fragment stripped + normalized");
        assert_eq!(list_hooks(&ctx, false).await.unwrap().len(), 1);
        assert_eq!(audit_count(&ctx, ACTION_CREATED).await, 1);
    }

    #[tokio::test]
    async fn url_validation_rejects_bad_urls() {
        let ctx = ctx().await;
        for bad in ["http://x.com", "https://user:pw@x.com", "https://127.0.0.1/x", "https://[::1]/x"] {
            let e = mk(&ctx, "h", bad).await.unwrap_err();
            assert_eq!(e.0, 400, "{} should 400", bad);
        }
    }

    #[tokio::test]
    async fn event_class_validation() {
        let ctx = ctx().await;
        // empty
        assert_eq!(create_hook(&ctx, "did:plc:super", "h", "https://x.com", &[], None, true, None).await.unwrap_err().0, 400);
        // unknown class
        assert_eq!(create_hook(&ctx, "did:plc:super", "h", "https://x.com", &["bogus".into()], None, true, None).await.unwrap_err().0, 400);
    }

    #[tokio::test]
    async fn cap_enforced() {
        let ctx = ctx().await;
        for i in 0..hooks_core::MAX_ACTIVE_HOOKS {
            sqlx::query(
                "INSERT INTO moderation_integration_hook (id, name, url, event_classes, enabled, created_at, created_by_did, last_modified_at, last_modified_by_did) \
                 VALUES ($1, 'n', 'https://x.com', '[\"account.created\"]', 1, 'now', 'op', 'now', 'op')")
                .bind(format!("seed-{}", i)).execute(&ctx.account_db).await.unwrap();
        }
        assert_eq!(mk(&ctx, "over", "https://x.com").await.unwrap_err().0, 400);
    }

    #[tokio::test]
    async fn edit_optimistic_concurrency_and_change_summary() {
        let ctx = ctx().await;
        let h = mk(&ctx, "orig", "https://a.example.com/x").await.unwrap();
        // Stale token → 409.
        assert_eq!(
            edit_hook(&ctx, "did:plc:super", &h.id, "STALE", "orig", "https://a.example.com/x", &["account.created".into()], None, true, None).await.unwrap_err().0,
            409
        );
        // Correct token → ok; change_summary captures name + event_classes diff.
        edit_hook(&ctx, "did:plc:super", &h.id, &h.last_modified_at, "renamed", "https://a.example.com/x",
            &["account.created".into(), "system.tier-changed".into()], None, false, None).await.unwrap();
        let payload: String = sqlx::query_scalar("SELECT payload FROM audit_chain_entry WHERE action=$1 ORDER BY sequence DESC LIMIT 1")
            .bind(ACTION_EDITED).fetch_one(&ctx.account_db).await.unwrap();
        assert!(payload.contains("\"field\":\"name\"") && payload.contains("\"before\":\"orig\"") && payload.contains("\"after\":\"renamed\""));
        assert!(payload.contains("\"field\":\"event_classes\"") && payload.contains("\"added\":[\"system.tier-changed\"]"));
        assert!(payload.contains("\"field\":\"enabled\""));
    }

    #[tokio::test]
    async fn delete_is_soft_and_excluded() {
        let ctx = ctx().await;
        let h = mk(&ctx, "h", "https://x.example.com/y").await.unwrap();
        delete_hook(&ctx, "did:plc:super", &h.id).await.unwrap();
        assert_eq!(list_hooks(&ctx, false).await.unwrap().len(), 0);
        assert_eq!(list_hooks(&ctx, true).await.unwrap().len(), 1);
        assert_eq!(audit_count(&ctx, ACTION_DELETED).await, 1);
    }

    #[tokio::test]
    async fn composite_state_shape() {
        let ctx = ctx().await;
        mk(&ctx, "h", "https://x.example.com/y").await.unwrap();
        let state = integration_hooks_state(&ctx).await.unwrap();
        assert_eq!(state["executionStatus"]["enabled"], false);
        assert_eq!(state["availableEventClasses"].as_array().unwrap().len(), 8);
        assert_eq!(state["hooks"].as_array().unwrap().len(), 1);
    }
}
