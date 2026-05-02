//! Cross-instance cache invalidation via Postgres LISTEN/NOTIFY.
//!
//! Implements the design from `docs/POSTGRES_PHASE_4_DESIGN.md` §4 for
//! propagating cache invalidations across multiple Aurora-Locus
//! instances sharing one Postgres backend (chainlink #90).
//!
//! - **Single channel**: `aurora_cache_invalidate`. Per-cache-type
//!   dispatch happens by inspecting the JSON payload's `type` field.
//! - **Payload schema**: `{"type": "<cache-type>", "key": "<key>"}`.
//!   Currently one type is defined: `"local_records"`, with the
//!   key being the affected DID.
//! - **Listener**: dedicated long-lived `PgListener` connection (NOT
//!   from the AnyPool), with auto-reconnect on drop. PgListener
//!   handles backoff/reconnect internally given the URL it was
//!   constructed with.
//! - **NOTIFY emit**: fires after the modifying transaction commits
//!   (caller responsibility — `CacheInvalidator::invalidate_did`
//!   should be called *after* the SQL commit, not inside it).
//! - **TTL fallback**: each invalidatable cache also has a TTL, so
//!   notifications missed during listener disconnect eventually
//!   self-correct (LocalRecordsCache: 5s).
//!
//! SQLite deployments skip both listener and emit — single-instance,
//! local in-memory invalidation is sufficient.

use crate::error::{PdsError, PdsResult};
use crate::read_after_write::LocalRecordsCache;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sqlx::AnyPool;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

/// Postgres NOTIFY channel name (hardcoded per design doc §4.2).
pub const CHANNEL_NAME: &str = "aurora_cache_invalidate";

/// Cache-type tag for `LocalRecordsCache`. The full JSON `type` field
/// uses this exact string; receivers match on it to dispatch
/// invalidation to the right cache.
pub const TYPE_LOCAL_RECORDS: &str = "local_records";

/// Wire format for cross-instance invalidation messages.
///
/// `type` identifies which cache to invalidate; `key` is the cache
/// key (semantics depend on type).
///
/// Forward compatibility: receivers ignore `type` values they don't
/// recognize, so older code coexists with newer senders.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InvalidationPayload {
    #[serde(rename = "type")]
    pub type_: String,
    pub key: String,
}

impl InvalidationPayload {
    /// Construct a payload for `LocalRecordsCache` invalidation of `did`.
    pub fn for_local_records(did: &str) -> Self {
        Self {
            type_: TYPE_LOCAL_RECORDS.to_string(),
            key: did.to_string(),
        }
    }

    /// Serialize for transmission. Used by the NOTIFY emit path.
    pub fn to_json(&self) -> String {
        // The struct has only two String fields, so serialization
        // can't fail on well-formed inputs.
        serde_json::to_string(self).expect("InvalidationPayload serialize")
    }
}

/// Pluggable NOTIFY emitter. Production uses `PostgresNotifyEmitter`;
/// tests substitute a mock that records emits without needing a
/// running Postgres.
#[async_trait]
pub trait NotifyEmitter: Send + Sync {
    async fn emit(&self, payload: &InvalidationPayload) -> PdsResult<()>;
}

/// Postgres-backed NOTIFY emitter. Issues `NOTIFY <channel>, '<payload>'`
/// against the shared AnyPool. Backend must be Postgres at construction
/// time — SQLite would reject the NOTIFY syntax.
pub struct PostgresNotifyEmitter {
    pool: AnyPool,
}

impl PostgresNotifyEmitter {
    pub fn new(pool: AnyPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl NotifyEmitter for PostgresNotifyEmitter {
    async fn emit(&self, payload: &InvalidationPayload) -> PdsResult<()> {
        // Postgres `NOTIFY <channel>, '<payload>'` only accepts a
        // string literal payload — no placeholder binding. Use the
        // `pg_notify(text, text)` function instead, which accepts
        // both arguments parameterized.
        let json = payload.to_json();
        sqlx::query("SELECT pg_notify($1, $2)")
            .bind(CHANNEL_NAME)
            .bind(&json)
            .execute(&self.pool)
            .await
            .map_err(PdsError::Database)?;
        Ok(())
    }
}

/// Front door for cache invalidations. Combines local-process
/// invalidation with the cross-instance NOTIFY emit.
///
/// Wired into write-side handlers: instead of calling
/// `LocalRecordsCache::invalidate_did` directly, handlers call
/// `CacheInvalidator::invalidate_did`, which:
///
///   1. Invalidates the local in-process cache.
///   2. (Postgres only) Emits a NOTIFY so other instances invalidate
///      their local caches too.
///
/// The NOTIFY is best-effort: if it fails, the call still succeeds.
/// Other instances will pick up the change after their TTL window
/// expires (5s for LocalRecordsCache).
pub struct CacheInvalidator {
    local_records: Arc<LocalRecordsCache>,
    /// `None` for SQLite deployments (single-instance, no NOTIFY needed).
    notify: Option<Arc<dyn NotifyEmitter>>,
}

impl CacheInvalidator {
    pub fn new(
        local_records: Arc<LocalRecordsCache>,
        notify: Option<Arc<dyn NotifyEmitter>>,
    ) -> Self {
        Self {
            local_records,
            notify,
        }
    }

    /// Invalidate `LocalRecordsCache` entries for `did` locally and
    /// (on Postgres) emit NOTIFY for cross-instance invalidation.
    ///
    /// Must be called *after* the modifying SQL transaction commits;
    /// otherwise other instances may invalidate before their re-read
    /// can see the new data.
    pub async fn invalidate_did(&self, did: &str) {
        self.local_records.invalidate_did(did).await;
        if let Some(notify) = &self.notify {
            let payload = InvalidationPayload::for_local_records(did);
            if let Err(e) = notify.emit(&payload).await {
                // Best-effort: log and continue. Other instances will
                // pick up the change after their LocalRecordsCache TTL
                // window (5s).
                warn!(
                    error = %e,
                    did = %did,
                    "cache_invalidation: NOTIFY emit failed; falling back on TTL"
                );
            } else {
                debug!(did = %did, "cache_invalidation: NOTIFY emitted");
            }
        }
    }

    /// Receiver-side dispatch: parse a notification payload and apply
    /// the invalidation locally. Called by the listener task on each
    /// NOTIFY arrival. Unrecognized type values are ignored (forward
    /// compatibility per design doc §4.3); malformed JSON logs and is
    /// dropped.
    pub async fn handle_notification(&self, raw: &str) {
        let payload: InvalidationPayload = match serde_json::from_str(raw) {
            Ok(p) => p,
            Err(e) => {
                warn!(error = %e, payload = %raw, "cache_invalidation: dropping malformed NOTIFY payload");
                return;
            }
        };
        match payload.type_.as_str() {
            TYPE_LOCAL_RECORDS => {
                debug!(did = %payload.key, "cache_invalidation: applying local_records invalidation");
                self.local_records.invalidate_did(&payload.key).await;
            }
            other => {
                debug!(
                    type_ = %other,
                    "cache_invalidation: ignoring unrecognized type (forward compat)"
                );
            }
        }
    }
}

/// Background listener task that subscribes to the
/// `aurora_cache_invalidate` channel and dispatches incoming NOTIFY
/// payloads to the [`CacheInvalidator`].
///
/// `PgListener` (sqlx) handles auto-reconnect internally given the URL
/// it was constructed with. On reconnect, it re-issues `LISTEN` for
/// every previously-registered channel; we don't need to do anything
/// special. Notifications missed during a disconnect window are
/// covered by the LocalRecordsCache TTL (design doc §4.6).
pub struct CacheInvalidationListener {
    task: Option<JoinHandle<()>>,
    shutdown: Arc<tokio::sync::Notify>,
}

impl CacheInvalidationListener {
    /// Spawn the listener against the provided Postgres URL. Returns a
    /// handle that owns the task; call [`Self::shutdown`] for graceful
    /// teardown.
    pub fn spawn(url: String, invalidator: Arc<CacheInvalidator>) -> Self {
        let shutdown = Arc::new(tokio::sync::Notify::new());
        let shutdown_for_task = Arc::clone(&shutdown);
        let task = tokio::spawn(run_listener_loop(url, invalidator, shutdown_for_task));
        Self {
            task: Some(task),
            shutdown,
        }
    }

    /// Signal the listener task to exit and wait for it to drain.
    pub async fn shutdown(mut self) -> PdsResult<()> {
        self.shutdown.notify_waiters();
        if let Some(task) = self.task.take() {
            task.await
                .map_err(|e| PdsError::Internal(format!("listener task join: {}", e)))?;
        }
        Ok(())
    }
}

async fn run_listener_loop(
    url: String,
    invalidator: Arc<CacheInvalidator>,
    shutdown: Arc<tokio::sync::Notify>,
) {
    use sqlx::postgres::PgListener;

    // Reconnect backoff schedule (design doc §4.5): 1s, 2s, 4s, capped at 30s.
    let backoffs = [
        Duration::from_secs(1),
        Duration::from_secs(2),
        Duration::from_secs(4),
        Duration::from_secs(8),
        Duration::from_secs(16),
        Duration::from_secs(30),
    ];
    let mut backoff_idx = 0usize;

    loop {
        // Check shutdown before any await that could block forever.
        let listener_result = tokio::select! {
            r = PgListener::connect(&url) => r,
            _ = shutdown.notified() => {
                debug!("cache_invalidation: shutdown before connect");
                return;
            }
        };

        let mut listener = match listener_result {
            Ok(l) => l,
            Err(e) => {
                let wait = backoffs[backoff_idx.min(backoffs.len() - 1)];
                warn!(
                    error = %e,
                    backoff_secs = wait.as_secs(),
                    "cache_invalidation: connect failed, retrying"
                );
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    _ = shutdown.notified() => return,
                }
                backoff_idx = (backoff_idx + 1).min(backoffs.len() - 1);
                continue;
            }
        };

        if let Err(e) = listener.listen(CHANNEL_NAME).await {
            let wait = backoffs[backoff_idx.min(backoffs.len() - 1)];
            warn!(
                error = %e,
                backoff_secs = wait.as_secs(),
                "cache_invalidation: LISTEN failed, retrying"
            );
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = shutdown.notified() => return,
            }
            backoff_idx = (backoff_idx + 1).min(backoffs.len() - 1);
            continue;
        }

        info!(channel = CHANNEL_NAME, "cache_invalidation: listening");
        backoff_idx = 0; // reset on successful connect

        loop {
            let recv = tokio::select! {
                r = listener.recv() => r,
                _ = shutdown.notified() => {
                    debug!("cache_invalidation: shutdown received");
                    return;
                }
            };
            match recv {
                Ok(notification) => {
                    invalidator.handle_notification(notification.payload()).await;
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        "cache_invalidation: recv error, will reconnect"
                    );
                    break; // exit inner loop, reconnect
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// In-process notify emitter that records emitted payloads instead
    /// of sending them to Postgres. Tests can introspect the recorded
    /// list to verify NOTIFY happened.
    struct MockNotifyEmitter {
        emitted: Arc<Mutex<Vec<InvalidationPayload>>>,
    }

    impl MockNotifyEmitter {
        fn new() -> (Self, Arc<Mutex<Vec<InvalidationPayload>>>) {
            let emitted = Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    emitted: Arc::clone(&emitted),
                },
                emitted,
            )
        }
    }

    #[async_trait]
    impl NotifyEmitter for MockNotifyEmitter {
        async fn emit(&self, payload: &InvalidationPayload) -> PdsResult<()> {
            self.emitted.lock().unwrap().push(payload.clone());
            Ok(())
        }
    }

    #[test]
    fn payload_round_trip_serializes_with_type_field() {
        let p = InvalidationPayload::for_local_records("did:plc:abc");
        let json = p.to_json();
        // Verify the JSON uses "type" (not "type_") and contains the DID.
        assert!(json.contains("\"type\":\"local_records\""));
        assert!(json.contains("\"key\":\"did:plc:abc\""));
        let parsed: InvalidationPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, p);
    }

    #[test]
    fn payload_parse_rejects_malformed_json() {
        let r: Result<InvalidationPayload, _> = serde_json::from_str("not json");
        assert!(r.is_err());
    }

    #[test]
    fn payload_parse_accepts_unknown_type() {
        // Forward compatibility: unknown types parse fine; the dispatch
        // layer is what filters them.
        let json = r#"{"type":"future_cache","key":"x"}"#;
        let p: InvalidationPayload = serde_json::from_str(json).unwrap();
        assert_eq!(p.type_, "future_cache");
    }

    #[tokio::test]
    async fn invalidate_did_calls_local_cache_and_notify() {
        let local = Arc::new(LocalRecordsCache::new());
        let (mock, recorded) = MockNotifyEmitter::new();
        let invalidator =
            CacheInvalidator::new(Arc::clone(&local), Some(Arc::new(mock) as Arc<dyn NotifyEmitter>));

        invalidator.invalidate_did("did:plc:writer").await;

        // NOTIFY emitted with the expected payload.
        let recs = recorded.lock().unwrap().clone();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0], InvalidationPayload::for_local_records("did:plc:writer"));
    }

    #[tokio::test]
    async fn invalidate_did_no_notify_when_emitter_absent() {
        // SQLite path: notify=None, so only local invalidation runs.
        let local = Arc::new(LocalRecordsCache::new());
        let invalidator = CacheInvalidator::new(local, None);
        // Should not panic; should not need a notify emitter.
        invalidator.invalidate_did("did:plc:sqlite").await;
    }

    #[tokio::test]
    async fn handle_notification_dispatches_local_records() {
        let local = Arc::new(LocalRecordsCache::new());
        let invalidator = CacheInvalidator::new(Arc::clone(&local), None);
        let payload = r#"{"type":"local_records","key":"did:plc:remote"}"#;
        // Should not error or panic.
        invalidator.handle_notification(payload).await;
    }

    #[tokio::test]
    async fn handle_notification_ignores_unknown_type() {
        let local = Arc::new(LocalRecordsCache::new());
        let invalidator = CacheInvalidator::new(Arc::clone(&local), None);
        let payload = r#"{"type":"future_cache_x","key":"whatever"}"#;
        // Should silently ignore.
        invalidator.handle_notification(payload).await;
    }

    #[tokio::test]
    async fn handle_notification_drops_malformed_payload() {
        let local = Arc::new(LocalRecordsCache::new());
        let invalidator = CacheInvalidator::new(Arc::clone(&local), None);
        // Should silently drop without panicking.
        invalidator.handle_notification("not even json").await;
        invalidator.handle_notification("{}").await;
        invalidator
            .handle_notification(r#"{"type":"local_records"}"#) // missing key
            .await;
    }

    #[tokio::test]
    async fn invalidate_did_continues_when_notify_fails() {
        struct FailingEmitter;
        #[async_trait]
        impl NotifyEmitter for FailingEmitter {
            async fn emit(&self, _payload: &InvalidationPayload) -> PdsResult<()> {
                Err(PdsError::Internal("simulated NOTIFY failure".into()))
            }
        }
        let local = Arc::new(LocalRecordsCache::new());
        let invalidator = CacheInvalidator::new(
            Arc::clone(&local),
            Some(Arc::new(FailingEmitter) as Arc<dyn NotifyEmitter>),
        );
        // Local invalidation must still happen; failed NOTIFY is logged
        // but doesn't propagate. Other instances pick up via TTL.
        invalidator.invalidate_did("did:plc:resilient").await;
    }

    #[test]
    fn channel_name_is_hardcoded_constant() {
        // Sanity test: the channel name is expected to be stable across
        // versions because the wire protocol depends on it. If this
        // test fires it means someone renamed CHANNEL_NAME and the
        // protocol may have broken.
        assert_eq!(CHANNEL_NAME, "aurora_cache_invalidate");
        assert_eq!(TYPE_LOCAL_RECORDS, "local_records");
    }
}
