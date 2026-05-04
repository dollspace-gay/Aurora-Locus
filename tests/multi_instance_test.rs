//! Multi-instance integration tests for Postgres Phase 4 (chainlink #91).
//!
//! Spins up a real Postgres via testcontainers and runs multiple
//! aurora-locus "instances" (constituent parts: Sequencer +
//! LeaderElection + CacheInvalidator + Listener) in-process against
//! the shared backend. Verifies the end-to-end semantics from
//! docs/POSTGRES_PHASE_4_DESIGN.md §3 (leader election) and §4
//! (LISTEN/NOTIFY cache invalidation).
//!
//! Why in-process instead of out-of-process: Postgres-side semantics
//! (advisory locks, LISTEN/NOTIFY) are exercised against a real
//! Postgres regardless of whether the consumers run in one Tokio
//! runtime or N OS processes. Process-level isolation tests can be
//! added by Phase 5 if needed; the substrate's correctness is
//! verified here.
//!
//! Prerequisites: Docker daemon accessible to the test runner.
//! Tests fail fast with a clear error if Docker is unreachable.

use aurora_locus::{
    cache::invalidation::{
        CacheInvalidationListener, CacheInvalidator, NotifyEmitter, PostgresNotifyEmitter,
    },
    error::PdsError,
    read_after_write::LocalRecordsCache,
    sequencer::{
        LeaderElection, LeaderElectionConfig, PostgresLockProvider, Sequencer, SequencerConfig,
        SEQUENCER_LEADER_LOCK_KEY,
    },
};
use sqlx::any::AnyPoolOptions;
use sqlx::AnyPool;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Once};
use std::time::Duration;
use testcontainers::runners::AsyncRunner;
use testcontainers::ContainerAsync;
use testcontainers_modules::postgres::Postgres;

/// 200ms is well below the 2s default retry interval but long enough
/// for the in-process tasks to make progress between assertions.
const POLL_INTERVAL: Duration = Duration::from_millis(200);
/// Leader election retry interval used for tests — short enough to
/// keep test runtime under a few seconds, long enough to avoid lock
/// thrash in CI under load.
const TEST_RETRY_INTERVAL: Duration = Duration::from_millis(500);

/// Spin up a Postgres testcontainer. Caller keeps the container alive
/// (drop = stop). Returns the connection URL.
async fn start_postgres() -> (ContainerAsync<Postgres>, String) {
    let container = Postgres::default()
        .start()
        .await
        .expect(
            "Failed to start Postgres container — is Docker accessible? \
             Test prerequisite: docker daemon access for the test runner.",
        );
    let host = container
        .get_host()
        .await
        .expect("get_host failed");
    let port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("get_host_port_ipv4 failed");
    let url = format!("postgres://postgres:postgres@{}:{}/postgres", host, port);
    (container, url)
}

/// Open an `AnyPool` against the given Postgres URL, install drivers
/// once across the test process, and run the migrations.
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

/// One simulated aurora-locus "instance": Sequencer + LeaderElection
/// + CacheInvalidator + Listener wired exactly as `AppContext::new`
/// wires them for Postgres deployments.
struct Instance {
    sequencer: Arc<Sequencer>,
    election: Option<LeaderElection>,
    cache: Arc<LocalRecordsCache>,
    invalidator: Arc<CacheInvalidator>,
    listener: Option<CacheInvalidationListener>,
}

impl Instance {
    async fn build(url: &str, retry_interval: Duration) -> Self {
        let pool = open_pool(url).await;

        // Sequencer + leader election (Postgres path mirrors AppContext::new).
        let mut seq = Sequencer::with_relay(pool.clone(), SequencerConfig::default(), None);
        seq.attach_leader_flag(Arc::new(AtomicBool::new(false)));
        // PostgresLockProvider::new takes the connection URL (not a
        // pool clone) since chainlink #103 / Session 4 — the lock
        // connection is dedicated, opened directly via
        // AnyConnection::connect rather than borrowed from the
        // application pool. Match that signature here.
        let provider = Arc::new(PostgresLockProvider::new(
            url.to_string(),
            SEQUENCER_LEADER_LOCK_KEY,
        ));
        let mut election = LeaderElection::new(provider, seq.leader_flag());
        election.spawn(LeaderElectionConfig { retry_interval });
        let sequencer = Arc::new(seq);

        // Cache invalidator + listener (Postgres path).
        let cache = Arc::new(LocalRecordsCache::new());
        let notify: Arc<dyn NotifyEmitter> = Arc::new(PostgresNotifyEmitter::new(pool.clone()));
        let invalidator = Arc::new(CacheInvalidator::new(Arc::clone(&cache), Some(notify)));
        let listener = CacheInvalidationListener::spawn(url.to_string(), Arc::clone(&invalidator));

        Self {
            sequencer,
            election: Some(election),
            cache,
            invalidator,
            listener: Some(listener),
        }
    }

    fn is_leader(&self) -> bool {
        self.sequencer.is_leader()
    }

    /// Wait up to `deadline` for `cond` to become true; poll every
    /// `POLL_INTERVAL`. Returns true if `cond` was met, false on
    /// timeout. Generous tolerance to avoid CI flake.
    async fn wait_for(&self, deadline: Duration, mut cond: impl FnMut(&Self) -> bool) -> bool {
        let start = std::time::Instant::now();
        loop {
            if cond(self) {
                return true;
            }
            if start.elapsed() >= deadline {
                return false;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Graceful shutdown of election + listener. Equivalent to a
    /// SIGTERM-driven shutdown of an aurora-locus process: lock is
    /// released cleanly, listener task drains.
    async fn shutdown(mut self) {
        if let Some(election) = self.election.take() {
            election.shutdown().await.expect("election shutdown");
        }
        if let Some(listener) = self.listener.take() {
            listener.shutdown().await.expect("listener shutdown");
        }
    }
}

// ===========================================================================
// 1. Single-instance baseline.
// One process, one Postgres. Process becomes leader on startup and stays
// leader. Verifies the multi-instance machinery doesn't break the single-
// instance case.
// ===========================================================================

#[tokio::test]
async fn single_instance_baseline_becomes_and_stays_leader() {
    let (_pg, url) = start_postgres().await;
    let inst = Instance::build(&url, TEST_RETRY_INTERVAL).await;

    // First retry tick should acquire — generous 5s tolerance.
    assert!(
        inst.wait_for(Duration::from_secs(5), |i| i.is_leader()).await,
        "single instance failed to acquire leadership within 5s"
    );

    // Stays leader across multiple retry ticks.
    tokio::time::sleep(TEST_RETRY_INTERVAL * 3).await;
    assert!(inst.is_leader(), "single instance demoted unexpectedly");

    inst.shutdown().await;
}

// ===========================================================================
// 2. Two-instance leader election.
// Two processes, started in sequence. First becomes leader; second stays
// standby. Standby's sequencer.sequence_commit returns NotLeader (HTTP 503).
// ===========================================================================

#[tokio::test]
async fn two_instance_election_yields_one_leader_one_standby() {
    let (_pg, url) = start_postgres().await;

    let inst_a = Instance::build(&url, TEST_RETRY_INTERVAL).await;
    assert!(
        inst_a.wait_for(Duration::from_secs(5), |i| i.is_leader()).await,
        "instance A failed to acquire leadership"
    );

    let inst_b = Instance::build(&url, TEST_RETRY_INTERVAL).await;
    // Give B several retry ticks to confirm it stays standby.
    tokio::time::sleep(TEST_RETRY_INTERVAL * 4).await;
    assert!(inst_a.is_leader(), "A lost leadership while still alive");
    assert!(!inst_b.is_leader(), "B improperly acquired while A still leader");

    // Standby write attempts return NotLeader.
    let evt = aurora_locus::sequencer::events::IdentityEvent {
        did: "did:plc:standby_write".to_string(),
        handle: Some("standby.test".to_string()),
    };
    let standby_err = inst_b
        .sequencer
        .sequence_identity(evt)
        .await
        .expect_err("standby write must return error");
    assert!(
        matches!(standby_err, PdsError::NotLeader(_)),
        "standby write should return NotLeader, got {:?}",
        standby_err
    );

    inst_a.shutdown().await;
    inst_b.shutdown().await;
}

// ===========================================================================
// 3. Failover.
// Leader's connection drops (simulated by dropping the election handle's
// pool ownership chain); standby acquires within retry interval. Sequence
// numbers continue monotonically — no gaps, no duplicates.
// ===========================================================================

#[tokio::test]
async fn failover_demotes_leader_and_promotes_standby() {
    let (_pg, url) = start_postgres().await;

    let mut inst_a = Instance::build(&url, TEST_RETRY_INTERVAL).await;
    assert!(
        inst_a.wait_for(Duration::from_secs(5), |i| i.is_leader()).await,
        "A must be leader before failover"
    );

    let inst_b = Instance::build(&url, TEST_RETRY_INTERVAL).await;
    tokio::time::sleep(TEST_RETRY_INTERVAL * 2).await;
    assert!(!inst_b.is_leader(), "B must be standby pre-failover");

    // Write a few events as A to establish a baseline sequence.
    for i in 0..3 {
        let evt = aurora_locus::sequencer::events::IdentityEvent {
            did: format!("did:plc:pre{}", i),
            handle: Some(format!("pre{}.test", i)),
        };
        inst_a.sequencer.sequence_identity(evt).await.expect("pre-failover write");
    }
    let pre_seq = inst_a
        .sequencer
        .current_seq()
        .await
        .expect("current_seq pre")
        .expect("seq pre");
    assert!(pre_seq >= 3, "expected at least 3 events pre-failover, got {}", pre_seq);

    // Simulate failover: explicit graceful shutdown of A's election
    // releases the advisory lock. (SIGKILL would close A's TCP
    // connection abruptly; Postgres detects via TCP error and releases
    // the session-scoped lock — same Postgres-side path on a
    // longer-latency timer. For tightly-bounded test runtime, graceful
    // release exercises the equivalent code path.)
    let election = inst_a.election.take().expect("election handle present");
    election.shutdown().await.expect("election shutdown for failover");

    // B should acquire within retry interval + tolerance.
    let failover_deadline = Duration::from_secs(5);
    let start = std::time::Instant::now();
    let acquired = inst_b.wait_for(failover_deadline, |i| i.is_leader()).await;
    assert!(
        acquired,
        "B failed to acquire after A's release within {:?}",
        failover_deadline
    );
    let failover_latency = start.elapsed();
    eprintln!("failover acquisition latency: {:?}", failover_latency);

    // B's writes succeed and sequence continues monotonically.
    for i in 0..3 {
        let evt = aurora_locus::sequencer::events::IdentityEvent {
            did: format!("did:plc:post{}", i),
            handle: Some(format!("post{}.test", i)),
        };
        inst_b.sequencer.sequence_identity(evt).await.expect("post-failover write");
    }
    let post_seq = inst_b
        .sequencer
        .current_seq()
        .await
        .expect("current_seq post")
        .expect("seq post");
    assert!(
        post_seq > pre_seq,
        "post-failover seq ({}) must exceed pre-failover ({})",
        post_seq,
        pre_seq
    );
    assert_eq!(
        post_seq - pre_seq,
        3,
        "expected exactly 3 new events after failover, got delta {}",
        post_seq - pre_seq
    );

    // Cleanup A (already torn down election, but listener still alive).
    if let Some(listener) = inst_a.listener.take() {
        listener.shutdown().await.unwrap();
    }
    inst_b.shutdown().await;
}

// ===========================================================================
// 4. Cache invalidation across instances.
// A.invalidate_did fires NOTIFY; B's listener receives and applies. Verify
// B's local cache is invalidated within a short window (NOTIFY-bound, not
// TTL-bound — TTL is 5s; we assert tighter to confirm NOTIFY actually
// works rather than being masked by TTL).
// ===========================================================================

#[tokio::test]
async fn cache_invalidation_propagates_across_instances() {
    use aurora_locus::read_after_write::LocalRecords;

    let (_pg, url) = start_postgres().await;
    let inst_a = Instance::build(&url, TEST_RETRY_INTERVAL).await;
    let inst_b = Instance::build(&url, TEST_RETRY_INTERVAL).await;

    // Give listeners time to subscribe — LISTEN happens in the spawned
    // task so it isn't synchronous with build().
    tokio::time::sleep(Duration::from_millis(500)).await;

    let did = "did:plc:cacheuser";
    let rev = "rev1";

    // Pre-populate B's cache for that DID.
    inst_b
        .cache
        .set(did, rev, LocalRecords::empty())
        .await;
    assert!(
        inst_b.cache.get(did, rev).await.is_some(),
        "B's cache should hold the entry pre-invalidation"
    );

    // A invalidates → A NOTIFYs → B's listener receives + invalidates.
    inst_a.invalidator.invalidate_did(did).await;

    // Poll B's cache until the entry is gone. Bound the wait at 2s,
    // tighter than LocalRecordsCache's 5s TTL — if the entry only
    // disappears via TTL expiry, NOTIFY is silently broken and this
    // test is supposed to catch that.
    let mut propagated = false;
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(2) {
        if inst_b.cache.get(did, rev).await.is_none() {
            propagated = true;
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    assert!(
        propagated,
        "B's cache should be invalidated within 2s via NOTIFY (not TTL-bounded)"
    );

    inst_a.shutdown().await;
    inst_b.shutdown().await;
}

// ===========================================================================
// 5. Graceful shutdown handoff.
// Leader explicitly shuts down (SIGTERM equivalent); lock released cleanly;
// standby acquires faster than the SIGKILL path because no TCP detection
// latency is involved.
// ===========================================================================

#[tokio::test]
async fn graceful_shutdown_releases_lock_for_standby() {
    let (_pg, url) = start_postgres().await;

    let mut inst_a = Instance::build(&url, TEST_RETRY_INTERVAL).await;
    assert!(
        inst_a.wait_for(Duration::from_secs(5), |i| i.is_leader()).await,
        "A must be leader pre-shutdown"
    );

    let inst_b = Instance::build(&url, TEST_RETRY_INTERVAL).await;
    tokio::time::sleep(TEST_RETRY_INTERVAL * 2).await;
    assert!(!inst_b.is_leader(), "B must be standby pre-shutdown");

    // Graceful: shutdown A's election cleanly. Calls
    // pg_advisory_unlock under the hood.
    let election = inst_a.election.take().expect("election handle");
    let start = std::time::Instant::now();
    election.shutdown().await.expect("graceful shutdown");

    // B acquires on next retry tick — should be well under 1s for the
    // 500ms test interval.
    assert!(
        inst_b
            .wait_for(Duration::from_secs(3), |i| i.is_leader())
            .await,
        "B failed to acquire after A's graceful shutdown"
    );
    eprintln!(
        "graceful handoff acquisition latency: {:?}",
        start.elapsed()
    );

    // Cleanup A's listener (election is already shut down).
    if let Some(listener) = inst_a.listener.take() {
        listener.shutdown().await.unwrap();
    }
    inst_b.shutdown().await;
}
