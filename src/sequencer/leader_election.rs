//! Sequencer leader election via Postgres advisory locks.
//!
//! Implements the design from `docs/AURORA_DESIGN.md` §5.4.1 for
//! multi-instance Aurora-Locus deployments against one Postgres backend.
//!
//! - One process holds a session-scoped advisory lock — that's the *leader*.
//! - Other processes are *standbys* that retry every `retry_interval_ms`.
//! - Connection drop releases the lock automatically (Postgres
//!   session-scoped semantics); standbys pick up on the next retry tick.
//! - Graceful shutdown explicitly releases the lock so the next standby
//!   doesn't have to wait for the retry interval.
//!
//! SQLite deployments don't run leader election — they're inherently
//! single-instance. `Sequencer::new` defaults `is_leader` to `true`
//! when no election is configured.
//!
//! # Testing
//!
//! The state machine is generic over a `LockProvider` trait so tests can
//! swap a mock implementation that doesn't need a live Postgres. The
//! Postgres-backed `PostgresLockProvider` is exercised end-to-end in
//! Phase 4.4 integration tests (chainlink #91), once Postgres is
//! available in CI. See chainlink #87 for the related test-infra debt.

use crate::error::{PdsError, PdsResult};
use async_trait::async_trait;
use sqlx::{AnyConnection, Connection};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

/// Re-exported from the project-wide advisory-lock registry. The
/// canonical definition lives in [`crate::db::advisory_locks`] so all
/// `pg_advisory_lock` keys in the codebase are visible in one file —
/// see that module's doc-comment for the no-collision discipline.
pub use crate::db::advisory_locks::SEQUENCER_LEADER_LOCK_KEY;

/// Role assigned by the leader-election state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaderRole {
    Leader,
    Standby,
}

/// Lock acquisition strategy. The trait abstracts over the actual locking
/// mechanism so tests can substitute a mock without needing a live
/// Postgres.
#[async_trait]
pub trait LockProvider: Send + Sync {
    /// Attempt to acquire the lock. Returns `true` if acquired (or
    /// already held), `false` if another holder has it.
    async fn try_acquire(&self) -> bool;
    /// Release the lock if held. Best-effort — connection drop also
    /// releases via Postgres session semantics, so this is for graceful
    /// shutdown only.
    async fn release(&self);
}

/// Postgres-backed lock provider. Holds a dedicated connection (NOT
/// borrowed from the pool) for the lifetime of the leader role; the
/// lock auto-releases on connection drop, which is how we get free
/// failure detection.
///
/// Per POSTGRES_PHASE_4 §5.1, the leader's advisory-lock connection
/// must be separate from the application pool — pool sizing assumes
/// `pool_size + 2` total connections per instance, where the +2 are
/// the lock connection (here) and the LISTEN connection (in
/// `cache::invalidation`). A pool-borrowed connection would
/// invisibly steal a pool slot for the leader's lifetime, which is
/// the entire process lifetime once acquired — that's the bug this
/// type avoids by directly connecting via `AnyConnection::connect`,
/// mirroring the listener's `PgListener::connect` pattern.
pub struct PostgresLockProvider {
    db_url: String,
    held_connection: Mutex<Option<AnyConnection>>,
    lock_key: i64,
}

impl PostgresLockProvider {
    /// `db_url` is the Postgres connection string used to open the
    /// dedicated lock connection. Callers thread this through from
    /// `config.database.url`; for SQLite deployments this provider
    /// never gets constructed.
    pub fn new(db_url: String, lock_key: i64) -> Self {
        Self {
            db_url,
            held_connection: Mutex::new(None),
            lock_key,
        }
    }
}

#[async_trait]
impl LockProvider for PostgresLockProvider {
    async fn try_acquire(&self) -> bool {
        let mut guard = self.held_connection.lock().await;
        if guard.is_some() {
            return true;
        }
        // Open a fresh connection dedicated to this provider — never
        // returned to a pool. The connection's lifetime equals the
        // leader-role lifetime; on shutdown or unexpected drop the
        // session-scoped advisory lock auto-releases.
        let mut conn = match AnyConnection::connect(&self.db_url).await {
            Ok(c) => c,
            Err(e) => {
                warn!(error = %e, "leader_election: dedicated connect failed");
                return false;
            }
        };
        let acquired: bool =
            match sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_lock($1)")
                .bind(self.lock_key)
                .fetch_one(&mut conn)
                .await
            {
                Ok(b) => b,
                Err(e) => {
                    warn!(error = %e, "leader_election: pg_try_advisory_lock failed");
                    // Drop conn explicitly so we don't leak it on failure.
                    let _ = conn.close().await;
                    return false;
                }
            };
        if acquired {
            *guard = Some(conn);
        } else {
            // Lock held elsewhere — release the connection so we don't
            // hold an idle session-scoped connection on the database
            // for no reason.
            let _ = conn.close().await;
        }
        acquired
    }

    async fn release(&self) {
        let mut guard = self.held_connection.lock().await;
        if let Some(mut conn) = guard.take() {
            if let Err(e) = sqlx::query("SELECT pg_advisory_unlock($1)")
                .bind(self.lock_key)
                .execute(&mut conn)
                .await
            {
                warn!(error = %e, "leader_election: pg_advisory_unlock failed");
            }
            // Close the dedicated connection cleanly. Postgres also
            // releases on connection drop (session-scoped advisory
            // lock), but explicit close gives a more deterministic
            // shutdown and shows up in pg_stat_activity faster.
            let _ = conn.close().await;
        }
    }

}

/// Configuration for the leader-election loop.
#[derive(Debug, Clone)]
pub struct LeaderElectionConfig {
    pub retry_interval: Duration,
}

impl Default for LeaderElectionConfig {
    fn default() -> Self {
        Self {
            retry_interval: Duration::from_millis(2000),
        }
    }
}

/// Handle for managing a running leader-election task.
pub struct LeaderElection {
    role_flag: Arc<AtomicBool>,
    provider: Arc<dyn LockProvider>,
    task: Option<JoinHandle<()>>,
    /// Set to true to ask the loop to exit. Checked at the top of every
    /// iteration so the signal is not lost even if the loop is currently
    /// inside `try_acquire` (`Notify::notify_waiters` only wakes
    /// already-waiting tasks).
    shutdown_flag: Arc<AtomicBool>,
    /// Wake-up nudge so shutdown doesn't have to wait for the retry
    /// timer to expire. Lossy on its own (race with `try_acquire`),
    /// which is why `shutdown_flag` exists too.
    shutdown_notify: Arc<tokio::sync::Notify>,
}

impl LeaderElection {
    /// Create a new leader-election handle. The `is_leader_flag` is
    /// shared with the [`Sequencer`](super::sequencer::Sequencer) so
    /// write methods can gate on it. `true` = leader, `false` = standby.
    pub fn new(provider: Arc<dyn LockProvider>, is_leader_flag: Arc<AtomicBool>) -> Self {
        Self {
            role_flag: is_leader_flag,
            provider,
            task: None,
            shutdown_flag: Arc::new(AtomicBool::new(false)),
            shutdown_notify: Arc::new(tokio::sync::Notify::new()),
        }
    }

    /// Spawn the election loop as a Tokio task.
    pub fn spawn(&mut self, config: LeaderElectionConfig) {
        if self.task.is_some() {
            warn!("leader_election: spawn called twice; ignoring second call");
            return;
        }
        let provider = Arc::clone(&self.provider);
        let role_flag = Arc::clone(&self.role_flag);
        let shutdown_flag = Arc::clone(&self.shutdown_flag);
        let shutdown_notify = Arc::clone(&self.shutdown_notify);
        let task = tokio::spawn(async move {
            loop {
                if shutdown_flag.load(Ordering::SeqCst) {
                    debug!("leader_election: shutdown flag observed pre-acquire");
                    break;
                }
                let acquired = provider.try_acquire().await;
                let was_leader = role_flag.swap(acquired, Ordering::SeqCst);
                if acquired && !was_leader {
                    info!("leader_election: acquired sequencer leadership");
                } else if !acquired && was_leader {
                    info!("leader_election: lost sequencer leadership; demoted to standby");
                }
                if shutdown_flag.load(Ordering::SeqCst) {
                    debug!("leader_election: shutdown flag observed post-acquire");
                    break;
                }
                tokio::select! {
                    _ = tokio::time::sleep(config.retry_interval) => {}
                    _ = shutdown_notify.notified() => {
                        debug!("leader_election: shutdown notify received");
                        break;
                    }
                }
            }
            // Graceful shutdown: release the lock if we hold it.
            provider.release().await;
            role_flag.store(false, Ordering::SeqCst);
        });
        self.task = Some(task);
    }

    /// Signal the election task to stop and wait for it to drain.
    /// Releases the advisory lock if held. Operator-side graceful-
    /// shutdown forward-substrate — no production caller today;
    /// integration-test consumer in `tests/multi_instance_test.rs`.
    #[allow(dead_code)]
    pub async fn shutdown(mut self) -> PdsResult<()> {
        self.shutdown_flag.store(true, Ordering::SeqCst);
        self.shutdown_notify.notify_waiters();
        if let Some(task) = self.task.take() {
            task.await
                .map_err(|e| PdsError::Internal(format!("leader_election task join: {}", e)))?;
        }
        Ok(())
    }

    /// Current role from the shared flag. Useful for diagnostics; the
    /// Sequencer reads the same flag directly. Consumed by inline tests
    /// (same module) for failover assertions.
    #[allow(dead_code)]
    pub fn current_role(&self) -> LeaderRole {
        if self.role_flag.load(Ordering::SeqCst) {
            LeaderRole::Leader
        } else {
            LeaderRole::Standby
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mock lock provider for testing the state machine without Postgres.
    /// All instances share an `Arc<Mutex<Option<usize>>>` representing the
    /// holder's id; `try_acquire` succeeds iff the slot is empty or
    /// already held by this provider.
    struct MockLockProvider {
        id: usize,
        shared: Arc<std::sync::Mutex<Option<usize>>>,
        /// Set to true to simulate connection-drop / lock-loss.
        force_release_on_next_try: Arc<AtomicBool>,
    }

    impl MockLockProvider {
        fn new(id: usize, shared: Arc<std::sync::Mutex<Option<usize>>>) -> Self {
            Self {
                id,
                shared,
                force_release_on_next_try: Arc::new(AtomicBool::new(false)),
            }
        }
    }

    #[async_trait]
    impl LockProvider for MockLockProvider {
        async fn try_acquire(&self) -> bool {
            if self.force_release_on_next_try.swap(false, Ordering::SeqCst) {
                let mut g = self.shared.lock().unwrap();
                if *g == Some(self.id) {
                    *g = None;
                }
                return false;
            }
            let mut g = self.shared.lock().unwrap();
            match *g {
                None => {
                    *g = Some(self.id);
                    true
                }
                Some(holder) => holder == self.id,
            }
        }

        async fn release(&self) {
            let mut g = self.shared.lock().unwrap();
            if *g == Some(self.id) {
                *g = None;
            }
        }
    }

    // The hash-derivation invariant for SEQUENCER_LEADER_LOCK_KEY now
    // lives in `crate::db::advisory_locks::tests` alongside the other
    // registry keys.

    #[tokio::test]
    async fn single_instance_acquires_on_first_tick() {
        let shared = Arc::new(std::sync::Mutex::new(None));
        let provider = Arc::new(MockLockProvider::new(1, Arc::clone(&shared)));
        let flag = Arc::new(AtomicBool::new(false));
        let mut elect = LeaderElection::new(provider, Arc::clone(&flag));
        elect.spawn(LeaderElectionConfig {
            retry_interval: Duration::from_millis(50),
        });
        // First tick should acquire immediately.
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(flag.load(Ordering::SeqCst));
        assert_eq!(elect.current_role(), LeaderRole::Leader);
        elect.shutdown().await.unwrap();
        assert!(shared.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn second_instance_stays_standby_while_leader_alive() {
        let shared = Arc::new(std::sync::Mutex::new(None));
        let p1 = Arc::new(MockLockProvider::new(1, Arc::clone(&shared)));
        let p2 = Arc::new(MockLockProvider::new(2, Arc::clone(&shared)));
        let flag1 = Arc::new(AtomicBool::new(false));
        let flag2 = Arc::new(AtomicBool::new(false));
        let mut e1 = LeaderElection::new(p1, Arc::clone(&flag1));
        let mut e2 = LeaderElection::new(p2, Arc::clone(&flag2));
        e1.spawn(LeaderElectionConfig {
            retry_interval: Duration::from_millis(50),
        });
        // Let e1 acquire first.
        tokio::time::sleep(Duration::from_millis(20)).await;
        e2.spawn(LeaderElectionConfig {
            retry_interval: Duration::from_millis(50),
        });
        // Multiple ticks; e2 stays standby because shared slot is taken.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(flag1.load(Ordering::SeqCst));
        assert!(!flag2.load(Ordering::SeqCst));
        assert_eq!(e1.current_role(), LeaderRole::Leader);
        assert_eq!(e2.current_role(), LeaderRole::Standby);
        e1.shutdown().await.unwrap();
        e2.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn standby_acquires_after_leader_graceful_shutdown() {
        let shared = Arc::new(std::sync::Mutex::new(None));
        let p1 = Arc::new(MockLockProvider::new(1, Arc::clone(&shared)));
        let p2 = Arc::new(MockLockProvider::new(2, Arc::clone(&shared)));
        let flag1 = Arc::new(AtomicBool::new(false));
        let flag2 = Arc::new(AtomicBool::new(false));
        let mut e1 = LeaderElection::new(p1, Arc::clone(&flag1));
        let mut e2 = LeaderElection::new(p2, Arc::clone(&flag2));
        e1.spawn(LeaderElectionConfig {
            retry_interval: Duration::from_millis(50),
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        e2.spawn(LeaderElectionConfig {
            retry_interval: Duration::from_millis(50),
        });
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(flag1.load(Ordering::SeqCst));
        assert!(!flag2.load(Ordering::SeqCst));
        // e1 graceful shutdown → e2 should acquire on next tick.
        e1.shutdown().await.unwrap();
        // Allow at least one retry interval for e2.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(flag2.load(Ordering::SeqCst));
        assert_eq!(e2.current_role(), LeaderRole::Leader);
        e2.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn standby_acquires_after_leader_connection_drop() {
        let shared = Arc::new(std::sync::Mutex::new(None));
        let p1 = Arc::new(MockLockProvider::new(1, Arc::clone(&shared)));
        let force_release = Arc::clone(&p1.force_release_on_next_try);
        let p2 = Arc::new(MockLockProvider::new(2, Arc::clone(&shared)));
        let flag1 = Arc::new(AtomicBool::new(false));
        let flag2 = Arc::new(AtomicBool::new(false));
        let mut e1 = LeaderElection::new(p1, Arc::clone(&flag1));
        let mut e2 = LeaderElection::new(p2, Arc::clone(&flag2));
        e1.spawn(LeaderElectionConfig {
            retry_interval: Duration::from_millis(50),
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        e2.spawn(LeaderElectionConfig {
            retry_interval: Duration::from_millis(50),
        });
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(flag1.load(Ordering::SeqCst));
        // Simulate connection drop on e1's next tick.
        force_release.store(true, Ordering::SeqCst);
        // Wait long enough for e1 to tick (and lose lock) AND e2 to tick (and acquire).
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(!flag1.load(Ordering::SeqCst));
        assert!(flag2.load(Ordering::SeqCst));
        e1.shutdown().await.unwrap();
        e2.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn leader_role_default_before_first_tick_is_standby() {
        let shared = Arc::new(std::sync::Mutex::new(None));
        let provider = Arc::new(MockLockProvider::new(1, shared));
        let flag = Arc::new(AtomicBool::new(false));
        let elect = LeaderElection::new(provider, Arc::clone(&flag));
        // Without spawning, role is Standby.
        assert_eq!(elect.current_role(), LeaderRole::Standby);
    }

    #[tokio::test]
    async fn double_spawn_is_a_noop() {
        let shared = Arc::new(std::sync::Mutex::new(None));
        let provider = Arc::new(MockLockProvider::new(1, shared));
        let flag = Arc::new(AtomicBool::new(false));
        let mut elect = LeaderElection::new(provider, flag);
        elect.spawn(LeaderElectionConfig {
            retry_interval: Duration::from_millis(50),
        });
        // Second spawn should be a no-op (warns + returns).
        elect.spawn(LeaderElectionConfig {
            retry_interval: Duration::from_millis(50),
        });
        elect.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn shutdown_releases_lock() {
        let shared = Arc::new(std::sync::Mutex::new(None));
        let provider = Arc::new(MockLockProvider::new(1, Arc::clone(&shared)));
        let flag = Arc::new(AtomicBool::new(false));
        let mut elect = LeaderElection::new(provider, flag);
        elect.spawn(LeaderElectionConfig {
            retry_interval: Duration::from_millis(50),
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(*shared.lock().unwrap(), Some(1));
        elect.shutdown().await.unwrap();
        assert!(shared.lock().unwrap().is_none());
    }
}
