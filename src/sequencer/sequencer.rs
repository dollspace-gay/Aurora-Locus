/// Main Sequencer implementation
use crate::{
    error::{PdsError, PdsResult},
    federation::RelayClient,
    sequencer::{
        events::{AccountEvent, CommitEvent, IdentityEvent, SyncEvent},
        EventType, SeqEvent, SeqRow,
    },
};
use chrono::Utc;
use serde_cbor;
use sqlx::{AnyPool, Row};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

/// Sequencer configuration.
///
/// `backfill_limit_secs` is consumed by [`Sequencer::earliest_after_time`]
/// via [`Sequencer::backfill_limit`] in the firehose handler's
/// pre-stream cursor-window check (Arc 14 §7.3.3).
#[derive(Debug, Clone)]
pub struct SequencerConfig {
    /// Maximum number of events to return in a single query
    pub max_query_limit: i64,

    /// Backfill time window in seconds (how far back cursors can resume).
    /// Default 1 day per Arc 14 Step 0 sub-step 0.C (bsky-PDS verified
    /// default: `repoBackfillLimitMs = DAY = 86_400_000 ms`).
    /// Env-overridable via `PDS_REPO_BACKFILL_LIMIT_MS` (read at
    /// AppContext construction).
    pub backfill_limit_secs: i64,
}

impl Default for SequencerConfig {
    fn default() -> Self {
        Self {
            max_query_limit: 1000,
            // Arc 14 §7.3.3 default: 1 day. Verified against bsky-PDS
            // (`packages/pds/src/config/config.ts:175` → `DAY` from
            // `@atproto/common`). Recon Sub-step 0.C closure.
            backfill_limit_secs: 24 * 60 * 60,
        }
    }
}

/// Main sequencer - manages event log
#[derive(Clone)]
pub struct Sequencer {
    db: AnyPool,
    config: SequencerConfig,
    last_seq: Arc<RwLock<Option<i64>>>,
    relay_client: Option<Arc<Mutex<RelayClient>>>,
    /// Multi-instance leadership flag (chainlink #89, design doc §3.5).
    /// `true` = this process is the sequencer leader and may write
    /// firehose events; `false` = standby, writes return NotLeader.
    /// Default `true` preserves single-instance behaviour for SQLite
    /// deployments and Postgres deployments without leader election
    /// wired up.
    is_leader: Arc<AtomicBool>,
}

impl Sequencer {
    /// Create a new sequencer (single-instance mode — `is_leader` defaults
    /// to true).
    #[allow(dead_code)] // Public API for future use
    pub fn new(db: AnyPool, config: SequencerConfig) -> Self {
        Self {
            db,
            config,
            last_seq: Arc::new(RwLock::new(None)),
            relay_client: None,
            is_leader: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Create a new sequencer with relay client for federation
    /// (single-instance mode — `is_leader` defaults to true).
    pub fn with_relay(
        db: AnyPool,
        config: SequencerConfig,
        relay_client: Option<Arc<Mutex<RelayClient>>>,
    ) -> Self {
        Self {
            db,
            config,
            last_seq: Arc::new(RwLock::new(None)),
            relay_client,
            is_leader: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Replace the internal leadership flag with a shared `Arc<AtomicBool>`
    /// owned by [`super::LeaderElection`]. Call this once during startup
    /// for multi-instance Postgres deployments before the sequencer sees
    /// any writes; SQLite and single-instance deployments leave the
    /// default-true flag in place. See chainlink #89 / design doc §3.5.
    pub fn attach_leader_flag(&mut self, flag: Arc<AtomicBool>) {
        self.is_leader = flag;
    }

    /// Cheap atomic load — checked at the top of every write path.
    /// Consumed by `tests/multi_instance_test.rs` for failover
    /// assertions; the `--lib` build doesn't see integration tests.
    #[allow(dead_code)]
    pub fn is_leader(&self) -> bool {
        self.is_leader.load(Ordering::SeqCst)
    }

    /// Returns a clone of the leadership flag handle so the leader-election
    /// task can mutate it.
    pub fn leader_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.is_leader)
    }

    /// Reject writes from non-leaders with `PdsError::NotLeader` (HTTP 503).
    /// Per design doc §3.1 / §7.1: load balancers retry on the next
    /// instance, eventually landing on the leader.
    fn check_leader(&self) -> PdsResult<()> {
        if self.is_leader.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(PdsError::NotLeader(
                "sequencer leader is on a different instance; retry".to_string(),
            ))
        }
    }

    /// Sequence a commit event
    pub async fn sequence_commit(&self, evt: CommitEvent) -> PdsResult<i64> {
        self.check_leader()?;
        let event_bytes = serde_cbor::to_vec(&evt)
            .map_err(|e| PdsError::Internal(format!("Failed to encode commit event: {}", e)))?;

        let seq = self
            .insert_event(&evt.repo, EventType::Commit, event_bytes)
            .await?;

        // Publish to relay if configured
        self.publish_to_relay("commit", &evt.repo, seq, Some(&evt.commit))
            .await;

        Ok(seq)
    }

    /// Sequence a sync event (lightweight repo state sync for account creation/activation)
    #[allow(dead_code)] // Will be used when implementing sync events
    pub async fn sequence_sync(&self, evt: SyncEvent) -> PdsResult<i64> {
        self.check_leader()?;
        let event_bytes = serde_cbor::to_vec(&evt)
            .map_err(|e| PdsError::Internal(format!("Failed to encode sync event: {}", e)))?;

        let seq = self
            .insert_event(&evt.did, EventType::Sync, event_bytes)
            .await?;

        // Publish to relay if configured
        self.publish_to_relay("sync", &evt.did, seq, None).await;

        Ok(seq)
    }

    /// Sequence an identity event
    pub async fn sequence_identity(&self, evt: IdentityEvent) -> PdsResult<i64> {
        self.check_leader()?;
        let event_bytes = serde_cbor::to_vec(&evt)
            .map_err(|e| PdsError::Internal(format!("Failed to encode identity event: {}", e)))?;

        let seq = self
            .insert_event(&evt.did, EventType::Identity, event_bytes)
            .await?;

        // Publish to relay if configured
        self.publish_to_relay("identity", &evt.did, seq, None).await;

        Ok(seq)
    }

    /// Sequence an account event
    pub async fn sequence_account(&self, evt: AccountEvent) -> PdsResult<i64> {
        self.check_leader()?;
        let event_bytes = serde_cbor::to_vec(&evt)
            .map_err(|e| PdsError::Internal(format!("Failed to encode account event: {}", e)))?;

        let seq = self
            .insert_event(&evt.did, EventType::Account, event_bytes)
            .await?;

        // Publish to relay if configured
        self.publish_to_relay("account", &evt.did, seq, None).await;

        Ok(seq)
    }

    /// Arc 15 §8.3.3 — retention helper for the deletion path:
    /// wipe every `repo_seq` row for `did` EXCEPT the seqs listed in
    /// `excluding`. Used by `delete_account` to retain the deletion
    /// `#account` event while clearing prior history.
    ///
    /// Two-await non-atomic semantics with the preceding
    /// `sequence_account(Deleted)` call — matches bsky-PDS pattern;
    /// race window documented in §8.5.5. Consumers
    /// duplicate-suppress on `did`.
    pub async fn delete_all_for_user(
        &self,
        did: &str,
        excluding: &[i64],
    ) -> PdsResult<u64> {
        self.check_leader()?;
        let result = if excluding.is_empty() {
            sqlx::query("DELETE FROM repo_seq WHERE did = $1")
                .bind(did)
                .execute(&self.db)
                .await
                .map_err(PdsError::Database)?
        } else {
            // Build the NOT IN clause inline — `excluding` is
            // operator-controlled (handler-supplied seqs only;
            // not user input) so binding as integers is safe.
            let placeholders: Vec<String> = (2..=excluding.len() + 1)
                .map(|i| format!("${}", i))
                .collect();
            let sql = format!(
                "DELETE FROM repo_seq WHERE did = $1 AND seq NOT IN ({})",
                placeholders.join(",")
            );
            let mut q = sqlx::query(&sql).bind(did);
            for seq in excluding {
                q = q.bind(*seq);
            }
            q.execute(&self.db).await.map_err(PdsError::Database)?
        };
        Ok(result.rows_affected())
    }

    /// Insert event into database
    async fn insert_event(
        &self,
        did: &str,
        event_type: EventType,
        event: Vec<u8>,
    ) -> PdsResult<i64> {
        let now = Utc::now().to_rfc3339();

        let result = sqlx::query(
            r#"
            INSERT INTO repo_seq (did, event_type, event, sequenced_at)
            VALUES ($1, $2, $3, $4)
            RETURNING seq
            "#,
        )
        .bind(did)
        .bind(event_type.as_str())
        .bind(&event)
        .bind(&now)
        .fetch_one(&self.db)
        .await
        .map_err(PdsError::Database)?;

        let seq: i64 = result.try_get("seq")?;

        // Update last seq
        let mut last = self.last_seq.write().await;
        *last = Some(seq);

        Ok(seq)
    }

    /// Arc 14 §7.3.3 / §7.4 Step 3: configured backfill window in
    /// seconds. The firehose handler consumes this to compute the
    /// cut-off time for the cursor-window OutdatedCursor check.
    pub fn backfill_limit_secs(&self) -> i64 {
        self.config.backfill_limit_secs
    }

    /// Arc 14 §7.3.3 / §7.4 Step 3: lowest emitted `seq` whose
    /// `sequenced_at >= time`. Returns `None` if the table is empty
    /// or every row predates `time`.
    ///
    /// Used by the firehose handler to advance an outdated cursor to
    /// the start of the configured backfill window. Empty-window
    /// fall-through to live-tail is the caller's responsibility
    /// (round-1 F8 closure).
    pub async fn earliest_after_time(
        &self,
        time: chrono::DateTime<Utc>,
    ) -> PdsResult<Option<i64>> {
        let time_str = time.to_rfc3339();
        let row = sqlx::query(
            r#"
            SELECT MIN(seq) as min_seq
            FROM repo_seq
            WHERE NOT invalidated AND sequenced_at >= $1
            "#,
        )
        .bind(time_str)
        .fetch_one(&self.db)
        .await
        .map_err(PdsError::Database)?;
        let min_seq: Option<i64> = row.try_get("min_seq").map_err(PdsError::Database)?;
        Ok(min_seq)
    }

    /// Get current maximum sequence number
    pub async fn current_seq(&self) -> PdsResult<Option<i64>> {
        // `MAX(seq)` over an empty (or fully-invalidated) `repo_seq` returns
        // NULL — SQL aggregate behaviour. Request `Option<i64>` explicitly so
        // sqlx maps that NULL onto `None` instead of falling back through
        // `.try_get(...).ok()`, which can produce `Some(0)` on some sqlx
        // versions when decoding NULL into a non-Option `i64`.
        let result = sqlx::query("SELECT MAX(seq) as max_seq FROM repo_seq WHERE NOT invalidated")
            .fetch_one(&self.db)
            .await
            .map_err(PdsError::Database)?;

        let max_seq: Option<i64> = result.try_get("max_seq").map_err(PdsError::Database)?;
        Ok(max_seq)
    }

    /// Get next event after cursor (single event)
    pub async fn next_event(&self, cursor: i64) -> PdsResult<Option<SeqRow>> {
        let result = sqlx::query(
            r#"
            SELECT seq, did, event_type, event, invalidated, sequenced_at
            FROM repo_seq
            WHERE seq > $1 AND NOT invalidated
            ORDER BY seq ASC
            LIMIT 1
            "#,
        )
        .bind(cursor)
        .fetch_optional(&self.db)
        .await
        .map_err(PdsError::Database)?;

        if let Some(row) = result {
            Ok(Some(self.row_to_seq_row(row)?))
        } else {
            Ok(None)
        }
    }

    /// Get multiple events after cursor (batch query for catch-up)
    ///
    /// This method is optimized for catch-up scenarios where a client is behind
    /// and needs to fetch multiple events efficiently. Returns raw SeqRow data
    /// for minimal overhead.
    ///
    /// # Arguments
    /// * `cursor` - Starting sequence number (exclusive)
    /// * `limit` - Maximum number of events to return (default: 500)
    pub async fn next_events(&self, cursor: i64, limit: Option<i64>) -> PdsResult<Vec<SeqRow>> {
        let limit = limit.unwrap_or(500).min(self.config.max_query_limit);

        let rows = sqlx::query(
            r#"
            SELECT seq, did, event_type, event, invalidated, sequenced_at
            FROM repo_seq
            WHERE seq > $1 AND NOT invalidated
            ORDER BY seq ASC
            LIMIT $2
            "#,
        )
        .bind(cursor)
        .bind(limit)
        .fetch_all(&self.db)
        .await
        .map_err(PdsError::Database)?;

        rows.into_iter()
            .map(|row| self.row_to_seq_row(row))
            .collect()
    }

    /// Request events in a sequence range
    #[allow(dead_code)] // Public API for firehose cursor-based queries
    pub async fn request_seq_range(
        &self,
        earliest_seq: Option<i64>,
        latest_seq: Option<i64>,
        limit: Option<i64>,
    ) -> PdsResult<Vec<SeqEvent>> {
        let limit = limit.unwrap_or(500).min(self.config.max_query_limit);

        let mut query_str = String::from(
            "SELECT seq, did, event_type, event, invalidated, sequenced_at FROM repo_seq WHERE NOT invalidated"
        );

        let mut conditions = Vec::new();
        if let Some(earliest) = earliest_seq {
            conditions.push(format!("seq > {}", earliest));
        }
        if let Some(latest) = latest_seq {
            conditions.push(format!("seq <= {}", latest));
        }

        if !conditions.is_empty() {
            query_str.push_str(" AND ");
            query_str.push_str(&conditions.join(" AND "));
        }

        query_str.push_str(&format!(" ORDER BY seq ASC LIMIT {}", limit));

        let rows = sqlx::query(&query_str)
            .fetch_all(&self.db)
            .await
            .map_err(PdsError::Database)?;

        let mut events = Vec::new();
        for row in rows {
            let seq_row = self.row_to_seq_row(row)?;
            if let Some(evt) = self.decode_event(seq_row)? {
                events.push(evt);
            }
        }

        Ok(events)
    }

    /// Convert database row to SeqRow
    fn row_to_seq_row(&self, row: sqlx::any::AnyRow) -> PdsResult<SeqRow> {
        use chrono::DateTime;

        Ok(SeqRow {
            seq: row.try_get("seq")?,
            did: row.try_get("did")?,
            event_type: row.try_get("event_type")?,
            event: row.try_get("event")?,
            // BOOLEAN on Postgres, INTEGER 0/1 on SQLite — sqlx::Any
            // requires reading SQLite INTEGER as i64 and converting; chainlink #76.
            invalidated: crate::db::read_bool(&row, "invalidated")?,
            sequenced_at: {
                let time_str: String = row.try_get("sequenced_at")?;
                DateTime::parse_from_rfc3339(&time_str)
                    .map_err(|e| PdsError::Internal(format!("Invalid timestamp: {}", e)))?
                    .with_timezone(&Utc)
            },
        })
    }

    /// Decode event from SeqRow
    #[allow(dead_code)] // Used by public query methods
    fn decode_event(&self, row: SeqRow) -> PdsResult<Option<SeqEvent>> {
        let time = row.sequenced_at.to_rfc3339();
        let event_type: EventType = row.event_type.into();

        match event_type {
            EventType::Commit => {
                let evt: CommitEvent = serde_cbor::from_slice(&row.event).map_err(|e| {
                    PdsError::Internal(format!("Failed to decode commit event: {}", e))
                })?;
                Ok(Some(SeqEvent::Commit {
                    seq: row.seq,
                    time,
                    evt,
                }))
            }
            EventType::Sync => {
                let evt: SyncEvent = serde_cbor::from_slice(&row.event).map_err(|e| {
                    PdsError::Internal(format!("Failed to decode sync event: {}", e))
                })?;
                Ok(Some(SeqEvent::Sync {
                    seq: row.seq,
                    time,
                    evt,
                }))
            }
            EventType::Identity => {
                let evt: IdentityEvent = serde_cbor::from_slice(&row.event).map_err(|e| {
                    PdsError::Internal(format!("Failed to decode identity event: {}", e))
                })?;
                Ok(Some(SeqEvent::Identity {
                    seq: row.seq,
                    time,
                    evt,
                }))
            }
            EventType::Account => {
                let evt: AccountEvent = serde_cbor::from_slice(&row.event).map_err(|e| {
                    PdsError::Internal(format!("Failed to decode account event: {}", e))
                })?;
                Ok(Some(SeqEvent::Account {
                    seq: row.seq,
                    time,
                    evt,
                }))
            }
        }
    }

    /// Publish event to relay (non-blocking, errors logged but not propagated)
    async fn publish_to_relay(
        &self,
        event_type: &str,
        did: &str,
        seq: i64,
        commit_cid: Option<&str>,
    ) {
        if let Some(ref relay_client) = self.relay_client {
            use crate::federation::relay::RelayEvent;

            let relay_event = RelayEvent {
                event_type: event_type.to_string(),
                did: did.to_string(),
                seq,
                commit: commit_cid.map(|cid| serde_json::json!({ "cid": cid })),
                time: Utc::now().to_rfc3339(),
            };

            let client = relay_client.clone();
            let event_type_owned = event_type.to_string();
            tokio::spawn(async move {
                if let Err(e) = client.lock().await.publish_event(&relay_event).await {
                    tracing::warn!(
                        "Failed to publish event to relay: {} seq={}: {}",
                        event_type_owned,
                        relay_event.seq,
                        e
                    );
                } else {
                    tracing::debug!(
                        "Event published to relay: {} seq={}",
                        event_type_owned,
                        relay_event.seq
                    );
                }
            });
        }
    }

    /// Get events for a specific DID
    #[allow(dead_code)] // Public API for DID-specific event queries
    pub async fn get_events_for_did(&self, did: &str, limit: i64) -> PdsResult<Vec<SeqEvent>> {
        let rows = sqlx::query(
            r#"
            SELECT seq, did, event_type, event, invalidated, sequenced_at
            FROM repo_seq
            WHERE did = $1 AND NOT invalidated
            ORDER BY seq DESC
            LIMIT $2
            "#,
        )
        .bind(did)
        .bind(limit.min(self.config.max_query_limit))
        .fetch_all(&self.db)
        .await
        .map_err(PdsError::Database)?;

        let mut events = Vec::new();
        for row in rows {
            let seq_row = self.row_to_seq_row(row)?;
            if let Some(evt) = self.decode_event(seq_row)? {
                events.push(evt);
            }
        }

        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    async fn open_test_pool() -> sqlx::AnyPool {
        use std::sync::Once;
        static INSTALL: Once = Once::new();
        INSTALL.call_once(sqlx::any::install_default_drivers);
        sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap()
    }

    use super::*;

    async fn create_test_sequencer() -> Sequencer {
        let db = open_test_pool().await;

        // Create table
        sqlx::query(
            r#"
            CREATE TABLE repo_seq (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                did TEXT NOT NULL,
                event_type TEXT NOT NULL,
                event BLOB NOT NULL,
                invalidated INTEGER NOT NULL DEFAULT 0,
                sequenced_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        Sequencer::new(db, SequencerConfig::default())
    }

    #[tokio::test]
    async fn test_sequence_commit() {
        let sequencer = create_test_sequencer().await;

        let evt = CommitEvent::new(
            "did:plc:test".to_string(),
            "bafyrei123".to_string(),
            "3".to_string(),
            None,
            None,
            vec![],
            vec![],
        );

        let seq = sequencer.sequence_commit(evt).await.unwrap();
        assert_eq!(seq, 1);
    }

    #[tokio::test]
    async fn test_current_seq() {
        let sequencer = create_test_sequencer().await;

        // Initially empty
        assert_eq!(sequencer.current_seq().await.unwrap(), None);

        // After inserting
        let evt = CommitEvent::new(
            "did:plc:test".to_string(),
            "bafyrei123".to_string(),
            "3".to_string(),
            None,
            None,
            vec![],
            vec![],
        );
        sequencer.sequence_commit(evt).await.unwrap();

        assert_eq!(sequencer.current_seq().await.unwrap(), Some(1));
    }

    #[tokio::test]
    async fn test_request_seq_range() {
        let sequencer = create_test_sequencer().await;

        // Insert multiple events
        for i in 1..=5 {
            let evt = CommitEvent::new(
                format!("did:plc:test{}", i),
                format!("bafyrei{}", i),
                "3".to_string(),
                None,
                None,
                vec![],
                vec![],
            );
            sequencer.sequence_commit(evt).await.unwrap();
        }

        // Query range
        let events = sequencer
            .request_seq_range(Some(2), Some(4), None)
            .await
            .unwrap();
        assert_eq!(events.len(), 2); // seq 3 and 4
    }

    /// Arc 14 §7.3.3 / Sub-step 0.C: default backfill window is 1 day
    /// (86_400 seconds), matching bsky-PDS's
    /// `repoBackfillLimitMs = DAY` default.
    #[test]
    fn test_backfill_limit_default_one_day() {
        let config = SequencerConfig::default();
        assert_eq!(config.backfill_limit_secs, 24 * 60 * 60);
    }

    /// Arc 14 §7.4 Step 3: `earliest_after_time` returns `None` when
    /// no events exist after the cutoff time (round-1 F8 closure
    /// covered by the firehose handler's fall-through).
    #[tokio::test]
    async fn test_earliest_after_time_empty_returns_none() {
        let sequencer = create_test_sequencer().await;
        let future = chrono::Utc::now() + chrono::Duration::hours(1);
        let result = sequencer.earliest_after_time(future).await.unwrap();
        assert_eq!(result, None);
    }

    /// Arc 15 §8.3.3 / Step 1.4: `delete_all_for_user` with an empty
    /// `excluding` list removes every event for the DID.
    #[tokio::test]
    async fn test_delete_all_for_user_wipes_when_excluding_empty() {
        let sequencer = create_test_sequencer().await;
        for i in 1..=3 {
            sequencer
                .sequence_commit(CommitEvent::new(
                    "did:plc:alice".to_string(),
                    format!("bafyrei{}", i),
                    "3".to_string(),
                    None,
                    None,
                    vec![],
                    vec![],
                ))
                .await
                .unwrap();
        }
        let removed = sequencer.delete_all_for_user("did:plc:alice", &[]).await.unwrap();
        assert_eq!(removed, 3);
        assert_eq!(sequencer.current_seq().await.unwrap(), None);
    }

    /// Arc 15 §8.3.3 / Step 1.4: with `excluding = [retain_seq]`,
    /// only the retained seq survives the DID's wipe. Matches the
    /// `delete_account` two-await sequence (sequence_account first,
    /// then delete_all_for_user excluding the deletion seq).
    #[tokio::test]
    async fn test_delete_all_for_user_retains_excluded_seqs() {
        let sequencer = create_test_sequencer().await;
        for i in 1..=3 {
            sequencer
                .sequence_commit(CommitEvent::new(
                    "did:plc:bob".to_string(),
                    format!("bafyrei{}", i),
                    "3".to_string(),
                    None,
                    None,
                    vec![],
                    vec![],
                ))
                .await
                .unwrap();
        }
        // Seq 1, 2, 3 exist. Retain seq=2.
        let removed = sequencer.delete_all_for_user("did:plc:bob", &[2]).await.unwrap();
        assert_eq!(removed, 2);
        // Verify only seq 2 remains.
        let row = sqlx::query("SELECT seq FROM repo_seq WHERE did = $1")
            .bind("did:plc:bob")
            .fetch_one(&sequencer.db)
            .await
            .unwrap();
        let seq: i64 = row.try_get("seq").unwrap();
        assert_eq!(seq, 2);
    }

    /// Arc 15 §8.3.3 / Step 1.4: events for other DIDs are NOT touched.
    #[tokio::test]
    async fn test_delete_all_for_user_scoped_to_did() {
        let sequencer = create_test_sequencer().await;
        sequencer
            .sequence_commit(CommitEvent::new(
                "did:plc:alice".to_string(),
                "bafyrei-alice".to_string(),
                "3".to_string(),
                None,
                None,
                vec![],
                vec![],
            ))
            .await
            .unwrap();
        sequencer
            .sequence_commit(CommitEvent::new(
                "did:plc:bob".to_string(),
                "bafyrei-bob".to_string(),
                "3".to_string(),
                None,
                None,
                vec![],
                vec![],
            ))
            .await
            .unwrap();
        let removed = sequencer.delete_all_for_user("did:plc:alice", &[]).await.unwrap();
        assert_eq!(removed, 1);
        // bob's event still present.
        assert_eq!(sequencer.current_seq().await.unwrap(), Some(2));
    }

    /// Arc 14 §7.4 Step 3: with events present, `earliest_after_time`
    /// with a cutoff earlier than all events returns the lowest seq.
    #[tokio::test]
    async fn test_earliest_after_time_returns_lowest_seq() {
        let sequencer = create_test_sequencer().await;
        for i in 1..=3 {
            let evt = CommitEvent::new(
                format!("did:plc:test{}", i),
                format!("bafyrei{}", i),
                "3".to_string(),
                None,
                None,
                vec![],
                vec![],
            );
            sequencer.sequence_commit(evt).await.unwrap();
        }
        let past = chrono::Utc::now() - chrono::Duration::hours(1);
        let result = sequencer.earliest_after_time(past).await.unwrap();
        assert_eq!(result, Some(1));
    }
}
