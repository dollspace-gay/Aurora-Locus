/// Account Moderation System
use crate::error::{PdsError, PdsResult};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

/// Moderation action types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModerationAction {
    /// Remove content from public view
    Takedown,
    /// Temporarily suspend account
    Suspend,
    /// Flag for review
    Flag,
    /// Warning to user
    Warn,
    /// Restore after takedown/suspension
    Restore,
}

impl ModerationAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            ModerationAction::Takedown => "takedown",
            ModerationAction::Suspend => "suspend",
            ModerationAction::Flag => "flag",
            ModerationAction::Warn => "warn",
            ModerationAction::Restore => "restore",
        }
    }

    pub fn from_str(s: &str) -> PdsResult<Self> {
        match s.to_lowercase().as_str() {
            "takedown" => Ok(ModerationAction::Takedown),
            "suspend" => Ok(ModerationAction::Suspend),
            "flag" => Ok(ModerationAction::Flag),
            "warn" => Ok(ModerationAction::Warn),
            "restore" => Ok(ModerationAction::Restore),
            _ => Err(PdsError::Validation(format!("Invalid moderation action: {}", s))),
        }
    }
}

/// Moderation record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModerationRecord {
    pub id: i64,
    pub did: String,
    pub action: ModerationAction,
    pub reason: String,
    pub moderated_by: String,
    pub moderated_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub reversed: bool,
    pub reversed_at: Option<DateTime<Utc>>,
    pub reversed_by: Option<String>,
    pub reversal_reason: Option<String>,
    pub report_id: Option<i64>,
    pub notes: Option<String>,
}

/// Moderation manager
#[derive(Clone)]
pub struct ModerationManager {
    db: SqlitePool,
}

impl ModerationManager {
    pub fn new(db: SqlitePool) -> Self {
        Self { db }
    }

    /// Apply moderation action to an account
    pub async fn apply_action(
        &self,
        did: &str,
        action: ModerationAction,
        reason: &str,
        moderated_by: &str,
        expires_in: Option<Duration>,
        report_id: Option<i64>,
        notes: Option<String>,
    ) -> PdsResult<ModerationRecord> {
        let now = Utc::now();
        let expires_at = expires_in.map(|d| now + d);

        let result = sqlx::query(
            r#"
            INSERT INTO account_moderation
            (did, action, reason, moderated_by, moderated_at, expires_at, report_id, notes)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(did)
        .bind(action.as_str())
        .bind(reason)
        .bind(moderated_by)
        .bind(now.to_rfc3339())
        .bind(expires_at.map(|dt| dt.to_rfc3339()))
        .bind(report_id)
        .bind(&notes)
        .execute(&self.db)
        .await?;

        let id = result.last_insert_rowid();

        Ok(ModerationRecord {
            id,
            did: did.to_string(),
            action,
            reason: reason.to_string(),
            moderated_by: moderated_by.to_string(),
            moderated_at: now,
            expires_at,
            reversed: false,
            reversed_at: None,
            reversed_by: None,
            reversal_reason: None,
            report_id,
            notes,
        })
    }

    /// Reverse a moderation action
    pub async fn reverse_action(
        &self,
        moderation_id: i64,
        reversed_by: &str,
        reason: &str,
    ) -> PdsResult<()> {
        let now = Utc::now();

        let result = sqlx::query(
            r#"
            UPDATE account_moderation
            SET reversed = 1,
                reversed_at = ?,
                reversed_by = ?,
                reversal_reason = ?
            WHERE id = ? AND reversed = 0
            "#,
        )
        .bind(now.to_rfc3339())
        .bind(reversed_by)
        .bind(reason)
        .bind(moderation_id)
        .execute(&self.db)
        .await?;

        if result.rows_affected() == 0 {
            return Err(PdsError::NotFound(format!(
                "Moderation record {} not found or already reversed",
                moderation_id
            )));
        }

        Ok(())
    }

    /// Get active moderation actions for an account
    pub async fn get_active_actions(&self, did: &str) -> PdsResult<Vec<ModerationRecord>> {
        let rows = sqlx::query(
            r#"
            SELECT id, did, action, reason, moderated_by, moderated_at,
                   expires_at, reversed, reversed_at, reversed_by,
                   reversal_reason, report_id, notes
            FROM account_moderation
            WHERE did = ? AND reversed = 0
            ORDER BY moderated_at DESC
            "#,
        )
        .bind(did)
        .fetch_all(&self.db)
        .await?;

        self.parse_moderation_records(rows).await
    }

    /// Check if account is currently taken down
    pub async fn is_taken_down(&self, did: &str) -> PdsResult<bool> {
        let actions = self.get_active_actions(did).await?;
        Ok(actions.iter().any(|a| a.action == ModerationAction::Takedown))
    }

    /// Check if account is currently suspended
    pub async fn is_suspended(&self, did: &str) -> PdsResult<bool> {
        let actions = self.get_active_actions(did).await?;
        let now = Utc::now();

        Ok(actions.iter().any(|a| {
            a.action == ModerationAction::Suspend &&
            a.expires_at.map_or(true, |exp| exp > now)
        }))
    }

    /// Get moderation history for an account
    pub async fn get_history(&self, did: &str) -> PdsResult<Vec<ModerationRecord>> {
        let rows = sqlx::query(
            r#"
            SELECT id, did, action, reason, moderated_by, moderated_at,
                   expires_at, reversed, reversed_at, reversed_by,
                   reversal_reason, report_id, notes
            FROM account_moderation
            WHERE did = ?
            ORDER BY moderated_at DESC
            "#,
        )
        .bind(did)
        .fetch_all(&self.db)
        .await?;

        self.parse_moderation_records(rows).await
    }

    /// Cleanup expired suspensions
    pub async fn cleanup_expired(&self) -> PdsResult<u64> {
        let now = Utc::now();

        let result = sqlx::query(
            r#"
            UPDATE account_moderation
            SET reversed = 1,
                reversed_at = ?,
                reversed_by = 'system',
                reversal_reason = 'Expired'
            WHERE action = 'suspend'
              AND reversed = 0
              AND expires_at IS NOT NULL
              AND expires_at < ?
            "#,
        )
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&self.db)
        .await?;

        Ok(result.rows_affected())
    }

    /// Parse database rows into ModerationRecord objects
    async fn parse_moderation_records(
        &self,
        rows: Vec<sqlx::sqlite::SqliteRow>,
    ) -> PdsResult<Vec<ModerationRecord>> {
        let mut records = Vec::new();

        for row in rows {
            let action_str: String = row.get("action");
            let action = ModerationAction::from_str(&action_str)?;

            let moderated_at_str: String = row.get("moderated_at");
            let moderated_at = DateTime::parse_from_rfc3339(&moderated_at_str)
                .map_err(|e| PdsError::Internal(format!("Invalid timestamp: {}", e)))?
                .with_timezone(&Utc);

            let expires_at = row
                .try_get::<String, _>("expires_at")
                .ok()
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|dt| dt.with_timezone(&Utc));

            let reversed_at = row
                .try_get::<String, _>("reversed_at")
                .ok()
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|dt| dt.with_timezone(&Utc));

            records.push(ModerationRecord {
                id: row.get("id"),
                did: row.get("did"),
                action,
                reason: row.get("reason"),
                moderated_by: row.get("moderated_by"),
                moderated_at,
                expires_at,
                reversed: row.get("reversed"),
                reversed_at,
                reversed_by: row.get("reversed_by"),
                reversal_reason: row.get("reversal_reason"),
                report_id: row.get("report_id"),
                notes: row.get("notes"),
            });
        }

        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_from_str() {
        assert_eq!(
            ModerationAction::from_str("takedown").unwrap(),
            ModerationAction::Takedown
        );
        assert_eq!(
            ModerationAction::from_str("suspend").unwrap(),
            ModerationAction::Suspend
        );
        assert!(ModerationAction::from_str("invalid").is_err());
    }

    #[tokio::test]
    async fn test_apply_and_get_action() {
        let db = SqlitePool::connect(":memory:").await.unwrap();

        sqlx::query(
            r#"
            CREATE TABLE account_moderation (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                did TEXT NOT NULL,
                action TEXT NOT NULL,
                reason TEXT NOT NULL,
                moderated_by TEXT NOT NULL,
                moderated_at TEXT NOT NULL,
                expires_at TEXT,
                reversed INTEGER NOT NULL DEFAULT 0,
                reversed_at TEXT,
                reversed_by TEXT,
                reversal_reason TEXT,
                report_id INTEGER,
                notes TEXT
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        let manager = ModerationManager::new(db);

        // Apply takedown
        let record = manager
            .apply_action(
                "did:plc:spam123",
                ModerationAction::Takedown,
                "Spam content",
                "did:plc:admin",
                None,
                None,
                Some("Automated detection".to_string()),
            )
            .await
            .unwrap();

        assert_eq!(record.action, ModerationAction::Takedown);
        assert!(!record.reversed);

        // Check if taken down
        assert!(manager.is_taken_down("did:plc:spam123").await.unwrap());

        // Get active actions
        let actions = manager.get_active_actions("did:plc:spam123").await.unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].action, ModerationAction::Takedown);
    }

    #[tokio::test]
    async fn test_suspend_with_expiration() {
        let db = SqlitePool::connect(":memory:").await.unwrap();

        sqlx::query(
            r#"
            CREATE TABLE account_moderation (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                did TEXT NOT NULL,
                action TEXT NOT NULL,
                reason TEXT NOT NULL,
                moderated_by TEXT NOT NULL,
                moderated_at TEXT NOT NULL,
                expires_at TEXT,
                reversed INTEGER NOT NULL DEFAULT 0,
                reversed_at TEXT,
                reversed_by TEXT,
                reversal_reason TEXT,
                report_id INTEGER,
                notes TEXT
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        let manager = ModerationManager::new(db);

        // Suspend for 7 days
        manager
            .apply_action(
                "did:plc:bad123",
                ModerationAction::Suspend,
                "Repeated violations",
                "did:plc:admin",
                Some(Duration::days(7)),
                None,
                None,
            )
            .await
            .unwrap();

        assert!(manager.is_suspended("did:plc:bad123").await.unwrap());
    }

    #[tokio::test]
    async fn test_reverse_action() {
        let db = SqlitePool::connect(":memory:").await.unwrap();

        sqlx::query(
            r#"
            CREATE TABLE account_moderation (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                did TEXT NOT NULL,
                action TEXT NOT NULL,
                reason TEXT NOT NULL,
                moderated_by TEXT NOT NULL,
                moderated_at TEXT NOT NULL,
                expires_at TEXT,
                reversed INTEGER NOT NULL DEFAULT 0,
                reversed_at TEXT,
                reversed_by TEXT,
                reversal_reason TEXT,
                report_id INTEGER,
                notes TEXT
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        let manager = ModerationManager::new(db);

        // Apply and reverse
        let record = manager
            .apply_action(
                "did:plc:false",
                ModerationAction::Takedown,
                "Mistake",
                "did:plc:admin",
                None,
                None,
                None,
            )
            .await
            .unwrap();

        manager
            .reverse_action(record.id, "did:plc:superadmin", "False positive")
            .await
            .unwrap();

        // Should no longer be taken down
        assert!(!manager.is_taken_down("did:plc:false").await.unwrap());
    }
}
