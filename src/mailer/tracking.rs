// Allow dead_code - email tracking for future use
#![allow(dead_code)]

//! Email Delivery Tracking System
//!
//! Tracks email delivery status, bounces, and provides analytics.

use crate::error::{PdsError, PdsResult};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::str::FromStr;

/// Email delivery status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmailStatus {
    /// Email queued for sending
    Queued,
    /// Email sent successfully
    Sent,
    /// Email delivery failed
    Failed,
    /// Email bounced (hard bounce)
    Bounced,
    /// Email marked as spam/complaint
    Complaint,
    /// Email delivery attempt (soft bounce, will retry)
    Deferred,
}

impl EmailStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            EmailStatus::Queued => "queued",
            EmailStatus::Sent => "sent",
            EmailStatus::Failed => "failed",
            EmailStatus::Bounced => "bounced",
            EmailStatus::Complaint => "complaint",
            EmailStatus::Deferred => "deferred",
        }
    }
}

impl FromStr for EmailStatus {
    type Err = PdsError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "queued" => Ok(EmailStatus::Queued),
            "sent" => Ok(EmailStatus::Sent),
            "failed" => Ok(EmailStatus::Failed),
            "bounced" => Ok(EmailStatus::Bounced),
            "complaint" => Ok(EmailStatus::Complaint),
            "deferred" => Ok(EmailStatus::Deferred),
            _ => Err(PdsError::Validation(format!("Invalid email status: {}", s))),
        }
    }
}

/// Email delivery record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailDelivery {
    pub id: i64,
    pub recipient: String,
    pub subject: String,
    pub template_type: String,
    pub status: EmailStatus,
    pub sent_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub error_message: Option<String>,
    pub retry_count: i32,
    pub message_id: Option<String>,
}

/// Email tracking manager
#[derive(Clone)]
pub struct EmailTracker {
    db: SqlitePool,
}

impl EmailTracker {
    pub fn new(db: SqlitePool) -> Self {
        Self { db }
    }

    /// Record email send attempt
    pub async fn record_send(
        &self,
        recipient: &str,
        subject: &str,
        template_type: &str,
        message_id: Option<&str>,
    ) -> PdsResult<i64> {
        let now = Utc::now();

        let result = sqlx::query(
            r#"
            INSERT INTO email_delivery (recipient, subject, template_type, status, created_at, message_id, retry_count)
            VALUES (?, ?, ?, 'queued', ?, ?, 0)
            "#,
        )
        .bind(recipient)
        .bind(subject)
        .bind(template_type)
        .bind(now.to_rfc3339())
        .bind(message_id)
        .execute(&self.db)
        .await?;

        Ok(result.last_insert_rowid())
    }

    /// Mark email as sent
    pub async fn mark_sent(&self, delivery_id: i64) -> PdsResult<()> {
        let now = Utc::now();

        sqlx::query("UPDATE email_delivery SET status = 'sent', sent_at = ? WHERE id = ?")
            .bind(now.to_rfc3339())
            .bind(delivery_id)
            .execute(&self.db)
            .await?;

        tracing::info!("Email delivery {} marked as sent", delivery_id);
        Ok(())
    }

    /// Mark email as failed
    pub async fn mark_failed(&self, delivery_id: i64, error: &str) -> PdsResult<()> {
        sqlx::query(
            r#"
            UPDATE email_delivery
            SET status = 'failed', error_message = ?, retry_count = retry_count + 1
            WHERE id = ?
            "#,
        )
        .bind(error)
        .bind(delivery_id)
        .execute(&self.db)
        .await?;

        tracing::warn!("Email delivery {} marked as failed: {}", delivery_id, error);
        Ok(())
    }

    /// Mark email as bounced
    pub async fn mark_bounced(&self, recipient: &str, reason: &str) -> PdsResult<()> {
        // Update latest email to this recipient
        sqlx::query(
            r#"
            UPDATE email_delivery
            SET status = 'bounced', error_message = ?
            WHERE recipient = ? AND id = (
                SELECT id FROM email_delivery WHERE recipient = ? ORDER BY created_at DESC LIMIT 1
            )
            "#,
        )
        .bind(reason)
        .bind(recipient)
        .bind(recipient)
        .execute(&self.db)
        .await?;

        tracing::warn!("Email to {} bounced: {}", recipient, reason);
        Ok(())
    }

    /// Check if email address has bounced
    pub async fn is_bounced(&self, email: &str) -> PdsResult<bool> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM email_delivery WHERE recipient = ? AND status = 'bounced'",
        )
        .bind(email)
        .fetch_one(&self.db)
        .await?;

        Ok(count > 0)
    }

    /// Get email delivery history for recipient
    pub async fn get_history(&self, recipient: &str, limit: i64) -> PdsResult<Vec<EmailDelivery>> {
        let rows = sqlx::query(
            r#"
            SELECT id, recipient, subject, template_type, status, sent_at, created_at, error_message, retry_count, message_id
            FROM email_delivery
            WHERE recipient = ?
            ORDER BY created_at DESC
            LIMIT ?
            "#,
        )
        .bind(recipient)
        .bind(limit)
        .fetch_all(&self.db)
        .await?;

        self.parse_deliveries(rows).await
    }

    /// Get failed deliveries that can be retried
    pub async fn get_retriable(&self, max_retries: i32) -> PdsResult<Vec<EmailDelivery>> {
        let rows = sqlx::query(
            r#"
            SELECT id, recipient, subject, template_type, status, sent_at, created_at, error_message, retry_count, message_id
            FROM email_delivery
            WHERE status = 'failed' AND retry_count < ?
            ORDER BY created_at ASC
            LIMIT 100
            "#,
        )
        .bind(max_retries)
        .fetch_all(&self.db)
        .await?;

        self.parse_deliveries(rows).await
    }

    /// Get email statistics
    pub async fn get_stats(&self, since: DateTime<Utc>) -> PdsResult<EmailStats> {
        let total: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM email_delivery WHERE created_at >= ?")
                .bind(since.to_rfc3339())
                .fetch_one(&self.db)
                .await?;

        let sent: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM email_delivery WHERE created_at >= ? AND status = 'sent'",
        )
        .bind(since.to_rfc3339())
        .fetch_one(&self.db)
        .await?;

        let failed: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM email_delivery WHERE created_at >= ? AND status = 'failed'",
        )
        .bind(since.to_rfc3339())
        .fetch_one(&self.db)
        .await?;

        let bounced: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM email_delivery WHERE created_at >= ? AND status = 'bounced'",
        )
        .bind(since.to_rfc3339())
        .fetch_one(&self.db)
        .await?;

        Ok(EmailStats {
            total: total as u64,
            sent: sent as u64,
            failed: failed as u64,
            bounced: bounced as u64,
        })
    }

    /// Parse database rows into EmailDelivery objects
    async fn parse_deliveries(
        &self,
        rows: Vec<sqlx::sqlite::SqliteRow>,
    ) -> PdsResult<Vec<EmailDelivery>> {
        let mut deliveries = Vec::new();

        for row in rows {
            let status_str: String = row.get("status");
            let status = EmailStatus::from_str(&status_str)?;

            let created_at_str: String = row.get("created_at");
            let created_at = DateTime::parse_from_rfc3339(&created_at_str)
                .map_err(|e| PdsError::Internal(format!("Invalid timestamp: {}", e)))?
                .with_timezone(&Utc);

            let sent_at = row
                .try_get::<String, _>("sent_at")
                .ok()
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|dt| dt.with_timezone(&Utc));

            deliveries.push(EmailDelivery {
                id: row.get("id"),
                recipient: row.get("recipient"),
                subject: row.get("subject"),
                template_type: row.get("template_type"),
                status,
                sent_at,
                created_at,
                error_message: row.get("error_message"),
                retry_count: row.get("retry_count"),
                message_id: row.get("message_id"),
            });
        }

        Ok(deliveries)
    }
}

/// Email delivery statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailStats {
    pub total: u64,
    pub sent: u64,
    pub failed: u64,
    pub bounced: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_email_tracking() {
        let db = SqlitePool::connect(":memory:").await.unwrap();

        sqlx::query(
            r#"
            CREATE TABLE email_delivery (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                recipient TEXT NOT NULL,
                subject TEXT NOT NULL,
                template_type TEXT NOT NULL,
                status TEXT NOT NULL,
                sent_at TEXT,
                created_at TEXT NOT NULL,
                error_message TEXT,
                retry_count INTEGER NOT NULL DEFAULT 0,
                message_id TEXT
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        let tracker = EmailTracker::new(db);

        // Record send
        let delivery_id = tracker
            .record_send(
                "alice@example.com",
                "Welcome!",
                "account_created",
                Some("msg-123"),
            )
            .await
            .unwrap();

        // Mark as sent
        tracker.mark_sent(delivery_id).await.unwrap();

        // Get history
        let history = tracker.get_history("alice@example.com", 10).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].status, EmailStatus::Sent);

        // Get stats
        let stats = tracker
            .get_stats(Utc::now() - chrono::Duration::hours(1))
            .await
            .unwrap();
        assert_eq!(stats.total, 1);
        assert_eq!(stats.sent, 1);
    }
}
