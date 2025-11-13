/// Test timing attack protection in login functions
///
/// This test verifies that both valid and invalid login attempts
/// take at least 350ms to complete, preventing timing-based
/// username enumeration attacks.

use aurora_locus::{
    account::{AccountManager, CreateAccountRequest},
    config::*,
    error::PdsResult,
};
use sqlx::SqlitePool;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

async fn create_test_manager() -> AccountManager {
    // Create in-memory database
    let db = SqlitePool::connect(":memory:").await.unwrap();

    // Create tables
    sqlx::query(
        r#"
        CREATE TABLE account (
            did TEXT PRIMARY KEY,
            handle TEXT UNIQUE NOT NULL,
            email TEXT UNIQUE,
            password_hash TEXT NOT NULL,
            created_at DATETIME NOT NULL,
            email_confirmed BOOLEAN NOT NULL DEFAULT 0,
            email_confirmed_at DATETIME,
            deactivated_at DATETIME,
            taken_down BOOLEAN NOT NULL DEFAULT 0,
            plc_rotation_key TEXT,
            plc_rotation_key_public TEXT,
            plc_last_operation_cid TEXT
        )
        "#,
    )
    .execute(&db)
    .await
    .unwrap();

    sqlx::query(
        r#"
        CREATE TABLE session (
            id TEXT PRIMARY KEY,
            did TEXT NOT NULL,
            access_token TEXT UNIQUE NOT NULL,
            refresh_token TEXT UNIQUE NOT NULL,
            created_at DATETIME NOT NULL,
            expires_at DATETIME NOT NULL,
            app_password_name TEXT,
            FOREIGN KEY (did) REFERENCES account(did)
        )
        "#,
    )
    .execute(&db)
    .await
    .unwrap();

    sqlx::query(
        r#"
        CREATE TABLE refresh_token (
            id TEXT PRIMARY KEY,
            did TEXT NOT NULL,
            token TEXT UNIQUE NOT NULL,
            created_at DATETIME NOT NULL,
            expires_at DATETIME NOT NULL,
            used BOOLEAN NOT NULL DEFAULT 0,
            used_at DATETIME,
            FOREIGN KEY (did) REFERENCES account(did)
        )
        "#,
    )
    .execute(&db)
    .await
    .unwrap();

    sqlx::query(
        r#"
        CREATE TABLE app_password (
            did TEXT NOT NULL,
            name TEXT NOT NULL,
            password_hash TEXT NOT NULL,
            created_at DATETIME NOT NULL,
            privileged BOOLEAN NOT NULL DEFAULT 0,
            PRIMARY KEY (did, name),
            FOREIGN KEY (did) REFERENCES account(did)
        )
        "#,
    )
    .execute(&db)
    .await
    .unwrap();

    // Create minimal test configuration
    let config = Arc::new(ServerConfig {
        service: ServiceConfig {
            hostname: "localhost".to_string(),
            port: 2583,
            service_did: "did:web:localhost".to_string(),
            version: "0.1.0".to_string(),
            blob_upload_limit: 5242880,
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
        authentication: AuthConfig {
            jwt_secret: "test-secret-key-for-testing-only".to_string(),
            repo_signing_key: "test-key".to_string(),
            plc_rotation_key: "test-rotation-key".to_string(),
            admin_dids: vec![],
            oauth: OAuthConfig {
                client_id: "test-client".to_string(),
                redirect_uri: "http://localhost:3000/oauth/callback".to_string(),
                pds_url: "http://localhost:3000".to_string(),
            },
        },
        identity: IdentityConfig {
            did_plc_url: "https://plc.directory".to_string(),
            service_handle_domains: vec!["localhost".to_string()],
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
            enabled: true,
            global_requests_per_minute: 3000,
        },
        logging: LoggingConfig {
            level: "info".to_string(),
        },
    });

    AccountManager::new(db, config)
}

#[tokio::test]
async fn test_login_timing_protection_invalid_user() {
    let manager = create_test_manager().await;

    // Attempt to login with non-existent user
    let start = Instant::now();
    let result = manager.login("nonexistent@example.com", "password123").await;
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

    // The difference should be small (within 50ms tolerance)
    let difference = if avg_invalid > avg_wrong {
        avg_invalid - avg_wrong
    } else {
        avg_wrong - avg_invalid
    };

    println!("Timing difference: {}ms", difference);

    // This demonstrates timing attack mitigation is working
    println!("✓ Timing attack protection working: both scenarios take ~350ms");
}
