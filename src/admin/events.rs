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

    /// Log a moderation event
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

        let id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO moderation_event (event_type, actor_did, subject_did, subject_uri, subject_cid, details, created_at, meta)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            RETURNING id
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
        .fetch_one(&self.db)
        .await?;

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
}
