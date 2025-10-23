/// Main Sequencer implementation
use crate::{
    error::{PdsError, PdsResult},
    sequencer::{
        events::{AccountEvent, CommitEvent, IdentityEvent},
        EventType, SeqEvent, SeqRow,
    },
};
use chrono::Utc;
use serde_cbor;
use sqlx::{Row, SqlitePool};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Sequencer configuration
#[derive(Debug, Clone)]
pub struct SequencerConfig {
    /// Maximum number of events to return in a single query
    pub max_query_limit: i64,

    /// Backfill time window in seconds (how far back cursors can resume)
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
    db: SqlitePool,
    config: SequencerConfig,
    last_seq: Arc<RwLock<Option<i64>>>,
}

impl Sequencer {
    /// Create a new sequencer
    pub fn new(db: SqlitePool, config: SequencerConfig) -> Self {
        Self {
            db,
            config,
            last_seq: Arc::new(RwLock::new(None)),
        }
    }

    /// Sequence a commit event
    pub async fn sequence_commit(&self, evt: CommitEvent) -> PdsResult<i64> {
        let event_bytes = serde_cbor::to_vec(&evt)
            .map_err(|e| PdsError::Internal(format!("Failed to encode commit event: {}", e)))?;

        self.insert_event(&evt.repo, EventType::Commit, event_bytes)
            .await
    }

    /// Sequence an identity event
    pub async fn sequence_identity(&self, evt: IdentityEvent) -> PdsResult<i64> {
        let event_bytes = serde_cbor::to_vec(&evt)
            .map_err(|e| PdsError::Internal(format!("Failed to encode identity event: {}", e)))?;

        self.insert_event(&evt.did, EventType::Identity, event_bytes)
            .await
    }

    /// Sequence an account event
    pub async fn sequence_account(&self, evt: AccountEvent) -> PdsResult<i64> {
        let event_bytes = serde_cbor::to_vec(&evt)
            .map_err(|e| PdsError::Internal(format!("Failed to encode account event: {}", e)))?;

        self.insert_event(&evt.did, EventType::Account, event_bytes)
            .await
    }

    /// Insert event into database
    async fn insert_event(&self, did: &str, event_type: EventType, event: Vec<u8>) -> PdsResult<i64> {
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
        .map_err(|e| PdsError::Database(e))?;

        let seq: i64 = result.try_get("seq")?;

        // Update last seq
        let mut last = self.last_seq.write().await;
        *last = Some(seq);

        Ok(seq)
    }

    /// Get current maximum sequence number
    pub async fn current_seq(&self) -> PdsResult<Option<i64>> {
        let result = sqlx::query("SELECT MAX(seq) as max_seq FROM repo_seq WHERE invalidated = 0")
            .fetch_one(&self.db)
            .await
            .map_err(|e| PdsError::Database(e))?;

        Ok(result.try_get("max_seq").ok())
    }

    /// Get next event after cursor
    pub async fn next_event(&self, cursor: i64) -> PdsResult<Option<SeqRow>> {
        let result = sqlx::query(
            r#"
            SELECT seq, did, event_type, event, invalidated, sequenced_at
            FROM repo_seq
            WHERE seq > ?1 AND invalidated = 0
            ORDER BY seq ASC
            LIMIT 1
            "#,
        )
        .bind(cursor)
        .fetch_optional(&self.db)
        .await
        .map_err(|e| PdsError::Database(e))?;

        if let Some(row) = result {
            Ok(Some(self.row_to_seq_row(row)?))
        } else {
            Ok(None)
        }
    }

    /// Request events in a sequence range
    pub async fn request_seq_range(
        &self,
        earliest_seq: Option<i64>,
        latest_seq: Option<i64>,
        limit: Option<i64>,
    ) -> PdsResult<Vec<SeqEvent>> {
        let limit = limit
            .unwrap_or(500)
            .min(self.config.max_query_limit);

        let mut query_str = String::from(
            "SELECT seq, did, event_type, event, invalidated, sequenced_at FROM repo_seq WHERE invalidated = 0"
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
            .map_err(|e| PdsError::Database(e))?;

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
    fn row_to_seq_row(&self, row: sqlx::sqlite::SqliteRow) -> PdsResult<SeqRow> {
        use chrono::DateTime;

        Ok(SeqRow {
            seq: row.try_get("seq")?,
            did: row.try_get("did")?,
            event_type: row.try_get("event_type")?,
            event: row.try_get("event")?,
            invalidated: row.try_get::<i32, _>("invalidated")? != 0,
            sequenced_at: {
                let time_str: String = row.try_get("sequenced_at")?;
                DateTime::parse_from_rfc3339(&time_str)
                    .map_err(|e| PdsError::Internal(format!("Invalid timestamp: {}", e)))?
                    .with_timezone(&Utc)
            },
        })
    }

    /// Decode event from SeqRow
    fn decode_event(&self, row: SeqRow) -> PdsResult<Option<SeqEvent>> {
        let time = row.sequenced_at.to_rfc3339();
        let event_type: EventType = row.event_type.into();

        match event_type {
            EventType::Commit => {
                let evt: CommitEvent = serde_cbor::from_slice(&row.event)
                    .map_err(|e| PdsError::Internal(format!("Failed to decode commit event: {}", e)))?;
                Ok(Some(SeqEvent::Commit {
                    seq: row.seq,
                    time,
                    evt,
                }))
            }
            EventType::Identity => {
                let evt: IdentityEvent = serde_cbor::from_slice(&row.event)
                    .map_err(|e| PdsError::Internal(format!("Failed to decode identity event: {}", e)))?;
                Ok(Some(SeqEvent::Identity {
                    seq: row.seq,
                    time,
                    evt,
                }))
            }
            EventType::Account => {
                let evt: AccountEvent = serde_cbor::from_slice(&row.event)
                    .map_err(|e| PdsError::Internal(format!("Failed to decode account event: {}", e)))?;
                Ok(Some(SeqEvent::Account {
                    seq: row.seq,
                    time,
                    evt,
                }))
            }
        }
    }

    /// Get events for a specific DID
    pub async fn get_events_for_did(&self, did: &str, limit: i64) -> PdsResult<Vec<SeqEvent>> {
        let rows = sqlx::query(
            r#"
            SELECT seq, did, event_type, event, invalidated, sequenced_at
            FROM repo_seq
            WHERE did = ?1 AND invalidated = 0
            ORDER BY seq DESC
            LIMIT ?2
            "#,
        )
        .bind(did)
        .bind(limit.min(self.config.max_query_limit))
        .fetch_all(&self.db)
        .await
        .map_err(|e| PdsError::Database(e))?;

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
    use super::*;

    async fn create_test_sequencer() -> Sequencer {
        let db = SqlitePool::connect(":memory:").await.unwrap();

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
        let events = sequencer.request_seq_range(Some(2), Some(4), None).await.unwrap();
        assert_eq!(events.len(), 2); // seq 3 and 4
    }
}
