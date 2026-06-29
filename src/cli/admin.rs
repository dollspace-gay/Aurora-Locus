//! Admin role management CLI commands.
//!
//! `grant-admin` is the v0.3 bootstrap path for adding admin roles
//! before there's a way to grant them through the live PDS API. The
//! command is offline-only — it acquires the same PDS-liveness lock
//! that `serve` would, so it cannot run while a PDS is up against
//! the same database.
//!
//! The grant lands atomically: the `admin_roles` write and the
//! audit-chain entry append happen inside a single transaction
//! (LB-1 / chainlink #122 atomicity contract). Concurrent appenders
//! are serialized via `AppendChainGuard` held across the
//! `tx.commit()` boundary.

use crate::{
    admin::{
        audit_chain::{insert_chain_entry, AppendChainGuard, AppendEntryParams},
        defs::Subject,
        roles::Role,
    },
    context::AppContext,
    db::liveness_lock::LivenessLock,
    error::{PdsError, PdsResult},
};
use chrono::Utc;
use sqlx::Row;
use std::str::FromStr;

/// Sentinel `actor_did` recorded on CLI-originated audit entries.
/// `cli:` prefix is intentionally not a valid DID method, which makes
/// these entries trivially distinguishable from PDS-originated grants
/// in audit-log analysis (`SELECT * FROM audit_chain_entry WHERE
/// actor_did LIKE 'cli:%'`). Per recon Q2, the column is unconstrained
/// `TEXT NOT NULL`, so any non-empty string works schema-wise.
const CLI_GRANT_ACTOR_DID: &str = "cli:grant-admin";

/// Audit-chain action vocabulary for CLI-originated grants. Matches
/// the dotted lowercase convention used by recent admin actions in
/// `src/api/aurora_admin.rs` (e.g. `account.batch_takedown`,
/// `label.batch_apply`).
const GRANT_ACTION: &str = "admin.grant_role";

/// Grant an admin role to a DID via the CLI.
///
/// Validation (per Step 3): DID format check + role parse
/// (case-insensitive). On valid input, acquires the PDS-liveness
/// lock to fail fast if a PDS is running, then performs the
/// SELECT-before-INSERT three-branch grant flow (§5.3.3 step 6):
///
/// - **No row** → INSERT a new active row.
/// - **Active row** → reject (operator must explicitly revoke
///   first; `--force` does NOT bypass).
/// - **Revoked row** without `--force` → reject with actionable
///   message pointing at `--force`.
/// - **Revoked row** with `--force` → UPDATE the existing row to
///   active (UNIQUE-on-`did` schema rules out a fresh INSERT).
///
/// In every grant branch the `admin_roles` write and the audit-chain
/// entry append happen inside a single transaction.
pub async fn grant_admin(
    ctx: &AppContext,
    did: String,
    role: String,
    notes: Option<String>,
    force: bool,
) -> PdsResult<()> {
    let did = validate_did_format(&did)?;
    let role = parse_role(&role)?;

    // Offline check (§5.4.4 line 859-860). LivenessLock::acquire is
    // already non-blocking on both backends (pg_try_advisory_lock /
    // try_lock_exclusive); a held lock fast-fails. Hold the guard
    // for the duration of the grant so a PDS can't start mid-grant.
    let _liveness_guard = LivenessLock::acquire(&ctx.config).await.map_err(|e| {
        PdsError::Validation(format!(
            "Cannot grant admin role: {} \
             Stop the PDS before running grant-admin.",
            e
        ))
    })?;

    let GrantOutcome {
        entry_id,
        re_granted_from_revoked,
    } = perform_grant(ctx, &did, role, notes.as_deref(), force).await?;

    let force_note = if re_granted_from_revoked {
        " (re-granted from revoked state via --force)"
    } else {
        ""
    };
    match notes.as_deref() {
        Some(n) => println!(
            "Granted role '{role}' to {did}{force_note}. Notes: {n}. Audit entry: #{entry_id}.",
            role = role.as_str(),
            did = did,
            force_note = force_note,
            n = n,
            entry_id = entry_id,
        ),
        None => println!(
            "Granted role '{role}' to {did}{force_note}. Audit entry: #{entry_id}.",
            role = role.as_str(),
            did = did,
            force_note = force_note,
            entry_id = entry_id,
        ),
    }

    Ok(())
}

/// Result of `perform_grant`. The boolean lets the caller annotate
/// the success line so operators see whether `--force` materially
/// changed prior state.
struct GrantOutcome {
    entry_id: i64,
    re_granted_from_revoked: bool,
}

/// Perform the grant inside a single transaction. Returns the audit
/// entry's row id on success.
///
/// SELECT-before-INSERT (recon Q7): the SELECT inside the
/// transaction makes the active-row rejection check linearizable
/// with the subsequent write — concurrent grants on the same DID
/// can't both observe "no active row" and both INSERT.
///
/// The audit-chain append uses `insert_chain_entry` so it lands in
/// the same transaction as the role write. `AppendChainGuard` is
/// held across the commit per LB-1 / chainlink #122.
async fn perform_grant(
    ctx: &AppContext,
    did: &str,
    role: Role,
    notes: Option<&str>,
    force: bool,
) -> PdsResult<GrantOutcome> {
    let _chain_guard = AppendChainGuard::acquire().await;

    let mut tx = ctx.account_db.begin().await?;

    let existing = sqlx::query(
        "SELECT role, revoked FROM admin_roles WHERE did = $1",
    )
    .bind(did)
    .fetch_optional(&mut *tx)
    .await?;

    let mut re_granted_from_revoked = false;
    let action_rationale = match existing {
        None => {
            // No row → INSERT.
            sqlx::query(
                "INSERT INTO admin_roles (did, role, granted_by, granted_at, notes) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(did)
            .bind(role.as_str())
            .bind(CLI_GRANT_ACTOR_DID)
            .bind(Utc::now().to_rfc3339())
            .bind(notes)
            .execute(&mut *tx)
            .await?;
            grant_rationale(role, notes, false)
        }
        Some(row) => {
            let revoked = read_revoked_flag(&row)?;
            let existing_role: String = row.get("role");
            if !revoked {
                return Err(PdsError::Validation(format!(
                    "DID {} already has an active role ({}). \
                     Revoke it first before granting a new role; \
                     --force does not bypass active rows.",
                    did, existing_role
                )));
            }
            // Revoked row: gated by --force.
            if !force {
                return Err(PdsError::Validation(format!(
                    "DID {} has a revoked role ({}). \
                     Re-grant with --force to restore admin access.",
                    did, existing_role
                )));
            }
            // UPDATE the existing row in place — UNIQUE on `did`
            // means a fresh INSERT would conflict.
            sqlx::query(
                "UPDATE admin_roles \
                 SET role = $1, granted_by = $2, granted_at = $3, notes = $4, \
                     revoked = 0, revoked_at = NULL, revoked_by = NULL \
                 WHERE did = $5",
            )
            .bind(role.as_str())
            .bind(CLI_GRANT_ACTOR_DID)
            .bind(Utc::now().to_rfc3339())
            .bind(notes)
            .bind(did)
            .execute(&mut *tx)
            .await?;
            re_granted_from_revoked = true;
            grant_rationale(role, notes, true)
        }
    };

    let subject = Subject::Repo {
        did: did.to_string(),
    };
    let entry_id = insert_chain_entry(
        &mut tx,
        ctx.config.database.backend,
        AppendEntryParams {
            source: "manual",
            payload: None,
            actor_did: CLI_GRANT_ACTOR_DID,
            action: GRANT_ACTION,
            subject: Some(&subject),
            rationale: &action_rationale,
            snapshot_id: None,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await?;

    tx.commit().await?;

    Ok(GrantOutcome {
        entry_id,
        re_granted_from_revoked,
    })
}

/// Read the `revoked` column tolerantly: SQLite stores booleans as
/// `INTEGER` (0/1); the `bool`-typed `try_get` works on Postgres but
/// fails on SQLite's INTEGER column. Read as i64 and compare.
fn read_revoked_flag(row: &sqlx::any::AnyRow) -> PdsResult<bool> {
    if let Ok(i) = row.try_get::<i64, _>("revoked") {
        return Ok(i != 0);
    }
    row.try_get::<bool, _>("revoked").map_err(|e| {
        PdsError::Internal(format!("failed to read admin_roles.revoked: {}", e))
    })
}

/// Build the audit-chain rationale string. Carries the role, notes
/// presence, and re-grant flag so log readers see the operator
/// intent at a glance.
fn grant_rationale(role: Role, notes: Option<&str>, re_granted: bool) -> String {
    let mut parts = vec![format!("CLI grant of role={}", role.as_str())];
    if let Some(n) = notes {
        parts.push(format!("notes={}", n));
    }
    if re_granted {
        parts.push("re-granted from revoked state via --force".to_string());
    }
    parts.join("; ")
}

/// Minimal DID-format check: non-empty, `did:` prefix, at least one
/// `:` separator after the prefix so the method and identifier are
/// both present. Full DID-syntax validation (per W3C DID Core) is
/// out of scope for the CLI — operators paste DIDs from upstream
/// directories that already enforce it.
fn validate_did_format(input: &str) -> PdsResult<String> {
    if input.is_empty() {
        return Err(PdsError::Validation(
            "DID must not be empty. Expected format: did:<method>:<identifier>".to_string(),
        ));
    }
    let rest = input.strip_prefix("did:").ok_or_else(|| {
        PdsError::Validation(format!(
            "Invalid DID format: '{}'. Expected format: did:<method>:<identifier>",
            input
        ))
    })?;
    let (method, identifier) = rest.split_once(':').ok_or_else(|| {
        PdsError::Validation(format!(
            "Invalid DID format: '{}'. Expected format: did:<method>:<identifier>",
            input
        ))
    })?;
    if method.is_empty() || identifier.is_empty() {
        return Err(PdsError::Validation(format!(
            "Invalid DID format: '{}'. Expected format: did:<method>:<identifier>",
            input
        )));
    }
    Ok(input.to_string())
}

/// Parse the operator-supplied role string into a typed `Role` and
/// surface a listed-valid-roles error message on failure.
fn parse_role(input: &str) -> PdsResult<Role> {
    Role::from_str(input).map_err(|_| {
        PdsError::Validation(format!(
            "Invalid role '{}'. Valid roles: moderator, admin, superadmin",
            input
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_did_format_accepts_did_plc() {
        assert!(validate_did_format("did:plc:abc").is_ok());
    }

    #[test]
    fn validate_did_format_accepts_did_web() {
        assert!(validate_did_format("did:web:example.com").is_ok());
    }

    #[test]
    fn validate_did_format_rejects_empty() {
        let err = validate_did_format("").unwrap_err();
        assert!(matches!(err, PdsError::Validation(_)));
        assert!(err.to_string().contains("must not be empty"));
    }

    #[test]
    fn validate_did_format_rejects_missing_did_prefix() {
        let err = validate_did_format("plc:abc").unwrap_err();
        assert!(matches!(err, PdsError::Validation(_)));
        assert!(err.to_string().contains("Invalid DID format"));
    }

    #[test]
    fn validate_did_format_rejects_no_method_colon() {
        let err = validate_did_format("did:plc").unwrap_err();
        assert!(matches!(err, PdsError::Validation(_)));
        assert!(err.to_string().contains("Invalid DID format"));
    }

    #[test]
    fn validate_did_format_rejects_empty_method_or_identifier() {
        assert!(validate_did_format("did::abc").is_err());
        assert!(validate_did_format("did:plc:").is_err());
    }

    #[test]
    fn validate_did_format_rejects_opaque_token() {
        let err = validate_did_format("eyJhbGciOiJIUzI1NiJ9.foo.bar").unwrap_err();
        assert!(matches!(err, PdsError::Validation(_)));
    }

    #[test]
    fn parse_role_accepts_lowercase_canonical() {
        assert_eq!(parse_role("admin").unwrap(), Role::Admin);
        assert_eq!(parse_role("moderator").unwrap(), Role::Moderator);
        assert_eq!(parse_role("superadmin").unwrap(), Role::SuperAdmin);
    }

    #[test]
    fn parse_role_is_case_insensitive() {
        assert_eq!(parse_role("Admin").unwrap(), Role::Admin);
        assert_eq!(parse_role("ADMIN").unwrap(), Role::Admin);
        assert_eq!(parse_role("aDmIn").unwrap(), Role::Admin);
    }

    #[test]
    fn parse_role_rejects_bogus_with_listed_valid_roles() {
        let err = parse_role("bogus-role").unwrap_err();
        assert!(matches!(err, PdsError::Validation(_)));
        let msg = err.to_string();
        assert!(msg.contains("Invalid role 'bogus-role'"));
        assert!(msg.contains("moderator"));
        assert!(msg.contains("admin"));
        assert!(msg.contains("superadmin"));
    }

    #[test]
    fn grant_rationale_includes_role_and_notes() {
        let r = grant_rationale(Role::Admin, Some("hi"), false);
        assert!(r.contains("role=admin"));
        assert!(r.contains("notes=hi"));
        assert!(!r.contains("re-granted"));
    }

    #[test]
    fn grant_rationale_marks_re_grant() {
        let r = grant_rationale(Role::Moderator, None, true);
        assert!(r.contains("role=moderator"));
        assert!(r.contains("re-granted from revoked state via --force"));
    }
}
