// Allow dead_code - appeals system is defined for future moderation features
#![allow(dead_code)]

//! Moderation Appeal System
//!
//! Allows users to appeal moderation decisions.
//! Provides due process and oversight for moderation actions.

use crate::error::{PdsError, PdsResult};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::str::FromStr;

/// Appeal status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppealStatus {
    /// Pending review
    Pending,
    /// Under review by moderator
    UnderReview,
    /// Appeal approved, action reversed
    Approved,
    /// Appeal denied, action upheld
    Denied,
    /// Appeal escalated to senior moderator
    Escalated,
}

impl AppealStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            AppealStatus::Pending => "pending",
            AppealStatus::UnderReview => "under_review",
            AppealStatus::Approved => "approved",
            AppealStatus::Denied => "denied",
            AppealStatus::Escalated => "escalated",
        }
    }
}

impl FromStr for AppealStatus {
    type Err = PdsError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "pending" => Ok(AppealStatus::Pending),
            "under_review" => Ok(AppealStatus::UnderReview),
            "approved" => Ok(AppealStatus::Approved),
            "denied" => Ok(AppealStatus::Denied),
            "escalated" => Ok(AppealStatus::Escalated),
            _ => Err(PdsError::Validation(format!(
                "Invalid appeal status: {}",
                s
            ))),
        }
    }
}

/// Appeal record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Appeal {
    pub id: i64,
    pub moderation_id: Option<i64>,
    pub report_id: Option<i64>,
    pub quarantine_id: Option<i64>,
    pub appellant_did: String,
    pub reason: String,
    pub details: Option<String>,
    pub submitted_at: DateTime<Utc>,
    pub status: AppealStatus,
    pub reviewed_by: Option<String>,
    pub reviewed_at: Option<DateTime<Utc>>,
    pub decision: Option<String>,
    pub notes: Option<String>,
}

/// Appeal manager
#[derive(Clone)]
pub struct AppealManager {
    db: SqlitePool,
}

impl AppealManager {
    pub fn new(db: SqlitePool) -> Self {
        Self { db }
    }

    /// Submit an appeal
    pub async fn submit_appeal(
        &self,
        moderation_id: Option<i64>,
        report_id: Option<i64>,
        quarantine_id: Option<i64>,
        appellant_did: &str,
        reason: &str,
        details: Option<&str>,
    ) -> PdsResult<Appeal> {
        let now = Utc::now();

        // Validate that at least one reference is provided
        if moderation_id.is_none() && report_id.is_none() && quarantine_id.is_none() {
            return Err(PdsError::Validation(
                "Must provide moderation_id, report_id, or quarantine_id".to_string(),
            ));
        }

        // Check for duplicate appeals
        if let Some(mod_id) = moderation_id {
            let existing: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM appeal WHERE moderation_id = ?1 AND status IN ('pending', 'under_review', 'escalated')"
            )
            .bind(mod_id)
            .fetch_one(&self.db)
            .await?;

            if existing > 0 {
                return Err(PdsError::Conflict(
                    "An active appeal already exists for this moderation action".to_string(),
                ));
            }
        }

        let result = sqlx::query(
            r#"
            INSERT INTO appeal (moderation_id, report_id, quarantine_id, appellant_did, reason, details, submitted_at, status)
            VALUES (?, ?, ?, ?, ?, ?, ?, 'pending')
            "#,
        )
        .bind(moderation_id)
        .bind(report_id)
        .bind(quarantine_id)
        .bind(appellant_did)
        .bind(reason)
        .bind(details)
        .bind(now.to_rfc3339())
        .execute(&self.db)
        .await?;

        tracing::info!(
            "Appeal submitted by {} for moderation_id: {:?}, report_id: {:?}",
            appellant_did,
            moderation_id,
            report_id
        );

        Ok(Appeal {
            id: result.last_insert_rowid(),
            moderation_id,
            report_id,
            quarantine_id,
            appellant_did: appellant_did.to_string(),
            reason: reason.to_string(),
            details: details.map(String::from),
            submitted_at: now,
            status: AppealStatus::Pending,
            reviewed_by: None,
            reviewed_at: None,
            decision: None,
            notes: None,
        })
    }

    /// Update appeal status
    pub async fn update_status(
        &self,
        appeal_id: i64,
        status: AppealStatus,
        reviewed_by: &str,
        decision: Option<&str>,
        notes: Option<&str>,
    ) -> PdsResult<()> {
        let now = Utc::now();

        let result = sqlx::query(
            r#"
            UPDATE appeal
            SET status = ?,
                reviewed_by = ?,
                reviewed_at = ?,
                decision = ?,
                notes = ?
            WHERE id = ?
            "#,
        )
        .bind(status.as_str())
        .bind(reviewed_by)
        .bind(now.to_rfc3339())
        .bind(decision)
        .bind(notes)
        .bind(appeal_id)
        .execute(&self.db)
        .await?;

        if result.rows_affected() == 0 {
            return Err(PdsError::NotFound(format!(
                "Appeal {} not found",
                appeal_id
            )));
        }

        tracing::info!(
            "Appeal {} updated to status: {:?} by {}",
            appeal_id,
            status,
            reviewed_by
        );

        Ok(())
    }

    /// Approve an appeal
    pub async fn approve_appeal(
        &self,
        appeal_id: i64,
        reviewed_by: &str,
        decision: &str,
    ) -> PdsResult<()> {
        self.update_status(
            appeal_id,
            AppealStatus::Approved,
            reviewed_by,
            Some(decision),
            None,
        )
        .await
    }

    /// Deny an appeal
    pub async fn deny_appeal(
        &self,
        appeal_id: i64,
        reviewed_by: &str,
        decision: &str,
    ) -> PdsResult<()> {
        self.update_status(
            appeal_id,
            AppealStatus::Denied,
            reviewed_by,
            Some(decision),
            None,
        )
        .await
    }

    /// Escalate an appeal
    pub async fn escalate_appeal(
        &self,
        appeal_id: i64,
        reviewed_by: &str,
        notes: &str,
    ) -> PdsResult<()> {
        self.update_status(
            appeal_id,
            AppealStatus::Escalated,
            reviewed_by,
            None,
            Some(notes),
        )
        .await
    }

    /// Get appeal by ID
    pub async fn get_appeal(&self, appeal_id: i64) -> PdsResult<Option<Appeal>> {
        let row = sqlx::query(
            r#"
            SELECT id, moderation_id, report_id, quarantine_id, appellant_did, reason, details,
                   submitted_at, status, reviewed_by, reviewed_at, decision, notes
            FROM appeal
            WHERE id = ?
            "#,
        )
        .bind(appeal_id)
        .fetch_optional(&self.db)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };

        Ok(Some(self.parse_appeal(row)?))
    }

    /// Get pending appeals
    pub async fn get_pending_appeals(&self, limit: i64) -> PdsResult<Vec<Appeal>> {
        let rows = sqlx::query(
            r#"
            SELECT id, moderation_id, report_id, quarantine_id, appellant_did, reason, details,
                   submitted_at, status, reviewed_by, reviewed_at, decision, notes
            FROM appeal
            WHERE status = 'pending'
            ORDER BY submitted_at ASC
            LIMIT ?
            "#,
        )
        .bind(limit)
        .fetch_all(&self.db)
        .await?;

        self.parse_appeals(rows).await
    }

    /// Get appeals by appellant
    pub async fn get_appeals_by_appellant(&self, did: &str) -> PdsResult<Vec<Appeal>> {
        let rows = sqlx::query(
            r#"
            SELECT id, moderation_id, report_id, quarantine_id, appellant_did, reason, details,
                   submitted_at, status, reviewed_by, reviewed_at, decision, notes
            FROM appeal
            WHERE appellant_did = ?
            ORDER BY submitted_at DESC
            "#,
        )
        .bind(did)
        .fetch_all(&self.db)
        .await?;

        self.parse_appeals(rows).await
    }

    /// Get appeals for a moderation action
    pub async fn get_appeals_for_moderation(&self, moderation_id: i64) -> PdsResult<Vec<Appeal>> {
        let rows = sqlx::query(
            r#"
            SELECT id, moderation_id, report_id, quarantine_id, appellant_did, reason, details,
                   submitted_at, status, reviewed_by, reviewed_at, decision, notes
            FROM appeal
            WHERE moderation_id = ?
            ORDER BY submitted_at DESC
            "#,
        )
        .bind(moderation_id)
        .fetch_all(&self.db)
        .await?;

        self.parse_appeals(rows).await
    }

    /// Parse database rows into Appeal objects
    async fn parse_appeals(&self, rows: Vec<sqlx::sqlite::SqliteRow>) -> PdsResult<Vec<Appeal>> {
        let mut appeals = Vec::new();
        for row in rows {
            appeals.push(self.parse_appeal(row)?);
        }
        Ok(appeals)
    }

    /// Parse single database row into Appeal
    fn parse_appeal(&self, row: sqlx::sqlite::SqliteRow) -> PdsResult<Appeal> {
        let status_str: String = row.get("status");
        let status = status_str.parse()?;

        let submitted_at_str: String = row.get("submitted_at");
        let submitted_at = DateTime::parse_from_rfc3339(&submitted_at_str)
            .map_err(|e| PdsError::Internal(format!("Invalid timestamp: {}", e)))?
            .with_timezone(&Utc);

        let reviewed_at = row
            .try_get::<String, _>("reviewed_at")
            .ok()
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|dt| dt.with_timezone(&Utc));

        Ok(Appeal {
            id: row.get("id"),
            moderation_id: row.get("moderation_id"),
            report_id: row.get("report_id"),
            quarantine_id: row.get("quarantine_id"),
            appellant_did: row.get("appellant_did"),
            reason: row.get("reason"),
            details: row.get("details"),
            submitted_at,
            status,
            reviewed_by: row.get("reviewed_by"),
            reviewed_at,
            decision: row.get("decision"),
            notes: row.get("notes"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_submit_and_process_appeal() {
        let db = SqlitePool::connect(":memory:").await.unwrap();

        sqlx::query(
            r#"
            CREATE TABLE appeal (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                moderation_id INTEGER,
                report_id INTEGER,
                quarantine_id INTEGER,
                appellant_did TEXT NOT NULL,
                reason TEXT NOT NULL,
                details TEXT,
                submitted_at TEXT NOT NULL,
                status TEXT NOT NULL,
                reviewed_by TEXT,
                reviewed_at TEXT,
                decision TEXT,
                notes TEXT
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        let manager = AppealManager::new(db);

        // Submit appeal
        let appeal = manager
            .submit_appeal(
                Some(123),
                None,
                None,
                "did:plc:user",
                "False positive",
                Some("This was a mistake, I did not violate any rules"),
            )
            .await
            .unwrap();

        assert_eq!(appeal.status, AppealStatus::Pending);
        assert_eq!(appeal.appellant_did, "did:plc:user");

        // Get pending appeals
        let pending = manager.get_pending_appeals(10).await.unwrap();
        assert_eq!(pending.len(), 1);

        // Approve appeal
        manager
            .approve_appeal(
                appeal.id,
                "did:plc:admin",
                "Appeal granted, action reversed",
            )
            .await
            .unwrap();

        // Verify approval
        let updated = manager.get_appeal(appeal.id).await.unwrap().unwrap();
        assert_eq!(updated.status, AppealStatus::Approved);
        assert_eq!(updated.reviewed_by, Some("did:plc:admin".to_string()));

        // No more pending appeals
        let pending = manager.get_pending_appeals(10).await.unwrap();
        assert_eq!(pending.len(), 0);
    }

    #[tokio::test]
    async fn test_duplicate_appeal_prevention() {
        let db = SqlitePool::connect(":memory:").await.unwrap();

        sqlx::query(
            r#"
            CREATE TABLE appeal (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                moderation_id INTEGER,
                report_id INTEGER,
                quarantine_id INTEGER,
                appellant_did TEXT NOT NULL,
                reason TEXT NOT NULL,
                details TEXT,
                submitted_at TEXT NOT NULL,
                status TEXT NOT NULL,
                reviewed_by TEXT,
                reviewed_at TEXT,
                decision TEXT,
                notes TEXT
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        let manager = AppealManager::new(db);

        // Submit first appeal
        manager
            .submit_appeal(Some(123), None, None, "did:plc:user", "First appeal", None)
            .await
            .unwrap();

        // Try to submit duplicate appeal
        let result = manager
            .submit_appeal(Some(123), None, None, "did:plc:user", "Second appeal", None)
            .await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), PdsError::Conflict(_)));
    }
}
