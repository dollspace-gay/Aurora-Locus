// Allow dead_code - invite management features for future use
#![allow(dead_code)]

/// Invite Code Management System
use crate::error::{PdsError, PdsResult};
use chrono::{DateTime, Utc};
use rand::{distributions::Alphanumeric, Rng};
use serde::{Deserialize, Serialize, Serializer};
use sqlx::{AnyPool, Row};

/// Custom serializer for DateTime that uses RFC3339 with millisecond precision
fn serialize_datetime<S>(dt: &DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
}

/// Custom serializer for Option<DateTime> that uses RFC3339 with millisecond precision
fn serialize_optional_datetime<S>(
    dt: &Option<DateTime<Utc>>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match dt {
        Some(dt) => serializer.serialize_str(&dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()),
        None => serializer.serialize_none(),
    }
}

/// Sort ordering for paginated invite-code listings
/// (lexicon `com.atproto.admin.getInviteCodes` parameter `sort`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InviteSortKey {
    /// `sort=recent` (default): order by `created_at` descending.
    Recent,
    /// `sort=usage`: order by total uses descending.
    Usage,
}

impl InviteSortKey {
    /// Parse the lexicon's `sort` query parameter. The lexicon declares
    /// `knownValues = ["recent", "usage"]` with `default = "recent"`.
    pub fn from_param(s: Option<&str>) -> Result<Self, String> {
        match s {
            None | Some("recent") => Ok(Self::Recent),
            Some("usage") => Ok(Self::Usage),
            Some(other) => Err(format!(
                "invalid sort value '{other}' (expected 'recent' or 'usage')"
            )),
        }
    }
}

/// Decoded pagination cursor for `list_codes_paginated`. The on-the-wire
/// form is base64url-encoded JSON tagged with `sort` so the cursor's
/// ordering can be validated against the request's `sort` parameter.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "sort", rename_all = "snake_case")]
pub enum InviteCursor {
    Recent {
        /// `created_at` of the last row returned in the previous page,
        /// serialised as RFC3339 to keep the cursor printable / debuggable
        /// when base64-decoded by an operator.
        after_created_at: String,
        after_code: String,
    },
    Usage {
        after_use_count: i64,
        after_code: String,
    },
}

impl InviteCursor {
    /// Which sort ordering this cursor was generated for.
    pub fn sort_key(&self) -> InviteSortKey {
        match self {
            Self::Recent { .. } => InviteSortKey::Recent,
            Self::Usage { .. } => InviteSortKey::Usage,
        }
    }
}

/// Invite code
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InviteCode {
    pub code: String,
    pub available: i32,
    pub disabled: bool,
    pub created_by: String,
    #[serde(serialize_with = "serialize_datetime")]
    pub created_at: DateTime<Utc>,
    #[serde(serialize_with = "serialize_optional_datetime")]
    pub expires_at: Option<DateTime<Utc>>,
    pub note: Option<String>,
    pub for_account: Option<String>,
}

/// Invite code manager
#[derive(Clone)]
pub struct InviteCodeManager {
    db: AnyPool,
}

impl InviteCodeManager {
    pub fn new(db: AnyPool) -> Self {
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
            VALUES ($1, $2, $3, $4, $5, $6, $7)
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
            WHERE code = $1
            "#,
        )
        .bind(code)
        .fetch_optional(&self.db)
        .await?;

        let row = row.ok_or_else(|| PdsError::NotFound("Invite code not found".to_string()))?;

        let available: i32 = row.get("available");
        let disabled: bool = row.get::<i64, _>("disabled") != 0;
        let for_account: Option<String> = row.get("for_account");

        // Validate code
        if disabled {
            return Err(PdsError::Validation("Invite code is disabled".to_string()));
        }

        if available <= 0 {
            return Err(PdsError::Validation(
                "Invite code has no uses remaining".to_string(),
            ));
        }

        if let Ok(Some(expires_at_str)) = row.try_get::<Option<String>, _>("expires_at") {
            if !expires_at_str.is_empty() {
                let expires_at = DateTime::parse_from_rfc3339(&expires_at_str)
                    .map_err(|e| PdsError::Internal(format!("Invalid timestamp: {}", e)))?
                    .with_timezone(&Utc);
                if expires_at < now {
                    return Err(PdsError::Validation("Invite code has expired".to_string()));
                }
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
            WHERE code = $1
            "#,
        )
        .bind(code)
        .execute(&self.db)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO invite_code_use (code, used_by, used_at)
            VALUES ($1, $2, $3)
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
            SET disabled = true
            WHERE code = $1
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

    /// Atomically disable a batch of invite codes and/or all codes issued for
    /// a set of accounts. All updates run inside a single SQLite transaction;
    /// either every update commits or none do.
    ///
    /// Codes or account DIDs that don't match any rows are silently skipped:
    /// the semantic is "ensure these are disabled," and a missing code is
    /// already vacuously disabled. Empty inputs are a no-op.
    pub async fn disable_codes_batch(
        &self,
        codes: &[String],
        accounts: &[String],
    ) -> PdsResult<()> {
        if codes.is_empty() && accounts.is_empty() {
            return Ok(());
        }

        let mut tx = self.db.begin().await?;

        for code in codes {
            sqlx::query("UPDATE invite_code SET disabled = true WHERE code = $1")
                .bind(code)
                .execute(&mut *tx)
                .await?;
        }

        for did in accounts {
            sqlx::query("UPDATE invite_code SET disabled = true WHERE for_account = $1")
                .bind(did)
                .execute(&mut *tx)
                .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// Get invite code details
    pub async fn get_code(&self, code: &str) -> PdsResult<Option<InviteCode>> {
        let row = sqlx::query(
            r#"
            SELECT code, available, disabled, created_by, created_at, expires_at, note, for_account
            FROM invite_code
            WHERE code = $1
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
                disabled: row.get::<i64, _>("disabled") != 0,
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

    /// Get the invite code that was used to create an account
    ///
    /// Returns the invite code that was used when creating the specified account,
    /// or None if the account wasn't created with an invite code.
    pub async fn get_invite_for_account(&self, did: &str) -> PdsResult<Option<InviteCode>> {
        // First, find the invite code that was used by this account
        let use_row = sqlx::query(
            r#"
            SELECT code FROM invite_code_use WHERE used_by = $1
            "#,
        )
        .bind(did)
        .fetch_optional(&self.db)
        .await?;

        if let Some(use_row) = use_row {
            let code: String = use_row.get("code");
            self.get_code(&code).await
        } else {
            Ok(None)
        }
    }

    /// Get all invite codes created by a specific account
    ///
    /// Returns all invite codes that were created by the specified DID.
    pub async fn get_codes_created_by(&self, did: &str) -> PdsResult<Vec<InviteCode>> {
        let rows = sqlx::query(
            r#"
            SELECT code, available, disabled, created_by, created_at, expires_at, note, for_account
            FROM invite_code
            WHERE created_by = $1
            ORDER BY created_at DESC
            "#,
        )
        .bind(did)
        .fetch_all(&self.db)
        .await?;

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
                disabled: row.get::<i64, _>("disabled") != 0,
                created_by: row.get("created_by"),
                created_at,
                expires_at,
                note: row.get("note"),
                for_account: row.get("for_account"),
            });
        }

        Ok(codes)
    }

    /// Paginated invite-code listing (Phase 1.10 / chainlink #65).
    ///
    /// Supports the lexicon's two sort orderings:
    /// - `Recent`: by `created_at` descending (matches `sort=recent` default).
    /// - `Usage`: by total use count from `invite_code_use` descending,
    ///   computed via LEFT JOIN + GROUP BY on the use table.
    ///
    /// Cursor pagination uses a tuple of (sort-field-value, code) so the
    /// page boundary is unique even when many rows share a `created_at`
    /// timestamp or use count. The cursor's `sort` discriminant must match
    /// the request's `sort` parameter; the handler is responsible for that
    /// check before invoking this method.
    ///
    /// Includes disabled codes in the result. The legacy `includeDisabled`
    /// filter on `getInviteCodes` was removed in Phase 1.10 (not in spec);
    /// disabled-only filtering will relocate to a `tools.aurora.ops.*`
    /// endpoint per ADMIN_MODERATION_ASSESSMENT.md Phase 2.
    ///
    /// Returns a `Vec<(InviteCode, use_count)>`. The tuple's `i64` is the
    /// number of rows in `invite_code_use` for that code; callers building
    /// the next cursor for `Usage` sort need it to seal the page boundary.
    pub async fn list_codes_paginated(
        &self,
        sort: InviteSortKey,
        cursor: Option<&InviteCursor>,
        limit: i64,
    ) -> PdsResult<Vec<(InviteCode, i64)>> {
        // Validate cursor/sort compatibility upstream of the SQL so we
        // don't fall through to a query that uses the wrong cursor variant.
        if let Some(c) = cursor {
            if c.sort_key() != sort {
                return Err(PdsError::Validation(
                    "cursor was issued for a different sort ordering".to_string(),
                ));
            }
        }

        match sort {
            InviteSortKey::Recent => self.list_recent(cursor, limit).await,
            InviteSortKey::Usage => self.list_by_usage(cursor, limit).await,
        }
    }

    async fn list_recent(
        &self,
        cursor: Option<&InviteCursor>,
        limit: i64,
    ) -> PdsResult<Vec<(InviteCode, i64)>> {
        // Tuple comparison via the portable `(a < ?) OR (a = ? AND b < ?)`
        // form — SQLite supports row-value comparisons since 3.15 but the
        // disjunction is friendlier to the query planner and EXPLAIN.
        let base = "SELECT ic.code, ic.available, ic.disabled, ic.created_by,
                          ic.created_at, ic.expires_at, ic.note, ic.for_account,
                          (SELECT COUNT(*) FROM invite_code_use icu WHERE icu.code = ic.code) AS use_count
                   FROM invite_code ic";
        let rows = if let Some(InviteCursor::Recent {
            after_created_at,
            after_code,
        }) = cursor
        {
            let sql = format!(
                "{base}
                 WHERE ic.created_at < ?
                    OR (ic.created_at = ? AND ic.code < ?)
                 ORDER BY ic.created_at DESC, ic.code DESC
                 LIMIT ?"
            );
            sqlx::query(&sql)
                .bind(after_created_at)
                .bind(after_created_at)
                .bind(after_code)
                .bind(limit)
                .fetch_all(&self.db)
                .await?
        } else {
            let sql = format!(
                "{base}
                 ORDER BY ic.created_at DESC, ic.code DESC
                 LIMIT ?"
            );
            sqlx::query(&sql)
                .bind(limit)
                .fetch_all(&self.db)
                .await?
        };

        Self::rows_to_codes_with_count(rows)
    }

    async fn list_by_usage(
        &self,
        cursor: Option<&InviteCursor>,
        limit: i64,
    ) -> PdsResult<Vec<(InviteCode, i64)>> {
        // `use_count` is the aggregate from invite_code_use. We compute it
        // via a correlated subquery rather than GROUP BY so cursor filtering
        // can sit cleanly in WHERE rather than HAVING.
        let base = "SELECT ic.code, ic.available, ic.disabled, ic.created_by,
                          ic.created_at, ic.expires_at, ic.note, ic.for_account,
                          (SELECT COUNT(*) FROM invite_code_use icu WHERE icu.code = ic.code) AS use_count
                   FROM invite_code ic";
        let rows = if let Some(InviteCursor::Usage {
            after_use_count,
            after_code,
        }) = cursor
        {
            let sql = format!(
                "{base}
                 WHERE (SELECT COUNT(*) FROM invite_code_use icu WHERE icu.code = ic.code) < ?
                    OR ((SELECT COUNT(*) FROM invite_code_use icu WHERE icu.code = ic.code) = ?
                        AND ic.code < ?)
                 ORDER BY use_count DESC, ic.code DESC
                 LIMIT ?"
            );
            sqlx::query(&sql)
                .bind(after_use_count)
                .bind(after_use_count)
                .bind(after_code)
                .bind(limit)
                .fetch_all(&self.db)
                .await?
        } else {
            let sql = format!(
                "{base}
                 ORDER BY use_count DESC, ic.code DESC
                 LIMIT ?"
            );
            sqlx::query(&sql)
                .bind(limit)
                .fetch_all(&self.db)
                .await?
        };

        Self::rows_to_codes_with_count(rows)
    }

    fn rows_to_codes_with_count(
        rows: Vec<sqlx::any::AnyRow>,
    ) -> PdsResult<Vec<(InviteCode, i64)>> {
        let mut out = Vec::with_capacity(rows.len());
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
            let use_count: i64 = row.get("use_count");
            out.push((
                InviteCode {
                    code: row.get("code"),
                    available: row.get("available"),
                    disabled: row.get::<i64, _>("disabled") != 0,
                    created_by: row.get("created_by"),
                    created_at,
                    expires_at,
                    note: row.get("note"),
                    for_account: row.get("for_account"),
                },
                use_count,
            ));
        }
        Ok(out)
    }

    /// List all invite codes
    pub async fn list_codes(&self, include_disabled: bool) -> PdsResult<Vec<InviteCode>> {
        let query = if include_disabled {
            "SELECT code, available, disabled, created_by, created_at, expires_at, note, for_account FROM invite_code ORDER BY created_at DESC"
        } else {
            "SELECT code, available, disabled, created_by, created_at, expires_at, note, for_account FROM invite_code WHERE NOT disabled ORDER BY created_at DESC"
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
                disabled: row.get::<i64, _>("disabled") != 0,
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
    async fn test_create_and_use_invite() {
        let db = open_test_pool().await;

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
            .create_invite(
                "did:plc:admin",
                1,
                None,
                Some("Test code".to_string()),
                None,
            )
            .await
            .unwrap();

        assert_eq!(code.available, 1);

        // Use code
        manager
            .use_code(&code.code, "did:plc:newuser")
            .await
            .unwrap();

        // Should fail to use again
        assert!(manager
            .use_code(&code.code, "did:plc:another")
            .await
            .is_err());
    }

    #[test]
    fn test_generate_code() {
        let code = InviteCodeManager::generate_code();
        assert!(code.starts_with("aurora-"));
        assert!(code.len() > 16);
    }
}
