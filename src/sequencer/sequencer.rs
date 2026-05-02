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
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

/// Sequencer configuration
#[derive(Debug, Clone)]
pub struct SequencerConfig {
    /// Maximum number of events to return in a single query
    pub max_query_limit: i64,

    /// Backfill time window in seconds (how far back cursors can resume)
    #[allow(dead_code)] // Will be used for cursor expiration logic
    pub backfill_limit_secs: i64,
}

impl Default for SequencerConfig {
    fn default() -> Self {
        Self {
            max_query_limit: 1000,
            backfill_limit_secs: 14 * 24 * 60 * 60, // 14 days
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
}

impl Sequencer {
    /// Create a new sequencer
    #[allow(dead_code)] // Public API for future use
    pub fn new(db: AnyPool, config: SequencerConfig) -> Self {
        Self {
            db,
            config,
            last_seq: Arc::new(RwLock::new(None)),
            relay_client: None,
        }
    }

    /// Create a new sequencer with relay client for federation
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
        }
    }

    /// Sequence a commit event
    pub async fn sequence_commit(&self, evt: CommitEvent) -> PdsResult<i64> {
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
    #[allow(dead_code)] // Will be used when implementing account status changes
    pub async fn sequence_account(&self, evt: AccountEvent) -> PdsResult<i64> {
        let event_bytes = serde_cbor::to_vec(&evt)
            .map_err(|e| PdsError::Internal(format!("Failed to encode account event: {}", e)))?;

        let seq = self
            .insert_event(&evt.did, EventType::Account, event_bytes)
            .await?;

        // Publish to relay if configured
        self.publish_to_relay("account", &evt.did, seq, None).await;

        Ok(seq)
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
            VALUES (?1, ?2, ?3, ?4)
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
            WHERE seq > ?1 AND NOT invalidated
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
            WHERE seq > ?1 AND NOT invalidated
            ORDER BY seq ASC
            LIMIT ?2
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
            invalidated: row.try_get::<i64, _>("invalidated")? != 0,
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
            WHERE did = ?1 AND NOT invalidated
            ORDER BY seq DESC
            LIMIT ?2
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
}
