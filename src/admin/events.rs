/// Moderation Event Logging System
///
/// Comprehensive audit log for all moderation actions.
/// Provides transparency, accountability, and compliance tracking.

use crate::error::{PdsError, PdsResult};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

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

    pub fn from_str(s: &str) -> PdsResult<Self> {
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

/// Moderation event logger
#[derive(Clone)]
pub struct ModerationEventLogger {
    db: SqlitePool,
}

impl ModerationEventLogger {
    pub fn new(db: SqlitePool) -> Self {
        Self { db }
    }

    /// Log a moderation event
    pub async fn log_event(
        &self,
        event_type: ModerationEventType,
        actor_did: &str,
        subject_did: Option<&str>,
        subject_uri: Option<&str>,
        subject_cid: Option<&str>,
        details: serde_json::Value,
        meta: Option<serde_json::Value>,
    ) -> PdsResult<ModerationEvent> {
        let now = Utc::now();

        let details_json = serde_json::to_string(&details)
            .map_err(|e| PdsError::Internal(format!("Failed to serialize details: {}", e)))?;

        let meta_json = meta
            .as_ref()
            .map(|m| serde_json::to_string(m))
            .transpose()
            .map_err(|e| PdsError::Internal(format!("Failed to serialize meta: {}", e)))?;

        let result = sqlx::query(
            r#"
            INSERT INTO moderation_event (event_type, actor_did, subject_did, subject_uri, subject_cid, details, created_at, meta)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(event_type.as_str())
        .bind(actor_did)
        .bind(subject_did)
        .bind(subject_uri)
        .bind(subject_cid)
        .bind(&details_json)
        .bind(now.to_rfc3339())
        .bind(&meta_json)
        .execute(&self.db)
        .await?;

        tracing::info!(
            "Logged moderation event: {:?} by {} (subject_did: {:?}, subject_uri: {:?})",
            event_type,
            actor_did,
            subject_did,
            subject_uri
        );

        Ok(ModerationEvent {
            id: result.last_insert_rowid(),
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
            WHERE actor_did = ?
            ORDER BY created_at DESC
            LIMIT ?
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
            WHERE event_type = ?
            ORDER BY created_at DESC
            LIMIT ?
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
            LIMIT ?
            "#,
        )
        .bind(limit)
        .fetch_all(&self.db)
        .await?;

        self.parse_events(rows).await
    }

    /// Parse database rows into ModerationEvent objects
    async fn parse_events(&self, rows: Vec<sqlx::sqlite::SqliteRow>) -> PdsResult<Vec<ModerationEvent>> {
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

    #[tokio::test]
    async fn test_log_and_retrieve_event() {
        let db = SqlitePool::connect(":memory:").await.unwrap();

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

        let logger = ModerationEventLogger::new(db);

        // Log event
        let details = serde_json::json!({
            "reason": "Spam content",
            "moderation_id": 123
        });

        let event = logger
            .log_event(
                ModerationEventType::AccountTakedown,
                "did:plc:admin",
                Some("did:plc:spammer"),
                None,
                None,
                details.clone(),
                None,
            )
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
}
