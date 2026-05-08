//! Admin role management CLI commands.
//!
//! Step 3 (§5.4.3) lands the scaffolding for `grant-admin`: clap
//! variant, dispatch arm, function signature, input validation,
//! typed-`Role` parse. The grant body itself is intentionally
//! `unimplemented!()` — Step 4 fills it in (offline check via the
//! cooperative lock from Step 0.11, `admin_roles` write, audit-chain
//! entry).
//!
//! The discipline of "parse once, pass typed value through" is set
//! up here: the `role: Role` binding produced by validation flows
//! directly into Step 4's grant logic — no re-parse between steps.

use crate::{
    admin::roles::Role,
    context::AppContext,
    error::{PdsError, PdsResult},
};
use std::str::FromStr;

/// Grant an admin role to a DID via the CLI.
///
/// Step 3 implements input validation only:
/// - DID format check (non-empty, `did:<method>:<id>` shape).
/// - Role parse via `Role::from_str` (case-insensitive per the
///   existing impl at `src/admin/roles.rs:67-78`).
///
/// On success the function prints an intent line so operators can
/// confirm their inputs were parsed correctly, then panics with
/// `unimplemented!()`. The panic is the contract that prevents Step
/// 3 from being mistaken for a working grant tool — Step 4 replaces
/// the panic with the actual grant write + offline check + audit
/// entry.
pub async fn grant_admin(
    ctx: &AppContext,
    did: String,
    role: String,
    notes: Option<String>,
    force: bool,
) -> PdsResult<()> {
    let did = validate_did_format(&did)?;
    let role = parse_role(&role)?;

    // ctx is unused in Step 3 (no DB writes) but kept in the
    // signature so Step 4 doesn't change the public surface.
    let _ = ctx;

    println!(
        "Grant intent: did={did}, role={role}, notes={notes:?}, force={force}",
        did = did,
        role = role.as_str(),
        notes = notes,
        force = force
    );

    unimplemented!("grant body — implemented in Step 4")
}

/// Minimal DID-format check: non-empty, `did:` prefix, at least one
/// `:` separator after the prefix so the method and identifier are
/// both present. Full DID-syntax validation (per W3C DID Core) is
/// out of scope for the CLI — operators paste DIDs from upstream
/// directories that already enforce it.
///
/// Returns the input string back on success so the caller can keep
/// using a single binding without re-borrowing through the original
/// `&str`.
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
    // After `did:` there must be a `<method>:<identifier>` body
    // — i.e., at least one more `:` and a non-empty method and
    // identifier.
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
///
/// The underlying `Role::FromStr` impl is case-insensitive, so
/// `Admin`, `ADMIN`, `aDmIn` all map to `Role::Admin`. We replace
/// its terse "Invalid role: X" message with one that lists the
/// valid roles in the lowercase canonical form operators see.
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
        // `did:plc` lacks the second `:` separating method from id.
        let err = validate_did_format("did:plc").unwrap_err();
        assert!(matches!(err, PdsError::Validation(_)));
        assert!(err.to_string().contains("Invalid DID format"));
    }

    #[test]
    fn validate_did_format_rejects_empty_method_or_identifier() {
        // `did::abc` has empty method.
        assert!(validate_did_format("did::abc").is_err());
        // `did:plc:` has empty identifier.
        assert!(validate_did_format("did:plc:").is_err());
    }

    #[test]
    fn validate_did_format_rejects_opaque_token() {
        // What an operator might paste if they grabbed a session
        // token instead of a DID.
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
        // Operator-friendly lowercase listing per Q6 audit.
        assert!(msg.contains("moderator"));
        assert!(msg.contains("admin"));
        assert!(msg.contains("superadmin"));
    }
}
