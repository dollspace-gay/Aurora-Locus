//! Blob Quarantine System
//!
//! Provides blob takedown and quarantine functionality for moderation.
//! Quarantined blobs are marked as taken down but retained for legal/compliance reasons.

use crate::error::{PdsError, PdsResult};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::str::FromStr;

/// Quarantine reason types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuarantineReason {
    /// DMCA takedown
    Dmca,
    /// Child safety concern
    Csam,
    /// Terms of service violation
    Tos,
    /// Legal request
    Legal,
    /// Malware or phishing
    Malware,
    /// Other reason
    Other,
}

impl QuarantineReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            QuarantineReason::Dmca => "dmca",
            QuarantineReason::Csam => "csam",
            QuarantineReason::Tos => "tos",
            QuarantineReason::Legal => "legal",
            QuarantineReason::Malware => "malware",
            QuarantineReason::Other => "other",
        }
    }
}

impl FromStr for QuarantineReason {
    type Err = PdsError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "dmca" => Ok(QuarantineReason::Dmca),
            "csam" => Ok(QuarantineReason::Csam),
            "tos" => Ok(QuarantineReason::Tos),
            "legal" => Ok(QuarantineReason::Legal),
            "malware" => Ok(QuarantineReason::Malware),
            "other" => Ok(QuarantineReason::Other),
            _ => Err(PdsError::Validation(format!("Invalid quarantine reason: {}", s))),
        }
    }
}

/// Quarantine record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuarantineRecord {
    pub id: i64,
    pub cid: String,
    pub reason: QuarantineReason,
    pub details: Option<String>,
    pub quarantined_by: String,
    pub quarantined_at: DateTime<Utc>,
    pub restored_at: Option<DateTime<Utc>>,
    pub restored_by: Option<String>,
    pub legal_reference: Option<String>,
}

/// Blob quarantine manager
#[derive(Clone)]
pub struct BlobQuarantine {
    db: SqlitePool,
}

impl BlobQuarantine {
    pub fn new(db: SqlitePool) -> Self {
        Self { db }
    }

    /// Quarantine a blob (mark as taken down)
    pub async fn quarantine_blob(
        &self,
        cid: &str,
        reason: QuarantineReason,
        details: Option<&str>,
        quarantined_by: &str,
        legal_reference: Option<&str>,
    ) -> PdsResult<QuarantineRecord> {
        let now = Utc::now();

        // Check if already quarantined
        let existing = self.is_quarantined(cid).await?;
        if existing {
            return Err(PdsError::Conflict(format!("Blob {} is already quarantined", cid)));
        }

        let result = sqlx::query(
            r#"
            INSERT INTO blob_quarantine (cid, reason, details, quarantined_by, quarantined_at, legal_reference)
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(cid)
        .bind(reason.as_str())
        .bind(details)
        .bind(quarantined_by)
        .bind(now.to_rfc3339())
        .bind(legal_reference)
        .execute(&self.db)
        .await?;

        // Update blob table to mark as taken down
        sqlx::query("UPDATE blob SET takedown = 1 WHERE cid = ?1")
            .bind(cid)
            .execute(&self.db)
            .await?;

        tracing::info!(
            "Quarantined blob CID: {} by {} for reason: {:?}",
            cid,
            quarantined_by,
            reason
        );

        Ok(QuarantineRecord {
            id: result.last_insert_rowid(),
            cid: cid.to_string(),
            reason,
            details: details.map(String::from),
            quarantined_by: quarantined_by.to_string(),
            quarantined_at: now,
            restored_at: None,
            restored_by: None,
            legal_reference: legal_reference.map(String::from),
        })
    }

    /// Restore a quarantined blob
    pub async fn restore_blob(
        &self,
        cid: &str,
        restored_by: &str,
    ) -> PdsResult<()> {
        let now = Utc::now();

        let result = sqlx::query(
            r#"
            UPDATE blob_quarantine
            SET restored_at = ?,
                restored_by = ?
            WHERE cid = ? AND restored_at IS NULL
            "#,
        )
        .bind(now.to_rfc3339())
        .bind(restored_by)
        .bind(cid)
        .execute(&self.db)
        .await?;

        if result.rows_affected() == 0 {
            return Err(PdsError::NotFound(format!(
                "No active quarantine found for blob {}",
                cid
            )));
        }

        // Update blob table to remove takedown
        sqlx::query("UPDATE blob SET takedown = 0 WHERE cid = ?1")
            .bind(cid)
            .execute(&self.db)
            .await?;

        tracing::info!("Restored blob CID: {} by {}", cid, restored_by);
        Ok(())
    }

    /// Check if blob is quarantined
    pub async fn is_quarantined(&self, cid: &str) -> PdsResult<bool> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM blob_quarantine WHERE cid = ?1 AND restored_at IS NULL"
        )
        .bind(cid)
        .fetch_one(&self.db)
        .await?;

        Ok(count > 0)
    }

    /// Get quarantine record for a blob
    pub async fn get_quarantine(&self, cid: &str) -> PdsResult<Option<QuarantineRecord>> {
        let row = sqlx::query(
            r#"
            SELECT id, cid, reason, details, quarantined_by, quarantined_at,
                   restored_at, restored_by, legal_reference
            FROM blob_quarantine
            WHERE cid = ? AND restored_at IS NULL
            "#,
        )
        .bind(cid)
        .fetch_optional(&self.db)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };

        Ok(Some(self.parse_quarantine_record(row)?))
    }

    /// Get all active quarantines
    pub async fn get_active_quarantines(&self, limit: i64) -> PdsResult<Vec<QuarantineRecord>> {
        let rows = sqlx::query(
            r#"
            SELECT id, cid, reason, details, quarantined_by, quarantined_at,
                   restored_at, restored_by, legal_reference
            FROM blob_quarantine
            WHERE restored_at IS NULL
            ORDER BY quarantined_at DESC
            LIMIT ?
            "#,
        )
        .bind(limit)
        .fetch_all(&self.db)
        .await?;

        let mut records = Vec::new();
        for row in rows {
            records.push(self.parse_quarantine_record(row)?);
        }

        Ok(records)
    }

    /// Get quarantine history for a blob
    pub async fn get_history(&self, cid: &str) -> PdsResult<Vec<QuarantineRecord>> {
        let rows = sqlx::query(
            r#"
            SELECT id, cid, reason, details, quarantined_by, quarantined_at,
                   restored_at, restored_by, legal_reference
            FROM blob_quarantine
            WHERE cid = ?
            ORDER BY quarantined_at DESC
            "#,
        )
        .bind(cid)
        .fetch_all(&self.db)
        .await?;

        let mut records = Vec::new();
        for row in rows {
            records.push(self.parse_quarantine_record(row)?);
        }

        Ok(records)
    }

    /// Parse database row into QuarantineRecord
    fn parse_quarantine_record(&self, row: sqlx::sqlite::SqliteRow) -> PdsResult<QuarantineRecord> {
        let reason_str: String = row.get("reason");
        let reason = QuarantineReason::from_str(&reason_str)?;

        let quarantined_at_str: String = row.get("quarantined_at");
        let quarantined_at = DateTime::parse_from_rfc3339(&quarantined_at_str)
            .map_err(|e| PdsError::Internal(format!("Invalid timestamp: {}", e)))?
            .with_timezone(&Utc);

        let restored_at = row
            .try_get::<String, _>("restored_at")
            .ok()
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|dt| dt.with_timezone(&Utc));

        Ok(QuarantineRecord {
            id: row.get("id"),
            cid: row.get("cid"),
            reason,
            details: row.get("details"),
            quarantined_by: row.get("quarantined_by"),
            quarantined_at,
            restored_at,
            restored_by: row.get("restored_by"),
            legal_reference: row.get("legal_reference"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_quarantine_and_restore() {
        let db = SqlitePool::connect(":memory:").await.unwrap();

        // Create tables
        sqlx::query(
            r#"
            CREATE TABLE blob (
                cid TEXT PRIMARY KEY,
                did TEXT NOT NULL,
                size INTEGER NOT NULL,
                mime_type TEXT NOT NULL,
                created_at TEXT NOT NULL,
                takedown INTEGER NOT NULL DEFAULT 0
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        sqlx::query(
            r#"
            CREATE TABLE blob_quarantine (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                cid TEXT NOT NULL,
                reason TEXT NOT NULL,
                details TEXT,
                quarantined_by TEXT NOT NULL,
                quarantined_at TEXT NOT NULL,
                restored_at TEXT,
                restored_by TEXT,
                legal_reference TEXT
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        // Insert test blob
        sqlx::query(
            "INSERT INTO blob (cid, did, size, mime_type, created_at) VALUES (?, ?, ?, ?, ?)"
        )
        .bind("bafytest123")
        .bind("did:plc:alice")
        .bind(1024)
        .bind("image/png")
        .bind(Utc::now().to_rfc3339())
        .execute(&db)
        .await
        .unwrap();

        let quarantine = BlobQuarantine::new(db.clone());

        // Quarantine blob
        let record = quarantine
            .quarantine_blob(
                "bafytest123",
                QuarantineReason::Dmca,
                Some("Copyright violation"),
                "did:plc:admin",
                Some("DMCA-2024-001"),
            )
            .await
            .unwrap();

        assert_eq!(record.reason, QuarantineReason::Dmca);
        assert!(quarantine.is_quarantined("bafytest123").await.unwrap());

        // Restore blob
        quarantine
            .restore_blob("bafytest123", "did:plc:admin")
            .await
            .unwrap();

        assert!(!quarantine.is_quarantined("bafytest123").await.unwrap());

        // Check history
        let history = quarantine.get_history("bafytest123").await.unwrap();
        assert_eq!(history.len(), 1);
        assert!(history[0].restored_at.is_some());
    }
}
