//! Per-DID admin security settings (chainlink #442): IP binding, session
//! lifetime override, and TOTP enrollment state, keyed by DID in the
//! `admin_security_config` table. A sibling to `admin_roles`; absence of a row
//! means all defaults, so a config is only written when a DID opts into a
//! feature. This commit (Phase 4 · 2/4) uses the store for the session-lifetime
//! override + the role-based sliding-refresh defaults; IP binding and TOTP wire
//! their columns in later commits.

use crate::error::{PdsError, PdsResult};
use chrono::Utc;
use sqlx::AnyPool;
use sqlx::Row;

use super::roles::Role;

/// Minimum accepted session-lifetime override — rejects footgun-short windows.
pub const MIN_SESSION_LIFETIME_SECS: i64 = 60;
/// Maximum accepted session-lifetime override — 30 days.
pub const MAX_SESSION_LIFETIME_SECS: i64 = 30 * 24 * 60 * 60;

/// Admin access-token lifetime — a fixed 15 minutes for every role. Access
/// tokens are short-lived per-request credentials, refreshed frequently, so a
/// stolen one has a small blast radius. Only the refresh (idle) lifetime is
/// role-based.
pub const ADMIN_ACCESS_TOKEN_LIFETIME_SECS: i64 = 15 * 60;

/// A DID's admin security settings. Callers treat a `None` from
/// [`AdminSecurityStore::get_config`] as "all defaults"; this struct is only
/// materialised for a DID that has a row.
#[derive(Debug, Clone)]
pub struct AdminSecurityConfig {
    pub did: String,
    pub ip_binding_enabled: bool,
    /// Refresh-token (idle) lifetime override in seconds; `None` = role default.
    pub session_lifetime_secs: Option<i64>,
    pub totp_secret_encrypted: Option<String>,
    pub totp_confirmed_at: Option<String>,
    pub updated_at: String,
}

/// Store over the `admin_security_config` table.
#[derive(Clone)]
pub struct AdminSecurityStore {
    db: AnyPool,
}

impl AdminSecurityStore {
    pub fn new(db: AnyPool) -> Self {
        Self { db }
    }

    /// Read a DID's security config, or `None` if it has opted into nothing.
    pub async fn get_config(&self, did: &str) -> PdsResult<Option<AdminSecurityConfig>> {
        let row = sqlx::query(
            "SELECT did, ip_binding_enabled, session_lifetime_secs, totp_secret_encrypted, \
                    totp_confirmed_at, updated_at \
             FROM admin_security_config WHERE did = $1",
        )
        .bind(did)
        .fetch_optional(&self.db)
        .await
        .map_err(PdsError::Database)?;

        let Some(row) = row else {
            return Ok(None);
        };
        Ok(Some(AdminSecurityConfig {
            did: row.get("did"),
            // `read_bool` papers over the SQLite INTEGER / Postgres BOOLEAN
            // difference for the `ip_binding_enabled` column.
            ip_binding_enabled: crate::db::read_bool(&row, "ip_binding_enabled")?,
            session_lifetime_secs: row.get("session_lifetime_secs"),
            totp_secret_encrypted: row.get("totp_secret_encrypted"),
            totp_confirmed_at: row.get("totp_confirmed_at"),
            updated_at: row.get("updated_at"),
        }))
    }

    /// Set (or clear, with `None`) a DID's session-lifetime override. `Some(secs)`
    /// must be within `[MIN, MAX]`; `None` reverts to the role default. Upserts —
    /// creates the row (other settings at their defaults) if absent, otherwise
    /// updates just the lifetime + `updated_at`, preserving IP-binding/TOTP.
    pub async fn set_session_lifetime(&self, did: &str, secs: Option<i64>) -> PdsResult<()> {
        if let Some(s) = secs {
            if !(MIN_SESSION_LIFETIME_SECS..=MAX_SESSION_LIFETIME_SECS).contains(&s) {
                return Err(PdsError::Validation(format!(
                    "session lifetime must be between {MIN_SESSION_LIFETIME_SECS} and \
                     {MAX_SESSION_LIFETIME_SECS} seconds"
                )));
            }
        }
        let now = Utc::now().to_rfc3339();

        // UPDATE-then-INSERT-if-absent: avoids a dual-dialect ON CONFLICT and a
        // separate existence read. The PK on `did` makes a lost race collide on
        // INSERT rather than double-write.
        let updated = sqlx::query(
            "UPDATE admin_security_config SET session_lifetime_secs = $1, updated_at = $2 WHERE did = $3",
        )
        .bind(secs)
        .bind(&now)
        .bind(did)
        .execute(&self.db)
        .await
        .map_err(PdsError::Database)?
        .rows_affected();

        if updated == 0 {
            sqlx::query(
                "INSERT INTO admin_security_config (did, ip_binding_enabled, session_lifetime_secs, updated_at) \
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(did)
            .bind(false)
            .bind(secs)
            .bind(&now)
            .execute(&self.db)
            .await
            .map_err(PdsError::Database)?;
        }
        Ok(())
    }
}

/// Role-based refresh-token (idle-timeout) lifetime, before any override.
fn role_default_lifetime_secs(role: Role) -> i64 {
    match role {
        Role::SuperAdmin => 60 * 60, // 1h — highest privilege, irreversible ops
        Role::Admin => 4 * 60 * 60,  // 4h — standard, mostly-reversible ops
        Role::Moderator => 8 * 60 * 60, // 8h — reversible actions, work-shift
    }
}

/// The admin refresh-token lifetime in seconds: the per-account override when
/// set (bounds-validated at write time, re-clamped here defensively against a
/// bad row), else the role default.
pub fn compute_admin_session_lifetime_secs(
    role: Role,
    config: Option<&AdminSecurityConfig>,
) -> i64 {
    if let Some(secs) = config.and_then(|c| c.session_lifetime_secs) {
        return secs.clamp(MIN_SESSION_LIFETIME_SECS, MAX_SESSION_LIFETIME_SECS);
    }
    role_default_lifetime_secs(role)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_defaults_are_1h_4h_8h() {
        assert_eq!(compute_admin_session_lifetime_secs(Role::SuperAdmin, None), 3600);
        assert_eq!(compute_admin_session_lifetime_secs(Role::Admin, None), 14400);
        assert_eq!(compute_admin_session_lifetime_secs(Role::Moderator, None), 28800);
    }

    fn cfg(secs: Option<i64>) -> AdminSecurityConfig {
        AdminSecurityConfig {
            did: "did:plc:x".to_string(),
            ip_binding_enabled: false,
            session_lifetime_secs: secs,
            totp_secret_encrypted: None,
            totp_confirmed_at: None,
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn override_applies_and_null_reverts_to_role_default() {
        assert_eq!(
            compute_admin_session_lifetime_secs(Role::SuperAdmin, Some(&cfg(Some(3600 * 2)))),
            7200,
        );
        // NULL override → role default (superadmin 1h), not the override.
        assert_eq!(
            compute_admin_session_lifetime_secs(Role::SuperAdmin, Some(&cfg(None))),
            3600,
        );
    }

    #[test]
    fn out_of_bounds_override_is_clamped_at_compute() {
        // A row somehow below/above bounds is clamped (defense in depth; the
        // store rejects out-of-bounds at write time).
        assert_eq!(
            compute_admin_session_lifetime_secs(Role::Admin, Some(&cfg(Some(1)))),
            MIN_SESSION_LIFETIME_SECS,
        );
        assert_eq!(
            compute_admin_session_lifetime_secs(Role::Admin, Some(&cfg(Some(i64::MAX)))),
            MAX_SESSION_LIFETIME_SECS,
        );
    }
}
