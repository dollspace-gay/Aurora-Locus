//! Per-DID admin security settings (chainlink #442): IP binding, session
//! lifetime override, and TOTP enrollment state, keyed by DID in the
//! `admin_security_config` table. A sibling to `admin_roles`; absence of a row
//! means all defaults, so a config is only written when a DID opts into a
//! feature. The store backs the session-lifetime override + the role-based
//! sliding-refresh defaults; IP binding and TOTP wire their columns in later
//! commits.
//!
//! Sliding-refresh model: access and refresh tokens share a single lifetime,
//! sized to the role's typical task time (SuperAdmin 15m / Admin 30m /
//! Moderator 1h). Both tokens are minted with that lifetime and every refresh
//! re-mints both, so activity slides the whole session forward and idle past
//! the window expires it. A per-account override replaces the role default.

use crate::error::{PdsError, PdsResult};
use chrono::Utc;
use sqlx::AnyPool;
use sqlx::Row;

use super::roles::Role;

/// Minimum accepted session-lifetime override — rejects footgun-short windows.
pub const MIN_SESSION_LIFETIME_SECS: i64 = 60;
/// Maximum accepted session-lifetime override — 30 days.
pub const MAX_SESSION_LIFETIME_SECS: i64 = 30 * 24 * 60 * 60;

/// Step-up freshness window: a hardening endpoint requires the caller to have
/// interactively authenticated within this many seconds. Measured from the
/// session's `authenticated_at` (login time, preserved across refreshes), so a
/// silent refresh does not satisfy it — only a real re-login does.
pub const STEP_UP_MAX_AGE_SECS: i64 = 5 * 60;

/// A DID's admin security settings. Callers treat a `None` from
/// [`AdminSecurityStore::get_config`] as "all defaults"; this struct is only
/// materialised for a DID that has a row.
#[derive(Debug, Clone)]
pub struct AdminSecurityConfig {
    pub did: String,
    pub ip_binding_enabled: bool,
    /// Session lifetime override in seconds — applies to both the access and
    /// refresh tokens; `None` = role default.
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
        validate_session_lifetime(secs)?;
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

    /// Transaction-aware [`set_session_lifetime`], so a mutating XRPC can write
    /// the override and its audit-chain entry atomically (LB-1, chainlink #122):
    /// a crash between the two leaves neither, not a config change without a
    /// breadcrumb. Same UPDATE-then-INSERT upsert, on the caller's transaction.
    pub async fn set_session_lifetime_in_tx<'c>(
        tx: &mut sqlx::Transaction<'c, sqlx::Any>,
        did: &str,
        secs: Option<i64>,
    ) -> PdsResult<()> {
        validate_session_lifetime(secs)?;
        let now = Utc::now().to_rfc3339();

        let updated = sqlx::query(
            "UPDATE admin_security_config SET session_lifetime_secs = $1, updated_at = $2 WHERE did = $3",
        )
        .bind(secs)
        .bind(&now)
        .bind(did)
        .execute(&mut **tx)
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
            .execute(&mut **tx)
            .await
            .map_err(PdsError::Database)?;
        }
        Ok(())
    }

    /// Store a pending (unconfirmed) TOTP secret for a DID (#442): sets
    /// `totp_secret_encrypted` and clears `totp_confirmed_at`, so a half-finished
    /// enrollment is not enforced at login. Upserts, preserving the other
    /// settings. Transaction-aware for atomic mutation + audit.
    pub async fn set_totp_pending_in_tx<'c>(
        tx: &mut sqlx::Transaction<'c, sqlx::Any>,
        did: &str,
        encrypted_secret: &str,
    ) -> PdsResult<()> {
        let now = Utc::now().to_rfc3339();
        let updated = sqlx::query(
            "UPDATE admin_security_config \
             SET totp_secret_encrypted = $1, totp_confirmed_at = NULL, updated_at = $2 \
             WHERE did = $3",
        )
        .bind(encrypted_secret)
        .bind(&now)
        .bind(did)
        .execute(&mut **tx)
        .await
        .map_err(PdsError::Database)?
        .rows_affected();

        if updated == 0 {
            sqlx::query(
                "INSERT INTO admin_security_config (did, ip_binding_enabled, totp_secret_encrypted, updated_at) \
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(did)
            .bind(false)
            .bind(encrypted_secret)
            .bind(&now)
            .execute(&mut **tx)
            .await
            .map_err(PdsError::Database)?;
        }
        Ok(())
    }

    /// Mark a DID's pending TOTP secret as confirmed (#442): sets
    /// `totp_confirmed_at = now`. Returns `false` if there was no row with a
    /// pending secret to confirm (caller maps to a 400). Transaction-aware.
    pub async fn confirm_totp_in_tx<'c>(
        tx: &mut sqlx::Transaction<'c, sqlx::Any>,
        did: &str,
    ) -> PdsResult<bool> {
        let now = Utc::now().to_rfc3339();
        let updated = sqlx::query(
            "UPDATE admin_security_config \
             SET totp_confirmed_at = $1, updated_at = $2 \
             WHERE did = $3 AND totp_secret_encrypted IS NOT NULL",
        )
        .bind(&now)
        .bind(&now)
        .bind(did)
        .execute(&mut **tx)
        .await
        .map_err(PdsError::Database)?
        .rows_affected();
        Ok(updated > 0)
    }

    /// Set a DID's `ip_binding_enabled` flag (#442). Upserts, preserving the
    /// other settings. Transaction-aware for atomic mutation + audit. The
    /// trust_proxy precondition is enforced at the handler, not here.
    pub async fn set_ip_binding_in_tx<'c>(
        tx: &mut sqlx::Transaction<'c, sqlx::Any>,
        did: &str,
        enabled: bool,
    ) -> PdsResult<()> {
        let now = Utc::now().to_rfc3339();
        let updated = sqlx::query(
            "UPDATE admin_security_config SET ip_binding_enabled = $1, updated_at = $2 WHERE did = $3",
        )
        .bind(enabled)
        .bind(&now)
        .bind(did)
        .execute(&mut **tx)
        .await
        .map_err(PdsError::Database)?
        .rows_affected();

        if updated == 0 {
            sqlx::query(
                "INSERT INTO admin_security_config (did, ip_binding_enabled, updated_at) \
                 VALUES ($1, $2, $3)",
            )
            .bind(did)
            .bind(enabled)
            .bind(&now)
            .execute(&mut **tx)
            .await
            .map_err(PdsError::Database)?;
        }
        Ok(())
    }

    /// Clear a DID's TOTP entirely (#442): nulls both the secret and the
    /// confirmation. A no-op (0 rows) when TOTP was never set. Transaction-aware.
    pub async fn clear_totp_in_tx<'c>(
        tx: &mut sqlx::Transaction<'c, sqlx::Any>,
        did: &str,
    ) -> PdsResult<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE admin_security_config \
             SET totp_secret_encrypted = NULL, totp_confirmed_at = NULL, updated_at = $1 \
             WHERE did = $2",
        )
        .bind(&now)
        .bind(did)
        .execute(&mut **tx)
        .await
        .map_err(PdsError::Database)?;
        Ok(())
    }
}

/// Validate a session-lifetime override is within `[MIN, MAX]`. `None` (revert
/// to the role default) is always valid. Callers surface a rejection as a
/// client error (400), not a server fault.
pub fn validate_session_lifetime(secs: Option<i64>) -> PdsResult<()> {
    if let Some(s) = secs {
        if !(MIN_SESSION_LIFETIME_SECS..=MAX_SESSION_LIFETIME_SECS).contains(&s) {
            return Err(PdsError::Validation(format!(
                "session lifetime must be between {MIN_SESSION_LIFETIME_SECS} and \
                 {MAX_SESSION_LIFETIME_SECS} seconds"
            )));
        }
    }
    Ok(())
}

/// Role-based session lifetime (both tokens, idle-timeout), before any
/// override. Sized to the role's typical task time: the shorter the burst of
/// work a role does, the tighter its idle window.
fn role_default_lifetime_secs(role: Role) -> i64 {
    match role {
        Role::SuperAdmin => 15 * 60, // 15m — one-off structural changes, short tasks
        Role::Admin => 30 * 60,      // 30m — triage + config + investigation
        Role::Moderator => 60 * 60,  // 1h — content review, appeals, evidence
    }
}

/// The admin session lifetime in seconds — governs both the access and refresh
/// tokens: the per-account override when set (bounds-validated at write time,
/// re-clamped here defensively against a bad row), else the role default.
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
    fn role_defaults_are_15m_30m_1h() {
        assert_eq!(compute_admin_session_lifetime_secs(Role::SuperAdmin, None), 15 * 60);
        assert_eq!(compute_admin_session_lifetime_secs(Role::Admin, None), 30 * 60);
        assert_eq!(compute_admin_session_lifetime_secs(Role::Moderator, None), 60 * 60);
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
        // NULL override → role default (superadmin 15m), not the override.
        assert_eq!(
            compute_admin_session_lifetime_secs(Role::SuperAdmin, Some(&cfg(None))),
            15 * 60,
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
