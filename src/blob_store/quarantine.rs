// Allow dead_code - blob quarantine features for future use
#![allow(dead_code)]

//! Blob Quarantine System
//!
//! Provides blob takedown and quarantine functionality for moderation.
//! Quarantined blobs are marked as taken down but retained for legal/compliance reasons.

use crate::error::{PdsError, PdsResult};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{AnyPool, Row};
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
            _ => Err(PdsError::Validation(format!(
                "Invalid quarantine reason: {}",
                s
            ))),
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
    db: AnyPool,
}

impl BlobQuarantine {
    pub fn new(db: AnyPool) -> Self {
        Self { db }
    }

    /// Quarantine a blob (mark as taken down). Pool-API wrapper that
    /// opens its own transaction; for atomic-with-chain entry, callers
    /// should use [`Self::quarantine_blob_in_tx`] (Arc 4 §8.4.0.5 /
    /// chainlink #131).
    pub async fn quarantine_blob(
        &self,
        cid: &str,
        reason: QuarantineReason,
        details: Option<&str>,
        quarantined_by: &str,
        legal_reference: Option<&str>,
    ) -> PdsResult<QuarantineRecord> {
        let mut tx = self.db.begin().await?;
        let record = Self::quarantine_blob_in_tx(
            &mut tx,
            cid,
            reason,
            details,
            quarantined_by,
            legal_reference,
        )
        .await?;
        tx.commit().await?;
        Ok(record)
    }

    /// Quarantine a blob inside an existing transaction. Arc 4 §8.4.0.5
    /// (chainlink #131) atomic-with-chain entry point: the caller
    /// (`dispatch_action`'s `QuarantineBlob` arm) writes the chain
    /// entry in the same tx, so a crash between the quarantine and the
    /// chain row leaves neither, not an audit-blind quarantine.
    ///
    /// The existence check (`SELECT ... FROM blob_quarantine WHERE
    /// cid = ? AND restored_at IS NULL`) runs against the wrapping tx
    /// so it sees the tx's snapshot — correctly rejects a
    /// double-quarantine attempt within the same tx.
    pub async fn quarantine_blob_in_tx<'tx>(
        tx: &mut sqlx::Transaction<'tx, sqlx::Any>,
        cid: &str,
        reason: QuarantineReason,
        details: Option<&str>,
        quarantined_by: &str,
        legal_reference: Option<&str>,
    ) -> PdsResult<QuarantineRecord> {
        let now = Utc::now();

        // Existence check inside the wrapping tx so the SELECT sees
        // pending writes from the same caller (e.g., a tx that wrote
        // a quarantine row earlier and is now trying to write
        // another for the same cid).
        let existing: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM blob_quarantine WHERE cid = $1 AND restored_at IS NULL",
        )
        .bind(cid)
        .fetch_one(&mut **tx)
        .await?;
        if existing > 0 {
            return Err(PdsError::Conflict(format!(
                "Blob {} is already quarantined",
                cid
            )));
        }

        let id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO blob_quarantine (cid, reason, details, quarantined_by, quarantined_at, legal_reference)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id
            "#,
        )
        .bind(cid)
        .bind(reason.as_str())
        .bind(details)
        .bind(quarantined_by)
        .bind(now.to_rfc3339())
        .bind(legal_reference)
        .fetch_one(&mut **tx)
        .await?;

        // Update blob table to mark as taken down (same tx).
        sqlx::query("UPDATE blob SET takedown = true WHERE cid = $1")
            .bind(cid)
            .execute(&mut **tx)
            .await?;

        tracing::info!(
            "Quarantined blob CID: {} by {} for reason: {:?} (in_tx)",
            cid,
            quarantined_by,
            reason
        );

        Ok(QuarantineRecord {
            id,
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

    /// Restore a quarantined blob. Pool-API wrapper; for atomic-with-chain
    /// entry, callers should use [`Self::restore_blob_in_tx`].
    pub async fn restore_blob(&self, cid: &str, restored_by: &str) -> PdsResult<()> {
        let mut tx = self.db.begin().await?;
        Self::restore_blob_in_tx(&mut tx, cid, restored_by).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Restore a quarantined blob inside an existing transaction.
    /// Arc 4 §8.4.0.5 (chainlink #131) atomic-with-chain entry point.
    pub async fn restore_blob_in_tx<'tx>(
        tx: &mut sqlx::Transaction<'tx, sqlx::Any>,
        cid: &str,
        restored_by: &str,
    ) -> PdsResult<()> {
        let now = Utc::now();

        let result = sqlx::query(
            r#"
            UPDATE blob_quarantine
            SET restored_at = $1,
                restored_by = $2
            WHERE cid = $3 AND restored_at IS NULL
            "#,
        )
        .bind(now.to_rfc3339())
        .bind(restored_by)
        .bind(cid)
        .execute(&mut **tx)
        .await?;

        if result.rows_affected() == 0 {
            return Err(PdsError::NotFound(format!(
                "No active quarantine found for blob {}",
                cid
            )));
        }

        // Update blob table to remove takedown (same tx).
        sqlx::query("UPDATE blob SET takedown = false WHERE cid = $1")
            .bind(cid)
            .execute(&mut **tx)
            .await?;

        tracing::info!("Restored blob CID: {} by {} (in_tx)", cid, restored_by);
        Ok(())
    }

    /// Check if blob is quarantined
    pub async fn is_quarantined(&self, cid: &str) -> PdsResult<bool> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM blob_quarantine WHERE cid = $1 AND restored_at IS NULL",
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
            WHERE cid = $1 AND restored_at IS NULL
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
            LIMIT $1
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
            WHERE cid = $1
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
    fn parse_quarantine_record(&self, row: sqlx::any::AnyRow) -> PdsResult<QuarantineRecord> {
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

    /// Open a single-connection SQLite-backed `AnyPool` for the in-memory
    /// test fixture. Single-connection is mandatory for `:memory:` SQLite
    /// (each connection has its own private database otherwise).
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
    async fn test_quarantine_and_restore() {
        let db = open_test_pool().await;

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
            "INSERT INTO blob (cid, did, size, mime_type, created_at) VALUES ($1, $2, $3, $4, $5)",
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

    // ====================================================================
    // Arc 4 §8.4.0.5 / chainlink #131 — _in_tx variants. Tests pin
    // commit + rollback semantics for both quarantine and restore.
    // ====================================================================

    /// Stand up the schema + a single test blob row. Used by every
    /// _in_tx test below.
    async fn setup_blob_pool(cid: &str) -> AnyPool {
        let db = open_test_pool().await;
        sqlx::query(
            "CREATE TABLE blob (
                cid TEXT PRIMARY KEY, did TEXT NOT NULL, size INTEGER NOT NULL,
                mime_type TEXT NOT NULL, created_at TEXT NOT NULL,
                takedown INTEGER NOT NULL DEFAULT 0
            )",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE blob_quarantine (
                id INTEGER PRIMARY KEY AUTOINCREMENT, cid TEXT NOT NULL, reason TEXT NOT NULL,
                details TEXT, quarantined_by TEXT NOT NULL, quarantined_at TEXT NOT NULL,
                restored_at TEXT, restored_by TEXT, legal_reference TEXT
            )",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO blob (cid, did, size, mime_type, created_at) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(cid)
        .bind("did:plc:alice")
        .bind(1024)
        .bind("image/png")
        .bind(Utc::now().to_rfc3339())
        .execute(&db)
        .await
        .unwrap();
        db
    }

    #[tokio::test]
    async fn quarantine_blob_in_tx_rolls_back_on_caller_rollback() {
        let db = setup_blob_pool("bafy_in_tx_rollback").await;
        {
            let mut tx = db.begin().await.unwrap();
            BlobQuarantine::quarantine_blob_in_tx(
                &mut tx,
                "bafy_in_tx_rollback",
                QuarantineReason::Dmca,
                None,
                "did:plc:m",
                None,
            )
            .await
            .unwrap();
            tx.rollback().await.unwrap();
        }
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM blob_quarantine")
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(count, 0, "rolled-back tx must not leave a quarantine row");
        // The blob's takedown flag must also have rolled back.
        let takedown: i64 = sqlx::query_scalar("SELECT takedown FROM blob WHERE cid = $1")
            .bind("bafy_in_tx_rollback")
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(takedown, 0);
    }

    #[tokio::test]
    async fn quarantine_blob_in_tx_commits_on_caller_commit() {
        let db = setup_blob_pool("bafy_in_tx_commit").await;
        let mut tx = db.begin().await.unwrap();
        let record = BlobQuarantine::quarantine_blob_in_tx(
            &mut tx,
            "bafy_in_tx_commit",
            QuarantineReason::Dmca,
            None,
            "did:plc:m",
            None,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();

        assert_eq!(record.reason, QuarantineReason::Dmca);
        let mgr = BlobQuarantine::new(db.clone());
        assert!(mgr.is_quarantined("bafy_in_tx_commit").await.unwrap());
    }

    #[tokio::test]
    async fn quarantine_blob_in_tx_rejects_double_quarantine_within_same_tx() {
        let db = setup_blob_pool("bafy_double").await;
        let mut tx = db.begin().await.unwrap();
        BlobQuarantine::quarantine_blob_in_tx(
            &mut tx,
            "bafy_double",
            QuarantineReason::Dmca,
            None,
            "did:plc:m",
            None,
        )
        .await
        .unwrap();
        // Second call within the SAME tx must see the pending write
        // and reject — that's the load-bearing in-tx-existence-check
        // behaviour.
        let err = BlobQuarantine::quarantine_blob_in_tx(
            &mut tx,
            "bafy_double",
            QuarantineReason::Dmca,
            None,
            "did:plc:m",
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, PdsError::Conflict(_)));
    }

    #[tokio::test]
    async fn restore_blob_in_tx_rolls_back_on_caller_rollback() {
        let db = setup_blob_pool("bafy_restore_rollback").await;
        // Quarantine first via the pool API so the rollback test
        // exercises the restore-then-rollback path against an
        // existing quarantine row.
        BlobQuarantine::new(db.clone())
            .quarantine_blob(
                "bafy_restore_rollback",
                QuarantineReason::Dmca,
                None,
                "did:plc:m",
                None,
            )
            .await
            .unwrap();
        {
            let mut tx = db.begin().await.unwrap();
            BlobQuarantine::restore_blob_in_tx(&mut tx, "bafy_restore_rollback", "did:plc:m")
                .await
                .unwrap();
            tx.rollback().await.unwrap();
        }
        // Quarantine row must still be active (restored_at NULL).
        let restored: Option<String> = sqlx::query_scalar(
            "SELECT restored_at FROM blob_quarantine WHERE cid = $1",
        )
        .bind("bafy_restore_rollback")
        .fetch_one(&db)
        .await
        .unwrap();
        assert!(
            restored.is_none(),
            "rolled-back restore must leave quarantine active"
        );
    }

    #[tokio::test]
    async fn restore_blob_in_tx_commits_on_caller_commit() {
        let db = setup_blob_pool("bafy_restore_commit").await;
        BlobQuarantine::new(db.clone())
            .quarantine_blob(
                "bafy_restore_commit",
                QuarantineReason::Dmca,
                None,
                "did:plc:m",
                None,
            )
            .await
            .unwrap();
        let mut tx = db.begin().await.unwrap();
        BlobQuarantine::restore_blob_in_tx(&mut tx, "bafy_restore_commit", "did:plc:m")
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let mgr = BlobQuarantine::new(db);
        assert!(!mgr.is_quarantined("bafy_restore_commit").await.unwrap());
    }
}
