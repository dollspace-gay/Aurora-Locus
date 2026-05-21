//! Integration tests for `aurora-locus grant-admin` (v0.3 §5.4.4).
//!
//! Covers the eight branches enumerated in the design:
//! - Offline-check rejection when a PDS-liveness lock is held.
//! - Bootstrap on a fresh DB (genesis chain self-bootstrap).
//! - Post-bootstrap runtime grant (chain continuity).
//! - All four SELECT-before-INSERT outcomes (no-row, active-row,
//!   revoked-row, with and without `--force` where it matters).
//!
//! Each test sets up a fresh on-disk SQLite DB inside a tempdir so
//! the SQLite flock lockfile path resolves correctly. Tempdirs are
//! ephemeral — the data is gone at test exit.

mod common;

use aurora_locus::admin::roles::Role;
use aurora_locus::cli::admin::grant_admin;
use aurora_locus::config::*;
use aurora_locus::context::AppContext;
use aurora_locus::error::PdsError;
use aurora_locus::validation::ValidationMode;
use sqlx::Row;
use std::path::PathBuf;
use tempfile::TempDir;

/// Build a real on-disk SQLite-backed AppContext rooted at a fresh
/// tempdir. The tempdir is returned alongside so the test can hold
/// it (and the on-disk DB + lockfile) for the duration of the test.
async fn build_test_ctx() -> (AppContext, TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_path_buf();
    let db_path = dir_path.join("test.db");
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
        },
        storage: StorageConfig {
            data_directory: dir_path.clone(),
            account_db: db_path.clone(),
            sequencer_db: dir_path.join("sequencer.db"),
            did_cache_db: dir_path.join("did_cache.db"),
            actor_store_directory: dir_path.join("actors"),
            blobstore: BlobstoreConfig::Disk {
                location: dir_path.join("blobs"),
                tmp_location: dir_path.join("temp"),
            },
        },
        database: Default::default(),
        authentication: AuthConfig {
            jwt_secret: "test-secret-key-grant-admin-integration-32".to_string(),
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
        validation_mode: ValidationMode::Optimistic,
        distributed_state_mode: Default::default(),
        maintenance_pool: Default::default(),
        gc_sweep: Default::default(),
        blob_metadata: Default::default(),
        entryway: None,
    };
    let ctx = AppContext::new(
        config,
        std::sync::Arc::new(aurora_locus::api::registry::RouteRegistry::default()),
    )
    .await
    .expect("AppContext::new");
    (ctx, dir)
}

async fn count_active_rows_for(ctx: &AppContext, did: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM admin_roles WHERE did = $1 AND NOT revoked",
    )
    .bind(did)
    .fetch_one(&ctx.account_db)
    .await
    .unwrap()
}

async fn count_chain_entries(ctx: &AppContext) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM audit_chain_entry")
        .fetch_one(&ctx.account_db)
        .await
        .unwrap()
}

/// Insert a revoked row for the given DID directly via SQL, used to
/// stage the revoked-branch tests without relying on a CLI revoke
/// command (which doesn't exist in v0.3).
async fn insert_revoked_row(ctx: &AppContext, did: &str, role: &str) {
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO admin_roles (did, role, granted_by, granted_at, revoked, revoked_at, revoked_by) \
         VALUES ($1, $2, $3, $4, 1, $4, $3)",
    )
    .bind(did)
    .bind(role)
    .bind("test:setup")
    .bind(&now)
    .execute(&ctx.account_db)
    .await
    .expect("insert revoked row");
}

// --------------------------- TEST 1 ---------------------------

#[tokio::test]
async fn test_1_offline_check_rejects_when_pds_running() {
    let (ctx, _dir) = build_test_ctx().await;
    // Simulate "PDS running" by holding the same liveness lock the
    // serve subcommand would acquire.
    let _guard = common::lock_holder::hold(&ctx.config).await.unwrap();

    let result = grant_admin(
        &ctx,
        "did:plc:test1234".to_string(),
        "admin".to_string(),
        None,
        false,
    )
    .await;

    match result {
        Err(PdsError::Validation(msg)) => {
            assert!(
                msg.contains("PDS instance"),
                "error must surface PDS-running cause; got: {}",
                msg
            );
            assert!(
                msg.contains("Stop the PDS"),
                "error must include actionable resolution; got: {}",
                msg
            );
        }
        other => panic!("expected Validation error, got {:?}", other),
    }
    assert_eq!(count_active_rows_for(&ctx, "did:plc:test1234").await, 0);
    assert_eq!(count_chain_entries(&ctx).await, 0);
}

// --------------------------- TEST 2 ---------------------------

#[tokio::test]
async fn test_2_bootstrap_on_fresh_db() {
    let (ctx, _dir) = build_test_ctx().await;

    grant_admin(
        &ctx,
        "did:plc:test1234".to_string(),
        "admin".to_string(),
        None,
        false,
    )
    .await
    .expect("bootstrap grant");

    // Row exists, active.
    let row = sqlx::query("SELECT role, revoked, granted_by FROM admin_roles WHERE did = $1")
        .bind("did:plc:test1234")
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
    let role: String = row.get("role");
    assert_eq!(role, "admin");
    let revoked: i64 = row.get("revoked");
    assert_eq!(revoked, 0);
    let granted_by: String = row.get("granted_by");
    assert_eq!(granted_by, "cli:grant-admin");

    // Chain: exactly one entry, sequence=1, prev_hash=NULL,
    // actor_did=cli:grant-admin.
    let chain = sqlx::query(
        "SELECT sequence, previous_hash, actor_did FROM audit_chain_entry ORDER BY sequence",
    )
    .fetch_all(&ctx.account_db)
    .await
    .unwrap();
    assert_eq!(chain.len(), 1);
    let seq: i64 = chain[0].get("sequence");
    assert_eq!(seq, 1);
    let prev: Option<String> = chain[0].get("previous_hash");
    assert!(prev.is_none(), "genesis entry must have NULL previous_hash");
    let actor: String = chain[0].get("actor_did");
    assert_eq!(actor, "cli:grant-admin");

    // Chain-walk verification: 1 entry, healthy.
    use aurora_locus::admin::audit_chain::verify_chain_range;
    verify_chain_range(&ctx.account_db, 1, 1)
        .await
        .expect("chain healthy after bootstrap");
}

// --------------------------- TEST 3 ---------------------------

#[tokio::test]
async fn test_3_post_bootstrap_runtime_grant() {
    let (ctx, _dir) = build_test_ctx().await;
    grant_admin(
        &ctx,
        "did:plc:first".to_string(),
        "admin".to_string(),
        None,
        false,
    )
    .await
    .expect("first grant");

    grant_admin(
        &ctx,
        "did:plc:second".to_string(),
        "moderator".to_string(),
        Some("test note".to_string()),
        false,
    )
    .await
    .expect("second grant");

    // Two active rows.
    assert_eq!(count_active_rows_for(&ctx, "did:plc:first").await, 1);
    assert_eq!(count_active_rows_for(&ctx, "did:plc:second").await, 1);

    // Two chain entries, second's previous_hash equals first's
    // current_hash.
    let chain = sqlx::query(
        "SELECT sequence, current_hash, previous_hash FROM audit_chain_entry ORDER BY sequence",
    )
    .fetch_all(&ctx.account_db)
    .await
    .unwrap();
    assert_eq!(chain.len(), 2);
    let seq2: i64 = chain[1].get("sequence");
    assert_eq!(seq2, 2);
    let first_current: String = chain[0].get("current_hash");
    let second_prev: Option<String> = chain[1].get("previous_hash");
    assert_eq!(second_prev.as_deref(), Some(first_current.as_str()));

    use aurora_locus::admin::audit_chain::verify_chain_range;
    verify_chain_range(&ctx.account_db, 1, 2)
        .await
        .expect("chain healthy after second grant");
}

// --------------------------- TEST 4 ---------------------------

#[tokio::test]
async fn test_4_no_row_plus_force_is_a_noop_flag() {
    let (ctx, _dir) = build_test_ctx().await;

    grant_admin(
        &ctx,
        "did:plc:test1234".to_string(),
        "admin".to_string(),
        None,
        true, // --force on a no-row case is irrelevant.
    )
    .await
    .expect("force on no-row should still succeed");

    assert_eq!(count_active_rows_for(&ctx, "did:plc:test1234").await, 1);
    assert_eq!(count_chain_entries(&ctx).await, 1);
}

// --------------------------- TEST 5 ---------------------------

#[tokio::test]
async fn test_5_active_row_plus_no_force_errors() {
    let (ctx, _dir) = build_test_ctx().await;
    grant_admin(
        &ctx,
        "did:plc:test1234".to_string(),
        "admin".to_string(),
        None,
        false,
    )
    .await
    .expect("first grant");
    let entries_before = count_chain_entries(&ctx).await;

    let result = grant_admin(
        &ctx,
        "did:plc:test1234".to_string(),
        "admin".to_string(),
        None,
        false,
    )
    .await;

    match result {
        Err(PdsError::Validation(msg)) => {
            assert!(
                msg.contains("already has an active role"),
                "error must mention active role; got: {}",
                msg
            );
        }
        other => panic!("expected Validation error, got {:?}", other),
    }
    // No new row, no new chain entry.
    assert_eq!(count_active_rows_for(&ctx, "did:plc:test1234").await, 1);
    assert_eq!(count_chain_entries(&ctx).await, entries_before);
}

// --------------------------- TEST 6 ---------------------------

#[tokio::test]
async fn test_6_active_row_plus_force_still_errors() {
    let (ctx, _dir) = build_test_ctx().await;
    grant_admin(
        &ctx,
        "did:plc:test1234".to_string(),
        "admin".to_string(),
        None,
        false,
    )
    .await
    .expect("first grant");
    let entries_before = count_chain_entries(&ctx).await;

    let result = grant_admin(
        &ctx,
        "did:plc:test1234".to_string(),
        "superadmin".to_string(),
        None,
        true, // --force does NOT bypass active rows.
    )
    .await;

    match result {
        Err(PdsError::Validation(msg)) => {
            assert!(
                msg.contains("active role"),
                "error must mention active role; got: {}",
                msg
            );
            assert!(
                msg.contains("--force does not bypass active rows"),
                "error must explicitly call out that --force is not the answer; got: {}",
                msg
            );
        }
        other => panic!("expected Validation error, got {:?}", other),
    }
    // Role unchanged (still 'admin', not 'superadmin'); no new
    // chain entry.
    let role: String = sqlx::query_scalar("SELECT role FROM admin_roles WHERE did = $1")
        .bind("did:plc:test1234")
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
    assert_eq!(role, "admin");
    assert_eq!(count_chain_entries(&ctx).await, entries_before);
}

// --------------------------- TEST 7 ---------------------------

#[tokio::test]
async fn test_7_revoked_row_plus_no_force_errors_with_actionable_message() {
    let (ctx, _dir) = build_test_ctx().await;
    insert_revoked_row(&ctx, "did:plc:test1234", "admin").await;
    let entries_before = count_chain_entries(&ctx).await;

    let result = grant_admin(
        &ctx,
        "did:plc:test1234".to_string(),
        "admin".to_string(),
        None,
        false,
    )
    .await;

    match result {
        Err(PdsError::Validation(msg)) => {
            assert!(
                msg.contains("revoked role"),
                "error must mention revoked role; got: {}",
                msg
            );
            assert!(
                msg.contains("--force"),
                "error must point operator at --force; got: {}",
                msg
            );
        }
        other => panic!("expected Validation error, got {:?}", other),
    }

    // Row still revoked, no new chain entry.
    let revoked: i64 = sqlx::query_scalar("SELECT revoked FROM admin_roles WHERE did = $1")
        .bind("did:plc:test1234")
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
    assert_eq!(revoked, 1);
    assert_eq!(count_chain_entries(&ctx).await, entries_before);
}

// --------------------------- TEST 8 ---------------------------

#[tokio::test]
async fn test_8_revoked_row_plus_force_succeeds_with_fresh_chain_entry() {
    let (ctx, _dir) = build_test_ctx().await;
    insert_revoked_row(&ctx, "did:plc:test1234", "moderator").await;
    let entries_before = count_chain_entries(&ctx).await;

    grant_admin(
        &ctx,
        "did:plc:test1234".to_string(),
        "admin".to_string(), // re-grant as a different role.
        Some("re-grant".to_string()),
        true,
    )
    .await
    .expect("re-grant with --force should succeed");

    // Row is now active and carries the new role + new granted_by
    // sentinel.
    let row = sqlx::query(
        "SELECT role, revoked, granted_by, revoked_at FROM admin_roles WHERE did = $1",
    )
    .bind("did:plc:test1234")
    .fetch_one(&ctx.account_db)
    .await
    .unwrap();
    let role: String = row.get("role");
    assert_eq!(role, "admin");
    let revoked: i64 = row.get("revoked");
    assert_eq!(revoked, 0);
    let granted_by: String = row.get("granted_by");
    assert_eq!(granted_by, "cli:grant-admin");
    let revoked_at: Option<String> = row.get("revoked_at");
    assert!(revoked_at.is_none(), "revoked_at must be cleared on re-grant");

    // New chain entry (the re-grant is itself an auditable event).
    assert_eq!(count_chain_entries(&ctx).await, entries_before + 1);
}
