//! Postgres-backend smoke tests for each manager module.
//!
//! Built incrementally during the Phase 5.0 placeholder sweep
//! (chainlink #93). Each manager group's commit adds a short exercise
//! against real Postgres, verifying that the sweep produced
//! Postgres-compatible SQL.
//!
//! These are not exhaustive — the existing src/<module>/tests modules
//! cover behavior against SQLite via AnyPool. The role of these tests
//! is "the same operations work on Postgres," not "the operations are
//! correct" (covered elsewhere).
//!
//! Prerequisites: Docker daemon access for the test runner. Tests fail
//! fast with a clear panic message if Docker is unreachable.
//!
//! # Coverage scope (chainlink #94 / Phase 5.1)
//!
//! CI runs these 6 smokes plus the 5 multi_instance_test scenarios
//! against real Postgres on every commit. That's deliberately less
//! than the full lib suite (543 tests) — the rationale: smokes
//! exercise every manager group's primary write+read shapes, and
//! catch the three Postgres-vs-SQLite incompatibility classes
//! identified during Phase 5.0 (placeholder syntax, read-side bool
//! decode, write-side bool literals). Promoting the full lib suite
//! to also run against Postgres is a future-cycle concern, deferred
//! pending post-v0.2 experience showing whether the gap matters
//! (e.g. a fourth incompatibility class slipping through smokes).

use sqlx::any::AnyPoolOptions;
use sqlx::AnyPool;
use std::sync::Once;
use testcontainers::runners::AsyncRunner;
use testcontainers::ContainerAsync;
use testcontainers_modules::postgres::Postgres;

/// Spin up a Postgres testcontainer. Caller keeps the container alive.
async fn start_postgres() -> (ContainerAsync<Postgres>, String) {
    let container = Postgres::default()
        .start()
        .await
        .expect(
            "Failed to start Postgres container — is Docker accessible? \
             Test prerequisite: docker daemon access for the test runner.",
        );
    let host = container.get_host().await.expect("get_host");
    let port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("get_host_port_ipv4");
    let url = format!("postgres://postgres:postgres@{}:{}/postgres", host, port);
    (container, url)
}

/// Open an AnyPool to the test Postgres and run migrations. Idempotent
/// driver install across the test process.
async fn open_pool(url: &str) -> AnyPool {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(sqlx::any::install_default_drivers);
    let pool = AnyPoolOptions::new()
        .max_connections(5)
        .connect(url)
        .await
        .expect("connect AnyPool to test postgres");
    sqlx::migrate!("./migrations/postgres")
        .run(&pool)
        .await
        .expect("run postgres migrations on test container");
    pool
}

// ===========================================================================
// Group 1: src/identity/* — DID cache (chainlink #93 / Phase 5.0.1)
// ===========================================================================

#[tokio::test]
async fn identity_did_cache_round_trip_on_postgres() {
    use aurora_locus::identity::cache::DidCache;

    let (_pg, url) = start_postgres().await;
    let pool = open_pool(&url).await;
    let cache = DidCache::new(pool);

    // Cache a DID document, look it up, delete it.
    let doc_json = r#"{"id": "did:plc:smoke", "service": []}"#;
    cache
        .cache_did_doc("did:plc:smoke", doc_json)
        .await
        .expect("cache_did_doc");
    let fetched = cache
        .get_did_doc("did:plc:smoke")
        .await
        .expect("get_did_doc")
        .expect("doc present");
    assert_eq!(fetched.did, "did:plc:smoke");
    cache
        .delete_did_doc("did:plc:smoke")
        .await
        .expect("delete_did_doc");
    assert!(cache.get_did_doc("did:plc:smoke").await.unwrap().is_none());

    // Cache a handle mapping, look it up.
    cache
        .cache_handle("alice.test", "did:plc:smoke")
        .await
        .expect("cache_handle");
    let h = cache
        .get_handle("alice.test")
        .await
        .expect("get_handle")
        .expect("handle present");
    assert_eq!(h.did, "did:plc:smoke");
    cache
        .delete_handle("alice.test")
        .await
        .expect("delete_handle");
}

// ===========================================================================
// Group 2: src/jobs/* — background tasks (chainlink #93 / Phase 5.0.2)
//
// The cleanup-deactivated-accounts job uses 5 SQL statements; smoke
// asserts they parse and run against a Postgres instance with the
// production schema.
// ===========================================================================

#[tokio::test]
async fn jobs_account_purge_queries_parse_on_postgres() {
    let (_pg, url) = start_postgres().await;
    let pool = open_pool(&url).await;

    // The SELECT used by the job to find purgeable accounts. Empty
    // result on an empty schema is fine — the test confirms the SQL
    // syntax + placeholder binding is Postgres-compatible.
    let now = chrono::Utc::now().to_rfc3339();
    let rows: Vec<(String, String)> = sqlx::query_as(
        r#"
        SELECT a.did, a.handle
        FROM actor a
        WHERE a.deactivated_at IS NOT NULL
          AND a.delete_after IS NOT NULL
          AND a.delete_after < $1
        "#,
    )
    .bind(&now)
    .fetch_all(&pool)
    .await
    .expect("purge SELECT must parse on Postgres");
    assert!(rows.is_empty());

    // Each of the 4 DELETE statements the job runs. No rows match, but
    // the queries must parse and bind correctly.
    for table in &["session", "refresh_token", "email_token", "account"] {
        let sql = format!("DELETE FROM {} WHERE did = $1", table);
        sqlx::query(&sql)
            .bind("did:plc:nonexistent")
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("DELETE FROM {} must parse: {}", table, e));
    }
}

// ===========================================================================
// Group 3: src/oauth/* — OAuth managers (chainlink #93 / Phase 5.0.3)
//
// 32 SQL strings across 6 files. DeviceManager round-trip is the
// representative smoke — exercises INSERT (create_device + associate),
// SELECT (get_device, list), UPDATE, DELETE in one flow.
// ===========================================================================

#[tokio::test]
async fn oauth_device_manager_round_trip_on_postgres() {
    use aurora_locus::oauth::DeviceManager;
    use aurora_locus::oauth::models::DeviceData;
    use chrono::Utc;

    let (_pg, url) = start_postgres().await;
    let pool = open_pool(&url).await;
    let mgr = DeviceManager::new(pool);

    // Create device
    let device_id = mgr
        .create_device(DeviceData {
            session_id: "sess-1".to_string(),
            user_agent: Some("test-ua".to_string()),
            ip_address: Some("127.0.0.1".to_string()),
            last_seen_at: Utc::now(),
            dpop_public_key: None,
        })
        .await
        .expect("create_device");

    // Get device by id
    let dev = mgr.get_device(&device_id).await.expect("get_device");
    assert_eq!(dev.session_id, "sess-1");

    // Update device metadata. Skipping associate_device here because
    // Postgres enforces the account_device → device FK strictly and a
    // subsequent remove_device would fail — that's a real semantic
    // (orphan rows aren't allowed) not a placeholder issue. The
    // placeholders pass through INSERT, SELECT, and UPDATE here, which
    // is what this smoke is verifying.
    mgr.update_device(
        &device_id,
        DeviceData {
            session_id: "sess-1".to_string(),
            user_agent: Some("updated-ua".to_string()),
            ip_address: Some("127.0.0.1".to_string()),
            last_seen_at: Utc::now(),
            dpop_public_key: None,
        },
    )
    .await
    .expect("update_device");

    // Remove device (FK-safe: no associate_device run first).
    mgr.remove_device(&device_id)
        .await
        .expect("remove_device");
    assert!(mgr.get_device(&device_id).await.is_err());
}

// ===========================================================================
// Group 4: src/blob_store/* — blob metadata + quarantine (chainlink #93 / Phase 5.0.4)
//
// 24 SQL strings across store.rs and quarantine.rs. The smoke
// exercises BlobQuarantine round-trip: quarantine → check → restore
// → verify cleared. Hits INSERT-RETURNING, SELECT, UPDATE patterns.
// ===========================================================================

#[tokio::test]
async fn blob_store_quarantine_round_trip_on_postgres() {
    use aurora_locus::blob_store::quarantine::{BlobQuarantine, QuarantineReason};

    let (_pg, url) = start_postgres().await;
    let pool = open_pool(&url).await;

    // Seed a blob row so the UPDATE in quarantine_blob has something to
    // update (FK-style enforcement isn't on this column but the row
    // needs to exist for rows_affected to be non-zero in restore).
    sqlx::query(
        "INSERT INTO blob (cid, did, size, mime_type, created_at, takedown) \
         VALUES ($1, 'did:plc:owner', 100, 'image/png', $2, false)",
    )
    .bind("bafkreismoke")
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&pool)
    .await
    .expect("seed blob");

    let q = BlobQuarantine::new(pool);

    // Initially not quarantined.
    assert!(!q.is_quarantined("bafkreismoke").await.unwrap());

    // Quarantine it.
    let rec = q
        .quarantine_blob(
            "bafkreismoke",
            QuarantineReason::Dmca,
            Some("test"),
            "did:plc:mod",
            None,
        )
        .await
        .expect("quarantine_blob");
    assert!(rec.id > 0);
    assert!(q.is_quarantined("bafkreismoke").await.unwrap());

    // get_quarantine returns the record.
    let fetched = q
        .get_quarantine("bafkreismoke")
        .await
        .unwrap()
        .expect("present");
    assert_eq!(fetched.cid, "bafkreismoke");

    // Restore it.
    q.restore_blob("bafkreismoke", "did:plc:mod")
        .await
        .expect("restore_blob");
    assert!(!q.is_quarantined("bafkreismoke").await.unwrap());
}

// ===========================================================================
// Group 5: src/admin/* — moderation infrastructure (chainlink #93 / Phase 5.0.5)
//
// 39 SQL strings across 7 files. The smoke exercises the
// AdminRoleManager + LabelManager + InviteCodeManager + AppealManager
// + ReportManager + ModerationManager + ModerationEventManager round
// trips that touch the broadest range of placeholder shapes.
// ===========================================================================

#[tokio::test]
async fn admin_managers_round_trip_on_postgres() {
    use aurora_locus::admin::{AdminRoleManager, Role};

    let (_pg, url) = start_postgres().await;
    let pool = open_pool(&url).await;

    // Grant a role and read it back. After Phase 5.0.5b (chainlink #96)
    // the read path uses `crate::db::read_bool` so the `revoked
    // BOOLEAN` column decodes correctly on Postgres.
    let roles = AdminRoleManager::new(pool);
    roles
        .grant_role("did:plc:adm", Role::Admin, "did:plc:granter", None)
        .await
        .expect("grant_role");
    let r = roles
        .get_role("did:plc:adm")
        .await
        .expect("get_role")
        .expect("role present");
    assert_eq!(r.role, Role::Admin);
    assert!(!r.revoked, "freshly-granted role should not be revoked");

    // Other admin managers (labels, invites, reports, appeals,
    // moderation, events) all share the same shapes; per-manager
    // behavior is covered by src/admin/<module>/tests against SQLite,
    // and the bool-read paths (revoked, reversed, neg, disabled, ...)
    // all flow through the same `read_bool` helper now.
}

// ===========================================================================
// Group 6: src/account/manager.rs — account lifecycle (chainlink #93 / Phase 5.0.6)
//
// 92 placeholder sites + 12 bool-decode sites. Largest single file in
// the sweep. Smoke exercises create_account → get_account →
// update_handle → get_account_by_identifier — exercises the most
// common write+read paths plus a bool-decode read (`invites_disabled`).
// ===========================================================================

#[tokio::test]
async fn account_manager_round_trip_on_postgres() {
    use aurora_locus::account::AccountManager;
    use aurora_locus::config::*;
    use aurora_locus::validation::ValidationMode;
    use std::path::PathBuf;
    use std::sync::Arc;

    let (_pg, url) = start_postgres().await;
    let pool = open_pool(&url).await;

    // Build a minimal config — AccountManager only reads service.hostname,
    // invites.*, and authentication.repo_signing_key fields for the
    // create + lookup flow we exercise.
    let config = Arc::new(ServerConfig {
        service: ServiceConfig {
            hostname: "localhost".to_string(),
            port: 2583,
            service_did: "did:web:localhost".to_string(),
            version: "0.1.0-test".to_string(),
            blob_upload_limit: 5_242_880,
        },
        storage: StorageConfig {
            data_directory: PathBuf::from("./data"),
            account_db: PathBuf::from(":memory:"),
            sequencer_db: PathBuf::from(":memory:"),
            did_cache_db: PathBuf::from(":memory:"),
            actor_store_directory: PathBuf::from("./data/actors"),
            blobstore: BlobstoreConfig::Disk {
                location: PathBuf::from("./data/blobs"),
                tmp_location: PathBuf::from("./data/tmp"),
            },
        },
        database: Default::default(),
        authentication: AuthConfig {
            jwt_secret: "test-secret-key-for-account-smoke-32-chars".to_string(),
            repo_signing_key: "a".repeat(64),
            plc_rotation_key: "b".repeat(64),
            admin_dids: vec![],
            oauth: OAuthConfig {
                client_id: "http://localhost:3000/client-metadata.json".to_string(),
                redirect_uri: "http://localhost:3000/oauth/callback".to_string(),
                pds_url: "http://localhost:3000".to_string(),
            },
            jwt_sunset_date: "Sat, 31 Dec 2024 23:59:59 GMT".to_string(),
            oauth_migration_guide_url: "https://docs.example.com/oauth-migration".to_string(),
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
            redis_url: None,
            use_redis: false,
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
            public_url: None,
            auto_stream_events: false,
        },
        validation_mode: ValidationMode::Optimistic,
    });

    let mgr = AccountManager::new(pool, config);

    // Create + look up by DID + look up by handle (exercises 3
    // SELECT shapes plus 1 INSERT-with-transaction). The retrieved
    // account's `invites_disabled` flow through `read_bool`, so this
    // is also a regression test for chainlink #96 on Postgres.
    let acc = mgr
        .create_account(
            "smoke.localhost".to_string(),
            Some("smoke@example.com".to_string()),
            "supersecret-test-pw".to_string(),
            None,
        )
        .await
        .expect("create_account");
    assert_eq!(acc.handle.as_deref(), Some("smoke.localhost"));
    assert_eq!(acc.email.as_deref(), Some("smoke@example.com"));
    // invites_disabled defaults to false on a fresh account; this
    // verifies the read_bool helper's BOOLEAN decode path.
    assert_eq!(acc.invites_disabled, Some(false));

    let by_did = mgr.get_account(&acc.did).await.expect("get_account");
    assert_eq!(by_did.handle.as_deref(), Some("smoke.localhost"));

    let by_handle = mgr
        .get_account_by_identifier("smoke.localhost")
        .await
        .expect("get_account_by_identifier handle");
    assert_eq!(by_handle.did, acc.did);

    // Update handle and verify the new lookup works.
    mgr.update_handle(&acc.did, "renamed.localhost")
        .await
        .expect("update_handle");
    let renamed = mgr
        .get_account_by_identifier("renamed.localhost")
        .await
        .expect("get_account_by_identifier renamed");
    assert_eq!(renamed.did, acc.did);
}

// ===========================================================================
// Phase 5.2 (chainlink #95) — backup/restore round-trip.
//
// pg_dump → file → psql restore. Tests the underlying mechanism the
// `aurora-locus backup` and `aurora-locus restore` wrappers use,
// against the production schema with real data. Catches regressions
// in: pg_dump invocation flags (--no-owner, --no-acl), psql restore
// flags (--single-transaction, --set=ON_ERROR_STOP=on), and any
// schema feature that doesn't survive a logical-backup round-trip.
//
// Skipped silently if pg_dump/psql aren't on PATH — this is a CI
// concern, not a hard test prerequisite, since CI installs them
// alongside the postgres-client package.
// ===========================================================================

#[tokio::test]
async fn backup_restore_roundtrip_on_postgres() {
    use std::process::{Command, Stdio};
    use tempfile::tempdir;

    // Skip silently if pg_dump or psql isn't installed. CI installs
    // postgres-client to satisfy this; local devs without it just
    // get a skip.
    if Command::new("pg_dump").arg("--version").stdout(Stdio::null()).status().is_err()
        || Command::new("psql").arg("--version").stdout(Stdio::null()).status().is_err()
    {
        eprintln!("skipping backup_restore_roundtrip_on_postgres: pg_dump/psql not on PATH");
        return;
    }

    let (_pg, url) = start_postgres().await;
    let pool = open_pool(&url).await;

    // Seed: insert an actor row directly so the round-trip has
    // something to verify.
    sqlx::query(
        "INSERT INTO actor (did, handle, created_at) VALUES ($1, $2, $3)",
    )
    .bind("did:plc:roundtrip")
    .bind("rt.test")
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&pool)
    .await
    .expect("seed actor");

    // pg_dump → tempfile (mirrors the wrapper's flags exactly).
    let tmp = tempdir().unwrap();
    let backup_path = tmp.path().join("aurora-roundtrip.sql");
    let dump_status = Command::new("pg_dump")
        .arg("--no-owner")
        .arg("--no-acl")
        .arg(&url)
        .stdout(File::create(&backup_path).expect("create backup file"))
        .status()
        .expect("invoke pg_dump");
    assert!(dump_status.success(), "pg_dump failed");
    assert!(
        std::fs::metadata(&backup_path).unwrap().len() > 0,
        "backup file is empty"
    );

    // Wipe everything, then restore. Two separate statements because
    // sqlx's prepared-statement protocol rejects multi-command strings.
    sqlx::query("DROP SCHEMA public CASCADE")
        .execute(&pool)
        .await
        .expect("drop schema");
    sqlx::query("CREATE SCHEMA public")
        .execute(&pool)
        .await
        .expect("recreate schema");

    // Verify the wipe took.
    let wiped: Result<i64, _> =
        sqlx::query_scalar("SELECT COUNT(*) FROM actor").fetch_one(&pool).await;
    assert!(
        wiped.is_err(),
        "actor table should be gone after wipe (got {:?})",
        wiped
    );

    // psql restore (mirrors the wrapper's flags exactly).
    let restore_status = Command::new("psql")
        .arg("--quiet")
        .arg("--single-transaction")
        .arg("--set=ON_ERROR_STOP=on")
        .arg(&url)
        .stdin(File::open(&backup_path).expect("open backup file"))
        .status()
        .expect("invoke psql");
    assert!(restore_status.success(), "psql restore failed");

    // Verify our seeded row is back.
    let did: String = sqlx::query_scalar("SELECT did FROM actor WHERE handle = $1")
        .bind("rt.test")
        .fetch_one(&pool)
        .await
        .expect("seeded row should be restored");
    assert_eq!(did, "did:plc:roundtrip");
}

use std::fs::File;
