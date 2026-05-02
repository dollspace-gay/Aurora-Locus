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
