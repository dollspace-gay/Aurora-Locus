/// Invite Code Management System
use crate::error::{PdsError, PdsResult};
use chrono::{DateTime, Utc};
use rand::{distributions::Alphanumeric, Rng};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

/// Invite code
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InviteCode {
    pub code: String,
    pub available: i32,
    pub disabled: bool,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub note: Option<String>,
    pub for_account: Option<String>,
}

/// Invite code manager
#[derive(Clone)]
pub struct InviteCodeManager {
    db: SqlitePool,
}

impl InviteCodeManager {
    pub fn new(db: SqlitePool) -> Self {
        Self { db }
    }

    /// Generate a new invite code
    pub fn generate_code() -> String {
        let code: String = rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(16)
            .map(char::from)
            .collect();

        format!("aurora-{}", code.to_lowercase())
    }

    /// Create invite code
    pub async fn create_invite(
        &self,
        created_by: &str,
        uses: i32,
        expires_in: Option<chrono::Duration>,
        note: Option<String>,
        for_account: Option<String>,
    ) -> PdsResult<InviteCode> {
        let code = Self::generate_code();
        let now = Utc::now();
        let expires_at = expires_in.map(|d| now + d);

        sqlx::query(
            r#"
            INSERT INTO invite_code (code, available, created_by, created_at, expires_at, note, for_account)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&code)
        .bind(uses)
        .bind(created_by)
        .bind(now.to_rfc3339())
        .bind(expires_at.map(|dt| dt.to_rfc3339()))
        .bind(&note)
        .bind(&for_account)
        .execute(&self.db)
        .await?;

        Ok(InviteCode {
            code,
            available: uses,
            disabled: false,
            created_by: created_by.to_string(),
            created_at: now,
            expires_at,
            note,
            for_account,
        })
    }

    /// Validate and use invite code
    pub async fn use_code(&self, code: &str, used_by: &str) -> PdsResult<()> {
        let now = Utc::now();

        // Get code details
        let row = sqlx::query(
            r#"
            SELECT available, disabled, expires_at, for_account
            FROM invite_code
            WHERE code = ?
            "#,
        )
        .bind(code)
        .fetch_optional(&self.db)
        .await?;

        let row = row.ok_or_else(|| PdsError::NotFound("Invite code not found".to_string()))?;

        let available: i32 = row.get("available");
        let disabled: bool = row.get("disabled");
        let for_account: Option<String> = row.get("for_account");

        // Validate code
        if disabled {
            return Err(PdsError::Validation("Invite code is disabled".to_string()));
        }

        if available <= 0 {
            return Err(PdsError::Validation("Invite code has no uses remaining".to_string()));
        }

        if let Some(expires_at_str) = row.try_get::<String, _>("expires_at").ok() {
            let expires_at = DateTime::parse_from_rfc3339(&expires_at_str)
                .map_err(|e| PdsError::Internal(format!("Invalid timestamp: {}", e)))?
                .with_timezone(&Utc);
            if expires_at < now {
                return Err(PdsError::Validation("Invite code has expired".to_string()));
            }
        }

        if let Some(specific_account) = for_account {
            if specific_account != used_by {
                return Err(PdsError::Authorization(
                    "This invite code is reserved for another account".to_string(),
                ));
            }
        }

        // Use code (decrement available and record usage)
        sqlx::query(
            r#"
            UPDATE invite_code
            SET available = available - 1
            WHERE code = ?
            "#,
        )
        .bind(code)
        .execute(&self.db)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO invite_code_use (code, used_by, used_at)
            VALUES (?, ?, ?)
            "#,
        )
        .bind(code)
        .bind(used_by)
        .bind(now.to_rfc3339())
        .execute(&self.db)
        .await?;

        Ok(())
    }

    /// Disable invite code
    pub async fn disable_code(&self, code: &str) -> PdsResult<()> {
        let result = sqlx::query(
            r#"
            UPDATE invite_code
            SET disabled = 1
            WHERE code = ?
            "#,
        )
        .bind(code)
        .execute(&self.db)
        .await?;

        if result.rows_affected() == 0 {
            return Err(PdsError::NotFound("Invite code not found".to_string()));
        }

        Ok(())
    }

    /// Get invite code details
    pub async fn get_code(&self, code: &str) -> PdsResult<Option<InviteCode>> {
        let row = sqlx::query(
            r#"
            SELECT code, available, disabled, created_by, created_at, expires_at, note, for_account
            FROM invite_code
            WHERE code = ?
            "#,
        )
        .bind(code)
        .fetch_optional(&self.db)
        .await?;

        if let Some(row) = row {
            let created_at_str: String = row.get("created_at");
            let created_at = DateTime::parse_from_rfc3339(&created_at_str)
                .map_err(|e| PdsError::Internal(format!("Invalid timestamp: {}", e)))?
                .with_timezone(&Utc);

            let expires_at = row
                .try_get::<String, _>("expires_at")
                .ok()
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|dt| dt.with_timezone(&Utc));

            Ok(Some(InviteCode {
                code: row.get("code"),
                available: row.get("available"),
                disabled: row.get("disabled"),
                created_by: row.get("created_by"),
                created_at,
                expires_at,
                note: row.get("note"),
                for_account: row.get("for_account"),
            }))
        } else {
            Ok(None)
        }
    }

    /// List all invite codes
    pub async fn list_codes(&self, include_disabled: bool) -> PdsResult<Vec<InviteCode>> {
        let query = if include_disabled {
            "SELECT code, available, disabled, created_by, created_at, expires_at, note, for_account FROM invite_code ORDER BY created_at DESC"
        } else {
            "SELECT code, available, disabled, created_by, created_at, expires_at, note, for_account FROM invite_code WHERE disabled = 0 ORDER BY created_at DESC"
        };

        let rows = sqlx::query(query).fetch_all(&self.db).await?;

        let mut codes = Vec::new();
        for row in rows {
            let created_at_str: String = row.get("created_at");
            let created_at = DateTime::parse_from_rfc3339(&created_at_str)
                .map_err(|e| PdsError::Internal(format!("Invalid timestamp: {}", e)))?
                .with_timezone(&Utc);

            let expires_at = row
                .try_get::<String, _>("expires_at")
                .ok()
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|dt| dt.with_timezone(&Utc));

            codes.push(InviteCode {
                code: row.get("code"),
                available: row.get("available"),
                disabled: row.get("disabled"),
                created_by: row.get("created_by"),
                created_at,
                expires_at,
                note: row.get("note"),
                for_account: row.get("for_account"),
            });
        }

        Ok(codes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_and_use_invite() {
        let db = SqlitePool::connect(":memory:").await.unwrap();

        sqlx::query(
            r#"
            CREATE TABLE invite_code (
                code TEXT PRIMARY KEY,
                available INTEGER NOT NULL DEFAULT 1,
                disabled INTEGER NOT NULL DEFAULT 0,
                created_by TEXT NOT NULL,
                created_at TEXT NOT NULL,
                expires_at TEXT,
                note TEXT,
                for_account TEXT
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        sqlx::query(
            r#"
            CREATE TABLE invite_code_use (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                code TEXT NOT NULL,
                used_by TEXT NOT NULL,
                used_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        let manager = InviteCodeManager::new(db);

        // Create code with 1 use
        let code = manager
            .create_invite("did:plc:admin", 1, None, Some("Test code".to_string()), None)
            .await
            .unwrap();

        assert_eq!(code.available, 1);

        // Use code
        manager.use_code(&code.code, "did:plc:newuser").await.unwrap();

        // Should fail to use again
        assert!(manager.use_code(&code.code, "did:plc:another").await.is_err());
    }

    #[test]
    fn test_generate_code() {
        let code = InviteCodeManager::generate_code();
        assert!(code.starts_with("aurora-"));
        assert!(code.len() > 16);
    }
}
