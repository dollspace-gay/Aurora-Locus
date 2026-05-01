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
    // Get account
    let account = account_manager.get_account(did).await?;

    // Admins and repo owners can access any repo state
    if is_admin_or_self {
        return Ok(());
    }

    // Check if account is takendown
    if account.takedown_ref.is_some() {
        return Err(PdsError::Validation(format!(
            "Repo has been takendown: {}",
            did
        )));
    }

    // Check if account is deactivated
    if account.deactivated_at.is_some() {
        return Err(PdsError::Validation(format!(
            "Repo has been deactivated: {}",
            did
        )));
    }

    Ok(())
}

/// Get repository status for sync endpoints
///
/// Returns (active, status) tuple where:
/// - `active` is true if repo is not takendown or deactivated
/// - `status` is Some("takendown") or Some("deactivated") if applicable, None otherwise
pub fn get_repo_status(
    taken_down: bool,
    deactivated_at: Option<&chrono::DateTime<chrono::Utc>>,
) -> (bool, Option<String>) {
    let active = !taken_down && deactivated_at.is_none();

    let status = if taken_down {
        Some("takendown".to_string())
    } else if deactivated_at.is_some() {
        Some("deactivated".to_string())
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
        let (active, status) = get_repo_status(false, None);
        assert!(active);
        assert!(status.is_none());
    }

    #[test]
    fn test_get_repo_status_takendown() {
        let (active, status) = get_repo_status(true, None);
        assert!(!active);
        assert_eq!(status, Some("takendown".to_string()));
    }

    #[test]
    fn test_get_repo_status_deactivated() {
        let deactivated = Some(chrono::Utc::now());
        let (active, status) = get_repo_status(false, deactivated.as_ref());
        assert!(!active);
        assert_eq!(status, Some("deactivated".to_string()));
    }

    #[test]
    fn test_get_repo_status_takendown_precedence() {
        // If both takendown and deactivated, takendown takes precedence
        let deactivated = Some(chrono::Utc::now());
        let (active, status) = get_repo_status(true, deactivated.as_ref());
        assert!(!active);
        assert_eq!(status, Some("takendown".to_string()));
    }
}
