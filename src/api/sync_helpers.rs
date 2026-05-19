//! Sync Protocol Helper Functions
//!
//! Shared utilities for ATProto sync endpoints (com.atproto.sync.*)

use crate::{
    account::AccountManager,
    error::{PdsError, PdsResult},
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
