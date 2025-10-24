//! Session Manager trait for the ATProto SDK
//!
//! This module defines the SessionManager trait which handles:
//! - Session authentication state
//! - HTTP request handling with session context
//! - DID (Decentralized Identifier) tracking
//! - Token refresh and persistence

use async_trait::async_trait;
use reqwest::{Request, Response};
use std::sync::{Arc, RwLock};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use crate::types::{Did, AtpSessionData, AtpSessionEvent};

/// Error type for session management operations
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Session error: {0}")]
    Session(String),

    #[error("No active session")]
    NoSession,

    #[error("Invalid DID: {0}")]
    InvalidDid(String),
}

/// Result type for session operations
pub type Result<T> = std::result::Result<T, SessionError>;

/// SessionManager trait defines the interface for managing authenticated sessions
///
/// This trait combines:
/// 1. Session state tracking (DID)
/// 2. HTTP request handling with session context
///
/// Implementations handle:
/// - Adding authentication headers to requests
/// - Token refresh logic
/// - Session persistence
///
/// # Example
///
/// ```rust,no_run
/// use atproto::session_manager::SessionManager;
/// use atproto::types::Did;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// // Implementations will provide session management
/// # Ok(())
/// # }
/// ```
#[async_trait]
pub trait SessionManager: Send + Sync {
    /// Returns the DID of the currently authenticated user, if any
    ///
    /// This is `None` when no user is authenticated.
    fn did(&self) -> Option<&Did>;

    /// Handles an HTTP request with session context
    ///
    /// This method:
    /// - Adds authentication headers if a session exists
    /// - Handles token refresh if needed
    /// - Executes the HTTP request
    ///
    /// # Arguments
    ///
    /// * `request` - The HTTP request to execute
    ///
    /// # Returns
    ///
    /// The HTTP response or an error
    async fn fetch(&self, request: Request) -> Result<Response>;

    /// Creates a clone of this session manager in a new Arc
    ///
    /// This allows sharing the session manager across multiple agents or threads
    fn clone_box(&self) -> Arc<dyn SessionManager>;

    /// Stores a session
    ///
    /// This updates the in-memory session and persists it if supported.
    ///
    /// # Arguments
    ///
    /// * `session_data` - The session data to store
    async fn store_session(&self, session_data: AtpSessionData) -> Result<()>;

    /// Loads a session from persistent storage if supported
    ///
    /// # Arguments
    ///
    /// * `did` - The DID of the account to load
    ///
    /// # Returns
    ///
    /// The session data if found, or None
    async fn load_session(&self, did: &str) -> Result<Option<AtpSessionData>>;

    /// Clears the current session
    async fn clear_session(&self) -> Result<()>;

    /// Refreshes the access token using the refresh token
    async fn refresh_token(&self) -> Result<()>;

    /// Gets the current session data
    fn get_session(&self) -> Option<AtpSessionData>;
}

/// A simple unauthenticated session manager
///
/// This implementation provides no authentication and simply
/// forwards HTTP requests without modification.
///
/// Use this for:
/// - Public API access
/// - Testing
/// - When authentication is not required
///
/// # Example
///
/// ```rust
/// use atproto::session_manager::{SessionManager, UnauthenticatedSessionManager};
///
/// let manager = UnauthenticatedSessionManager::new();
/// assert!(manager.did().is_none());
/// ```
pub struct UnauthenticatedSessionManager {
    client: reqwest::Client,
}

impl UnauthenticatedSessionManager {
    /// Creates a new unauthenticated session manager
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    /// Creates a new unauthenticated session manager with a custom HTTP client
    pub fn with_client(client: reqwest::Client) -> Self {
        Self { client }
    }
}

impl Default for UnauthenticatedSessionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SessionManager for UnauthenticatedSessionManager {
    fn did(&self) -> Option<&Did> {
        None
    }

    async fn fetch(&self, request: Request) -> Result<Response> {
        Ok(self.client.execute(request).await?)
    }

    fn clone_box(&self) -> Arc<dyn SessionManager> {
        Arc::new(Self {
            client: self.client.clone(),
        })
    }

    async fn store_session(&self, _session_data: AtpSessionData) -> Result<()> {
        // Unauthenticated manager doesn't store sessions
        Err(SessionError::Session("Cannot store session in unauthenticated mode".to_string()))
    }

    async fn load_session(&self, _did: &str) -> Result<Option<AtpSessionData>> {
        // Unauthenticated manager has no sessions to load
        Ok(None)
    }

    async fn clear_session(&self) -> Result<()> {
        // Unauthenticated manager has no session to clear
        Ok(())
    }

    async fn refresh_token(&self) -> Result<()> {
        // Unauthenticated manager has no token to refresh
        Err(SessionError::NoSession)
    }

    fn get_session(&self) -> Option<AtpSessionData> {
        None
    }
}

/// Session callback function type
///
/// Called whenever a session event occurs (create, update, delete)
pub type SessionCallback = Arc<dyn Fn(AtpSessionEvent, Option<&AtpSessionData>) + Send + Sync>;

/// Persistent session manager with file-based storage and automatic token refresh
///
/// This implementation provides:
/// - Automatic session persistence to disk
/// - Automatic token refresh before expiration
/// - Session event callbacks
/// - Multi-account support
///
/// # Example
///
/// ```no_run
/// use atproto::session_manager::PersistentSessionManager;
/// use std::path::PathBuf;
///
/// let manager = PersistentSessionManager::new(
///     PathBuf::from("./sessions"),
///     "https://bsky.social".to_string()
/// );
/// ```
pub struct PersistentSessionManager {
    /// Current session data
    session: Arc<RwLock<Option<AtpSessionData>>>,

    /// HTTP client for making requests
    client: reqwest::Client,

    /// Directory to store session files
    session_dir: PathBuf,

    /// PDS service URL for token refresh
    service_url: String,

    /// Session event callback
    callback: Option<SessionCallback>,

    /// Whether automatic token refresh is enabled
    auto_refresh: bool,
}

impl PersistentSessionManager {
    /// Creates a new persistent session manager
    ///
    /// # Arguments
    ///
    /// * `session_dir` - Directory to store session files
    /// * `service_url` - PDS service URL (e.g., "https://bsky.social")
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use atproto::session_manager::PersistentSessionManager;
    /// use std::path::PathBuf;
    ///
    /// let manager = PersistentSessionManager::new(
    ///     PathBuf::from("./sessions"),
    ///     "https://bsky.social".to_string()
    /// );
    /// ```
    pub fn new(session_dir: PathBuf, service_url: String) -> Self {
        Self {
            session: Arc::new(RwLock::new(None)),
            client: reqwest::Client::new(),
            session_dir,
            service_url,
            callback: None,
            auto_refresh: true,
        }
    }

    /// Sets a callback function to be called on session events
    ///
    /// # Arguments
    ///
    /// * `callback` - Function to call on session events
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use atproto::session_manager::PersistentSessionManager;
    /// use atproto::types::AtpSessionEvent;
    /// use std::path::PathBuf;
    /// use std::sync::Arc;
    ///
    /// let mut manager = PersistentSessionManager::new(
    ///     PathBuf::from("./sessions"),
    ///     "https://bsky.social".to_string()
    /// );
    ///
    /// manager.on_session_event(Arc::new(|event, session| {
    ///     println!("Session event: {:?}", event);
    /// }));
    /// ```
    pub fn on_session_event(&mut self, callback: SessionCallback) {
        self.callback = Some(callback);
    }

    /// Disables automatic token refresh
    ///
    /// By default, tokens are automatically refreshed before expiration.
    /// Call this to disable automatic refresh.
    pub fn disable_auto_refresh(&mut self) {
        self.auto_refresh = false;
    }

    /// Enables automatic token refresh (default)
    pub fn enable_auto_refresh(&mut self) {
        self.auto_refresh = true;
    }

    /// Stores a session
    ///
    /// This updates the in-memory session and persists it to disk.
    ///
    /// # Arguments
    ///
    /// * `session_data` - The session data to store
    pub async fn store_session(&self, session_data: AtpSessionData) -> Result<()> {
        // Update in-memory session
        {
            let mut session = self.session.write().unwrap();
            *session = Some(session_data.clone());
        }

        // Persist to disk
        self.save_session_to_disk(&session_data).await?;

        // Call callback
        if let Some(ref callback) = self.callback {
            callback(AtpSessionEvent::Create, Some(&session_data));
        }

        Ok(())
    }

    /// Loads a session from disk
    ///
    /// # Arguments
    ///
    /// * `did` - The DID of the account to load
    ///
    /// # Returns
    ///
    /// The session data if found, or None
    pub async fn load_session(&self, did: &str) -> Result<Option<AtpSessionData>> {
        let session_path = self.session_file_path(did);

        if !session_path.exists() {
            return Ok(None);
        }

        let contents = tokio::fs::read_to_string(&session_path)
            .await
            .map_err(|e| SessionError::Session(format!("Failed to read session file: {}", e)))?;

        let session_data: AtpSessionData = serde_json::from_str(&contents)
            .map_err(|e| SessionError::Session(format!("Failed to parse session data: {}", e)))?;

        // Update in-memory session
        {
            let mut session = self.session.write().unwrap();
            *session = Some(session_data.clone());
        }

        // Call callback
        if let Some(ref callback) = self.callback {
            callback(AtpSessionEvent::Create, Some(&session_data));
        }

        Ok(Some(session_data))
    }

    /// Clears the current session
    pub async fn clear_session(&self) -> Result<()> {
        let session_data = {
            let mut session = self.session.write().unwrap();
            session.take()
        };

        if let Some(data) = session_data {
            // Delete session file
            let session_path = self.session_file_path(&data.did);
            if session_path.exists() {
                tokio::fs::remove_file(&session_path)
                    .await
                    .map_err(|e| SessionError::Session(format!("Failed to delete session file: {}", e)))?;
            }

            // Call callback
            if let Some(ref callback) = self.callback {
                callback(AtpSessionEvent::Delete, Some(&data));
            }
        }

        Ok(())
    }

    /// Refreshes the access token using the refresh token
    pub async fn refresh_token(&self) -> Result<()> {
        let refresh_jwt = {
            let session = self.session.read().unwrap();
            session.as_ref()
                .map(|s| s.refresh_jwt.clone())
                .ok_or(SessionError::NoSession)?
        };

        // Call refresh endpoint
        let url = format!("{}/xrpc/com.atproto.server.refreshSession", self.service_url);
        let response = self.client
            .post(&url)
            .header("Authorization", format!("Bearer {}", refresh_jwt))
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(SessionError::Session(format!(
                "Failed to refresh token: HTTP {}",
                response.status()
            )));
        }

        let refresh_response: serde_json::Value = response.json().await?;

        // Update session with new tokens
        let mut session_data = {
            let session = self.session.read().unwrap();
            session.as_ref()
                .ok_or(SessionError::NoSession)?
                .clone()
        };

        if let Some(access_jwt) = refresh_response.get("accessJwt").and_then(|v| v.as_str()) {
            session_data.access_jwt = access_jwt.to_string();
        }

        if let Some(refresh_jwt) = refresh_response.get("refreshJwt").and_then(|v| v.as_str()) {
            session_data.refresh_jwt = refresh_jwt.to_string();
        }

        // Update in-memory and persist
        {
            let mut session = self.session.write().unwrap();
            *session = Some(session_data.clone());
        }

        self.save_session_to_disk(&session_data).await?;

        // Call callback
        if let Some(ref callback) = self.callback {
            callback(AtpSessionEvent::Update, Some(&session_data));
        }

        Ok(())
    }

    /// Checks if the access token needs refresh (expires within 2 minutes)
    fn needs_refresh(&self) -> bool {
        let session = self.session.read().unwrap();
        if let Some(ref session_data) = *session {
            // Parse JWT to check expiration
            if let Some(exp) = self.extract_jwt_expiration(&session_data.access_jwt) {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs();

                // Refresh if expiring within 2 minutes (120 seconds)
                return exp < now + 120;
            }
        }
        false
    }

    /// Extracts the expiration time from a JWT
    fn extract_jwt_expiration(&self, jwt: &str) -> Option<u64> {
        // JWT format: header.payload.signature
        let parts: Vec<&str> = jwt.split('.').collect();
        if parts.len() != 3 {
            return None;
        }

        // Decode payload (base64url)
        let payload = parts[1];
        let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
        let json: serde_json::Value = serde_json::from_slice(&decoded).ok()?;

        json.get("exp")?.as_u64()
    }

    /// Returns the path to the session file for a given DID
    fn session_file_path(&self, did: &str) -> PathBuf {
        // Sanitize DID for filename (replace : and / with _)
        let safe_did = did.replace([':', '/'], "_");
        self.session_dir.join(format!("{}.json", safe_did))
    }

    /// Saves session data to disk
    async fn save_session_to_disk(&self, session_data: &AtpSessionData) -> Result<()> {
        // Create session directory if it doesn't exist
        if !self.session_dir.exists() {
            tokio::fs::create_dir_all(&self.session_dir)
                .await
                .map_err(|e| SessionError::Session(format!("Failed to create session directory: {}", e)))?;
        }

        let session_path = self.session_file_path(&session_data.did);
        let json = serde_json::to_string_pretty(session_data)
            .map_err(|e| SessionError::Session(format!("Failed to serialize session: {}", e)))?;

        tokio::fs::write(&session_path, json)
            .await
            .map_err(|e| SessionError::Session(format!("Failed to write session file: {}", e)))?;

        Ok(())
    }

    /// Lists all stored sessions
    ///
    /// # Returns
    ///
    /// A vector of DIDs for all stored sessions
    pub async fn list_sessions(&self) -> Result<Vec<String>> {
        if !self.session_dir.exists() {
            return Ok(Vec::new());
        }

        let mut dids = Vec::new();
        let mut entries = tokio::fs::read_dir(&self.session_dir)
            .await
            .map_err(|e| SessionError::Session(format!("Failed to read session directory: {}", e)))?;

        while let Some(entry) = entries.next_entry().await.map_err(|e| SessionError::Session(format!("Failed to read directory entry: {}", e)))? {
            if let Some(filename) = entry.file_name().to_str() {
                if filename.ends_with(".json") {
                    // Extract DID from filename (remove .json and convert _ back to :)
                    let did = filename
                        .trim_end_matches(".json")
                        .replace('_', ":");
                    dids.push(did);
                }
            }
        }

        Ok(dids)
    }
}

#[async_trait]
impl SessionManager for PersistentSessionManager {
    fn did(&self) -> Option<&Did> {
        // This is a bit tricky - we need to return a reference that lives long enough
        // For now, we'll return None and users should use the session data directly
        // TODO: Consider a different API design
        None
    }

    async fn fetch(&self, mut request: Request) -> Result<Response> {
        // Check if we need to refresh the token
        if self.auto_refresh && self.needs_refresh() {
            self.refresh_token().await?;
        }

        // Add authorization header if we have a session
        {
            let session = self.session.read().unwrap();
            if let Some(ref session_data) = *session {
                request.headers_mut().insert(
                    "Authorization",
                    format!("Bearer {}", session_data.access_jwt)
                        .parse()
                        .map_err(|e| SessionError::Session(format!("Invalid auth header: {}", e)))?
                );
            }
        }

        Ok(self.client.execute(request).await?)
    }

    fn clone_box(&self) -> Arc<dyn SessionManager> {
        Arc::new(Self {
            session: Arc::clone(&self.session),
            client: self.client.clone(),
            session_dir: self.session_dir.clone(),
            service_url: self.service_url.clone(),
            callback: self.callback.clone(),
            auto_refresh: self.auto_refresh,
        })
    }

    async fn store_session(&self, session_data: AtpSessionData) -> Result<()> {
        // Update in-memory session
        {
            let mut session = self.session.write().unwrap();
            *session = Some(session_data.clone());
        }

        // Persist to disk
        self.save_session_to_disk(&session_data).await?;

        // Call callback
        if let Some(ref callback) = self.callback {
            callback(AtpSessionEvent::Create, Some(&session_data));
        }

        Ok(())
    }

    async fn load_session(&self, did: &str) -> Result<Option<AtpSessionData>> {
        let session_path = self.session_file_path(did);

        if !session_path.exists() {
            return Ok(None);
        }

        let contents = tokio::fs::read_to_string(&session_path)
            .await
            .map_err(|e| SessionError::Session(format!("Failed to read session file: {}", e)))?;

        let session_data: AtpSessionData = serde_json::from_str(&contents)
            .map_err(|e| SessionError::Session(format!("Failed to parse session data: {}", e)))?;

        // Update in-memory session
        {
            let mut session = self.session.write().unwrap();
            *session = Some(session_data.clone());
        }

        // Call callback
        if let Some(ref callback) = self.callback {
            callback(AtpSessionEvent::Create, Some(&session_data));
        }

        Ok(Some(session_data))
    }

    async fn clear_session(&self) -> Result<()> {
        let session_data = {
            let mut session = self.session.write().unwrap();
            session.take()
        };

        if let Some(data) = session_data {
            // Delete session file
            let session_path = self.session_file_path(&data.did);
            if session_path.exists() {
                tokio::fs::remove_file(&session_path)
                    .await
                    .map_err(|e| SessionError::Session(format!("Failed to delete session file: {}", e)))?;
            }

            // Call callback
            if let Some(ref callback) = self.callback {
                callback(AtpSessionEvent::Delete, Some(&data));
            }
        }

        Ok(())
    }

    async fn refresh_token(&self) -> Result<()> {
        let refresh_jwt = {
            let session = self.session.read().unwrap();
            session.as_ref()
                .map(|s| s.refresh_jwt.clone())
                .ok_or(SessionError::NoSession)?
        };

        // Call refresh endpoint
        let url = format!("{}/xrpc/com.atproto.server.refreshSession", self.service_url);
        let response = self.client
            .post(&url)
            .header("Authorization", format!("Bearer {}", refresh_jwt))
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(SessionError::Session(format!(
                "Failed to refresh token: HTTP {}",
                response.status()
            )));
        }

        let refresh_response: serde_json::Value = response.json().await?;

        // Update session with new tokens
        let mut session_data = {
            let session = self.session.read().unwrap();
            session.as_ref()
                .ok_or(SessionError::NoSession)?
                .clone()
        };

        if let Some(access_jwt) = refresh_response.get("accessJwt").and_then(|v| v.as_str()) {
            session_data.access_jwt = access_jwt.to_string();
        }

        if let Some(refresh_jwt) = refresh_response.get("refreshJwt").and_then(|v| v.as_str()) {
            session_data.refresh_jwt = refresh_jwt.to_string();
        }

        // Update in-memory and persist
        {
            let mut session = self.session.write().unwrap();
            *session = Some(session_data.clone());
        }

        self.save_session_to_disk(&session_data).await?;

        // Call callback
        if let Some(ref callback) = self.callback {
            callback(AtpSessionEvent::Update, Some(&session_data));
        }

        Ok(())
    }

    fn get_session(&self) -> Option<AtpSessionData> {
        let session = self.session.read().unwrap();
        session.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unauthenticated_session_manager_new() {
        let manager = UnauthenticatedSessionManager::new();
        assert!(manager.did().is_none());
    }

    #[test]
    fn test_unauthenticated_session_manager_default() {
        let manager = UnauthenticatedSessionManager::default();
        assert!(manager.did().is_none());
    }

    #[test]
    fn test_unauthenticated_session_manager_with_client() {
        let client = reqwest::Client::new();
        let manager = UnauthenticatedSessionManager::with_client(client);
        assert!(manager.did().is_none());
    }

    #[tokio::test]
    async fn test_unauthenticated_session_manager_clone_box() {
        let manager = UnauthenticatedSessionManager::new();
        let cloned = manager.clone_box();
        assert!(cloned.did().is_none());
    }

    #[test]
    fn test_session_error_display() {
        let err = SessionError::NoSession;
        assert_eq!(err.to_string(), "No active session");

        let err = SessionError::Session("test error".to_string());
        assert_eq!(err.to_string(), "Session error: test error");

        let err = SessionError::InvalidDid("bad-did".to_string());
        assert_eq!(err.to_string(), "Invalid DID: bad-did");
    }

    #[test]
    fn test_session_error_types() {
        // Test that SessionError implements required traits
        let err = SessionError::NoSession;
        let _debug = format!("{:?}", err);
        let _display = format!("{}", err);

        // Test conversion chain
        let err = SessionError::Session("test".to_string());
        assert!(matches!(err, SessionError::Session(_)));
    }
}
