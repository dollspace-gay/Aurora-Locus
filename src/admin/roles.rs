// Allow dead_code - role management features for future use
#![allow(dead_code)]

/// Admin Role Management
use crate::error::{PdsError, PdsResult};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::str::FromStr;

/// Parse a timestamp string from the database, tolerating both RFC3339
/// (e.g., "2026-04-28T20:13:42.777Z") and SQLite native DATETIME format
/// (e.g., "2026-04-28 20:13:42").
///
/// SQLite's `datetime('now')` produces space-separated timestamps without a
/// timezone designator, which is not RFC3339. This helper accepts both forms
/// to avoid coupling to a specific timestamp format at the SQL layer.
fn parse_timestamp_lenient(s: &str) -> PdsResult<DateTime<Utc>> {
    // Try RFC3339 first (the strictly-correct format)
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }

    // Fall back to SQLite's native format: "YYYY-MM-DD HH:MM:SS"
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Ok(DateTime::from_naive_utc_and_offset(naive, Utc));
    }

    // Also accept the variant with subsecond precision: "YYYY-MM-DD HH:MM:SS.fff"
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f") {
        return Ok(DateTime::from_naive_utc_and_offset(naive, Utc));
    }

    Err(PdsError::Internal(format!(
        "Could not parse timestamp '{}' as RFC3339 or SQLite DATETIME",
        s
    )))
}

/// Admin role levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Can view only, no actions
    Moderator,
    /// Can perform most admin actions
    Admin,
    /// Full access, can grant/revoke roles
    SuperAdmin,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Moderator => "moderator",
            Role::Admin => "admin",
            Role::SuperAdmin => "superadmin",
        }
    }

    /// Check if this role can perform actions requiring another role
    pub fn can_act_as(&self, required: Role) -> bool {
        self >= &required
    }
}

impl FromStr for Role {
    type Err = PdsError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "moderator" => Ok(Role::Moderator),
            "admin" => Ok(Role::Admin),
            "superadmin" => Ok(Role::SuperAdmin),
            _ => Err(PdsError::Validation(format!("Invalid role: {}", s))),
        }
    }
}

/// Admin role record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminRole {
    pub id: i64,
    pub did: String,
    pub role: Role,
    pub granted_by: Option<String>,
    pub granted_at: DateTime<Utc>,
    pub revoked: bool,
    pub revoked_at: Option<DateTime<Utc>>,
    pub revoked_by: Option<String>,
    pub notes: Option<String>,
}

/// Admin role manager
#[derive(Clone)]
pub struct AdminRoleManager {
    db: SqlitePool,
}

impl AdminRoleManager {
    pub fn new(db: SqlitePool) -> Self {
        Self { db }
    }

    /// Grant admin role to a DID
    pub async fn grant_role(
        &self,
        did: &str,
        role: Role,
        granted_by: &str,
        notes: Option<String>,
    ) -> PdsResult<AdminRole> {
        let now = Utc::now();

        // Check if role already exists and is active
        if let Some(existing) = self.get_role(did).await? {
            if !existing.revoked {
                return Err(PdsError::Conflict(format!(
                    "User already has active role: {}",
                    existing.role.as_str()
                )));
            }
        }

        let result = sqlx::query(
            r#"
            INSERT INTO admin_roles (did, role, granted_by, granted_at, notes)
            VALUES (?, ?, ?, ?, ?)
            "#,
        )
        .bind(did)
        .bind(role.as_str())
        .bind(granted_by)
        .bind(now.to_rfc3339())
        .bind(&notes)
        .execute(&self.db)
        .await?;

        let id = result.last_insert_rowid();

        Ok(AdminRole {
            id,
            did: did.to_string(),
            role,
            granted_by: Some(granted_by.to_string()),
            granted_at: now,
            revoked: false,
            revoked_at: None,
            revoked_by: None,
            notes,
        })
    }

    /// Revoke admin role
    pub async fn revoke_role(
        &self,
        did: &str,
        revoked_by: &str,
        reason: Option<String>,
    ) -> PdsResult<()> {
        let now = Utc::now();

        let result = sqlx::query(
            r#"
            UPDATE admin_roles
            SET revoked = 1,
                revoked_at = ?,
                revoked_by = ?,
                notes = COALESCE(?, notes)
            WHERE did = ? AND revoked = 0
            "#,
        )
        .bind(now.to_rfc3339())
        .bind(revoked_by)
        .bind(&reason)
        .bind(did)
        .execute(&self.db)
        .await?;

        if result.rows_affected() == 0 {
            return Err(PdsError::NotFound(format!(
                "No active role found for {}",
                did
            )));
        }

        Ok(())
    }

    /// Get active admin role for a DID
    pub async fn get_role(&self, did: &str) -> PdsResult<Option<AdminRole>> {
        let row = sqlx::query(
            r#"
            SELECT id, did, role, granted_by, granted_at, revoked, revoked_at, revoked_by, notes
            FROM admin_roles
            WHERE did = ? AND revoked = 0
            ORDER BY granted_at DESC
            LIMIT 1
            "#,
        )
        .bind(did)
        .fetch_optional(&self.db)
        .await?;

        if let Some(row) = row {
            let role_str: String = row.get("role");
            let role = Role::from_str(&role_str)?;

            let granted_at_str: String = row.get("granted_at");
            let granted_at = parse_timestamp_lenient(&granted_at_str)?;

            let revoked_at = row
                .try_get::<String, _>("revoked_at")
                .ok()
                .and_then(|s| parse_timestamp_lenient(&s).ok());

            Ok(Some(AdminRole {
                id: row.get("id"),
                did: row.get("did"),
                role,
                granted_by: row.get("granted_by"),
                granted_at,
                revoked: row.get("revoked"),
                revoked_at,
                revoked_by: row.get("revoked_by"),
                notes: row.get("notes"),
            }))
        } else {
            Ok(None)
        }
    }

    /// Check if a DID has at least a specific role
    pub async fn has_role(&self, did: &str, required_role: Role) -> PdsResult<bool> {
        if let Some(admin_role) = self.get_role(did).await? {
            Ok(admin_role.role.can_act_as(required_role))
        } else {
            Ok(false)
        }
    }

    /// List all active admin roles
    pub async fn list_active_roles(&self) -> PdsResult<Vec<AdminRole>> {
        let rows = sqlx::query(
            r#"
            SELECT id, did, role, granted_by, granted_at, revoked, revoked_at, revoked_by, notes
            FROM admin_roles
            WHERE revoked = 0
            ORDER BY granted_at DESC
            "#,
        )
        .fetch_all(&self.db)
        .await?;

        let mut roles = Vec::new();
        for row in rows {
            let role_str: String = row.get("role");
            let role = Role::from_str(&role_str)?;

            let granted_at_str: String = row.get("granted_at");
            let granted_at = DateTime::parse_from_rfc3339(&granted_at_str)
                .map_err(|e| PdsError::Internal(format!("Invalid timestamp: {}", e)))?
                .with_timezone(&Utc);

            let revoked_at = row
                .try_get::<String, _>("revoked_at")
                .ok()
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|dt| dt.with_timezone(&Utc));

            roles.push(AdminRole {
                id: row.get("id"),
                did: row.get("did"),
                role,
                granted_by: row.get("granted_by"),
                granted_at,
                revoked: row.get("revoked"),
                revoked_at,
                revoked_by: row.get("revoked_by"),
                notes: row.get("notes"),
            });
        }

        Ok(roles)
    }

    /// Log admin action to audit log
    pub async fn log_action(
        &self,
        admin_did: &str,
        action: &str,
        subject_did: Option<&str>,
        details: Option<&str>,
        ip_address: Option<&str>,
    ) -> PdsResult<()> {
        let now = Utc::now();

        sqlx::query(
            r#"
            INSERT INTO admin_audit_log (admin_did, action, subject_did, details, timestamp, ip_address)
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(admin_did)
        .bind(action)
        .bind(subject_did)
        .bind(details)
        .bind(now.to_rfc3339())
        .bind(ip_address)
        .execute(&self.db)
        .await?;

        Ok(())
    }

    /// Get audit log entries with optional filters
    ///
    /// Returns audit log entries, optionally filtered by admin DID or action type.
    /// Results are ordered by timestamp descending (most recent first).
    pub async fn get_audit_logs(
        &self,
        admin_did: Option<&str>,
        action: Option<&str>,
        subject_did: Option<&str>,
        limit: i64,
        cursor: Option<i64>,
    ) -> PdsResult<Vec<super::AuditLogEntry>> {
        // Build query with optional filters
        let mut query = String::from(
            "SELECT id, admin_did, action, subject_did, details, timestamp, ip_address
             FROM admin_audit_log WHERE 1=1",
        );

        if admin_did.is_some() {
            query.push_str(" AND admin_did = ?");
        }
        if action.is_some() {
            query.push_str(" AND action = ?");
        }
        if subject_did.is_some() {
            query.push_str(" AND subject_did = ?");
        }
        if cursor.is_some() {
            query.push_str(" AND id < ?");
        }

        query.push_str(" ORDER BY id DESC LIMIT ?");

        // Execute with dynamic binding
        let mut q = sqlx::query(&query);

        if let Some(did) = admin_did {
            q = q.bind(did);
        }
        if let Some(act) = action {
            q = q.bind(act);
        }
        if let Some(subj) = subject_did {
            q = q.bind(subj);
        }
        if let Some(cur) = cursor {
            q = q.bind(cur);
        }
        q = q.bind(limit);

        let rows = q.fetch_all(&self.db).await?;

        let mut entries = Vec::new();
        for row in rows {
            let timestamp_str: String = row.get("timestamp");
            let timestamp = DateTime::parse_from_rfc3339(&timestamp_str)
                .map_err(|e| PdsError::Internal(format!("Invalid timestamp: {}", e)))?
                .with_timezone(&Utc);

            entries.push(super::AuditLogEntry {
                id: row.get("id"),
                admin_did: row.get("admin_did"),
                action: row.get("action"),
                subject_did: row.get("subject_did"),
                details: row.get("details"),
                timestamp,
                ip_address: row.get("ip_address"),
            });
        }

        Ok(entries)
    }

    /// Get count of audit log entries
    pub async fn get_audit_log_count(&self) -> PdsResult<i64> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM admin_audit_log")
            .fetch_one(&self.db)
            .await?;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_role_hierarchy() {
        assert!(Role::SuperAdmin > Role::Admin);
        assert!(Role::Admin > Role::Moderator);

        assert!(Role::SuperAdmin.can_act_as(Role::Admin));
        assert!(Role::SuperAdmin.can_act_as(Role::Moderator));
        assert!(Role::Admin.can_act_as(Role::Moderator));

        assert!(!Role::Moderator.can_act_as(Role::Admin));
        assert!(!Role::Admin.can_act_as(Role::SuperAdmin));
    }

    #[test]
    fn test_role_from_str() {
        assert_eq!(Role::from_str("moderator").unwrap(), Role::Moderator);
        assert_eq!(Role::from_str("admin").unwrap(), Role::Admin);
        assert_eq!(Role::from_str("superadmin").unwrap(), Role::SuperAdmin);
        assert_eq!(Role::from_str("ADMIN").unwrap(), Role::Admin);

        assert!(Role::from_str("invalid").is_err());
    }

    #[tokio::test]
    async fn test_grant_and_get_role() {
        let db = SqlitePool::connect(":memory:").await.unwrap();

        // Create table
        sqlx::query(
            r#"
            CREATE TABLE admin_roles (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                did TEXT NOT NULL UNIQUE,
                role TEXT NOT NULL,
                granted_by TEXT,
                granted_at TEXT NOT NULL,
                revoked INTEGER NOT NULL DEFAULT 0,
                revoked_at TEXT,
                revoked_by TEXT,
                notes TEXT
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        sqlx::query(
            r#"
            CREATE TABLE admin_audit_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                admin_did TEXT NOT NULL,
                action TEXT NOT NULL,
                subject_did TEXT,
                details TEXT,
                timestamp TEXT NOT NULL,
                ip_address TEXT
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        let manager = AdminRoleManager::new(db);

        // Grant role
        let role = manager
            .grant_role(
                "did:plc:alice",
                Role::Admin,
                "did:plc:superadmin",
                Some("First admin".to_string()),
            )
            .await
            .unwrap();

        assert_eq!(role.did, "did:plc:alice");
        assert_eq!(role.role, Role::Admin);
        assert!(!role.revoked);

        // Get role
        let retrieved = manager.get_role("did:plc:alice").await.unwrap().unwrap();
        assert_eq!(retrieved.role, Role::Admin);

        // Check role
        assert!(manager
            .has_role("did:plc:alice", Role::Admin)
            .await
            .unwrap());
        assert!(manager
            .has_role("did:plc:alice", Role::Moderator)
            .await
            .unwrap());
        assert!(!manager
            .has_role("did:plc:alice", Role::SuperAdmin)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_revoke_role() {
        let db = SqlitePool::connect(":memory:").await.unwrap();

        sqlx::query(
            r#"
            CREATE TABLE admin_roles (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                did TEXT NOT NULL UNIQUE,
                role TEXT NOT NULL,
                granted_by TEXT,
                granted_at TEXT NOT NULL,
                revoked INTEGER NOT NULL DEFAULT 0,
                revoked_at TEXT,
                revoked_by TEXT,
                notes TEXT
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        sqlx::query(
            r#"
            CREATE TABLE admin_audit_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                admin_did TEXT NOT NULL,
                action TEXT NOT NULL,
                subject_did TEXT,
                details TEXT,
                timestamp TEXT NOT NULL,
                ip_address TEXT
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        let manager = AdminRoleManager::new(db);

        // Grant then revoke
        manager
            .grant_role("did:plc:bob", Role::Moderator, "did:plc:admin", None)
            .await
            .unwrap();

        manager
            .revoke_role(
                "did:plc:bob",
                "did:plc:admin",
                Some("No longer needed".to_string()),
            )
            .await
            .unwrap();

        // Should not have active role
        assert!(manager.get_role("did:plc:bob").await.unwrap().is_none());
        assert!(!manager
            .has_role("did:plc:bob", Role::Moderator)
            .await
            .unwrap());
    }
}
