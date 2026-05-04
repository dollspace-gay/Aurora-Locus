//! Moderation Event Logging System

// Allow dead_code - public APIs defined for future feature completion
#![allow(dead_code)]
//!
//! Comprehensive audit log for all moderation actions.
//! Provides transparency, accountability, and compliance tracking.

use crate::error::{PdsError, PdsResult};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{AnyPool, Row};
use std::str::FromStr;

/// Insert a moderation event PLUS its corresponding `mod_event_seq`
/// row in the same transaction. Per chainlink #115 / §3.5, every
/// `moderation_event` INSERT must also land a row in
/// `mod_event_seq` so the live subscription channel has a
/// retention-bounded source. Two writes inside one transaction
/// guarantee both rows land or neither — silent divergence between
/// the historical record and the streaming channel is the
/// invariant this helper enforces.
///
/// `details_json` and `meta_json` are pre-serialized strings so the
/// caller controls JSON shape (the existing helpers and direct
/// inserts use varying detail shapes; this helper matches that).
///
/// Callers that already hold a transaction (the batch handlers in
/// aurora_admin.rs) should use this helper directly. Callers that
/// don't hold a transaction (the `ModerationEventLogger::log_event`
/// canonical writer) open one, call the helper, and commit.
///
/// Returns the new `moderation_event.id` so callers can reference
/// it from related rows (label batches, account_moderation rows).
#[allow(clippy::too_many_arguments)]
pub async fn insert_moderation_event_in_tx<'c>(
    tx: &mut sqlx::Transaction<'c, sqlx::Any>,
    event_type: &str,
    actor_did: &str,
    subject_did: Option<&str>,
    subject_uri: Option<&str>,
    subject_cid: Option<&str>,
    details_json: &str,
    created_at: &str,
    meta_json: Option<&str>,
) -> Result<i64, sqlx::Error> {
    let event_id: i64 = sqlx::query_scalar(
        "INSERT INTO moderation_event \
         (event_type, actor_did, subject_did, subject_uri, subject_cid, \
          details, created_at, meta) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
         RETURNING id",
    )
    .bind(event_type)
    .bind(actor_did)
    .bind(subject_did)
    .bind(subject_uri)
    .bind(subject_cid)
    .bind(details_json)
    .bind(created_at)
    .bind(meta_json)
    .fetch_one(&mut **tx)
    .await?;

    // mod_event_seq mirrors the subset of moderation_event that the
    // `Event` wire variant emits. `meta` is intentionally NOT mirrored
    // — the wire format doesn't carry it.
    sqlx::query(
        "INSERT INTO mod_event_seq \
         (moderation_event_id, actor_did, action, subject_did, \
          subject_uri, subject_cid, detail, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(event_id)
    .bind(actor_did)
    .bind(event_type)
    .bind(subject_did)
    .bind(subject_uri)
    .bind(subject_cid)
    .bind(details_json)
    .bind(created_at)
    .execute(&mut **tx)
    .await?;

    Ok(event_id)
}

/// Moderation event types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModerationEventType {
    /// Account takedown
    AccountTakedown,
    /// Account suspension
    AccountSuspend,
    /// Account warning
    AccountWarn,
    /// Account restoration
    AccountRestore,
    /// Content label applied
    LabelCreate,
    /// Content label removed
    LabelRemove,
    /// Blob quarantined
    BlobQuarantine,
    /// Blob restored
    BlobRestore,
    /// Report submitted
    ReportSubmit,
    /// Report reviewed
    ReportReview,
    /// Appeal submitted
    AppealSubmit,
    /// Appeal reviewed
    AppealReview,
}

impl ModerationEventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ModerationEventType::AccountTakedown => "account_takedown",
            ModerationEventType::AccountSuspend => "account_suspend",
            ModerationEventType::AccountWarn => "account_warn",
            ModerationEventType::AccountRestore => "account_restore",
            ModerationEventType::LabelCreate => "label_create",
            ModerationEventType::LabelRemove => "label_remove",
            ModerationEventType::BlobQuarantine => "blob_quarantine",
            ModerationEventType::BlobRestore => "blob_restore",
            ModerationEventType::ReportSubmit => "report_submit",
            ModerationEventType::ReportReview => "report_review",
            ModerationEventType::AppealSubmit => "appeal_submit",
            ModerationEventType::AppealReview => "appeal_review",
        }
    }
}

impl FromStr for ModerationEventType {
    type Err = PdsError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "account_takedown" => Ok(ModerationEventType::AccountTakedown),
            "account_suspend" => Ok(ModerationEventType::AccountSuspend),
            "account_warn" => Ok(ModerationEventType::AccountWarn),
            "account_restore" => Ok(ModerationEventType::AccountRestore),
            "label_create" => Ok(ModerationEventType::LabelCreate),
            "label_remove" => Ok(ModerationEventType::LabelRemove),
            "blob_quarantine" => Ok(ModerationEventType::BlobQuarantine),
            "blob_restore" => Ok(ModerationEventType::BlobRestore),
            "report_submit" => Ok(ModerationEventType::ReportSubmit),
            "report_review" => Ok(ModerationEventType::ReportReview),
            "appeal_submit" => Ok(ModerationEventType::AppealSubmit),
            "appeal_review" => Ok(ModerationEventType::AppealReview),
            _ => Err(PdsError::Validation(format!("Invalid event type: {}", s))),
        }
    }
}

/// Moderation event record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModerationEvent {
    pub id: i64,
    pub event_type: ModerationEventType,
    pub actor_did: String,
    pub subject_did: Option<String>,
    pub subject_uri: Option<String>,
    pub subject_cid: Option<String>,
    pub details: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub meta: Option<serde_json::Value>,
}

/// Parameters for logging a moderation event
pub struct LogEventParams<'a> {
    pub event_type: ModerationEventType,
    pub actor_did: &'a str,
    pub subject_did: Option<&'a str>,
    pub subject_uri: Option<&'a str>,
    pub subject_cid: Option<&'a str>,
    pub details: serde_json::Value,
    pub meta: Option<serde_json::Value>,
}

/// Moderation event logger
#[derive(Clone)]
pub struct ModerationEventLogger {
    db: AnyPool,
}

impl ModerationEventLogger {
    pub fn new(db: AnyPool) -> Self {
        Self { db }
    }

    /// Log a moderation event. Routes the actual insert through
    /// [`insert_moderation_event_in_tx`] so every `moderation_event`
    /// row also gets a `mod_event_seq` row in the same transaction —
    /// the dual-write invariant chainlink #115 requires.
    pub async fn log_event(&self, params: LogEventParams<'_>) -> PdsResult<ModerationEvent> {
        let LogEventParams {
            event_type,
            actor_did,
            subject_did,
            subject_uri,
            subject_cid,
            details,
            meta,
        } = params;

        let now = Utc::now();

        let details_json = serde_json::to_string(&details)
            .map_err(|e| PdsError::Internal(format!("Failed to serialize details: {}", e)))?;

        let meta_json = meta
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| PdsError::Internal(format!("Failed to serialize meta: {}", e)))?;

        let mut tx = self.db.begin().await?;
        let id = insert_moderation_event_in_tx(
            &mut tx,
            event_type.as_str(),
            actor_did,
            subject_did,
            subject_uri,
            subject_cid,
            &details_json,
            &now.to_rfc3339(),
            meta_json.as_deref(),
        )
        .await?;
        tx.commit().await?;

        tracing::info!(
            "Logged moderation event: {:?} by {} (subject_did: {:?}, subject_uri: {:?})",
            event_type,
            actor_did,
            subject_did,
            subject_uri
        );

        Ok(ModerationEvent {
            id,
            event_type,
            actor_did: actor_did.to_string(),
            subject_did: subject_did.map(String::from),
            subject_uri: subject_uri.map(String::from),
            subject_cid: subject_cid.map(String::from),
            details,
            created_at: now,
            meta,
        })
    }

    /// Get events for a subject (DID or URI)
    pub async fn get_events_for_subject(
        &self,
        subject_did: Option<&str>,
        subject_uri: Option<&str>,
        limit: i64,
    ) -> PdsResult<Vec<ModerationEvent>> {
        let query = if subject_did.is_some() {
            r#"
            SELECT id, event_type, actor_did, subject_did, subject_uri, subject_cid, details, created_at, meta
            FROM moderation_event
            WHERE subject_did = ?
            ORDER BY created_at DESC
            LIMIT ?
            "#
        } else {
            r#"
            SELECT id, event_type, actor_did, subject_did, subject_uri, subject_cid, details, created_at, meta
            FROM moderation_event
            WHERE subject_uri = ?
            ORDER BY created_at DESC
            LIMIT ?
            "#
        };

        let rows = sqlx::query(query)
            .bind(subject_did.or(subject_uri).unwrap_or(""))
            .bind(limit)
            .fetch_all(&self.db)
            .await?;

        self.parse_events(rows).await
    }

    /// Get events by actor
    pub async fn get_events_by_actor(
        &self,
        actor_did: &str,
        limit: i64,
    ) -> PdsResult<Vec<ModerationEvent>> {
        let rows = sqlx::query(
            r#"
            SELECT id, event_type, actor_did, subject_did, subject_uri, subject_cid, details, created_at, meta
            FROM moderation_event
            WHERE actor_did = $1
            ORDER BY created_at DESC
            LIMIT $2
            "#,
        )
        .bind(actor_did)
        .bind(limit)
        .fetch_all(&self.db)
        .await?;

        self.parse_events(rows).await
    }

    /// Get events by type
    pub async fn get_events_by_type(
        &self,
        event_type: ModerationEventType,
        limit: i64,
    ) -> PdsResult<Vec<ModerationEvent>> {
        let rows = sqlx::query(
            r#"
            SELECT id, event_type, actor_did, subject_did, subject_uri, subject_cid, details, created_at, meta
            FROM moderation_event
            WHERE event_type = $1
            ORDER BY created_at DESC
            LIMIT $2
            "#,
        )
        .bind(event_type.as_str())
        .bind(limit)
        .fetch_all(&self.db)
        .await?;

        self.parse_events(rows).await
    }

    /// Get recent events
    pub async fn get_recent_events(&self, limit: i64) -> PdsResult<Vec<ModerationEvent>> {
        let rows = sqlx::query(
            r#"
            SELECT id, event_type, actor_did, subject_did, subject_uri, subject_cid, details, created_at, meta
            FROM moderation_event
            ORDER BY created_at DESC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.db)
        .await?;

        self.parse_events(rows).await
    }

    /// Parse database rows into ModerationEvent objects
    async fn parse_events(
        &self,
        rows: Vec<sqlx::any::AnyRow>,
    ) -> PdsResult<Vec<ModerationEvent>> {
        let mut events = Vec::new();

        for row in rows {
            let event_type_str: String = row.get("event_type");
            let event_type = ModerationEventType::from_str(&event_type_str)?;

            let created_at_str: String = row.get("created_at");
            let created_at = DateTime::parse_from_rfc3339(&created_at_str)
                .map_err(|e| PdsError::Internal(format!("Invalid timestamp: {}", e)))?
                .with_timezone(&Utc);

            let details_json: String = row.get("details");
            let details: serde_json::Value = serde_json::from_str(&details_json)
                .map_err(|e| PdsError::Internal(format!("Failed to parse details: {}", e)))?;

            let meta = row
                .try_get::<String, _>("meta")
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok());

            events.push(ModerationEvent {
                id: row.get("id"),
                event_type,
                actor_did: row.get("actor_did"),
                subject_did: row.get("subject_did"),
                subject_uri: row.get("subject_uri"),
                subject_cid: row.get("subject_cid"),
                details,
                created_at,
                meta,
            });
        }

        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn open_test_pool() -> AnyPool {
        use std::sync::Once;
        static INSTALL: Once = Once::new();
        INSTALL.call_once(sqlx::any::install_default_drivers);
        sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn test_log_and_retrieve_event() {
        let db = open_test_pool().await;

        sqlx::query(
            r#"
            CREATE TABLE moderation_event (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                event_type TEXT NOT NULL,
                actor_did TEXT NOT NULL,
                subject_did TEXT,
                subject_uri TEXT,
                subject_cid TEXT,
                details TEXT NOT NULL,
                created_at TEXT NOT NULL,
                meta TEXT
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        // mod_event_seq is the dual-write target the helper writes
        // alongside moderation_event. Tests that exercise log_event
        // need both tables present; chainlink #115.
        sqlx::query(
            r#"
            CREATE TABLE mod_event_seq (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                moderation_event_id INTEGER NOT NULL,
                actor_did TEXT NOT NULL,
                action TEXT NOT NULL,
                subject_did TEXT,
                subject_uri TEXT,
                subject_cid TEXT,
                detail TEXT,
                created_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        let logger = ModerationEventLogger::new(db);

        // Log event
        let details = serde_json::json!({
            "reason": "Spam content",
            "moderation_id": 123
        });

        let event = logger
            .log_event(LogEventParams {
                event_type: ModerationEventType::AccountTakedown,
                actor_did: "did:plc:admin",
                subject_did: Some("did:plc:spammer"),
                subject_uri: None,
                subject_cid: None,
                details: details.clone(),
                meta: None,
            })
            .await
            .unwrap();

        assert_eq!(event.event_type, ModerationEventType::AccountTakedown);
        assert_eq!(event.actor_did, "did:plc:admin");
        assert_eq!(event.details, details);

        // Retrieve by subject
        let events = logger
            .get_events_for_subject(Some("did:plc:spammer"), None, 10)
            .await
            .unwrap();

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, ModerationEventType::AccountTakedown);

        // Retrieve by actor
        let events = logger
            .get_events_by_actor("did:plc:admin", 10)
            .await
            .unwrap();

        assert_eq!(events.len(), 1);
    }

    /// Helper: create the same schema setup `test_log_and_retrieve_event`
    /// uses, returning the pool. Pulled out so the dual-write tests
    /// don't duplicate ~30 lines of CREATE TABLE.
    async fn open_test_pool_with_schema() -> AnyPool {
        let db = open_test_pool().await;
        sqlx::query(
            r#"
            CREATE TABLE moderation_event (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                event_type TEXT NOT NULL,
                actor_did TEXT NOT NULL,
                subject_did TEXT,
                subject_uri TEXT,
                subject_cid TEXT,
                details TEXT NOT NULL,
                created_at TEXT NOT NULL,
                meta TEXT
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query(
            r#"
            CREATE TABLE mod_event_seq (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                moderation_event_id INTEGER NOT NULL,
                actor_did TEXT NOT NULL,
                action TEXT NOT NULL,
                subject_did TEXT,
                subject_uri TEXT,
                subject_cid TEXT,
                detail TEXT,
                created_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();
        db
    }

    #[tokio::test]
    async fn log_event_dual_writes_moderation_event_and_mod_event_seq() {
        // chainlink #115 invariant: every moderation_event row must
        // have a corresponding mod_event_seq row. log_event is the
        // canonical writer; this test exercises that path end-to-end
        // and asserts both tables receive a row.
        let db = open_test_pool_with_schema().await;
        let logger = ModerationEventLogger::new(db.clone());
        let details = serde_json::json!({ "reason": "test" });

        let event = logger
            .log_event(LogEventParams {
                event_type: ModerationEventType::AccountTakedown,
                actor_did: "did:plc:m1",
                subject_did: Some("did:plc:s1"),
                subject_uri: None,
                subject_cid: None,
                details,
                meta: None,
            })
            .await
            .unwrap();

        // moderation_event row (the historical aggregate).
        let me_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM moderation_event WHERE id = ?",
        )
        .bind(event.id)
        .fetch_one(&db)
        .await
        .unwrap();
        assert_eq!(me_count, 1);

        // mod_event_seq row (the live subscription channel).
        let seq_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM mod_event_seq WHERE moderation_event_id = ?",
        )
        .bind(event.id)
        .fetch_one(&db)
        .await
        .unwrap();
        assert_eq!(seq_count, 1, "dual-write: mod_event_seq row missing");

        // Field mirror check: actor_did, action, subject_did, detail
        // all match between the two rows.
        let row: (String, String, Option<String>, String) = sqlx::query_as(
            "SELECT actor_did, action, subject_did, detail FROM mod_event_seq \
             WHERE moderation_event_id = ?",
        )
        .bind(event.id)
        .fetch_one(&db)
        .await
        .unwrap();
        assert_eq!(row.0, "did:plc:m1");
        assert_eq!(row.1, "account_takedown");
        assert_eq!(row.2.as_deref(), Some("did:plc:s1"));
        assert!(row.3.contains("\"reason\":\"test\""));
    }

    #[tokio::test]
    async fn direct_insert_bypassing_helper_does_not_populate_mod_event_seq() {
        // Pin the helper as the canonical write path (chainlink #115).
        // A direct INSERT INTO moderation_event (simulating a future
        // contributor who bypasses insert_moderation_event_in_tx)
        // must NOT cause a mod_event_seq row to materialize. If this
        // ever changes (e.g., the dual-write moves to a database
        // trigger), the test will fail and the next contributor will
        // know to update the helper-pin assumption.
        let db = open_test_pool_with_schema().await;

        let _id: i64 = sqlx::query_scalar(
            "INSERT INTO moderation_event \
             (event_type, actor_did, details, created_at) \
             VALUES ($1, $2, $3, $4) RETURNING id",
        )
        .bind("account_takedown")
        .bind("did:plc:m1")
        .bind("{}")
        .bind(Utc::now().to_rfc3339())
        .fetch_one(&db)
        .await
        .unwrap();

        let seq_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mod_event_seq")
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(
            seq_count, 0,
            "bypassing the helper must not populate mod_event_seq"
        );
    }
}
