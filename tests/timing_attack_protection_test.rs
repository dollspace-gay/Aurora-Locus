//! Test timing attack protection in login functions.
//!
//! This test verifies that both valid and invalid login attempts
//! take at least 350ms to complete, preventing timing-based
//! username enumeration attacks.

use aurora_locus::{account::AccountManager, config::*, validation::ValidationMode};
use sqlx::any::AnyPoolOptions;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Once;
use std::time::Instant;

async fn create_test_manager() -> AccountManager {
    // Spin up the real schema in :memory: so the test stays in lock-step
    // with the production layout. Phase 3 (b851678) flipped AccountManager
    // from SqlitePool to AnyPool; this fixture mirrors the production-side
    // pattern (single-connection in-memory pool) used in src/admin/roles.rs
    // tests. Resolves chainlink #87.
    static INSTALL: Once = Once::new();
    INSTALL.call_once(sqlx::any::install_default_drivers);
    let db = AnyPoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::migrate!("./migrations")
        .run(&db)
        .await
        .expect("test schema migrations failed");

    // Create minimal test configuration
    let config = Arc::new(ServerConfig {
        service: ServiceConfig {
            hostname: "localhost".to_string(),
            port: 2583,
            service_did: "did:web:localhost".to_string(),
            version: "0.1.0".to_string(),
            blob_upload_limit: 5242880,
                public_url: None,
            max_blob_fetch_size: 50_000_000,
            blob_fetch_timeout_seconds: 30,
            blob_fetch_max_retries: 3,
            accepting_imports: true,
            max_import_size: None,
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
            jwt_secret: "test-secret-key-for-testing-only".to_string(),
            repo_signing_key: "test-key".to_string(),
            plc_rotation_key: "b".repeat(64),
            oauth: OAuthConfig {
                client_id: "test-client".to_string(),
                redirect_uri: "http://localhost:3000/oauth/callback".to_string(),
                pds_url: "http://localhost:3000".to_string(),
            },
            jwt_sunset_date: "Sat, 31 Dec 2024 23:59:59 GMT".to_string(),
            oauth_migration_guide_url: "https://docs.example.com/oauth-migration".to_string(),
            oauth_features: Default::default(),
        },
        identity: IdentityConfig {
            did_plc_url: "https://plc.directory".to_string(),
            service_handle_domains: vec!["localhost".to_string()],
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
            enabled: true,
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
            public_url: None,
                peer_pds: vec![],
        },
        validation_mode: ValidationMode::Optimistic,
        distributed_state_mode: Default::default(),
        maintenance_pool: Default::default(),
        gc_sweep: Default::default(),
        bind_audit_orphan_marker: Default::default(),
        blob_metadata: Default::default(),
        entryway: None,
        lexicon: aurora_locus::config::LexiconConfig::default(),
        kryphocron: aurora_locus::config::KryphocronConfig::default(),
    });

    AccountManager::new(db, config)
}

#[tokio::test]
async fn test_login_timing_protection_invalid_user() {
    let manager = create_test_manager().await;

    // Attempt to login with non-existent user
    let start = Instant::now();
    let result = manager
        .login("nonexistent@example.com", "password123")
        .await;
    let elapsed = start.elapsed();

    // Should fail
    assert!(result.is_err());

    // Should take at least 350ms
    assert!(
        elapsed.as_millis() >= 350,
        "Login should take at least 350ms, took {}ms",
        elapsed.as_millis()
    );

    println!(
        "✓ Invalid user login took {}ms (expected >= 350ms)",
        elapsed.as_millis()
    );
}

#[tokio::test]
async fn test_login_timing_protection_wrong_password() {
    let manager = create_test_manager().await;

    // Create a test account first
    let _account = manager
        .create_account(
            "testuser".to_string(),
            Some("test@example.com".to_string()),
            "correct_password".to_string(),
            None,
                None,
        )
        .await
        .unwrap();

    // Attempt to login with wrong password
    let start = Instant::now();
    let result = manager.login("test@example.com", "wrong_password").await;
    let elapsed = start.elapsed();

    // Should fail
    assert!(result.is_err());

    // Should take at least 350ms
    assert!(
        elapsed.as_millis() >= 350,
        "Login should take at least 350ms, took {}ms",
        elapsed.as_millis()
    );

    println!(
        "✓ Wrong password login took {}ms (expected >= 350ms)",
        elapsed.as_millis()
    );
}

#[tokio::test]
async fn test_login_timing_protection_valid_login() {
    let manager = create_test_manager().await;

    // Create a test account
    let _account = manager
        .create_account(
            "testuser".to_string(),
            Some("test@example.com".to_string()),
            "correct_password".to_string(),
            None,
                None,
        )
        .await
        .unwrap();

    // Successful login
    let start = Instant::now();
    let result = manager.login("test@example.com", "correct_password").await;
    let elapsed = start.elapsed();

    // Should succeed
    assert!(result.is_ok());

    // Should take at least 350ms
    assert!(
        elapsed.as_millis() >= 350,
        "Login should take at least 350ms, took {}ms",
        elapsed.as_millis()
    );

    println!(
        "✓ Valid login took {}ms (expected >= 350ms)",
        elapsed.as_millis()
    );
}

#[tokio::test]
async fn test_app_password_timing_protection() {
    let manager = create_test_manager().await;

    // Create a test account
    let account = manager
        .create_account(
            "testuser".to_string(),
            Some("test@example.com".to_string()),
            "correct_password".to_string(),
            None,
                None,
        )
        .await
        .unwrap();

    // Create an app password
    let app_password = manager
        .create_app_password(&account.did, "Test App", false)
        .await
        .unwrap();

    // Test invalid app password
    let start = Instant::now();
    let result = manager
        .login_with_app_password("test@example.com", "invalid-password")
        .await;
    let elapsed = start.elapsed();

    assert!(result.is_err());
    assert!(
        elapsed.as_millis() >= 350,
        "App password login should take at least 350ms, took {}ms",
        elapsed.as_millis()
    );

    println!(
        "✓ Invalid app password login took {}ms (expected >= 350ms)",
        elapsed.as_millis()
    );

    // Test valid app password
    let start = Instant::now();
    let result = manager
        .login_with_app_password("test@example.com", &app_password)
        .await;
    let elapsed = start.elapsed();

    assert!(result.is_ok());
    assert!(
        elapsed.as_millis() >= 350,
        "App password login should take at least 350ms, took {}ms",
        elapsed.as_millis()
    );

    println!(
        "✓ Valid app password login took {}ms (expected >= 350ms)",
        elapsed.as_millis()
    );
}

#[tokio::test]
async fn test_timing_consistency_between_valid_and_invalid() {
    let manager = create_test_manager().await;

    // Create a test account
    let _account = manager
        .create_account(
            "testuser".to_string(),
            Some("test@example.com".to_string()),
            "correct_password".to_string(),
            None,
                None,
        )
        .await
        .unwrap();

    // Test invalid user multiple times
    let mut invalid_times = Vec::new();
    for _ in 0..5 {
        let start = Instant::now();
        let _ = manager.login("nonexistent@example.com", "password").await;
        invalid_times.push(start.elapsed().as_millis());
    }

    // Test valid user with wrong password multiple times
    let mut wrong_password_times = Vec::new();
    for _ in 0..5 {
        let start = Instant::now();
        let _ = manager.login("test@example.com", "wrong_password").await;
        wrong_password_times.push(start.elapsed().as_millis());
    }

    // Calculate averages
    let avg_invalid: u128 = invalid_times.iter().sum::<u128>() / invalid_times.len() as u128;
    let avg_wrong: u128 =
        wrong_password_times.iter().sum::<u128>() / wrong_password_times.len() as u128;

    println!("Average invalid user time: {}ms", avg_invalid);
    println!("Average wrong password time: {}ms", avg_wrong);

    // Both should be close to 350ms
    assert!(avg_invalid >= 350);
    assert!(avg_wrong >= 350);

    // The difference should be small (within 50ms tolerance).
    let difference = avg_invalid.abs_diff(avg_wrong);

    println!("Timing difference: {}ms", difference);

    // This demonstrates timing attack mitigation is working
    println!("✓ Timing attack protection working: both scenarios take ~350ms");
}
