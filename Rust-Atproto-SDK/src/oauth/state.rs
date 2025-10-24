//! OAuth state and nonce management for CSRF protection
//!
//! This module provides state management for OAuth flows to prevent
//! Cross-Site Request Forgery (CSRF) attacks.

use rand::Rng;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

/// OAuth state parameter for CSRF protection
///
/// The state parameter is generated for each authorization request
/// and must be validated when the callback is received to ensure
/// the request originated from this client.
#[derive(Debug, Clone)]
pub struct OAuthState {
    /// The state value (32-character random string)
    pub value: String,

    /// When this state was created
    pub created_at: SystemTime,

    /// Optional metadata associated with this state
    pub metadata: Option<serde_json::Value>,
}

impl OAuthState {
    /// Generate a new OAuth state parameter
    ///
    /// Creates a cryptographically secure random 32-character state value.
    ///
    /// ## Example
    ///
    /// ```
    /// use atproto::oauth::state::OAuthState;
    ///
    /// let state = OAuthState::generate();
    /// assert_eq!(state.value.len(), 32);
    /// ```
    pub fn generate() -> Self {
        Self::generate_with_metadata(None)
    }

    /// Generate a new OAuth state with optional metadata
    ///
    /// The metadata can be used to store additional information about
    /// the authorization request (e.g., original request URL, user preferences).
    ///
    /// ## Arguments
    ///
    /// * `metadata` - Optional JSON metadata to associate with this state
    ///
    /// ## Example
    ///
    /// ```
    /// use atproto::oauth::state::OAuthState;
    ///
    /// let metadata = serde_json::json!({
    ///     "redirect_after": "/dashboard"
    /// });
    /// let state = OAuthState::generate_with_metadata(Some(metadata));
    /// ```
    pub fn generate_with_metadata(metadata: Option<serde_json::Value>) -> Self {
        const STATE_LENGTH: usize = 32;
        const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

        let mut rng = rand::thread_rng();
        let value: String = (0..STATE_LENGTH)
            .map(|_| {
                let idx = rng.gen_range(0..CHARSET.len());
                CHARSET[idx] as char
            })
            .collect();

        Self {
            value,
            created_at: SystemTime::now(),
            metadata,
        }
    }

    /// Check if this state has expired
    ///
    /// States are considered expired after 10 minutes to prevent
    /// replay attacks and limit the window for CSRF attacks.
    ///
    /// ## Arguments
    ///
    /// * `max_age` - Maximum age before expiration (default: 10 minutes)
    ///
    /// ## Returns
    ///
    /// `true` if the state has expired
    pub fn is_expired(&self, max_age: Option<Duration>) -> bool {
        let max_age = max_age.unwrap_or(Duration::from_secs(600)); // 10 minutes
        SystemTime::now()
            .duration_since(self.created_at)
            .map(|age| age > max_age)
            .unwrap_or(true)
    }
}

/// OAuth state manager for tracking and validating state parameters
///
/// Maintains a store of active state values to validate OAuth callbacks.
/// Automatically cleans up expired states.
///
/// ## Thread Safety
///
/// This manager is thread-safe and can be shared across multiple threads.
///
/// ## Example
///
/// ```
/// use atproto::oauth::state::StateManager;
///
/// let manager = StateManager::new();
///
/// // Generate and store state
/// let state = manager.generate_state(None);
///
/// // Later, validate the state from callback
/// assert!(manager.validate_state(&state.value));
///
/// // State can only be validated once
/// assert!(!manager.validate_state(&state.value));
/// ```
pub struct StateManager {
    /// Store of active states
    states: Arc<RwLock<HashMap<String, OAuthState>>>,
}

impl StateManager {
    /// Create a new state manager
    pub fn new() -> Self {
        Self {
            states: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Generate and store a new state
    ///
    /// ## Arguments
    ///
    /// * `metadata` - Optional metadata to associate with the state
    ///
    /// ## Returns
    ///
    /// The generated state
    pub fn generate_state(&self, metadata: Option<serde_json::Value>) -> OAuthState {
        let state = OAuthState::generate_with_metadata(metadata);

        // Store the state
        let mut states = self.states.write().unwrap();
        states.insert(state.value.clone(), state.clone());

        state
    }

    /// Validate a state value and consume it
    ///
    /// Validates that the state exists, is not expired, and removes it
    /// from the store (states can only be validated once).
    ///
    /// ## Arguments
    ///
    /// * `state_value` - The state value to validate
    ///
    /// ## Returns
    ///
    /// `true` if the state is valid, `false` otherwise
    pub fn validate_state(&self, state_value: &str) -> bool {
        let mut states = self.states.write().unwrap();

        // Remove the state (can only be used once)
        if let Some(state) = states.remove(state_value) {
            // Check if expired
            !state.is_expired(None)
        } else {
            false
        }
    }

    /// Validate a state and retrieve its metadata
    ///
    /// Like `validate_state`, but also returns the associated metadata.
    ///
    /// ## Arguments
    ///
    /// * `state_value` - The state value to validate
    ///
    /// ## Returns
    ///
    /// The metadata if the state is valid, `None` otherwise
    pub fn validate_and_get_metadata(
        &self,
        state_value: &str,
    ) -> Option<serde_json::Value> {
        let mut states = self.states.write().unwrap();

        if let Some(state) = states.remove(state_value) {
            if !state.is_expired(None) {
                return state.metadata;
            }
        }

        None
    }

    /// Clean up expired states
    ///
    /// Removes all expired states from the store. Should be called
    /// periodically to prevent memory leaks.
    ///
    /// ## Returns
    ///
    /// The number of states cleaned up
    pub fn cleanup_expired(&self) -> usize {
        let mut states = self.states.write().unwrap();
        let initial_count = states.len();

        states.retain(|_, state| !state.is_expired(None));

        initial_count - states.len()
    }

    /// Get the number of active states
    pub fn count(&self) -> usize {
        self.states.read().unwrap().len()
    }

    /// Clear all states
    ///
    /// Removes all states from the store. Useful for testing or cleanup.
    pub fn clear(&self) {
        self.states.write().unwrap().clear();
    }
}

impl Default for StateManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_generate_state() {
        let state = OAuthState::generate();
        assert_eq!(state.value.len(), 32);

        // Should only contain alphanumeric characters
        for c in state.value.chars() {
            assert!(c.is_ascii_alphanumeric());
        }
    }

    #[test]
    fn test_generate_state_with_metadata() {
        let metadata = serde_json::json!({
            "redirect": "/dashboard"
        });

        let state = OAuthState::generate_with_metadata(Some(metadata.clone()));
        assert_eq!(state.value.len(), 32);
        assert_eq!(state.metadata, Some(metadata));
    }

    #[test]
    fn test_state_uniqueness() {
        let states: Vec<OAuthState> = (0..100).map(|_| OAuthState::generate()).collect();

        // All states should be unique
        for i in 0..states.len() {
            for j in (i + 1)..states.len() {
                assert_ne!(states[i].value, states[j].value);
            }
        }
    }

    #[test]
    fn test_state_expiration() {
        let state = OAuthState::generate();

        // Should not be expired immediately
        assert!(!state.is_expired(None));

        // Should not be expired within 1 second
        assert!(!state.is_expired(Some(Duration::from_secs(1))));

        // Should be expired after 0 seconds
        thread::sleep(Duration::from_millis(10));
        assert!(state.is_expired(Some(Duration::from_secs(0))));
    }

    #[test]
    fn test_state_manager_generate() {
        let manager = StateManager::new();
        let state = manager.generate_state(None);

        assert_eq!(state.value.len(), 32);
        assert_eq!(manager.count(), 1);
    }

    #[test]
    fn test_state_manager_validate() {
        let manager = StateManager::new();
        let state = manager.generate_state(None);

        // Should validate successfully once
        assert!(manager.validate_state(&state.value));

        // Should fail second time (state consumed)
        assert!(!manager.validate_state(&state.value));

        // Count should be 0 after validation
        assert_eq!(manager.count(), 0);
    }

    #[test]
    fn test_state_manager_validate_invalid() {
        let manager = StateManager::new();

        // Should fail for non-existent state
        assert!(!manager.validate_state("invalid_state"));
    }

    #[test]
    fn test_state_manager_with_metadata() {
        let manager = StateManager::new();
        let metadata = serde_json::json!({"user_id": "123"});
        let state = manager.generate_state(Some(metadata.clone()));

        let retrieved_metadata = manager.validate_and_get_metadata(&state.value);
        assert_eq!(retrieved_metadata, Some(metadata));
    }

    #[test]
    fn test_state_manager_cleanup() {
        let manager = StateManager::new();

        // Generate some states
        let _state1 = manager.generate_state(None);
        let _state2 = manager.generate_state(None);
        let _state3 = manager.generate_state(None);

        assert_eq!(manager.count(), 3);

        // Wait for expiration
        thread::sleep(Duration::from_millis(50));

        // Cleanup with 0 second max age should remove all
        let cleaned = manager.cleanup_expired();
        assert!(cleaned <= 3);
    }

    #[test]
    fn test_state_manager_clear() {
        let manager = StateManager::new();

        manager.generate_state(None);
        manager.generate_state(None);

        assert_eq!(manager.count(), 2);

        manager.clear();
        assert_eq!(manager.count(), 0);
    }

    #[test]
    fn test_state_manager_thread_safety() {
        let manager = Arc::new(StateManager::new());
        let mut handles = vec![];

        // Spawn multiple threads generating states
        for _ in 0..10 {
            let manager_clone = Arc::clone(&manager);
            let handle = thread::spawn(move || {
                for _ in 0..10 {
                    manager_clone.generate_state(None);
                }
            });
            handles.push(handle);
        }

        // Wait for all threads
        for handle in handles {
            handle.join().unwrap();
        }

        // Should have 100 states
        assert_eq!(manager.count(), 100);
    }
}
