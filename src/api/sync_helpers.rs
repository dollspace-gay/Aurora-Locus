//! Sync Protocol Helper Functions
//!
//! Shared utilities for ATProto sync endpoints (com.atproto.sync.*)

use crate::{
    account::AccountManager,
    db::account::ActorAccount,
    error::{PdsError, PdsResult},
    sequencer::events::AccountStatus,
};

/// Check if repository is available for sync access
///
/// This mimics Bluesky's `assertRepoAvailability` function.
///
/// # Behavior
///
/// - Returns `Ok(())` if repo is accessible
/// - Returns `Err(RepoNotFound)` if account doesn't exist
/// - Returns `Err(RepoTakendown)` if takendown (unless `is_admin_or_self`)
/// - Returns `Err(RepoDeactivated)` if deactivated (unless `is_admin_or_self`)
///
/// # Arguments
///
/// * `account_manager` - The account manager to check account status
/// * `did` - The DID to check
/// * `is_admin_or_self` - Whether the requester is an admin or the repo owner
///
/// # Returns
///
/// Ok(()) if the repo is available, or an appropriate error
pub async fn assert_repo_availability(
    account_manager: &AccountManager,
    did: &str,
    is_admin_or_self: bool,
) -> PdsResult<()> {
    // Arc 14 §7.3.5 / §7.6.5: typed-error envelope per Sub-step 0.D
    // Case A. RepoNotFound when the actor row is absent.
    let account = match account_manager.get_account(did).await {
        Ok(a) => a,
        Err(PdsError::NotFound(_)) => {
            return Err(PdsError::RepoNotFound(did.to_string()));
        }
        Err(e) => return Err(e),
    };

    // Admins and repo owners can access any repo state.
    if is_admin_or_self {
        return Ok(());
    }

    if account.takedown_ref.is_some() {
        return Err(PdsError::RepoTakendown(did.to_string()));
    }

    if account.deactivated_at.is_some() {
        return Err(PdsError::RepoDeactivated(did.to_string()));
    }

    // Arc 14 §7.3.6 / migration 0010: suspended/desync columns. In
    // v0.5 these are populated only by test-affordance direct DB
    // writes (no production setter yet — v0.6+ for both).
    if account.suspended_at.is_some() {
        return Err(PdsError::RepoSuspended(did.to_string()));
    }

    if account.desynchronized_at.is_some() {
        return Err(PdsError::RepoDesynchronized(did.to_string()));
    }

    Ok(())
}

/// Arc 15 §8.3.2 — derive emit-side account status from an account
/// row. Matches bsky-PDS's `formatAccountStatus` body. Used by the
/// account-lifecycle handlers (deactivate / reactivate / takedown)
/// when they invoke `sequence_account` per Pattern B (status from
/// freshly-read row).
///
/// Branches only on `takedown_ref` + `deactivated_at` — the
/// wire-emission set per §8.1.1 is 4: Active, Takendown,
/// Deactivated, Deleted. Suspended/Desynchronized/Throttled are
/// out of v0.5 wire-emission scope (Arc 14's 6-variant enum is
/// forward-compatibility); callers that need them emit via
/// `AccountEvent::from_status` directly with the relevant variant.
///
/// `Deleted` is never returned here — the row is gone by the time
/// the delete-emit fires; use `AccountEvent::from_status(did,
/// Deleted)` at the call site (Pattern A).
pub fn get_account_status(
    account: &ActorAccount,
) -> (bool, Option<AccountStatus>) {
    if account.takedown_ref.is_some() {
        (false, Some(AccountStatus::Takendown))
    } else if account.deactivated_at.is_some() {
        (false, Some(AccountStatus::Deactivated))
    } else {
        (true, None)
    }
}

/// Get repository status for sync endpoints (Arc 14 §7.3.8 + §7.1.2).
///
/// Returns (active, status) tuple where:
/// - `active` is true if repo is not in any non-active state.
/// - `status` is `Some("takendown" | "deactivated" | "suspended" | "desynchronized")`
///   when one of those states applies, `None` for active.
///
/// Precedence (highest first): `takendown` > `deactivated` > `suspended`
/// > `desynchronized`. Matches admin-action severity ordering.
pub fn get_repo_status(
    taken_down: bool,
    deactivated_at: Option<&chrono::DateTime<chrono::Utc>>,
    suspended_at: Option<&chrono::DateTime<chrono::Utc>>,
    desynchronized_at: Option<&chrono::DateTime<chrono::Utc>>,
) -> (bool, Option<String>) {
    let active = !taken_down
        && deactivated_at.is_none()
        && suspended_at.is_none()
        && desynchronized_at.is_none();

    let status = if taken_down {
        Some("takendown".to_string())
    } else if deactivated_at.is_some() {
        Some("deactivated".to_string())
    } else if suspended_at.is_some() {
        Some("suspended".to_string())
    } else if desynchronized_at.is_some() {
        Some("desynchronized".to_string())
    } else {
        None
    };

    (active, status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn _mk_account(
        takedown: Option<&str>,
        deactivated: Option<chrono::DateTime<chrono::Utc>>,
    ) -> ActorAccount {
        ActorAccount {
            did: "did:plc:test".into(),
            handle: Some("test.localhost".into()),
            created_at: Utc::now(),
            takedown_ref: takedown.map(|s| s.to_string()),
            deactivated_at: deactivated,
            delete_after: None,
            suspended_at: None,
            desynchronized_at: None,
            email: None,
            password_hash: None,
            email_confirmed_at: None,
            invites_disabled: Some(false),
        }
    }

    /// Arc 15 §8.3.2: active account → (true, None).
    #[test]
    fn get_account_status_active() {
        let acc = _mk_account(None, None);
        assert_eq!(get_account_status(&acc), (true, None));
    }

    /// Arc 15 §8.3.2: takendown → (false, Some(Takendown)).
    #[test]
    fn get_account_status_takendown() {
        let acc = _mk_account(Some("admin-action"), None);
        assert_eq!(get_account_status(&acc), (false, Some(AccountStatus::Takendown)));
    }

    /// Arc 15 §8.3.2: deactivated → (false, Some(Deactivated)).
    #[test]
    fn get_account_status_deactivated() {
        let acc = _mk_account(None, Some(Utc::now()));
        assert_eq!(get_account_status(&acc), (false, Some(AccountStatus::Deactivated)));
    }

    /// Arc 15 §8.3.2: takedown takes precedence over deactivated.
    #[test]
    fn get_account_status_takedown_precedence() {
        let acc = _mk_account(Some("admin-action"), Some(Utc::now()));
        assert_eq!(get_account_status(&acc), (false, Some(AccountStatus::Takendown)));
    }

    #[test]
    fn test_get_repo_status_active() {
        let (active, status) = get_repo_status(false, None, None, None);
        assert!(active);
        assert!(status.is_none());
    }

    #[test]
    fn test_get_repo_status_takendown() {
        let (active, status) = get_repo_status(true, None, None, None);
        assert!(!active);
        assert_eq!(status, Some("takendown".to_string()));
    }

    #[test]
    fn test_get_repo_status_deactivated() {
        let deactivated = Some(chrono::Utc::now());
        let (active, status) = get_repo_status(false, deactivated.as_ref(), None, None);
        assert!(!active);
        assert_eq!(status, Some("deactivated".to_string()));
    }

    /// Arc 14 §7.3.8: suspended status emitted when only suspended_at set.
    #[test]
    fn test_get_repo_status_suspended() {
        let suspended = Some(chrono::Utc::now());
        let (active, status) = get_repo_status(false, None, suspended.as_ref(), None);
        assert!(!active);
        assert_eq!(status, Some("suspended".to_string()));
    }

    /// Arc 14 §7.3.8: desynchronized status emitted when only desynchronized_at set.
    #[test]
    fn test_get_repo_status_desynchronized() {
        let desync = Some(chrono::Utc::now());
        let (active, status) = get_repo_status(false, None, None, desync.as_ref());
        assert!(!active);
        assert_eq!(status, Some("desynchronized".to_string()));
    }

    #[test]
    fn test_get_repo_status_takendown_precedence() {
        // If both takendown and deactivated, takendown takes precedence
        let deactivated = Some(chrono::Utc::now());
        let (active, status) = get_repo_status(true, deactivated.as_ref(), None, None);
        assert!(!active);
        assert_eq!(status, Some("takendown".to_string()));
    }

    /// Arc 14 §7.3.8: precedence — takendown > deactivated > suspended > desync.
    #[test]
    fn test_get_repo_status_full_precedence() {
        let t = Some(chrono::Utc::now());
        let (_, status_all) = get_repo_status(true, t.as_ref(), t.as_ref(), t.as_ref());
        assert_eq!(status_all, Some("takendown".to_string()));
        let (_, status_no_td) = get_repo_status(false, t.as_ref(), t.as_ref(), t.as_ref());
        assert_eq!(status_no_td, Some("deactivated".to_string()));
        let (_, status_only_susp_desync) = get_repo_status(false, None, t.as_ref(), t.as_ref());
        assert_eq!(status_only_susp_desync, Some("suspended".to_string()));
    }
}
