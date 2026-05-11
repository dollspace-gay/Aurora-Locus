//! Multi-instance integration tests for the Arc 7
//! `DistributedStore` substrate (chainlink #53).
//!
//! Spins up two `PostgresCasStore` instances pointed at the
//! same testcontainers Postgres and exercises the substrate's
//! cross-instance correctness guarantees. Mirrors the in-
//! process pattern used by `tests/multi_instance_test.rs` for
//! sequencer + cache invalidation (chainlink #91): real
//! Postgres semantics (UNIQUE-violation detection, atomic
//! UPDATEs, lease-based filtering) are exercised against the
//! shared backend regardless of whether the two consumers run
//! in one Tokio runtime or N OS processes.
//!
//! HTTP-level cross-instance tests (e.g., two `AppContext`s +
//! two `axum::serve`s on different ports) are not yet built;
//! per the Step 0 Q9 / Step 0.7 PARTIALLY FIRES decision,
//! substrate-level coverage is sufficient for Step 1's
//! verification criterion. Step 2 or Step 3 may trigger the
//! full HTTP-level scaffolding if the consumer-level tests
//! require it.
//!
//! Prerequisites: Docker daemon accessible to the test runner.
//! Tests panic with a clear message if Docker is unreachable.

use std::sync::{Arc, Once};
use std::time::Duration;

use aurora_locus::distributed::{
    DistributedError, DistributedStore, Lease, PostgresCasStore,
};
use aurora_locus::oauth::flow_state_adapter::{
    decode_request, encode_request_data, OAuthFlowStateAdapter,
};
use aurora_locus::oauth::models::AuthorizationRequestData;
use sqlx::any::AnyPoolOptions;
use sqlx::AnyPool;
use testcontainers::runners::AsyncRunner;
use testcontainers::ContainerAsync;
use testcontainers_modules::postgres::Postgres;

/// Spin up a Postgres testcontainer. Caller keeps the
/// container alive for the duration of the test (drop = stop).
async fn start_postgres() -> (ContainerAsync<Postgres>, String) {
    let container = Postgres::default().start().await.expect(
        "Failed to start Postgres container — is Docker accessible? \
         Test prerequisite: docker daemon access for the test runner.",
    );
    let host = container.get_host().await.expect("get_host failed");
    let port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("get_host_port_ipv4 failed");
    let url = format!("postgres://postgres:postgres@{}:{}/postgres", host, port);
    (container, url)
}

/// Open an `AnyPool` against the given Postgres URL, install
/// drivers once across the test process, and run the migrations
/// (including `0007_distributed_state.sql`).
async fn open_pool(url: &str) -> Arc<AnyPool> {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(sqlx::any::install_default_drivers);
    let pool = AnyPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(Duration::from_secs(30))
        .connect(url)
        .await
        .expect("connect AnyPool to test postgres");
    sqlx::migrate!("./migrations/postgres")
        .run(&pool)
        .await
        .expect("run postgres migrations on test container");
    Arc::new(pool)
}

/// Build a `PostgresCasStore` against the given Postgres URL.
/// Each call constructs its own pool so the test exercises the
/// no-shared-state-in-process property — both stores see the
/// world only through Postgres, the way two Aurora-Locus
/// instances would.
async fn build_store(url: &str) -> PostgresCasStore {
    let pool = open_pool(url).await;
    PostgresCasStore::new(pool)
}

/// JSON body for a `dpop_jti_replay` row. Mirrors the wire
/// shape in `src/distributed/postgres_cas.rs::DpopJtiReplayValue`.
fn jti_value(jkt: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({ "jkt": jkt })).unwrap()
}

/// Step 1's headline cross-instance correctness check: a JTI
/// accepted by instance A is rejected by instance B as a
/// replay. This is the substrate property that makes DPoP
/// single-use semantics coherent across instances — the
/// motivating concern for V04_DESIGN.md §6.3.4.
///
/// Steps 2 and 3 will exercise the same property at the
/// consumer layer (with the actual DPoP proof verification
/// path); this test pins it at the substrate layer so
/// regressions surface here first.
#[tokio::test]
async fn cross_instance_dpop_jti_replay_rejection() {
    let (_pg, url) = start_postgres().await;
    let store_a = build_store(&url).await;
    let store_b = build_store(&url).await;

    let lease = Lease::from_now(chrono::Duration::seconds(60));
    let jti = "cross-instance-jti";

    // Instance A accepts the first sighting.
    store_a
        .insert("dpop_jti_replay", jti, &jti_value("thumb"), Some(lease))
        .await
        .expect("instance A: first sighting must succeed");

    // Instance B (separate pool, fresh connection) rejects the
    // replay. This is the cross-instance correctness guarantee:
    // without the shared Postgres backend, two per-instance
    // in-memory JTI sets would both accept this JTI.
    let err = store_b
        .insert("dpop_jti_replay", jti, &jti_value("thumb"), Some(lease))
        .await
        .expect_err("instance B: replay must be rejected");

    match err {
        DistributedError::KeyExists { table, key } => {
            assert_eq!(table, "dpop_jti_replay");
            assert_eq!(key, jti);
        }
        other => panic!("expected KeyExists from instance B, got {:?}", other),
    }
}

/// Concurrent inserts of the same JTI from both instances:
/// exactly one wins, the other gets KeyExists. Pins the
/// atomicity guarantee — both writers race against a single
/// primary-key constraint that Postgres serializes, regardless
/// of which connection got there first.
#[tokio::test]
async fn concurrent_cross_instance_inserts_serialize_to_one_winner() {
    let (_pg, url) = start_postgres().await;
    let store_a = build_store(&url).await;
    let store_b = build_store(&url).await;

    let lease = Lease::from_now(chrono::Duration::seconds(60));
    let jti = "race-jti";

    // Bind the JSON payloads outside the join so the borrows
    // stay alive across both futures (E0716 — the temporary
    // would otherwise drop at end-of-statement while the
    // futures still reference it).
    let value_a = jti_value("thumb-a");
    let value_b = jti_value("thumb-b");

    // Fire both inserts concurrently — joinset would also
    // work; tokio::join is sufficient for two-party.
    let (res_a, res_b) = tokio::join!(
        store_a.insert("dpop_jti_replay", jti, &value_a, Some(lease)),
        store_b.insert("dpop_jti_replay", jti, &value_b, Some(lease)),
    );

    // Exactly one of the two should succeed; the other should
    // see KeyExists. Both succeeding would mean the unique
    // constraint isn't being enforced — that's the regression
    // this test catches.
    let outcomes = [res_a.is_ok(), res_b.is_ok()];
    let successes = outcomes.iter().filter(|ok| **ok).count();
    assert_eq!(
        successes, 1,
        "exactly one insert must succeed, got {:?}",
        outcomes
    );

    // The losing side must report KeyExists, not some other
    // backend error. This pins the is_unique_violation()
    // detection against real Postgres SQLSTATE 23505.
    let loser_err = if res_a.is_err() {
        res_a.unwrap_err()
    } else {
        res_b.unwrap_err()
    };
    assert!(
        matches!(loser_err, DistributedError::KeyExists { .. }),
        "loser must report KeyExists, got {:?}",
        loser_err
    );
}

/// Cross-instance delete-then-reap: A consumes, B sweeps
/// expired stragglers. Confirms `delete` is visible to other
/// instances immediately and that `reap_expired` sweeps rows
/// based on the wall-clock the caller passes, not the
/// database-side clock (which could drift).
#[tokio::test]
async fn cross_instance_delete_and_reap_visible_to_siblings() {
    let (_pg, url) = start_postgres().await;
    let store_a = build_store(&url).await;
    let store_b = build_store(&url).await;

    // Two rows: one we delete via A, one we leave to reap.
    let future_lease = Lease::from_now(chrono::Duration::seconds(60));
    let past_lease = Lease::until(0); // already expired

    store_a
        .insert("dpop_jti_replay", "live", &jti_value("k1"), Some(future_lease))
        .await
        .unwrap();
    store_a
        .insert(
            "dpop_jti_replay",
            "stale",
            &jti_value("k2"),
            Some(past_lease),
        )
        .await
        .unwrap();

    // A explicitly deletes one. B's `get` should immediately
    // return None — no caching, no replication lag (same
    // backend).
    assert!(
        store_a.delete("dpop_jti_replay", "live").await.unwrap(),
        "delete returns true on first call"
    );
    assert!(
        store_b
            .get("dpop_jti_replay", "live")
            .await
            .unwrap()
            .is_none(),
        "B sees the deletion immediately"
    );

    // B sweeps with a `now_epoch_ms` past the stale row's
    // lease. The stale row goes; if A had any non-stale row,
    // it would be untouched (none here — `live` already
    // gone).
    let now_ms = chrono::Utc::now().timestamp_millis();
    let swept = store_b
        .reap_expired("dpop_jti_replay", now_ms)
        .await
        .expect("B's reaper sweep succeeds");
    assert_eq!(swept, 1, "B sweeps exactly the stale row");

    // A's view also reflects the sweep.
    assert!(
        store_a
            .get("dpop_jti_replay", "stale")
            .await
            .unwrap()
            .is_none(),
        "A sees B's sweep result"
    );
}

/// CAS race across instances: A and B both attempt to update
/// the same `rate_limit_buckets` row from version 0. Exactly
/// one succeeds; the other observes Conflict with the winner's
/// new version (1). Pins that the §6.3.5 atomic UPDATE-with-
/// version-check actually serializes through Postgres's row
/// lock.
#[tokio::test]
async fn cross_instance_cas_race_yields_one_winner_and_one_conflict() {
    use aurora_locus::distributed::CasResult;

    let (_pg, url) = start_postgres().await;
    let store_a = build_store(&url).await;
    let store_b = build_store(&url).await;

    let initial = serde_json::to_vec(&serde_json::json!({
        "tokens_remaining": 100i64,
        "max_tokens": 100i64,
        "refill_rate": 10i64,
        "window_start_at_epoch_ms": 0i64,
    }))
    .unwrap();
    store_a
        .insert("rate_limit_buckets", "bucket-X", &initial, None)
        .await
        .unwrap();

    let update_a = serde_json::to_vec(&serde_json::json!({
        "tokens_remaining": 90i64,
        "max_tokens": 100i64,
        "refill_rate": 10i64,
        "window_start_at_epoch_ms": 1i64,
    }))
    .unwrap();
    let update_b = serde_json::to_vec(&serde_json::json!({
        "tokens_remaining": 95i64,
        "max_tokens": 100i64,
        "refill_rate": 10i64,
        "window_start_at_epoch_ms": 1i64,
    }))
    .unwrap();

    let (res_a, res_b) = tokio::join!(
        store_a.cas("rate_limit_buckets", "bucket-X", 0, &update_a),
        store_b.cas("rate_limit_buckets", "bucket-X", 0, &update_b),
    );

    let res_a = res_a.expect("CAS A produced an error result wrapper");
    let res_b = res_b.expect("CAS B produced an error result wrapper");

    let successes = [matches!(res_a, CasResult::Success { .. }), matches!(res_b, CasResult::Success { .. })];
    let success_count = successes.iter().filter(|ok| **ok).count();
    assert_eq!(
        success_count, 1,
        "exactly one CAS must succeed, got {:?} / {:?}",
        res_a, res_b
    );

    // The loser's Conflict must report version 1 (the winner's
    // new version) — that's what lets retry loops refetch +
    // recompute against the actual current state.
    let loser = if matches!(res_a, CasResult::Conflict { .. }) {
        res_a
    } else {
        res_b
    };
    match loser {
        CasResult::Conflict { current_version } => {
            assert_eq!(
                current_version, 1,
                "loser must observe winner's new version"
            );
        }
        other => panic!("expected Conflict on loser, got {:?}", other),
    }
}

// =====================================================================
// OAuth flow state adapter — cross-instance tests (Arc 7 Step 2).
//
// Same in-process pattern as the substrate tests above: two
// `OAuthFlowStateAdapter` instances against the same Postgres
// testcontainer, each with its own pool. The
// `authorization_request` table lives in `account_db` (not the
// substrate's maintenance pool) but the testcontainer hosts
// both schemas in one DB, so we reuse `open_pool`.
//
// What we're verifying: OAuth state inserted on instance A is
// visible to instance B's `get`; consumed by instance B via
// `delete`; subsequent reads on either instance return None.
// These properties always worked at the storage layer (both
// instances share `account_db`); Step 2 surfaces them through
// the trait so the cross-instance correctness story is
// uniform across the substrate's three tables.
// =====================================================================

/// Build an `OAuthFlowStateAdapter` against the given Postgres URL.
async fn build_oauth_adapter(url: &str) -> OAuthFlowStateAdapter {
    let pool = open_pool(url).await;
    OAuthFlowStateAdapter::new(pool)
}

fn sample_authorization_request() -> AuthorizationRequestData {
    AuthorizationRequestData {
        did: "did:web:alice.example.com".to_string(),
        client_id: "https://client.example.com/metadata.json".to_string(),
        code_challenge: "abc_xyz_challenge".to_string(),
        code_challenge_method: "S256".to_string(),
        scope: "atproto:read atproto:write".to_string(),
        redirect_uri: "https://client.example.com/cb".to_string(),
        state: Some("client-csrf-state".to_string()),
    }
}

/// Instance A inserts an OAuth flow state; instance B reads it
/// successfully via the trait. The behavior has always worked
/// at the storage layer (shared account_db), but Step 2
/// surfaces it through the trait so the cross-instance read
/// story matches the substrate's other tables.
#[tokio::test]
async fn cross_instance_oauth_state_visible_to_siblings() {
    let (_pg, url) = start_postgres().await;
    let adapter_a = build_oauth_adapter(&url).await;
    let adapter_b = build_oauth_adapter(&url).await;

    let request_id = "req-cross-instance-visible";
    let value = encode_request_data(&sample_authorization_request());
    let lease = Lease::from_now(chrono::Duration::minutes(10));

    // A inserts.
    adapter_a
        .insert("oauth_flow_state", request_id, &value, Some(lease))
        .await
        .expect("instance A: insert succeeds");

    // B reads — sees the same row.
    let bytes = adapter_b
        .get("oauth_flow_state", request_id)
        .await
        .expect("instance B: get without error")
        .expect("instance B: row visible to siblings");
    let request = decode_request(&bytes).expect("decode AuthorizationRequest");
    assert_eq!(request.request_id, request_id);
    assert_eq!(request.did, "did:web:alice.example.com");
    assert!(!request.code_used);
}

/// Cross-instance consume-and-reject-replay: A inserts, B
/// consumes (delete returns true), A's subsequent read returns
/// None. Pins the single-use-across-instances guarantee that
/// makes OAuth code redemption coherent in multi-instance
/// deployments.
#[tokio::test]
async fn cross_instance_oauth_state_consume_rejects_replay() {
    let (_pg, url) = start_postgres().await;
    let adapter_a = build_oauth_adapter(&url).await;
    let adapter_b = build_oauth_adapter(&url).await;

    let request_id = "req-cross-instance-consume";
    let value = encode_request_data(&sample_authorization_request());
    let lease = Lease::from_now(chrono::Duration::minutes(10));

    adapter_a
        .insert("oauth_flow_state", request_id, &value, Some(lease))
        .await
        .expect("A insert");

    // B consumes — returns true.
    assert!(
        adapter_b
            .delete("oauth_flow_state", request_id)
            .await
            .expect("B delete ok"),
        "first consume returns true"
    );

    // A's subsequent read sees None (filtered: code_used = TRUE).
    assert!(
        adapter_a
            .get("oauth_flow_state", request_id)
            .await
            .expect("A get ok")
            .is_none(),
        "post-consume row is None on the sibling instance"
    );

    // B's repeat consume is idempotent — returns false (already used).
    assert!(
        !adapter_b
            .delete("oauth_flow_state", request_id)
            .await
            .expect("B re-delete ok"),
        "second consume returns false"
    );

    // A racing the same consume after B's success: also false.
    assert!(
        !adapter_a
            .delete("oauth_flow_state", request_id)
            .await
            .expect("A delete after B consume ok"),
        "sibling consume after B's success returns false"
    );
}

/// Concurrent consume race: A and B both attempt to consume
/// the same OAuth flow state. Exactly one wins. Pins the
/// atomic UPDATE-with-predicate serialization through
/// Postgres's row lock.
#[tokio::test]
async fn cross_instance_oauth_consume_race_yields_one_winner() {
    let (_pg, url) = start_postgres().await;
    let adapter_a = build_oauth_adapter(&url).await;
    let adapter_b = build_oauth_adapter(&url).await;

    let request_id = "req-race-consume";
    let value = encode_request_data(&sample_authorization_request());
    let lease = Lease::from_now(chrono::Duration::minutes(10));
    adapter_a
        .insert("oauth_flow_state", request_id, &value, Some(lease))
        .await
        .unwrap();

    let (res_a, res_b) = tokio::join!(
        adapter_a.delete("oauth_flow_state", request_id),
        adapter_b.delete("oauth_flow_state", request_id),
    );
    let res_a = res_a.expect("A delete ok");
    let res_b = res_b.expect("B delete ok");

    let winners = [res_a, res_b].iter().filter(|w| **w).count();
    assert_eq!(
        winners, 1,
        "exactly one consume must succeed under concurrent race \
         (A={}, B={})",
        res_a, res_b
    );
}

/// Cross-instance reaper sweep: A inserts a row with a stale
/// lease; B sweeps; A no longer sees the row. Confirms the
/// trait-routed reaper path operates against the shared table
/// the way the pre-Arc-7 direct-SQL cleanup did.
#[tokio::test]
async fn cross_instance_oauth_reap_sweeps_for_siblings() {
    let (_pg, url) = start_postgres().await;
    let adapter_a = build_oauth_adapter(&url).await;
    let adapter_b = build_oauth_adapter(&url).await;

    let request_id = "req-stale-for-reap";
    let value = encode_request_data(&sample_authorization_request());
    let past_lease = Lease::until(chrono::Utc::now().timestamp_millis() - 60_000);

    adapter_a
        .insert("oauth_flow_state", request_id, &value, Some(past_lease))
        .await
        .unwrap();

    // A's get already returns None (lease-expired filter
    // applies regardless of reaper).
    assert!(adapter_a
        .get("oauth_flow_state", request_id)
        .await
        .unwrap()
        .is_none());

    // B sweeps; row is physically deleted.
    let swept = adapter_b
        .reap_expired("oauth_flow_state", chrono::Utc::now().timestamp_millis())
        .await
        .expect("B reaper ok");
    assert!(
        swept >= 1,
        "reaper sweeps at least the stale row (got {})",
        swept
    );
}
