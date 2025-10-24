//! Agent - Core ATProto client
//!
//! The Agent is the main entry point for interacting with ATProto services.
//! It provides:
//! - XRPC request handling
//! - Session management and authentication
//! - Labeler configuration
//! - Proxy configuration
//! - DID resolution
//!
//! ## Example
//!
//! ```no_run
//! use atproto::agent::Agent;
//!
//! #[tokio::main]
//! async fn main() {
//!     let agent = Agent::new("https://bsky.social".to_string());
//!
//!     // Configure labelers
//!     agent.configure_labelers(vec!["did:plc:labeler123".to_string()]);
//!
//!     // Make XRPC requests
//!     // (Full client API methods will be added via code generation)
//! }
//! ```

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio::sync::Mutex as AsyncMutex;

use crate::consts::BSKY_LABELER_DID;
use crate::namespaces::{AppNS, ChatNS, ComNS, ToolsNS};
use crate::session_manager::SessionManager;
use crate::types::{AtpSessionData, AtprotoProxy, AtprotoServiceType, Did};
use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse};

/// Error types for Agent operations
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("Not authenticated")]
    NotAuthenticated,

    #[error("Invalid DID: {0}")]
    InvalidDid(String),

    #[error("XRPC error: {0}")]
    XrpcError(#[from] crate::xrpc::XrpcError),

    #[error("Session error: {0}")]
    SessionError(String),
}

/// Parameters for replying to a post with a link embed
pub struct ReplyWithLinkEmbedParams<'a> {
    /// The text content of the reply
    pub text: &'a str,
    /// URL of the link to embed
    pub url: &'a str,
    /// Title of the link
    pub title: &'a str,
    /// Description of the link
    pub description: &'a str,
    /// Optional thumbnail blob
    pub thumb_blob: Option<serde_json::Value>,
    /// AT-URI of the post being replied to
    pub parent_uri: &'a str,
    /// CID of the post being replied to
    pub parent_cid: &'a str,
    /// AT-URI of the root post in the thread
    pub root_uri: &'a str,
    /// CID of the root post in the thread
    pub root_cid: &'a str,
}

/// Core ATProto agent
///
/// The Agent wraps an XRPC client and provides session management,
/// labeler configuration, and proxy support.
pub struct Agent {
    /// XRPC client for making requests
    client: Arc<crate::xrpc::XrpcClientImpl>,

    /// Session manager for authentication
    session_manager: Arc<RwLock<Box<dyn SessionManager>>>,

    /// Current session data (stored separately for easy access)
    session_data: Arc<RwLock<Option<AtpSessionData>>>,

    /// PDS service URL
    service: String,

    /// Configured labeler DIDs
    labelers: Arc<RwLock<Vec<String>>>,

    /// App-wide labeler DIDs (static configuration)
    app_labelers: Arc<RwLock<Vec<String>>>,

    /// Proxy configuration
    proxy: Arc<RwLock<Option<AtprotoProxy>>>,

    /// Custom headers
    headers: Arc<RwLock<HashMap<String, String>>>,

    /// Mutex for atomic preference updates
    prefs_lock: Arc<AsyncMutex<()>>,
}

impl Agent {
    /// Create a new Agent with a PDS service URL
    ///
    /// # Arguments
    ///
    /// * `service` - The PDS service URL (e.g., "https://bsky.social")
    ///
    /// # Examples
    ///
    /// ```
    /// use atproto::agent::Agent;
    ///
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// ```
    pub fn new(service: String) -> Self {
        let client = Arc::new(crate::xrpc::XrpcClientImpl::new(service.clone()));
        let session_manager = Arc::new(RwLock::new(Box::new(
            crate::session_manager::UnauthenticatedSessionManager::new(),
        ) as Box<dyn SessionManager>));

        // Default app labelers includes the Bluesky labeler
        let app_labelers = Arc::new(RwLock::new(vec![BSKY_LABELER_DID.to_string()]));

        Self {
            client,
            session_manager,
            session_data: Arc::new(RwLock::new(None)),
            service,
            labelers: Arc::new(RwLock::new(Vec::new())),
            app_labelers,
            proxy: Arc::new(RwLock::new(None)),
            headers: Arc::new(RwLock::new(HashMap::new())),
            prefs_lock: Arc::new(AsyncMutex::new(())),
        }
    }

    /// Get the PDS service URL
    pub fn service(&self) -> &str {
        &self.service
    }

    /// Get the authenticated user's DID, if any
    pub fn did(&self) -> Option<String> {
        self.session_data
            .read()
            .unwrap()
            .as_ref()
            .map(|s| s.did.clone())
    }

    /// Assert that the user is authenticated and return their DID
    ///
    /// # Errors
    ///
    /// Returns an error if the user is not authenticated.
    pub fn assert_did(&self) -> Result<String, AgentError> {
        self.did().ok_or(AgentError::NotAuthenticated)
    }

    /// Check if the user is authenticated
    pub fn is_authenticated(&self) -> bool {
        self.did().is_some()
    }

    /// Configure instance-specific labelers
    ///
    /// These labelers are used for this specific Agent instance.
    ///
    /// # Arguments
    ///
    /// * `labeler_dids` - Vector of labeler DIDs
    pub fn configure_labelers(&self, labeler_dids: Vec<String>) {
        // Validate DIDs
        let validated: Vec<String> = labeler_dids
            .into_iter()
            .filter(|did| Did::new(did).is_ok())
            .collect();

        *self.labelers.write().unwrap() = validated;
    }

    /// Configure app-wide labelers
    ///
    /// These labelers apply to all Agent instances (static configuration).
    ///
    /// # Arguments
    ///
    /// * `labeler_dids` - Vector of labeler DIDs
    pub fn configure_app_labelers(&self, labeler_dids: Vec<String>) {
        let validated: Vec<String> = labeler_dids
            .into_iter()
            .filter(|did| Did::new(did).is_ok())
            .collect();

        *self.app_labelers.write().unwrap() = validated;
    }

    /// Get configured labelers (instance + app)
    pub fn get_all_labelers(&self) -> Vec<String> {
        let instance = self.labelers.read().unwrap().clone();
        let app = self.app_labelers.read().unwrap().clone();

        let mut all = Vec::new();
        all.extend(app);
        all.extend(instance);
        all
    }

    /// Configure proxy
    ///
    /// # Arguments
    ///
    /// * `proxy` - Optional proxy configuration (did#service_type)
    pub fn configure_proxy(&self, proxy: Option<AtprotoProxy>) {
        *self.proxy.write().unwrap() = proxy;
    }

    /// Configure proxy from DID and service type
    ///
    /// # Arguments
    ///
    /// * `service_type` - Service type (e.g., "atproto_labeler")
    /// * `did` - DID of the service
    pub fn configure_proxy_from_parts(&self, service_type: AtprotoServiceType, did: String) {
        if let Ok(did_obj) = Did::new(&did) {
            let proxy = AtprotoProxy::new(did_obj, service_type);
            self.configure_proxy(Some(proxy));
        }
    }

    /// Get configured proxy
    pub fn get_proxy(&self) -> Option<AtprotoProxy> {
        self.proxy.read().unwrap().clone()
    }

    /// Set a custom header
    ///
    /// # Arguments
    ///
    /// * `key` - Header name
    /// * `value` - Header value
    pub fn set_header(&self, key: String, value: String) {
        self.headers.write().unwrap().insert(key, value);
    }

    /// Clear a custom header
    ///
    /// # Arguments
    ///
    /// * `key` - Header name to remove
    pub fn clear_header(&self, key: &str) {
        self.headers.write().unwrap().remove(key);
    }

    /// Clear all custom headers
    pub fn clear_all_headers(&self) {
        self.headers.write().unwrap().clear();
    }

    /// Get all custom headers
    pub fn get_headers(&self) -> HashMap<String, String> {
        self.headers.read().unwrap().clone()
    }

    /// Clone this Agent into a new instance
    ///
    /// The new instance will have the same configuration but independent state.
    pub fn clone_agent(&self) -> Self {
        let new_agent = Self::new(self.service.clone());

        // Copy configuration
        new_agent.configure_labelers(self.labelers.read().unwrap().clone());
        new_agent.configure_app_labelers(self.app_labelers.read().unwrap().clone());
        new_agent.configure_proxy(self.proxy.read().unwrap().clone());

        // Copy headers
        let headers = self.headers.read().unwrap();
        for (key, value) in headers.iter() {
            new_agent.set_header(key.clone(), value.clone());
        }

        new_agent
    }

    /// Create a new Agent instance with a proxy configured
    ///
    /// # Arguments
    ///
    /// * `service_type` - Service type for the proxy
    /// * `did` - DID of the proxy service
    pub fn with_proxy(&self, service_type: AtprotoServiceType, did: String) -> Self {
        let new_agent = self.clone_agent();
        new_agent.configure_proxy_from_parts(service_type, did);
        new_agent
    }

    // ============================================================================
    // Namespace Accessors
    // ============================================================================

    /// Access com.* APIs
    ///
    /// Provides access to all com.atproto.* endpoints.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    ///
    /// // Access server endpoints
    /// // let session = agent.com().atproto().server().create_session(...).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn com(&self) -> ComNS {
        ComNS {
            client: self.client.clone(),
        }
    }

    /// Access app.* APIs
    ///
    /// Provides access to all app.bsky.* endpoints.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    ///
    /// // Access feed endpoints
    /// // let timeline = agent.app().bsky().feed().get_timeline(...).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn app(&self) -> AppNS {
        AppNS {
            client: self.client.clone(),
        }
    }

    /// Access chat.* APIs
    ///
    /// Provides access to all chat.bsky.* endpoints.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    ///
    /// // Access chat endpoints
    /// // let conversations = agent.chat().bsky().convo().list_convos(...).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn chat(&self) -> ChatNS {
        ChatNS {
            client: self.client.clone(),
        }
    }

    /// Access tools.* APIs
    ///
    /// Provides access to all tools.ozone.* endpoints (moderation tools).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    ///
    /// // Access moderation tools
    /// // let events = agent.tools().ozone().moderation().query_events(...).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn tools(&self) -> ToolsNS {
        ToolsNS {
            client: self.client.clone(),
        }
    }

    /// Get the internal XRPC client (for advanced use cases)
    ///
    /// This provides direct access to the XRPC client for making custom requests.
    pub fn xrpc(&self) -> Arc<crate::xrpc::XrpcClientImpl> {
        self.client.clone()
    }

    /// Build headers for an XRPC request
    ///
    /// Includes labelers, proxy, and custom headers.
    fn build_request_headers(&self) -> HashMap<String, String> {
        let mut headers = self.headers.read().unwrap().clone();

        // Add proxy header if configured
        if let Some(proxy) = self.get_proxy() {
            headers.insert("atproto-proxy".to_string(), proxy.to_string());
        }

        // Add labelers header
        let labelers = self.get_all_labelers();
        if !labelers.is_empty() {
            let labelers_str = labelers
                .iter()
                .map(|did| format!("{};redact", did))
                .collect::<Vec<_>>()
                .join(", ");

            headers.insert("atproto-accept-labelers".to_string(), labelers_str);
        }

        headers
    }

    /// Make an XRPC request
    ///
    /// This is a low-level method. Most users should use the generated
    /// client API methods instead.
    ///
    /// # Arguments
    ///
    /// * `request` - The XRPC request to make
    pub async fn request<T: serde::de::DeserializeOwned>(
        &self,
        mut request: XrpcRequest,
    ) -> Result<XrpcResponse<T>, AgentError> {
        // Add headers
        let headers = self.build_request_headers();
        for (key, value) in headers {
            if let Ok(header_value) = reqwest::header::HeaderValue::from_str(&value) {
                request.headers.insert(
                    reqwest::header::HeaderName::from_bytes(key.as_bytes()).unwrap(),
                    header_value,
                );
            }
        }

        // Make request through XRPC client
        self.client
            .request(request)
            .await
            .map_err(AgentError::from)
    }

    // ============================================================================
    // Authentication Methods
    // ============================================================================

    /// Login with identifier and password
    ///
    /// Creates a new session and stores the authentication tokens.
    ///
    /// # Arguments
    ///
    /// * `identifier` - Handle (e.g., "alice.bsky.social") or DID
    /// * `password` - Account password or app password
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// agent.login("alice.bsky.social", "my-app-password").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn login(&self, identifier: &str, password: &str) -> Result<(), AgentError> {
        use crate::client::com::atproto::server::create_session;

        let input = create_session::Input {
            identifier: identifier.to_string(),
            password: password.to_string(),
            auth_factor_token: None,
            allow_takendown: None,
        };

        let response = create_session::create_session(&*self.client, input).await?;

        // Create session data
        let session_data = AtpSessionData {
            did: response.data.did.to_string(),
            handle: response.data.handle,
            email: response.data.email,
            email_confirmed: response.data.email_confirmed,
            email_auth_factor: response.data.email_auth_factor,
            access_jwt: response.data.access_jwt.clone(),
            refresh_jwt: response.data.refresh_jwt.clone(),
            active: response.data.active.unwrap_or(true),
            status: response.data.status,
        };

        // Store session via session_manager (optional - may not support persistence)
        let session_manager = {
            let sm = self.session_manager.read().unwrap();
            sm.clone_box()
        };
        // Ignore errors if session manager doesn't support storage (e.g., UnauthenticatedSessionManager)
        let _ = session_manager.store_session(session_data.clone()).await;

        // Update local cache (always works)
        {
            let mut session = self.session_data.write().unwrap();
            *session = Some(session_data.clone());
        }

        // Set authorization header on the XRPC client
        let auth_header = format!("Bearer {}", response.data.access_jwt);
        self.client
            .set_header("Authorization", &auth_header);

        Ok(())
    }

    /// Resume a session with existing tokens
    ///
    /// # Arguments
    ///
    /// * `access_token` - Access JWT token
    /// * `refresh_token` - Refresh JWT token
    /// * `did` - User's DID
    /// * `handle` - User's handle
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// agent.resume_session(
    ///     "access_jwt_token".to_string(),
    ///     "refresh_jwt_token".to_string(),
    ///     "did:plc:abc123".to_string(),
    ///     "alice.bsky.social".to_string(),
    /// ).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn resume_session(
        &self,
        access_token: String,
        refresh_token: String,
        did: String,
        handle: String,
    ) -> Result<(), AgentError> {
        // Validate DID
        Did::new(&did).map_err(|_| AgentError::InvalidDid(did.clone()))?;

        let session_data = AtpSessionData {
            did: did.clone(),
            handle,
            email: None,
            email_confirmed: None,
            email_auth_factor: None,
            access_jwt: access_token.clone(),
            refresh_jwt: refresh_token,
            active: true,
            status: None,
        };

        // Store session via session_manager
        let session_manager = {
            let sm = self.session_manager.read().unwrap();
            sm.clone_box()
        };
        session_manager.store_session(session_data.clone()).await
            .map_err(|e| AgentError::SessionError(format!("Failed to store session: {}", e)))?;

        // Also update local cache for backward compatibility
        {
            let mut session = self.session_data.write().unwrap();
            *session = Some(session_data);
        }

        // Set authorization header on the XRPC client
        let auth_header = format!("Bearer {}", access_token);
        self.client
            .set_header("Authorization", &auth_header);

        Ok(())
    }

    /// Logout and clear the session
    ///
    /// Calls the server's deleteSession endpoint and clears local session data.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// agent.login("alice.bsky.social", "password").await?;
    /// agent.logout().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn logout(&self) -> Result<(), AgentError> {
        use crate::client::com::atproto::server::delete_session;

        if self.is_authenticated() {
            // Call deleteSession endpoint (best effort - errors are ignored)
            let _ = delete_session::delete_session(&*self.client).await;
        }

        // Clear session via session_manager
        let session_manager = {
            let sm = self.session_manager.read().unwrap();
            sm.clone_box()
        };
        session_manager.clear_session().await
            .map_err(|e| AgentError::SessionError(format!("Failed to clear session: {}", e)))?;

        // Also clear local cache for backward compatibility
        {
            let mut session = self.session_data.write().unwrap();
            *session = None;
        }

        // Clear authorization header from XRPC client
        self.client.remove_header("Authorization");

        Ok(())
    }

    /// Refresh the authentication session
    ///
    /// Uses the refresh token to get new access/refresh tokens.
    /// This is called automatically by the SessionManager when tokens are about to expire.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// agent.login("alice.bsky.social", "password").await?;
    ///
    /// // Manually refresh if needed
    /// agent.refresh_session().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn refresh_session(&self) -> Result<(), AgentError> {
        // Delegate to session_manager
        let session_manager = {
            let sm = self.session_manager.read().unwrap();
            sm.clone_box()
        };
        session_manager.refresh_token().await
            .map_err(|e| AgentError::SessionError(format!("Failed to refresh token: {}", e)))?;

        // Update local cache and client headers from refreshed session
        if let Some(session_data) = session_manager.get_session() {
            // Update local cache for backward compatibility
            {
                let mut session = self.session_data.write().unwrap();
                *session = Some(session_data.clone());
            }

            // Update authorization header with new access token
            let auth_header = format!("Bearer {}", session_data.access_jwt);
            self.client
                .set_header("Authorization", &auth_header);
        }

        Ok(())
    }

    // ============================================================================
    // Handle Resolution
    // ============================================================================

    /// Resolve a handle to a DID
    ///
    /// # Arguments
    ///
    /// * `handle` - The handle to resolve (e.g., "alice.bsky.social" or "@alice.bsky.social")
    ///
    /// # Returns
    ///
    /// Returns the DID if found, or an error if resolution fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    ///
    /// let did = agent.resolve_handle("alice.bsky.social").await?;
    /// println!("DID: {}", did);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn resolve_handle(&self, handle: &str) -> Result<String, AgentError> {
        use crate::client::com::atproto::identity::resolve_handle;

        // Strip @ prefix if present
        let clean_handle = handle.trim_start_matches('@');

        let params = resolve_handle::QueryParams {
            handle: clean_handle.to_string(),
        };

        let response = resolve_handle::resolve_handle(&*self.client, params).await?;
        Ok(response.data.did.to_string())
    }

    /// Detect and resolve facets in text
    ///
    /// This method detects mentions, links, and hashtags in the text and resolves
    /// mentions to DIDs.
    ///
    /// # Arguments
    ///
    /// * `text` - The text to process
    ///
    /// # Returns
    ///
    /// Returns a tuple of (text, facets_json) where facets_json is None if no facets found.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    ///
    /// let (text, facets) = agent.detect_facets("Hello @alice.bsky.social! Check https://example.com #cool").await?;
    /// # Ok(())
    /// # }
    /// ```
    async fn detect_facets(&self, text: &str) -> Result<(String, Option<serde_json::Value>), AgentError> {
        use crate::rich_text::{UnicodeString, detect_facets, FacetFeature};

        let unicode_text = UnicodeString::new(text);
        let detected_facets = detect_facets(&unicode_text);

        if let Some(mut facets) = detected_facets {
            // Resolve mentions to DIDs
            for facet in &mut facets {
                for feature in &mut facet.features {
                    if let FacetFeature::Mention { did } = feature {
                        // The detection puts the handle in the did field, we need to resolve it
                        let handle = did.clone();
                        match self.resolve_handle(&handle).await {
                            Ok(resolved_did) => {
                                *did = resolved_did;
                            }
                            Err(e) => {
                                // Log warning but don't fail the whole post
                                eprintln!("Warning: Failed to resolve handle {}: {}", handle, e);
                                // Keep the handle as-is if resolution fails
                            }
                        }
                    }
                }
            }

            // Convert to JSON
            let facets_json = serde_json::to_value(&facets)
                .map_err(|e| AgentError::SessionError(format!("Failed to serialize facets: {}", e)))?;

            Ok((text.to_string(), Some(facets_json)))
        } else {
            Ok((text.to_string(), None))
        }
    }

    // ============================================================================
    // Blob Upload
    // ============================================================================

    /// Upload a blob (image, video, or other binary data)
    ///
    /// # Arguments
    ///
    /// * `data` - The binary data to upload
    /// * `content_type` - The MIME type (e.g., "image/jpeg", "image/png", "video/mp4")
    ///
    /// # Returns
    ///
    /// Returns a blob reference that can be used in posts and other records.
    /// The blob will be deleted if not referenced within a few minutes.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// agent.login("alice.bsky.social", "password").await?;
    ///
    /// // Upload an image
    /// let image_data = std::fs::read("photo.jpg")?;
    /// let blob = agent.upload_blob(image_data, "image/jpeg").await?;
    ///
    /// // Use blob in a post (requires embed support - coming soon!)
    /// # Ok(())
    /// # }
    /// ```
    pub async fn upload_blob(&self, data: Vec<u8>, _content_type: &str) -> Result<serde_json::Value, AgentError> {
        use crate::client::com::atproto::repo::upload_blob;

        // Note: content_type header support needs to be added to XRPC client
        // For now, the server will attempt to detect content type from the data
        let response = upload_blob::upload_blob(&*self.client, data).await?;
        Ok(response.data.blob)
    }

    // ============================================================================
    // Convenience Methods for Common Operations
    // ============================================================================

    /// Create a text post with automatic facet detection
    ///
    /// Automatically detects and creates clickable links for:
    /// - Mentions (@alice.bsky.social)
    /// - URLs (https://example.com)
    /// - Hashtags (#rustlang)
    ///
    /// # Arguments
    ///
    /// * `text` - The post content
    ///
    /// # Returns
    ///
    /// Returns the AT-URI of the created post
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// agent.login("alice.bsky.social", "password").await?;
    ///
    /// // Post with mentions, links, and hashtags
    /// let uri = agent.post("Hey @alice.bsky.social check out https://example.com #rustlang").await?;
    /// println!("Posted: {}", uri);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn post(&self, text: &str) -> Result<String, AgentError> {
        use crate::client::com::atproto::repo::create_record;
        use crate::client::app::bsky::feed::post;

        let did = self.assert_did()?;

        // Detect facets (mentions, links, hashtags)
        let (_text, facets) = self.detect_facets(text).await?;

        // Create the post record
        let now = chrono::Utc::now().to_rfc3339();
        let post_record = post::Post {
            text: text.to_string(),
            created_at: now,
            reply: None,
            embed: None,
            langs: None,
            entities: None,
            tags: None,
            facets,
            labels: None,
        };

        // Serialize to JSON
        let record_json = serde_json::to_value(&post_record)
            .map_err(|e| AgentError::SessionError(format!("Failed to serialize post: {}", e)))?;

        let input = create_record::Input {
            repo: did,
            collection: "app.bsky.feed.post".to_string(),
            rkey: None,
            validate: None,
            record: record_json,
            swap_commit: None,
        };

        let response = create_record::create_record(&*self.client, input).await?;

        Ok(response.data.uri.to_string())
    }

    /// Create a post with images and automatic facet detection
    ///
    /// Automatically detects and creates clickable links for mentions, URLs, and hashtags.
    ///
    /// # Arguments
    ///
    /// * `text` - The post text content
    /// * `images` - Vector of (image_data, alt_text) tuples. Maximum 4 images.
    ///
    /// # Returns
    ///
    /// Returns the AT-URI of the created post
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// agent.login("alice.bsky.social", "password").await?;
    ///
    /// // Upload a post with image and mentions
    /// let image_data = std::fs::read("photo.jpg")?;
    /// let images = vec![(image_data, "A beautiful sunset".to_string())];
    ///
    /// let uri = agent.post_with_images("Check out this photo @alice.bsky.social! #photography", images).await?;
    /// println!("Posted with image: {}", uri);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn post_with_images(&self, text: &str, images: Vec<(Vec<u8>, String)>) -> Result<String, AgentError> {
        use crate::client::com::atproto::repo::create_record;
        use crate::client::app::bsky::feed::post;

        if images.is_empty() {
            return Err(AgentError::SessionError("At least one image required".to_string()));
        }

        if images.len() > 4 {
            return Err(AgentError::SessionError("Maximum 4 images allowed per post".to_string()));
        }

        let did = self.assert_did()?;

        // Detect facets (mentions, links, hashtags)
        let (_text, facets) = self.detect_facets(text).await?;

        // Upload all images and collect blob references
        let mut image_blobs = Vec::new();
        for (image_data, alt_text) in images {
            // Detect MIME type from data
            let mime_type = crate::blob::detect_mime_type_from_data(&image_data)
                .unwrap_or("image/jpeg");

            // Upload the blob
            let blob_ref = self.upload_blob(image_data, mime_type).await?;

            // Create image object
            let image_obj = serde_json::json!({
                "alt": alt_text,
                "image": blob_ref,
            });

            image_blobs.push(image_obj);
        }

        // Create embed object
        let embed = serde_json::json!({
            "$type": "app.bsky.embed.images",
            "images": image_blobs,
        });

        // Create the post record with embed
        let now = chrono::Utc::now().to_rfc3339();
        let post_record = post::Post {
            text: text.to_string(),
            created_at: now,
            reply: None,
            embed: Some(embed),
            langs: None,
            entities: None,
            tags: None,
            facets,
            labels: None,
        };

        // Serialize to JSON
        let record_json = serde_json::to_value(&post_record)
            .map_err(|e| AgentError::SessionError(format!("Failed to serialize post: {}", e)))?;

        let input = create_record::Input {
            repo: did,
            collection: "app.bsky.feed.post".to_string(),
            rkey: None,
            validate: None,
            record: record_json,
            swap_commit: None,
        };

        let response = create_record::create_record(&*self.client, input).await?;

        Ok(response.data.uri.to_string())
    }

    /// Create a reply to a post with automatic facet detection
    ///
    /// Creates a post that replies to another post. Automatically detects and creates
    /// clickable links for mentions, URLs, and hashtags.
    ///
    /// # Arguments
    ///
    /// * `text` - The reply text content
    /// * `parent_uri` - AT-URI of the post being replied to
    /// * `parent_cid` - CID of the post being replied to
    /// * `root_uri` - AT-URI of the root post in the thread
    /// * `root_cid` - CID of the root post in the thread
    ///
    /// # Returns
    ///
    /// Returns the AT-URI of the created reply
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// agent.login("alice.bsky.social", "password").await?;
    ///
    /// // Reply to a post
    /// let uri = agent.post_reply(
    ///     "Great point! I agree completely.",
    ///     "at://did:plc:abc/app.bsky.feed.post/xyz",
    ///     "bafyreiabc...",
    ///     "at://did:plc:abc/app.bsky.feed.post/xyz",  // Same as parent for direct reply
    ///     "bafyreiabc...",
    /// ).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn post_reply(
        &self,
        text: &str,
        parent_uri: &str,
        parent_cid: &str,
        root_uri: &str,
        root_cid: &str,
    ) -> Result<String, AgentError> {
        use crate::client::com::atproto::repo::create_record;
        use crate::client::app::bsky::feed::post::{self, ReplyRef};
        use crate::client::com::atproto::repo::strong_ref::Main as StrongRef;

        let did = self.assert_did()?;

        // Detect facets (mentions, links, hashtags)
        let (_text, facets) = self.detect_facets(text).await?;

        // Create reply reference
        let parent_ref = StrongRef {
            uri: crate::syntax::AtUri::try_from(parent_uri).map_err(|e| AgentError::SessionError(format!("Invalid parent URI: {}", e)))?,
            cid: parent_cid.to_string(),
        };
        let root_ref = StrongRef {
            uri: crate::syntax::AtUri::try_from(root_uri).map_err(|e| AgentError::SessionError(format!("Invalid root URI: {}", e)))?,
            cid: root_cid.to_string(),
        };

        let reply_ref = ReplyRef {
            parent: serde_json::to_value(&parent_ref).map_err(|e| {
                AgentError::SessionError(format!("Failed to serialize parent ref: {}", e))
            })?,
            root: serde_json::to_value(&root_ref).map_err(|e| {
                AgentError::SessionError(format!("Failed to serialize root ref: {}", e))
            })?,
        };

        // Create the post record with reply
        let now = chrono::Utc::now().to_rfc3339();
        let post_record = post::Post {
            text: text.to_string(),
            created_at: now,
            reply: Some(serde_json::to_value(&reply_ref).map_err(|e| {
                AgentError::SessionError(format!("Failed to serialize reply ref: {}", e))
            })?),
            embed: None,
            langs: None,
            entities: None,
            tags: None,
            facets,
            labels: None,
        };

        // Serialize to JSON
        let record_json = serde_json::to_value(&post_record)
            .map_err(|e| AgentError::SessionError(format!("Failed to serialize post: {}", e)))?;

        let input = create_record::Input {
            repo: did,
            collection: "app.bsky.feed.post".to_string(),
            rkey: None,
            validate: None,
            record: record_json,
            swap_commit: None,
        };

        let response = create_record::create_record(&*self.client, input).await?;

        Ok(response.data.uri.to_string())
    }

    /// Create a reply with images to a post
    ///
    /// Creates a post with images that replies to another post. Automatically detects
    /// and creates clickable links for mentions, URLs, and hashtags.
    ///
    /// # Arguments
    ///
    /// * `text` - The reply text content
    /// * `images` - Vector of (image_data, alt_text) tuples. Maximum 4 images.
    /// * `parent_uri` - AT-URI of the post being replied to
    /// * `parent_cid` - CID of the post being replied to
    /// * `root_uri` - AT-URI of the root post in the thread
    /// * `root_cid` - CID of the root post in the thread
    ///
    /// # Returns
    ///
    /// Returns the AT-URI of the created reply
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// agent.login("alice.bsky.social", "password").await?;
    ///
    /// let image_data = std::fs::read("reaction.jpg")?;
    /// let images = vec![(image_data, "My reaction".to_string())];
    ///
    /// let uri = agent.post_reply_with_images(
    ///     "This is my reaction!",
    ///     images,
    ///     "at://did:plc:abc/app.bsky.feed.post/xyz",
    ///     "bafyreiabc...",
    ///     "at://did:plc:abc/app.bsky.feed.post/xyz",
    ///     "bafyreiabc...",
    /// ).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn post_reply_with_images(
        &self,
        text: &str,
        images: Vec<(Vec<u8>, String)>,
        parent_uri: &str,
        parent_cid: &str,
        root_uri: &str,
        root_cid: &str,
    ) -> Result<String, AgentError> {
        use crate::client::com::atproto::repo::create_record;
        use crate::client::app::bsky::feed::post::{self, ReplyRef};
        use crate::client::com::atproto::repo::strong_ref::Main as StrongRef;

        if images.is_empty() {
            return Err(AgentError::SessionError(
                "At least one image required".to_string(),
            ));
        }

        if images.len() > 4 {
            return Err(AgentError::SessionError(
                "Maximum 4 images allowed per post".to_string(),
            ));
        }

        let did = self.assert_did()?;

        // Detect facets (mentions, links, hashtags)
        let (_text, facets) = self.detect_facets(text).await?;

        // Upload all images and collect blob references
        let mut image_blobs = Vec::new();
        for (image_data, alt_text) in images {
            // Detect MIME type from data
            let mime_type =
                crate::blob::detect_mime_type_from_data(&image_data).unwrap_or("image/jpeg");

            // Upload the blob
            let blob_ref = self.upload_blob(image_data, mime_type).await?;

            // Create image object
            let image_obj = serde_json::json!({
                "alt": alt_text,
                "image": blob_ref,
            });

            image_blobs.push(image_obj);
        }

        // Create embed object
        let embed = serde_json::json!({
            "$type": "app.bsky.embed.images",
            "images": image_blobs,
        });

        // Create reply reference
        let parent_ref = StrongRef {
            uri: crate::syntax::AtUri::try_from(parent_uri).map_err(|e| AgentError::SessionError(format!("Invalid parent URI: {}", e)))?,
            cid: parent_cid.to_string(),
        };
        let root_ref = StrongRef {
            uri: crate::syntax::AtUri::try_from(root_uri).map_err(|e| AgentError::SessionError(format!("Invalid root URI: {}", e)))?,
            cid: root_cid.to_string(),
        };

        let reply_ref = ReplyRef {
            parent: serde_json::to_value(&parent_ref).map_err(|e| {
                AgentError::SessionError(format!("Failed to serialize parent ref: {}", e))
            })?,
            root: serde_json::to_value(&root_ref).map_err(|e| {
                AgentError::SessionError(format!("Failed to serialize root ref: {}", e))
            })?,
        };

        // Create the post record with reply and embed
        let now = chrono::Utc::now().to_rfc3339();
        let post_record = post::Post {
            text: text.to_string(),
            created_at: now,
            reply: Some(serde_json::to_value(&reply_ref).map_err(|e| {
                AgentError::SessionError(format!("Failed to serialize reply ref: {}", e))
            })?),
            embed: Some(embed),
            langs: None,
            entities: None,
            tags: None,
            facets,
            labels: None,
        };

        // Serialize to JSON
        let record_json = serde_json::to_value(&post_record)
            .map_err(|e| AgentError::SessionError(format!("Failed to serialize post: {}", e)))?;

        let input = create_record::Input {
            repo: did,
            collection: "app.bsky.feed.post".to_string(),
            rkey: None,
            validate: None,
            record: record_json,
            swap_commit: None,
        };

        let response = create_record::create_record(&*self.client, input).await?;

        Ok(response.data.uri.to_string())
    }

    /// Create a post with an external link embed (link preview card)
    ///
    /// Creates a post with a rich link preview card showing a URL with title,
    /// description, and optional thumbnail image.
    ///
    /// # Arguments
    ///
    /// * `text` - The post text content
    /// * `url` - The URL to embed
    /// * `title` - Title for the link card
    /// * `description` - Description for the link card
    /// * `thumb_blob` - Optional thumbnail image blob (already uploaded)
    ///
    /// # Returns
    ///
    /// Returns the AT-URI of the created post
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// agent.login("alice.bsky.social", "password").await?;
    ///
    /// // Post with link card (no thumbnail)
    /// let uri = agent.post_with_link_embed(
    ///     "Check out this amazing article!",
    ///     "https://rust-lang.org/",
    ///     "The Rust Programming Language",
    ///     "A language empowering everyone to build reliable and efficient software.",
    ///     None,
    /// ).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn post_with_link_embed(
        &self,
        text: &str,
        url: &str,
        title: &str,
        description: &str,
        thumb_blob: Option<serde_json::Value>,
    ) -> Result<String, AgentError> {
        use crate::client::com::atproto::repo::create_record;
        use crate::client::app::bsky::feed::post;

        let did = self.assert_did()?;

        // Detect facets (mentions, links, hashtags)
        let (_text, facets) = self.detect_facets(text).await?;

        // Create external embed
        let external = serde_json::json!({
            "uri": url,
            "title": title,
            "description": description,
            "thumb": thumb_blob,
        });

        let embed = serde_json::json!({
            "$type": "app.bsky.embed.external",
            "external": external,
        });

        // Create the post record with embed
        let now = chrono::Utc::now().to_rfc3339();
        let post_record = post::Post {
            text: text.to_string(),
            created_at: now,
            reply: None,
            embed: Some(embed),
            langs: None,
            entities: None,
            tags: None,
            facets,
            labels: None,
        };

        // Serialize to JSON
        let record_json = serde_json::to_value(&post_record)
            .map_err(|e| AgentError::SessionError(format!("Failed to serialize post: {}", e)))?;

        let input = create_record::Input {
            repo: did,
            collection: "app.bsky.feed.post".to_string(),
            rkey: None,
            validate: None,
            record: record_json,
            swap_commit: None,
        };

        let response = create_record::create_record(&*self.client, input).await?;

        Ok(response.data.uri.to_string())
    }

    /// Create a post with an external link embed including a thumbnail
    ///
    /// Convenience method that uploads a thumbnail image and creates a post
    /// with a rich link preview card.
    ///
    /// # Arguments
    ///
    /// * `text` - The post text content
    /// * `url` - The URL to embed
    /// * `title` - Title for the link card
    /// * `description` - Description for the link card
    /// * `thumb_image` - Thumbnail image data (will be uploaded)
    ///
    /// # Returns
    ///
    /// Returns the AT-URI of the created post
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// agent.login("alice.bsky.social", "password").await?;
    ///
    /// // Post with link card and thumbnail
    /// let thumb_data = std::fs::read("thumbnail.jpg")?;
    /// let uri = agent.post_with_link_card(
    ///     "Great article about Rust!",
    ///     "https://blog.rust-lang.org/",
    ///     "Rust Blog",
    ///     "Official blog of the Rust programming language",
    ///     thumb_data,
    /// ).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn post_with_link_card(
        &self,
        text: &str,
        url: &str,
        title: &str,
        description: &str,
        thumb_image: Vec<u8>,
    ) -> Result<String, AgentError> {
        // Detect MIME type from data
        let mime_type =
            crate::blob::detect_mime_type_from_data(&thumb_image).unwrap_or("image/jpeg");

        // Upload the thumbnail
        let thumb_blob = self.upload_blob(thumb_image, mime_type).await?;

        // Create post with link embed
        self.post_with_link_embed(text, url, title, description, Some(thumb_blob))
            .await
    }

    /// Create a reply with an external link embed
    ///
    /// Creates a reply post with a rich link preview card.
    ///
    /// # Arguments
    ///
    /// * `text` - The reply text content
    /// * `url` - The URL to embed
    /// * `title` - Title for the link card
    /// * `description` - Description for the link card
    /// * `thumb_blob` - Optional thumbnail image blob (already uploaded)
    /// * `parent_uri` - AT-URI of the post being replied to
    /// * `parent_cid` - CID of the post being replied to
    /// * `root_uri` - AT-URI of the root post in the thread
    /// * `root_cid` - CID of the root post in the thread
    ///
    /// # Returns
    ///
    /// Returns the AT-URI of the created reply
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use atproto::agent::{Agent, ReplyWithLinkEmbedParams};
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// agent.login("alice.bsky.social", "password").await?;
    ///
    /// let params = ReplyWithLinkEmbedParams {
    ///     text: "Here's the source for that!",
    ///     url: "https://doc.rust-lang.org/",
    ///     title: "Rust Documentation",
    ///     description: "Official Rust programming language documentation",
    ///     thumb_blob: None,
    ///     parent_uri: "at://did:plc:abc/app.bsky.feed.post/xyz",
    ///     parent_cid: "bafyreiabc",
    ///     root_uri: "at://did:plc:abc/app.bsky.feed.post/xyz",
    ///     root_cid: "bafyreiabc",
    /// };
    /// let uri = agent.reply_with_link_embed(params).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn reply_with_link_embed(
        &self,
        params: ReplyWithLinkEmbedParams<'_>,
    ) -> Result<String, AgentError> {
        use crate::client::com::atproto::repo::create_record;
        use crate::client::app::bsky::feed::post::{self, ReplyRef};
        use crate::client::com::atproto::repo::strong_ref::Main as StrongRef;

        let did = self.assert_did()?;

        // Detect facets (mentions, links, hashtags)
        let (text, facets) = self.detect_facets(params.text).await?;

        // Create external embed
        let external = serde_json::json!({
            "uri": params.url,
            "title": params.title,
            "description": params.description,
            "thumb": params.thumb_blob,
        });

        let embed = serde_json::json!({
            "$type": "app.bsky.embed.external",
            "external": external,
        });

        // Create reply reference
        let parent_ref = StrongRef {
            uri: crate::syntax::AtUri::try_from(params.parent_uri).map_err(|e| AgentError::SessionError(format!("Invalid parent URI: {}", e)))?,
            cid: params.parent_cid.to_string(),
        };
        let root_ref = StrongRef {
            uri: crate::syntax::AtUri::try_from(params.root_uri).map_err(|e| AgentError::SessionError(format!("Invalid root URI: {}", e)))?,
            cid: params.root_cid.to_string(),
        };

        let reply_ref = ReplyRef {
            parent: serde_json::to_value(&parent_ref).map_err(|e| {
                AgentError::SessionError(format!("Failed to serialize parent ref: {}", e))
            })?,
            root: serde_json::to_value(&root_ref).map_err(|e| {
                AgentError::SessionError(format!("Failed to serialize root ref: {}", e))
            })?,
        };

        // Create the post record with reply and embed
        let now = chrono::Utc::now().to_rfc3339();
        let post_record = post::Post {
            text: text.to_string(),
            created_at: now,
            reply: Some(serde_json::to_value(&reply_ref).map_err(|e| {
                AgentError::SessionError(format!("Failed to serialize reply ref: {}", e))
            })?),
            embed: Some(embed),
            langs: None,
            entities: None,
            tags: None,
            facets,
            labels: None,
        };

        // Serialize to JSON
        let record_json = serde_json::to_value(&post_record)
            .map_err(|e| AgentError::SessionError(format!("Failed to serialize post: {}", e)))?;

        let input = create_record::Input {
            repo: did,
            collection: "app.bsky.feed.post".to_string(),
            rkey: None,
            validate: None,
            record: record_json,
            swap_commit: None,
        };

        let response = create_record::create_record(&*self.client, input).await?;

        Ok(response.data.uri.to_string())
    }

    /// Follow a user
    ///
    /// # Arguments
    ///
    /// * `subject_did` - DID of the user to follow
    ///
    /// # Returns
    ///
    /// Returns the AT-URI of the follow record
    pub async fn follow(&self, subject_did: &str) -> Result<String, AgentError> {
        use crate::client::com::atproto::repo::create_record;
        use crate::client::app::bsky::graph::follow;

        let did = self.assert_did()?;

        let now = chrono::Utc::now().to_rfc3339();
        let subject_did_obj = Did::new(subject_did)
            .map_err(|_| AgentError::InvalidDid(subject_did.to_string()))?;
        let follow_record = follow::Follow {
            subject: subject_did_obj,
            created_at: now,
        };

        let record_json = serde_json::to_value(&follow_record)
            .map_err(|e| AgentError::SessionError(format!("Failed to serialize follow: {}", e)))?;

        let input = create_record::Input {
            repo: did,
            collection: "app.bsky.graph.follow".to_string(),
            rkey: None,
            validate: None,
            record: record_json,
            swap_commit: None,
        };

        let response = create_record::create_record(&*self.client, input).await?;

        Ok(response.data.uri.to_string())
    }

    /// Like a post
    ///
    /// # Arguments
    ///
    /// * `uri` - AT-URI of the post to like
    /// * `cid` - CID of the post
    ///
    /// # Returns
    ///
    /// Returns the AT-URI of the like record
    pub async fn like(&self, uri: &str, cid: &str) -> Result<String, AgentError> {
        use crate::client::com::atproto::repo::create_record;
        use crate::client::app::bsky::feed::like;

        let did = self.assert_did()?;

        let now = chrono::Utc::now().to_rfc3339();

        // Create subject JSON
        let subject = serde_json::json!({
            "uri": uri,
            "cid": cid
        });

        let like_record = like::Like {
            subject,
            created_at: now,
            via: None,
        };

        let record_json = serde_json::to_value(&like_record)
            .map_err(|e| AgentError::SessionError(format!("Failed to serialize like: {}", e)))?;

        let input = create_record::Input {
            repo: did,
            collection: "app.bsky.feed.like".to_string(),
            rkey: None,
            validate: None,
            record: record_json,
            swap_commit: None,
        };

        let response = create_record::create_record(&*self.client, input).await?;

        Ok(response.data.uri.to_string())
    }

    /// Repost a post
    ///
    /// # Arguments
    ///
    /// * `uri` - AT-URI of the post to repost
    /// * `cid` - CID of the post
    ///
    /// # Returns
    ///
    /// Returns the AT-URI of the repost record
    pub async fn repost(&self, uri: &str, cid: &str) -> Result<String, AgentError> {
        use crate::client::com::atproto::repo::create_record;
        use crate::client::app::bsky::feed::repost;

        let did = self.assert_did()?;

        let now = chrono::Utc::now().to_rfc3339();

        // Create subject JSON
        let subject = serde_json::json!({
            "uri": uri,
            "cid": cid
        });

        let repost_record = repost::Repost {
            subject,
            created_at: now,
            via: None,
        };

        let record_json = serde_json::to_value(&repost_record)
            .map_err(|e| AgentError::SessionError(format!("Failed to serialize repost: {}", e)))?;

        let input = create_record::Input {
            repo: did,
            collection: "app.bsky.feed.repost".to_string(),
            rkey: None,
            validate: None,
            record: record_json,
            swap_commit: None,
        };

        let response = create_record::create_record(&*self.client, input).await?;

        Ok(response.data.uri.to_string())
    }

    /// Delete a record
    ///
    /// # Arguments
    ///
    /// * `uri` - AT-URI of the record to delete
    pub async fn delete_record(&self, uri: &str) -> Result<(), AgentError> {
        use crate::client::com::atproto::repo::delete_record;
        use crate::syntax::AtUri;

        let at_uri = AtUri::new(uri)
            .map_err(|e| AgentError::SessionError(format!("Invalid AT-URI: {}", e)))?;

        let input = delete_record::Input {
            repo: at_uri.hostname().to_string(),
            collection: at_uri.collection().to_string(),
            rkey: at_uri.rkey().unwrap_or("").to_string(),
            swap_record: None,
            swap_commit: None,
        };

        delete_record::delete_record(&*self.client, input).await?;

        Ok(())
    }

    /// Get the user's timeline
    ///
    /// # Arguments
    ///
    /// * `limit` - Maximum number of posts to fetch (default: 50)
    ///
    /// # Returns
    ///
    /// Returns the timeline feed
    pub async fn get_timeline(&self, limit: Option<i64>) -> Result<serde_json::Value, AgentError> {
        use crate::client::app::bsky::feed::get_timeline;

        self.assert_did()?;

        let params = get_timeline::QueryParams {
            algorithm: None,
            limit: limit.or(Some(50)),
            cursor: None,
        };

        let response = get_timeline::get_timeline(&*self.client, params).await?;

        serde_json::to_value(&response.data)
            .map_err(|e| AgentError::SessionError(format!("Failed to serialize timeline: {}", e)))
    }

    /// Get a profile
    ///
    /// # Arguments
    ///
    /// * `actor` - Handle or DID of the profile to fetch
    ///
    /// # Returns
    ///
    /// Returns the profile data
    pub async fn get_profile(&self, actor: &str) -> Result<serde_json::Value, AgentError> {
        use crate::client::app::bsky::actor::get_profile;

        let params = get_profile::QueryParams {
            actor: actor.to_string(),
        };

        let response = get_profile::get_profile(&*self.client, params).await?;

        serde_json::to_value(&response.data)
            .map_err(|e| AgentError::SessionError(format!("Failed to serialize profile: {}", e)))
    }

    /// Get multiple profiles
    ///
    /// # Arguments
    ///
    /// * `actors` - Vector of handles or DIDs to fetch
    ///
    /// # Returns
    ///
    /// Returns the profiles data
    pub async fn get_profiles(&self, actors: Vec<String>) -> Result<serde_json::Value, AgentError> {
        use crate::client::app::bsky::actor::get_profiles;

        let params = get_profiles::QueryParams {
            actors: serde_json::to_value(actors)
                .map_err(|e| AgentError::SessionError(format!("Failed to serialize actors: {}", e)))?,
        };

        let response = get_profiles::get_profiles(&*self.client, params).await?;

        serde_json::to_value(&response.data)
            .map_err(|e| AgentError::SessionError(format!("Failed to serialize profiles: {}", e)))
    }

    /// Get actor suggestions
    ///
    /// # Arguments
    ///
    /// * `limit` - Maximum number of suggestions to return
    /// * `cursor` - Optional pagination cursor
    ///
    /// # Returns
    ///
    /// Returns suggested actors
    pub async fn get_suggestions(&self, limit: Option<i64>, cursor: Option<String>) -> Result<serde_json::Value, AgentError> {
        use crate::client::app::bsky::actor::get_suggestions;

        let params = get_suggestions::QueryParams {
            limit,
            cursor,
        };

        let response = get_suggestions::get_suggestions(&*self.client, params).await?;

        serde_json::to_value(&response.data)
            .map_err(|e| AgentError::SessionError(format!("Failed to serialize suggestions: {}", e)))
    }

    /// Search for actors
    ///
    /// # Arguments
    ///
    /// * `query` - Search query string
    /// * `limit` - Maximum number of results to return
    /// * `cursor` - Optional pagination cursor
    ///
    /// # Returns
    ///
    /// Returns search results
    pub async fn search_actors(&self, query: &str, limit: Option<i64>, cursor: Option<String>) -> Result<serde_json::Value, AgentError> {
        use crate::client::app::bsky::actor::search_actors;

        let params = search_actors::QueryParams {
            q: Some(query.to_string()),
            term: None,
            limit,
            cursor,
        };

        let response = search_actors::search_actors(&*self.client, params).await?;

        serde_json::to_value(&response.data)
            .map_err(|e| AgentError::SessionError(format!("Failed to serialize search results: {}", e)))
    }

    /// Search for actors (typeahead)
    ///
    /// # Arguments
    ///
    /// * `query` - Search query string
    /// * `limit` - Maximum number of results to return
    ///
    /// # Returns
    ///
    /// Returns typeahead search results
    pub async fn search_actors_typeahead(&self, query: &str, limit: Option<i64>) -> Result<serde_json::Value, AgentError> {
        use crate::client::app::bsky::actor::search_actors_typeahead;

        let params = search_actors_typeahead::QueryParams {
            q: Some(query.to_string()),
            term: None,
            limit,
        };

        let response = search_actors_typeahead::search_actors_typeahead(&*self.client, params).await?;

        serde_json::to_value(&response.data)
            .map_err(|e| AgentError::SessionError(format!("Failed to serialize typeahead results: {}", e)))
    }

    /// Get an author's feed
    ///
    /// # Arguments
    ///
    /// * `actor` - Handle or DID of the author
    /// * `limit` - Maximum number of posts to return
    /// * `cursor` - Optional pagination cursor
    /// * `filter` - Optional filter (posts_with_replies, posts_no_replies, posts_with_media, posts_and_author_threads)
    ///
    /// # Returns
    ///
    /// Returns the author's feed
    pub async fn get_author_feed(
        &self,
        actor: &str,
        limit: Option<i64>,
        cursor: Option<String>,
        filter: Option<String>,
    ) -> Result<serde_json::Value, AgentError> {
        use crate::client::app::bsky::feed::get_author_feed;

        let params = get_author_feed::QueryParams {
            actor: actor.to_string(),
            limit,
            cursor,
            filter,
            include_pins: None,
        };

        let response = get_author_feed::get_author_feed(&*self.client, params).await?;

        serde_json::to_value(&response.data)
            .map_err(|e| AgentError::SessionError(format!("Failed to serialize author feed: {}", e)))
    }

    /// Get a post thread
    ///
    /// # Arguments
    ///
    /// * `uri` - AT-URI of the post
    /// * `depth` - How many levels of reply depth to fetch
    /// * `parent_height` - How many levels of parent posts to fetch
    ///
    /// # Returns
    ///
    /// Returns the post thread
    pub async fn get_post_thread(
        &self,
        uri: &str,
        depth: Option<i64>,
        parent_height: Option<i64>,
    ) -> Result<serde_json::Value, AgentError> {
        use crate::client::app::bsky::feed::get_post_thread;
        use crate::syntax::AtUri;

        let params = get_post_thread::QueryParams {
            uri: AtUri::try_from(uri.to_string())
                .map_err(|e| AgentError::SessionError(format!("Invalid AT-URI: {}", e)))?,
            depth,
            parent_height,
        };

        let response = get_post_thread::get_post_thread(&*self.client, params).await?;

        serde_json::to_value(&response.data)
            .map_err(|e| AgentError::SessionError(format!("Failed to serialize post thread: {}", e)))
    }

    /// Get multiple posts
    ///
    /// # Arguments
    ///
    /// * `uris` - Vector of AT-URIs of posts to fetch
    ///
    /// # Returns
    ///
    /// Returns the posts
    pub async fn get_posts(&self, uris: Vec<String>) -> Result<serde_json::Value, AgentError> {
        use crate::client::app::bsky::feed::get_posts;

        let params = get_posts::QueryParams {
            uris: serde_json::to_value(uris)
                .map_err(|e| AgentError::SessionError(format!("Failed to serialize uris: {}", e)))?,
        };

        let response = get_posts::get_posts(&*self.client, params).await?;

        serde_json::to_value(&response.data)
            .map_err(|e| AgentError::SessionError(format!("Failed to serialize posts: {}", e)))
    }

    /// Get posts liked by an actor
    ///
    /// # Arguments
    ///
    /// * `actor` - Handle or DID of the actor
    /// * `limit` - Maximum number of likes to return
    /// * `cursor` - Optional pagination cursor
    ///
    /// # Returns
    ///
    /// Returns the actor's likes
    pub async fn get_actor_likes(
        &self,
        actor: &str,
        limit: Option<i64>,
        cursor: Option<String>,
    ) -> Result<serde_json::Value, AgentError> {
        use crate::client::app::bsky::feed::get_actor_likes;

        let params = get_actor_likes::QueryParams {
            actor: actor.to_string(),
            limit,
            cursor,
        };

        let response = get_actor_likes::get_actor_likes(&*self.client, params).await?;

        serde_json::to_value(&response.data)
            .map_err(|e| AgentError::SessionError(format!("Failed to serialize actor likes: {}", e)))
    }

    /// Get likes for a post
    ///
    /// # Arguments
    ///
    /// * `uri` - AT-URI of the post
    /// * `cid` - Optional CID of the post
    /// * `limit` - Maximum number of likes to return
    /// * `cursor` - Optional pagination cursor
    ///
    /// # Returns
    ///
    /// Returns the post's likes
    pub async fn get_likes(
        &self,
        uri: &str,
        cid: Option<String>,
        limit: Option<i64>,
        cursor: Option<String>,
    ) -> Result<serde_json::Value, AgentError> {
        use crate::client::app::bsky::feed::get_likes;
        use crate::syntax::AtUri;

        let params = get_likes::QueryParams {
            uri: AtUri::try_from(uri.to_string())
                .map_err(|e| AgentError::SessionError(format!("Invalid AT-URI: {}", e)))?,
            cid,
            limit,
            cursor,
        };

        let response = get_likes::get_likes(&*self.client, params).await?;

        serde_json::to_value(&response.data)
            .map_err(|e| AgentError::SessionError(format!("Failed to serialize likes: {}", e)))
    }

    /// Get actors who reposted a post
    ///
    /// # Arguments
    ///
    /// * `uri` - AT-URI of the post
    /// * `cid` - Optional CID of the post
    /// * `limit` - Maximum number of reposts to return
    /// * `cursor` - Optional pagination cursor
    ///
    /// # Returns
    ///
    /// Returns actors who reposted
    pub async fn get_reposted_by(
        &self,
        uri: &str,
        cid: Option<String>,
        limit: Option<i64>,
        cursor: Option<String>,
    ) -> Result<serde_json::Value, AgentError> {
        use crate::client::app::bsky::feed::get_reposted_by;
        use crate::syntax::AtUri;

        let params = get_reposted_by::QueryParams {
            uri: AtUri::try_from(uri.to_string())
                .map_err(|e| AgentError::SessionError(format!("Invalid AT-URI: {}", e)))?,
            cid,
            limit,
            cursor,
        };

        let response = get_reposted_by::get_reposted_by(&*self.client, params).await?;

        serde_json::to_value(&response.data)
            .map_err(|e| AgentError::SessionError(format!("Failed to serialize reposted by: {}", e)))
    }

    /// Get follows for an actor
    ///
    /// # Arguments
    ///
    /// * `actor` - Handle or DID of the actor
    /// * `limit` - Maximum number of follows to return
    /// * `cursor` - Optional pagination cursor
    ///
    /// # Returns
    ///
    /// Returns the actor's follows
    pub async fn get_follows(
        &self,
        actor: &str,
        limit: Option<i64>,
        cursor: Option<String>,
    ) -> Result<serde_json::Value, AgentError> {
        use crate::client::app::bsky::graph::get_follows;

        let params = get_follows::QueryParams {
            actor: actor.to_string(),
            limit,
            cursor,
        };

        let response = get_follows::get_follows(&*self.client, params).await?;

        serde_json::to_value(&response.data)
            .map_err(|e| AgentError::SessionError(format!("Failed to serialize follows: {}", e)))
    }

    /// Get followers for an actor
    ///
    /// # Arguments
    ///
    /// * `actor` - Handle or DID of the actor
    /// * `limit` - Maximum number of followers to return
    /// * `cursor` - Optional pagination cursor
    ///
    /// # Returns
    ///
    /// Returns the actor's followers
    pub async fn get_followers(
        &self,
        actor: &str,
        limit: Option<i64>,
        cursor: Option<String>,
    ) -> Result<serde_json::Value, AgentError> {
        use crate::client::app::bsky::graph::get_followers;

        let params = get_followers::QueryParams {
            actor: actor.to_string(),
            limit,
            cursor,
        };

        let response = get_followers::get_followers(&*self.client, params).await?;

        serde_json::to_value(&response.data)
            .map_err(|e| AgentError::SessionError(format!("Failed to serialize followers: {}", e)))
    }

    /// Delete a follow record
    ///
    /// # Arguments
    ///
    /// * `follow_uri` - AT-URI of the follow record to delete
    ///
    /// # Returns
    ///
    /// Returns Ok if successful
    pub async fn delete_follow(&self, follow_uri: &str) -> Result<(), AgentError> {
        self.delete_record(follow_uri).await
    }

    /// Delete a like record
    ///
    /// # Arguments
    ///
    /// * `like_uri` - AT-URI of the like record to delete
    ///
    /// # Returns
    ///
    /// Returns Ok if successful
    pub async fn delete_like(&self, like_uri: &str) -> Result<(), AgentError> {
        self.delete_record(like_uri).await
    }

    /// Delete a repost record
    ///
    /// # Arguments
    ///
    /// * `repost_uri` - AT-URI of the repost record to delete
    ///
    /// # Returns
    ///
    /// Returns Ok if successful
    pub async fn delete_repost(&self, repost_uri: &str) -> Result<(), AgentError> {
        self.delete_record(repost_uri).await
    }

    // ============================================================================
    // Notifications
    // ============================================================================

    /// List notifications for the authenticated user
    ///
    /// # Arguments
    ///
    /// * `limit` - Maximum number of notifications to return
    /// * `cursor` - Optional pagination cursor
    /// * `seen_at` - Optional timestamp to filter notifications
    /// * `priority` - Optional flag to filter priority notifications
    ///
    /// # Returns
    ///
    /// Returns the notifications list
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// agent.login("alice.bsky.social", "app-password").await?;
    ///
    /// // Get recent notifications
    /// let notifications = agent.list_notifications(Some(50), None, None, None).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn list_notifications(
        &self,
        limit: Option<i64>,
        cursor: Option<String>,
        seen_at: Option<String>,
        priority: Option<bool>,
    ) -> Result<serde_json::Value, AgentError> {
        use crate::client::app::bsky::notification::list_notifications;

        self.assert_did()?;

        let params = list_notifications::QueryParams {
            limit,
            cursor,
            seen_at,
            priority,
            reasons: None,
        };

        let response = list_notifications::list_notifications(&*self.client, params).await?;

        serde_json::to_value(&response.data)
            .map_err(|e| AgentError::SessionError(format!("Failed to serialize notifications: {}", e)))
    }

    /// Count unread notifications for the authenticated user
    ///
    /// # Arguments
    ///
    /// * `priority` - Optional flag to count only priority notifications
    /// * `seen_at` - Optional timestamp to count notifications since
    ///
    /// # Returns
    ///
    /// Returns the count of unread notifications
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// agent.login("alice.bsky.social", "app-password").await?;
    ///
    /// // Get unread count
    /// let count = agent.count_unread_notifications(None, None).await?;
    /// println!("Unread notifications: {}", count);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn count_unread_notifications(
        &self,
        priority: Option<bool>,
        seen_at: Option<String>,
    ) -> Result<i64, AgentError> {
        use crate::client::app::bsky::notification::get_unread_count;

        self.assert_did()?;

        let params = get_unread_count::QueryParams {
            priority,
            seen_at,
        };

        let response = get_unread_count::get_unread_count(&*self.client, params).await?;

        Ok(response.data.count)
    }

    /// Update the seen timestamp for notifications
    ///
    /// Marks all notifications up to the given timestamp as seen.
    ///
    /// # Arguments
    ///
    /// * `seen_at` - Optional ISO 8601 timestamp. If None, uses current time.
    ///
    /// # Returns
    ///
    /// Returns Ok if successful
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// agent.login("alice.bsky.social", "app-password").await?;
    ///
    /// // Mark all notifications as seen
    /// agent.update_seen_notifications(None).await?;
    ///
    /// // Or with a specific timestamp
    /// agent.update_seen_notifications(Some("2024-01-01T00:00:00Z".to_string())).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn update_seen_notifications(&self, seen_at: Option<String>) -> Result<(), AgentError> {
        use crate::client::app::bsky::notification::update_seen;

        self.assert_did()?;

        // Use provided timestamp or current time
        let timestamp = seen_at.unwrap_or_else(|| {
            chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        });

        let input = update_seen::Input {
            seen_at: timestamp,
        };

        update_seen::update_seen(&*self.client, input).await?;

        Ok(())
    }

    // ============================================================================
    // Preferences Management
    // ============================================================================

    /// Get user preferences
    ///
    /// Fetches and parses all user preferences from the server, including:
    /// - Saved feeds (V1 & V2 with automatic migration)
    /// - Feed view preferences
    /// - Thread view preferences
    /// - Moderation preferences (adult content, labels, labelers)
    /// - Muted words
    /// - Hidden posts
    /// - Personal details (birth date)
    /// - Interests
    /// - App state (nudges, NUX, progress guides)
    /// - Post interaction settings
    /// - Verification preferences
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// agent.login("alice.bsky.social", "password").await?;
    ///
    /// let prefs = agent.get_preferences().await?;
    /// println!("Adult content enabled: {}", prefs.moderation_prefs.adult_content_enabled);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_preferences(&self) -> Result<crate::preferences::BskyPreferences, AgentError> {
        use crate::client::app::bsky::actor::get_preferences;
        use crate::preferences::*;

        // Fetch raw preferences from server
        let response = get_preferences::get_preferences(&*self.client, get_preferences::QueryParams {}).await?;

        // Initialize default preferences structure
        let mut prefs = BskyPreferences::default();

        // Initialize app labelers
        prefs.moderation_prefs.labelers = self.app_labelers
            .read()
            .unwrap()
            .iter()
            .map(|did| ModerationPrefsLabeler {
                did: did.clone(),
                labels: HashMap::new(),
            })
            .collect();

        // Parse each preference type
        if let Some(prefs_array) = response.data.preferences.as_array() {
            for pref in prefs_array {
                // Extract $type field
                let pref_type = pref.get("$type").and_then(|v| v.as_str()).unwrap_or("");

            match pref_type {
                "app.bsky.actor.defs#adultContentPref" => {
                    if let Some(enabled) = pref.get("enabled").and_then(|v| v.as_bool()) {
                        prefs.moderation_prefs.adult_content_enabled = enabled;
                    }
                }
                "app.bsky.actor.defs#contentLabelPref" => {
                    if let Some(label) = pref.get("label").and_then(|v| v.as_str()) {
                        if let Some(visibility) = pref.get("visibility").and_then(|v| v.as_str()) {
                            let labeler_did = pref.get("labelerDid").and_then(|v| v.as_str());

                            if let Some(labeler_did) = labeler_did {
                                // Labeler-specific preference
                                if let Some(labeler) = prefs.moderation_prefs.labelers.iter_mut().find(|l| l.did == labeler_did) {
                                    labeler.labels.insert(label.to_string(), visibility.to_string());
                                }
                            } else {
                                // Global preference
                                prefs.moderation_prefs.labels.insert(label.to_string(), visibility.to_string());
                            }
                        }
                    }
                }
                "app.bsky.actor.defs#labelersPref" => {
                    if let Some(labelers_array) = pref.get("labelers").and_then(|v| v.as_array()) {
                        for labeler in labelers_array {
                            if let Some(did) = labeler.get("did").and_then(|v| v.as_str()) {
                                // Add if not already in list
                                if !prefs.moderation_prefs.labelers.iter().any(|l| l.did == did) {
                                    prefs.moderation_prefs.labelers.push(ModerationPrefsLabeler {
                                        did: did.to_string(),
                                        labels: HashMap::new(),
                                    });
                                }
                            }
                        }
                    }
                }
                "app.bsky.actor.defs#savedFeedsPrefV2" => {
                    if let Ok(saved_feeds) = serde_json::from_value::<Vec<crate::client::app::bsky::actor::defs::SavedFeed>>(
                        pref.get("items").cloned().unwrap_or(serde_json::json!([]))
                    ) {
                        prefs.saved_feeds = saved_feeds;
                    }
                }
                "app.bsky.actor.defs#savedFeedsPref" => {
                    // V1 feeds (deprecated, but used for migration)
                    if let Some(saved) = pref.get("saved").and_then(|v| v.as_array()) {
                        let saved_vec: Vec<String> = saved.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect();
                        if prefs.feeds.is_none() {
                            prefs.feeds = Some(LegacyFeedsPreference::default());
                        }
                        if let Some(ref mut feeds) = prefs.feeds {
                            feeds.saved = Some(saved_vec);
                        }
                    }
                    if let Some(pinned) = pref.get("pinned").and_then(|v| v.as_array()) {
                        let pinned_vec: Vec<String> = pinned.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect();
                        if prefs.feeds.is_none() {
                            prefs.feeds = Some(LegacyFeedsPreference::default());
                        }
                        if let Some(ref mut feeds) = prefs.feeds {
                            feeds.pinned = Some(pinned_vec);
                        }
                    }
                }
                "app.bsky.actor.defs#personalDetailsPref" => {
                    if let Some(birth_date_str) = pref.get("birthDate").and_then(|v| v.as_str()) {
                        if let Ok(birth_date) = chrono::DateTime::parse_from_rfc3339(birth_date_str) {
                            prefs.birth_date = Some(birth_date.with_timezone(&chrono::Utc));
                        }
                    }
                }
                "app.bsky.actor.defs#feedViewPref" => {
                    if let Some(feed) = pref.get("feed").and_then(|v| v.as_str()) {
                        if let Ok(feed_view_pref) = serde_json::from_value::<BskyFeedViewPreference>(pref.clone()) {
                            prefs.feed_view_prefs.insert(feed.to_string(), feed_view_pref);
                        }
                    }
                }
                "app.bsky.actor.defs#threadViewPref" => {
                    if let Ok(thread_view_pref) = serde_json::from_value::<BskyThreadViewPreference>(pref.clone()) {
                        prefs.thread_view_prefs = thread_view_pref;
                    }
                }
                "app.bsky.actor.defs#interestsPref" => {
                    if let Ok(interests_pref) = serde_json::from_value::<BskyInterestsPreference>(pref.clone()) {
                        prefs.interests = interests_pref;
                    }
                }
                "app.bsky.actor.defs#mutedWordsPref" => {
                    if let Ok(muted_words) = serde_json::from_value::<Vec<crate::client::app::bsky::actor::defs::MutedWord>>(
                        pref.get("items").cloned().unwrap_or(serde_json::json!([]))
                    ) {
                        prefs.moderation_prefs.muted_words = muted_words;
                    }
                }
                "app.bsky.actor.defs#hiddenPostsPref" => {
                    if let Some(items) = pref.get("items").and_then(|v| v.as_array()) {
                        prefs.moderation_prefs.hidden_posts = items.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect();
                    }
                }
                "app.bsky.actor.defs#bskyAppStatePref" => {
                    if let Some(queued_nudges) = pref.get("queuedNudges").and_then(|v| v.as_array()) {
                        prefs.bsky_app_state.queued_nudges = queued_nudges.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect();
                    }
                    prefs.bsky_app_state.active_progress_guide = pref.get("activeProgressGuide").cloned();
                    if let Some(nuxs) = pref.get("nuxs").and_then(|v| v.as_array()) {
                        prefs.bsky_app_state.nuxs = nuxs.clone();
                    }
                }
                "app.bsky.actor.defs#postInteractionSettingsPref" => {
                    prefs.post_interaction_settings = pref.clone();
                }
                "app.bsky.actor.defs#verificationPrefs" => {
                    prefs.verification_prefs = pref.clone();
                }
                _ => {
                    // Unknown preference type - ignore
                }
            }
        }
        }

        // Migrate V1 feeds to V2 if needed
        if prefs.saved_feeds.is_empty() {
            if let Some(ref legacy_feeds) = prefs.feeds {
                if let (Some(saved), Some(pinned)) = (&legacy_feeds.saved, &legacy_feeds.pinned) {
                    // Perform migration
                    prefs.saved_feeds = self.migrate_feeds_v1_to_v2(saved, pinned).await?;
                    // Save migrated feeds
                    self.overwrite_saved_feeds(prefs.saved_feeds.clone()).await?;
                }
            }
            // If no V1 feeds either, create default
            if prefs.saved_feeds.is_empty() {
                prefs.saved_feeds = vec![
                    crate::client::app::bsky::actor::defs::SavedFeed {
                        id: crate::tid::Tid::next().unwrap().to_string(),
                        r#type: "timeline".to_string(),
                        value: "following".to_string(),
                        pinned: true,
                    }
                ];
            }
        }

        // Auto-configure labelers
        let labeler_dids: Vec<String> = prefs.moderation_prefs.labelers.iter()
            .map(|l| l.did.clone())
            .collect();
        self.configure_labelers(labeler_dids);

        Ok(prefs)
    }

    /// Migrate V1 feeds to V2 format
    async fn migrate_feeds_v1_to_v2(
        &self,
        saved: &[String],
        pinned: &[String],
    ) -> Result<Vec<crate::client::app::bsky::actor::defs::SavedFeed>, AgentError> {
        let mut feeds = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // Add "Following" feed first
        feeds.push(crate::client::app::bsky::actor::defs::SavedFeed {
            id: crate::tid::Tid::next().unwrap().to_string(),
            r#type: "timeline".to_string(),
            value: "following".to_string(),
            pinned: true,
        });
        seen.insert("timeline".to_string());

        // Add pinned feeds
        for uri in pinned {
            if !seen.contains(uri) {
                if let Ok(feed_type) = crate::util::get_saved_feed_type(uri) {
                    if feed_type != crate::util::SavedFeedType::Unknown {
                        feeds.push(crate::client::app::bsky::actor::defs::SavedFeed {
                            id: crate::tid::Tid::next().unwrap().to_string(),
                            r#type: match feed_type {
                                crate::util::SavedFeedType::Feed => "feed",
                                crate::util::SavedFeedType::List => "list",
                                _ => "unknown",
                            }.to_string(),
                            value: uri.clone(),
                            pinned: true,
                        });
                        seen.insert(uri.clone());
                    }
                }
            }
        }

        // Add saved (but not pinned) feeds
        for uri in saved {
            if !seen.contains(uri) {
                if let Ok(feed_type) = crate::util::get_saved_feed_type(uri) {
                    if feed_type != crate::util::SavedFeedType::Unknown {
                        feeds.push(crate::client::app::bsky::actor::defs::SavedFeed {
                            id: crate::tid::Tid::next().unwrap().to_string(),
                            r#type: match feed_type {
                                crate::util::SavedFeedType::Feed => "feed",
                                crate::util::SavedFeedType::List => "list",
                                _ => "unknown",
                            }.to_string(),
                            value: uri.clone(),
                            pinned: false,
                        });
                        seen.insert(uri.clone());
                    }
                }
            }
        }

        Ok(feeds)
    }

    /// Replace saved feeds list
    ///
    /// Validates and overwrites the entire saved feeds list.
    ///
    /// # Arguments
    ///
    /// * `saved_feeds` - New list of saved feeds
    pub async fn overwrite_saved_feeds(
        &self,
        saved_feeds: Vec<crate::client::app::bsky::actor::defs::SavedFeed>,
    ) -> Result<(), AgentError> {
        // Validate all feeds
        for feed in &saved_feeds {
            // Validate feed has an ID
            if feed.id.is_empty() {
                return Err(AgentError::SessionError("Saved feed must have an ID (use a TID)".to_string()));
            }

            // Validate feed type matches URI if it's a feed or list
            if feed.r#type == "feed" || feed.r#type == "list" {
                if let Ok(uri) = crate::syntax::AtUri::new(&feed.value) {
                    let collection = uri.collection();
                    let is_feed = collection == "app.bsky.feed.generator";
                    let is_list = collection == "app.bsky.graph.list";

                    if feed.r#type == "feed" && !is_feed {
                        return Err(AgentError::SessionError(format!(
                            "Saved feed of type 'feed' must be a feed generator, got {}",
                            collection
                        )));
                    }
                    if feed.r#type == "list" && !is_list {
                        return Err(AgentError::SessionError(format!(
                            "Saved feed of type 'list' must be a list, got {}",
                            collection
                        )));
                    }
                }
            }
        }

        // Remove duplicates, keeping last occurrence
        let mut unique_feeds = std::collections::HashMap::new();
        for feed in saved_feeds {
            unique_feeds.insert(feed.id.clone(), feed);
        }
        let unique_vec: Vec<_> = unique_feeds.into_values().collect();

        self.update_saved_feeds_v2_preferences(|_| unique_vec).await?;
        Ok(())
    }

    /// Internal: Update saved feeds V2 preferences
    async fn update_saved_feeds_v2_preferences<F>(
        &self,
        callback: F,
    ) -> Result<Vec<crate::client::app::bsky::actor::defs::SavedFeed>, AgentError>
    where
        F: FnOnce(Vec<crate::client::app::bsky::actor::defs::SavedFeed>) -> Vec<crate::client::app::bsky::actor::defs::SavedFeed>,
    {
        use crate::client::app::bsky::actor::{get_preferences, put_preferences};

        let _lock = self.prefs_lock.lock().await;

        // Get current preferences
        let response = get_preferences::get_preferences(&*self.client, get_preferences::QueryParams {}).await?;
        let preferences = response.data.preferences;

        // Convert preferences Value to Vec for manipulation
        let mut prefs_vec = if let Some(arr) = preferences.as_array() {
            arr.clone()
        } else {
            vec![]
        };

        // Find existing V2 pref
        let existing_v2 = prefs_vec.iter()
            .find(|p| p.get("$type").and_then(|v| v.as_str()) == Some("app.bsky.actor.defs#savedFeedsPrefV2"))
            .and_then(|p| {
                serde_json::from_value::<Vec<crate::client::app::bsky::actor::defs::SavedFeed>>(
                    p.get("items").cloned().unwrap_or(serde_json::json!([]))
                ).ok()
            })
            .unwrap_or_default();

        // Apply callback
        let new_saved_feeds = callback(existing_v2);

        // Sort: pinned first, then saved
        let mut sorted = new_saved_feeds;
        sorted.sort_by(|a, b| {
            match (a.pinned, b.pinned) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => std::cmp::Ordering::Equal,
            }
        });

        // Create new V2 pref
        let v2_pref = serde_json::json!({
            "$type": "app.bsky.actor.defs#savedFeedsPrefV2",
            "items": sorted
        });

        // Remove old V2 pref and add new one
        prefs_vec.retain(|p| {
            p.get("$type").and_then(|v| v.as_str()) != Some("app.bsky.actor.defs#savedFeedsPrefV2")
        });
        prefs_vec.push(v2_pref);

        // Save to server
        let input = put_preferences::Input {
            preferences: serde_json::Value::Array(prefs_vec),
        };
        put_preferences::put_preferences(&*self.client, input).await?;

        Ok(sorted)
    }

    // ============================================================================
    // Moderation Preferences
    // ============================================================================

    /// Set adult content enabled preference
    ///
    /// Controls whether adult/mature content is shown.
    ///
    /// # Arguments
    ///
    /// * `enabled` - Whether to enable adult content
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// agent.login("alice.bsky.social", "password").await?;
    ///
    /// agent.set_adult_content_enabled(true).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn set_adult_content_enabled(&self, enabled: bool) -> Result<(), AgentError> {
        self.update_preferences(|mut prefs| {
            // Find or create adult content pref
            let mut found = false;
            for pref in &mut prefs {
                if pref.get("$type").and_then(|v| v.as_str()) == Some("app.bsky.actor.defs#adultContentPref") {
                    if let Some(obj) = pref.as_object_mut() {
                        obj.insert("enabled".to_string(), serde_json::Value::Bool(enabled));
                        found = true;
                        break;
                    }
                }
            }

            if !found {
                prefs.push(serde_json::json!({
                    "$type": "app.bsky.actor.defs#adultContentPref",
                    "enabled": enabled
                }));
            }

            prefs
        }).await?;

        Ok(())
    }

    /// Set content label preference
    ///
    /// Controls how content with specific labels should be displayed.
    ///
    /// # Arguments
    ///
    /// * `label` - The label name (e.g., "porn", "sexual", "graphic-media")
    /// * `visibility` - How to display: "ignore", "warn", or "hide"
    /// * `labeler_did` - Optional labeler DID for labeler-specific preferences
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// agent.login("alice.bsky.social", "password").await?;
    ///
    /// // Set global preference
    /// agent.set_content_label_pref("porn", "hide", None).await?;
    ///
    /// // Set labeler-specific preference
    /// agent.set_content_label_pref("spam", "warn", Some("did:plc:labeler123")).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn set_content_label_pref(
        &self,
        label: &str,
        visibility: &str,
        labeler_did: Option<&str>,
    ) -> Result<(), AgentError> {
        // Validate labeler DID if provided
        if let Some(did) = labeler_did {
            crate::syntax::ensure_valid_did(did)
                .map_err(|e| AgentError::InvalidDid(e.to_string()))?;
        }

        self.update_preferences(|mut prefs| {
            // Find or create content label pref
            let mut found = false;
            for pref in &mut prefs {
                if pref.get("$type").and_then(|v| v.as_str()) == Some("app.bsky.actor.defs#contentLabelPref") {
                    let matches_label = pref.get("label").and_then(|v| v.as_str()) == Some(label);
                    let matches_labeler = pref.get("labelerDid").and_then(|v| v.as_str()) == labeler_did;

                    if matches_label && matches_labeler {
                        if let Some(obj) = pref.as_object_mut() {
                            obj.insert("visibility".to_string(), serde_json::Value::String(visibility.to_string()));
                            found = true;
                            break;
                        }
                    }
                }
            }

            if !found {
                let mut new_pref = serde_json::json!({
                    "$type": "app.bsky.actor.defs#contentLabelPref",
                    "label": label,
                    "visibility": visibility
                });

                if let Some(did) = labeler_did {
                    new_pref["labelerDid"] = serde_json::Value::String(did.to_string());
                }

                prefs.push(new_pref);
            }

            // Handle legacy label mapping (write both new and old)
            if labeler_did.is_none() {
                let legacy_map: std::collections::HashMap<&str, &str> = [
                    ("graphic-media", "gore"),
                    ("porn", "nsfw"),
                    ("sexual", "suggestive"),
                ].iter().cloned().collect();

                if let Some(&legacy_label) = legacy_map.get(label) {
                    // Write legacy label too
                    let mut legacy_found = false;
                    for pref in &mut prefs {
                        if pref.get("$type").and_then(|v| v.as_str()) == Some("app.bsky.actor.defs#contentLabelPref") {
                            let matches_label = pref.get("label").and_then(|v| v.as_str()) == Some(legacy_label);
                            let is_global = pref.get("labelerDid").is_none();

                            if matches_label && is_global {
                                if let Some(obj) = pref.as_object_mut() {
                                    obj.insert("visibility".to_string(), serde_json::Value::String(visibility.to_string()));
                                    legacy_found = true;
                                    break;
                                }
                            }
                        }
                    }

                    if !legacy_found {
                        prefs.push(serde_json::json!({
                            "$type": "app.bsky.actor.defs#contentLabelPref",
                            "label": legacy_label,
                            "visibility": visibility
                        }));
                    }
                }
            }

            prefs
        }).await?;

        Ok(())
    }

    /// Add a labeler to user preferences
    ///
    /// # Arguments
    ///
    /// * `did` - The DID of the labeler to add
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// agent.login("alice.bsky.social", "password").await?;
    ///
    /// agent.add_labeler("did:plc:labeler123").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn add_labeler(&self, did: &str) -> Result<(), AgentError> {
        crate::syntax::ensure_valid_did(did)
            .map_err(|e| AgentError::InvalidDid(e.to_string()))?;

        let prefs = self.update_preferences(|mut prefs| {
            // Find or create labelers pref
            let mut found_pref = false;
            for pref in &mut prefs {
                if pref.get("$type").and_then(|v| v.as_str()) == Some("app.bsky.actor.defs#labelersPref") {
                    if let Some(obj) = pref.as_object_mut() {
                        let mut labelers = obj.get("labelers")
                            .and_then(|v| v.as_array())
                            .cloned()
                            .unwrap_or_default();

                        // Check if labeler already exists
                        let already_exists = labelers.iter().any(|l| {
                            l.get("did").and_then(|v| v.as_str()) == Some(did)
                        });

                        if !already_exists {
                            labelers.push(serde_json::json!({ "did": did }));
                            obj.insert("labelers".to_string(), serde_json::Value::Array(labelers));
                        }

                        found_pref = true;
                        break;
                    }
                }
            }

            if !found_pref {
                prefs.push(serde_json::json!({
                    "$type": "app.bsky.actor.defs#labelersPref",
                    "labelers": [{ "did": did }]
                }));
            }

            prefs
        }).await?;

        // Auto-configure labelers from updated preferences
        self.configure_labelers_from_prefs(&prefs);

        Ok(())
    }

    /// Remove a labeler from user preferences
    ///
    /// # Arguments
    ///
    /// * `did` - The DID of the labeler to remove
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// agent.login("alice.bsky.social", "password").await?;
    ///
    /// agent.remove_labeler("did:plc:labeler123").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn remove_labeler(&self, did: &str) -> Result<(), AgentError> {
        let prefs = self.update_preferences(|mut prefs| {
            for pref in &mut prefs {
                if pref.get("$type").and_then(|v| v.as_str()) == Some("app.bsky.actor.defs#labelersPref") {
                    if let Some(obj) = pref.as_object_mut() {
                        let labelers = obj.get("labelers")
                            .and_then(|v| v.as_array())
                            .cloned()
                            .unwrap_or_default();

                        let filtered: Vec<_> = labelers.into_iter()
                            .filter(|l| l.get("did").and_then(|v| v.as_str()) != Some(did))
                            .collect();

                        obj.insert("labelers".to_string(), serde_json::Value::Array(filtered));
                        break;
                    }
                }
            }

            prefs
        }).await?;

        // Auto-configure labelers from updated preferences
        self.configure_labelers_from_prefs(&prefs);

        Ok(())
    }

    /// Internal helper to configure labelers from preferences array
    fn configure_labelers_from_prefs(&self, prefs: &[serde_json::Value]) {
        let mut labeler_dids = Vec::new();

        for pref in prefs {
            if pref.get("$type").and_then(|v| v.as_str()) == Some("app.bsky.actor.defs#labelersPref") {
                if let Some(labelers) = pref.get("labelers").and_then(|v| v.as_array()) {
                    for labeler in labelers {
                        if let Some(did) = labeler.get("did").and_then(|v| v.as_str()) {
                            labeler_dids.push(did.to_string());
                        }
                    }
                }
            }
        }

        self.configure_labelers(labeler_dids);
    }

    /// Add a muted word
    ///
    /// # Arguments
    ///
    /// * `value` - The word or phrase to mute
    /// * `targets` - Where to apply: "content" and/or "tag"
    /// * `actor_target` - Who to mute from: "all" or "exclude-following"
    /// * `expires_at` - Optional expiration time
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// agent.login("alice.bsky.social", "password").await?;
    ///
    /// agent.add_muted_word("spam", vec!["content".to_string()], "all", None).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn add_muted_word(
        &self,
        value: &str,
        targets: Vec<String>,
        actor_target: &str,
        expires_at: Option<String>,
    ) -> Result<(), AgentError> {
        let sanitized = crate::util::sanitize_muted_word_value(value);
        if sanitized.is_empty() {
            return Ok(());
        }

        self.update_preferences(move |mut prefs| {
            let new_word = serde_json::json!({
                "id": crate::tid::Tid::next().unwrap().to_string(),
                "value": sanitized.clone(),
                "targets": targets.clone(),
                "actorTarget": actor_target,
                "expiresAt": expires_at.clone()
            });

            // Find or create muted words pref
            let mut found = false;
            for pref in &mut prefs {
                if pref.get("$type").and_then(|v| v.as_str()) == Some("app.bsky.actor.defs#mutedWordsPref") {
                    if let Some(obj) = pref.as_object_mut() {
                        let mut items = obj.get("items")
                            .and_then(|v| v.as_array())
                            .cloned()
                            .unwrap_or_default();

                        items.push(new_word.clone());
                        obj.insert("items".to_string(), serde_json::Value::Array(items));
                        found = true;
                        break;
                    }
                }
            }

            if !found {
                prefs.push(serde_json::json!({
                    "$type": "app.bsky.actor.defs#mutedWordsPref",
                    "items": [new_word]
                }));
            }

            prefs
        }).await?;

        Ok(())
    }

    /// Update an existing muted word
    ///
    /// # Arguments
    ///
    /// * `id` - The ID of the muted word to update
    /// * `value` - New value (word/phrase)
    /// * `targets` - New targets
    /// * `actor_target` - New actor target
    /// * `expires_at` - New expiration
    pub async fn update_muted_word(
        &self,
        id: &str,
        value: &str,
        targets: Vec<String>,
        actor_target: &str,
        expires_at: Option<String>,
    ) -> Result<(), AgentError> {
        let sanitized = crate::util::sanitize_muted_word_value(value);
        if sanitized.is_empty() {
            return Err(AgentError::SessionError("Muted word value cannot be empty".to_string()));
        }

        self.update_preferences(|mut prefs| {
            for pref in &mut prefs {
                if pref.get("$type").and_then(|v| v.as_str()) == Some("app.bsky.actor.defs#mutedWordsPref") {
                    if let Some(obj) = pref.as_object_mut() {
                        let items = obj.get("items")
                            .and_then(|v| v.as_array())
                            .cloned()
                            .unwrap_or_default();

                        let updated: Vec<_> = items.into_iter().map(|mut item| {
                            if item.get("id").and_then(|v| v.as_str()) == Some(id) {
                                // Update this item
                                if let Some(obj) = item.as_object_mut() {
                                    obj.insert("value".to_string(), serde_json::Value::String(sanitized.clone()));
                                    obj.insert("targets".to_string(), serde_json::to_value(&targets).unwrap());
                                    obj.insert("actorTarget".to_string(), serde_json::Value::String(actor_target.to_string()));
                                    if let Some(exp) = &expires_at {
                                        obj.insert("expiresAt".to_string(), serde_json::Value::String(exp.clone()));
                                    } else {
                                        obj.remove("expiresAt");
                                    }
                                }
                            }
                            item
                        }).collect();

                        obj.insert("items".to_string(), serde_json::Value::Array(updated));
                        break;
                    }
                }
            }

            prefs
        }).await?;

        Ok(())
    }

    /// Remove a muted word
    ///
    /// # Arguments
    ///
    /// * `id` - The ID of the muted word to remove
    pub async fn remove_muted_word(&self, id: &str) -> Result<(), AgentError> {
        self.update_preferences(|mut prefs| {
            for pref in &mut prefs {
                if pref.get("$type").and_then(|v| v.as_str()) == Some("app.bsky.actor.defs#mutedWordsPref") {
                    if let Some(obj) = pref.as_object_mut() {
                        let items = obj.get("items")
                            .and_then(|v| v.as_array())
                            .cloned()
                            .unwrap_or_default();

                        let filtered: Vec<_> = items.into_iter()
                            .filter(|item| item.get("id").and_then(|v| v.as_str()) != Some(id))
                            .collect();

                        obj.insert("items".to_string(), serde_json::Value::Array(filtered));
                        break;
                    }
                }
            }

            prefs
        }).await?;

        Ok(())
    }

    /// Update saved feeds by ID
    ///
    /// Updates existing saved feeds based on their IDs. This allows you to reorder
    /// or modify saved feeds while preserving their IDs.
    ///
    /// # Arguments
    ///
    /// * `saved_feeds` - List of saved feed configurations with IDs
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// // ... authenticate ...
    ///
    /// let feeds = vec![
    ///     serde_json::json!({
    ///         "id": "feed-123",
    ///         "type": "timeline",
    ///         "value": "at://did:plc:xyz/app.bsky.feed.generator/my-feed",
    ///         "pinned": true
    ///     })
    /// ];
    /// agent.update_saved_feeds(feeds).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn update_saved_feeds(&self, saved_feeds: Vec<serde_json::Value>) -> Result<(), AgentError> {
        // Convert JSON values to SavedFeed type
        let mut typed_feeds = Vec::new();
        for feed in saved_feeds {
            let typed_feed: crate::client::app::bsky::actor::defs::SavedFeed = serde_json::from_value(feed)
                .map_err(|e| AgentError::SessionError(format!("Invalid saved feed format: {}", e)))?;
            typed_feeds.push(typed_feed);
        }

        self.overwrite_saved_feeds(typed_feeds).await
    }

    /// Add saved feeds
    ///
    /// Adds new feeds to the saved feeds list. Automatically generates unique IDs
    /// for new feeds.
    ///
    /// # Arguments
    ///
    /// * `feeds` - List of feed configurations to add
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// // ... authenticate ...
    ///
    /// let new_feeds = vec![
    ///     serde_json::json!({
    ///         "type": "feed",
    ///         "value": "at://did:plc:abc/app.bsky.feed.generator/tech-news",
    ///         "pinned": true
    ///     })
    /// ];
    /// agent.add_saved_feeds(new_feeds).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn add_saved_feeds(&self, feeds: Vec<serde_json::Value>) -> Result<(), AgentError> {
        self.update_preferences(|mut prefs| {
            // Find or create saved feeds preference
            let mut saved_feeds_pref = None;
            for (idx, pref) in prefs.iter().enumerate() {
                if pref.get("$type").and_then(|v| v.as_str()) == Some("app.bsky.actor.defs#savedFeedsPref") {
                    saved_feeds_pref = Some(idx);
                    break;
                }
            }

            let pref_idx = if let Some(idx) = saved_feeds_pref {
                idx
            } else {
                prefs.push(serde_json::json!({
                    "$type": "app.bsky.actor.defs#savedFeedsPref",
                    "saved": [],
                    "pinned": []
                }));
                prefs.len() - 1
            };

            // Get current saved feeds
            let mut current_saved = prefs[pref_idx].get("saved")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            // Add new feeds with generated IDs
            for feed in feeds {
                let mut new_feed = feed.clone();
                if let Some(obj) = new_feed.as_object_mut() {
                    // Generate unique ID if not present
                    if !obj.contains_key("id") {
                        let timestamp = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis();
                        let random_part: u32 = rand::random();
                        obj.insert("id".to_string(), serde_json::Value::String(
                            format!("feed-{}-{}", timestamp, random_part)
                        ));
                    }
                }
                current_saved.push(new_feed);
            }

            // Update the preference
            if let Some(obj) = prefs[pref_idx].as_object_mut() {
                obj.insert("saved".to_string(), serde_json::Value::Array(current_saved));
            }

            prefs
        }).await?;

        Ok(())
    }

    /// Remove saved feeds by ID
    ///
    /// Removes feeds from the saved feeds list based on their IDs.
    ///
    /// # Arguments
    ///
    /// * `feed_ids` - List of feed IDs to remove
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// // ... authenticate ...
    ///
    /// agent.remove_saved_feeds(vec!["feed-123".to_string(), "feed-456".to_string()]).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn remove_saved_feeds(&self, feed_ids: Vec<String>) -> Result<(), AgentError> {
        self.update_preferences(|mut prefs| {
            for pref in &mut prefs {
                if pref.get("$type").and_then(|v| v.as_str()) == Some("app.bsky.actor.defs#savedFeedsPref") {
                    if let Some(obj) = pref.as_object_mut() {
                        // Filter out feeds with matching IDs
                        let saved = obj.get("saved")
                            .and_then(|v| v.as_array())
                            .cloned()
                            .unwrap_or_default();

                        let filtered: Vec<_> = saved.into_iter()
                            .filter(|feed| {
                                let id = feed.get("id").and_then(|v| v.as_str()).unwrap_or("");
                                !feed_ids.contains(&id.to_string())
                            })
                            .collect();

                        obj.insert("saved".to_string(), serde_json::Value::Array(filtered));

                        // Also remove from pinned if present
                        let pinned = obj.get("pinned")
                            .and_then(|v| v.as_array())
                            .cloned()
                            .unwrap_or_default();

                        let filtered_pinned: Vec<_> = pinned.into_iter()
                            .filter(|uri| !feed_ids.iter().any(|id| uri.as_str().is_some_and(|s| s.contains(id))))
                            .collect();

                        obj.insert("pinned".to_string(), serde_json::Value::Array(filtered_pinned));
                        break;
                    }
                }
            }

            prefs
        }).await?;

        Ok(())
    }

    /// Set feed view preferences for a specific feed
    ///
    /// Configures view preferences (hide replies, hide reposts, etc.) for a specific feed.
    ///
    /// # Arguments
    ///
    /// * `feed` - The feed URI
    /// * `prefs` - Feed view preferences to set
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// // ... authenticate ...
    ///
    /// let prefs = serde_json::json!({
    ///     "hideReplies": true,
    ///     "hideReposts": false,
    ///     "hideQuotePosts": false
    /// });
    /// agent.set_feed_view_prefs("at://did:plc:xyz/app.bsky.feed.generator/my-feed", prefs).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn set_feed_view_prefs(&self, feed: &str, feed_prefs: serde_json::Value) -> Result<(), AgentError> {
        self.update_preferences(|mut prefs| {
            // Find existing preference for this feed
            let mut found = false;
            for pref in &mut prefs {
                if pref.get("$type").and_then(|v| v.as_str()) == Some("app.bsky.actor.defs#feedViewPref")
                    && pref.get("feed").and_then(|v| v.as_str()) == Some(feed) {
                    // Merge preferences
                    if let Some(obj) = pref.as_object_mut() {
                        if let Some(new_prefs) = feed_prefs.as_object() {
                            for (key, value) in new_prefs {
                                obj.insert(key.clone(), value.clone());
                            }
                        }
                    }
                    found = true;
                    break;
                }
            }

            // If not found, create new preference
            if !found {
                let mut new_pref = serde_json::json!({
                    "$type": "app.bsky.actor.defs#feedViewPref",
                    "feed": feed
                });
                if let Some(obj) = new_pref.as_object_mut() {
                    if let Some(new_prefs) = feed_prefs.as_object() {
                        for (key, value) in new_prefs {
                            obj.insert(key.clone(), value.clone());
                        }
                    }
                }
                prefs.push(new_pref);
            }

            prefs
        }).await?;

        Ok(())
    }

    /// Set thread view preferences
    ///
    /// Configures global thread view preferences (sort order, prioritize followed users, etc.).
    ///
    /// # Arguments
    ///
    /// * `prefs` - Thread view preferences to set
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// // ... authenticate ...
    ///
    /// let prefs = serde_json::json!({
    ///     "sort": "oldest",
    ///     "prioritizeFollowedUsers": true
    /// });
    /// agent.set_thread_view_prefs(prefs).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn set_thread_view_prefs(&self, thread_prefs: serde_json::Value) -> Result<(), AgentError> {
        self.update_preferences(|mut prefs| {
            // Find existing thread view preference
            let mut found = false;
            for pref in &mut prefs {
                if pref.get("$type").and_then(|v| v.as_str()) == Some("app.bsky.actor.defs#threadViewPref") {
                    // Merge preferences
                    if let Some(obj) = pref.as_object_mut() {
                        if let Some(new_prefs) = thread_prefs.as_object() {
                            for (key, value) in new_prefs {
                                obj.insert(key.clone(), value.clone());
                            }
                        }
                    }
                    found = true;
                    break;
                }
            }

            // If not found, create new preference
            if !found {
                let mut new_pref = serde_json::json!({
                    "$type": "app.bsky.actor.defs#threadViewPref"
                });
                if let Some(obj) = new_pref.as_object_mut() {
                    if let Some(new_prefs) = thread_prefs.as_object() {
                        for (key, value) in new_prefs {
                            obj.insert(key.clone(), value.clone());
                        }
                    }
                }
                prefs.push(new_pref);
            }

            prefs
        }).await?;

        Ok(())
    }

    /// Set interests preference
    ///
    /// Sets the user's interest tags.
    ///
    /// # Arguments
    ///
    /// * `tags` - List of interest tags
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// // ... authenticate ...
    ///
    /// agent.set_interests_pref(vec!["technology".to_string(), "science".to_string()]).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn set_interests_pref(&self, tags: Vec<String>) -> Result<(), AgentError> {
        self.update_preferences(|mut prefs| {
            // Find existing interests preference
            let mut found = false;
            for pref in &mut prefs {
                if pref.get("$type").and_then(|v| v.as_str()) == Some("app.bsky.actor.defs#interestsPref") {
                    if let Some(obj) = pref.as_object_mut() {
                        obj.insert("tags".to_string(), serde_json::to_value(&tags).unwrap_or_default());
                    }
                    found = true;
                    break;
                }
            }

            // If not found, create new preference
            if !found {
                prefs.push(serde_json::json!({
                    "$type": "app.bsky.actor.defs#interestsPref",
                    "tags": tags
                }));
            }

            prefs
        }).await?;

        Ok(())
    }

    /// Set personal details
    ///
    /// Sets personal details like birth date.
    ///
    /// # Arguments
    ///
    /// * `birth_date` - Birth date in ISO 8601 format (YYYY-MM-DD)
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// // ... authenticate ...
    ///
    /// agent.set_personal_details("1990-01-15".to_string()).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn set_personal_details(&self, birth_date: String) -> Result<(), AgentError> {
        self.update_preferences(|mut prefs| {
            // Find existing personal details preference
            let mut found = false;
            for pref in &mut prefs {
                if pref.get("$type").and_then(|v| v.as_str()) == Some("app.bsky.actor.defs#personalDetailsPref") {
                    if let Some(obj) = pref.as_object_mut() {
                        obj.insert("birthDate".to_string(), serde_json::Value::String(birth_date.clone()));
                    }
                    found = true;
                    break;
                }
            }

            // If not found, create new preference
            if !found {
                prefs.push(serde_json::json!({
                    "$type": "app.bsky.actor.defs#personalDetailsPref",
                    "birthDate": birth_date
                }));
            }

            prefs
        }).await?;

        Ok(())
    }

    /// Update user profile with retry logic for conflicts
    ///
    /// Updates the user's profile record. Automatically retries on version conflicts
    /// (InvalidSwapError) by fetching the latest version and re-applying changes.
    ///
    /// # Arguments
    ///
    /// * `updates` - Profile fields to update (displayName, description, avatar, banner, etc.)
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// // ... authenticate ...
    ///
    /// let updates = serde_json::json!({
    ///     "displayName": "New Display Name",
    ///     "description": "Software developer interested in Rust and decentralized systems"
    /// });
    /// agent.upsert_profile(updates).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn upsert_profile(&self, updates: serde_json::Value) -> Result<(), AgentError> {
        use crate::client::com::atproto::repo::{get_record, put_record};

        const MAX_RETRIES: u32 = 5;

        for attempt in 0..MAX_RETRIES {
            // Get current DID
            let did = self.did().ok_or(AgentError::NotAuthenticated)?;

            // Fetch current profile
            let params = get_record::QueryParams {
                repo: did.clone(),
                collection: "app.bsky.actor.profile".to_string(),
                rkey: "self".to_string(),
                cid: None,
            };

            let profile_result = get_record::get_record(&*self.client, params).await;

            let (current_profile, swap_record) = match profile_result {
                Ok(response) => {
                    let value = response.data.value.clone();
                    let cid = response.data.cid.clone();
                    (value, cid)
                }
                Err(_) => {
                    // Profile doesn't exist, create new one
                    (serde_json::json!({"$type": "app.bsky.actor.profile"}), None)
                }
            };

            // Merge updates into current profile
            let mut merged_profile = current_profile.clone();
            if let Some(obj) = merged_profile.as_object_mut() {
                if let Some(updates_obj) = updates.as_object() {
                    for (key, value) in updates_obj {
                        obj.insert(key.clone(), value.clone());
                    }
                }
                // Ensure $type is set
                obj.insert("$type".to_string(), serde_json::Value::String("app.bsky.actor.profile".to_string()));
            }

            // Attempt to put record
            let put_input = put_record::Input {
                repo: did.clone(),
                collection: "app.bsky.actor.profile".to_string(),
                rkey: "self".to_string(),
                record: merged_profile,
                swap_record,
                swap_commit: None,
                validate: Some(true),
            };

            let put_result = put_record::put_record(&*self.client, put_input).await;

            match put_result {
                Ok(_) => return Ok(()),
                Err(e) => {
                    // Check if it's a swap error (version conflict)
                    let error_msg = format!("{:?}", e);
                    if error_msg.contains("InvalidSwap") && attempt < MAX_RETRIES - 1 {
                        // Retry with exponential backoff
                        let delay = std::time::Duration::from_millis(100 * 2u64.pow(attempt));
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    return Err(AgentError::XrpcError(e));
                }
            }
        }

        Err(AgentError::SessionError(format!("Failed to update profile after {} retries", MAX_RETRIES)))
    }

    /// Hide a post
    ///
    /// Adds a post URI to the hidden posts list.
    ///
    /// # Arguments
    ///
    /// * `post_uri` - The AT URI of the post to hide
    pub async fn hide_post(&self, post_uri: &str) -> Result<(), AgentError> {
        self.update_hidden_post(post_uri, true).await
    }

    /// Unhide a post
    ///
    /// Removes a post URI from the hidden posts list.
    ///
    /// # Arguments
    ///
    /// * `post_uri` - The AT URI of the post to unhide
    pub async fn unhide_post(&self, post_uri: &str) -> Result<(), AgentError> {
        self.update_hidden_post(post_uri, false).await
    }

    // ============================================================================
    // Moderation Actions
    // ============================================================================

    /// Mute an actor
    ///
    /// Creates a mute relationship for the specified account. Mutes are private.
    ///
    /// # Arguments
    ///
    /// * `actor` - Handle or DID of the actor to mute
    ///
    /// # Returns
    ///
    /// Returns Ok if successful
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// agent.login("alice.bsky.social", "app-password").await?;
    ///
    /// agent.mute("spammer.bsky.social").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn mute(&self, actor: &str) -> Result<(), AgentError> {
        use crate::client::app::bsky::graph::mute_actor;

        self.assert_did()?;

        let input = mute_actor::Input {
            actor: actor.to_string(),
        };

        mute_actor::mute_actor(&*self.client, input).await?;

        Ok(())
    }

    /// Unmute an actor
    ///
    /// Removes the mute relationship for the specified account.
    ///
    /// # Arguments
    ///
    /// * `actor` - Handle or DID of the actor to unmute
    ///
    /// # Returns
    ///
    /// Returns Ok if successful
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// agent.login("alice.bsky.social", "app-password").await?;
    ///
    /// agent.unmute("someone.bsky.social").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn unmute(&self, actor: &str) -> Result<(), AgentError> {
        use crate::client::app::bsky::graph::unmute_actor;

        self.assert_did()?;

        let input = unmute_actor::Input {
            actor: actor.to_string(),
        };

        unmute_actor::unmute_actor(&*self.client, input).await?;

        Ok(())
    }

    /// Mute a moderation list
    ///
    /// Creates a mute relationship for all accounts in the specified list.
    ///
    /// # Arguments
    ///
    /// * `list_uri` - AT-URI of the moderation list to mute
    ///
    /// # Returns
    ///
    /// Returns Ok if successful
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// agent.login("alice.bsky.social", "app-password").await?;
    ///
    /// agent.mute_mod_list("at://did:plc:xxx/app.bsky.graph.list/yyy").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn mute_mod_list(&self, list_uri: &str) -> Result<(), AgentError> {
        use crate::client::app::bsky::graph::mute_actor_list;
        use crate::syntax::AtUri;

        self.assert_did()?;

        let input = mute_actor_list::Input {
            list: AtUri::try_from(list_uri.to_string())
                .map_err(|e| AgentError::SessionError(format!("Invalid list URI: {}", e)))?,
        };

        mute_actor_list::mute_actor_list(&*self.client, input).await?;

        Ok(())
    }

    /// Unmute a moderation list
    ///
    /// Removes the mute relationship for all accounts in the specified list.
    ///
    /// # Arguments
    ///
    /// * `list_uri` - AT-URI of the moderation list to unmute
    ///
    /// # Returns
    ///
    /// Returns Ok if successful
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// agent.login("alice.bsky.social", "app-password").await?;
    ///
    /// agent.unmute_mod_list("at://did:plc:xxx/app.bsky.graph.list/yyy").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn unmute_mod_list(&self, list_uri: &str) -> Result<(), AgentError> {
        use crate::client::app::bsky::graph::unmute_actor_list;
        use crate::syntax::AtUri;

        self.assert_did()?;

        let input = unmute_actor_list::Input {
            list: AtUri::try_from(list_uri.to_string())
                .map_err(|e| AgentError::SessionError(format!("Invalid list URI: {}", e)))?,
        };

        unmute_actor_list::unmute_actor_list(&*self.client, input).await?;

        Ok(())
    }

    /// Block a moderation list
    ///
    /// Creates a block record for the specified list, blocking all accounts in it.
    ///
    /// # Arguments
    ///
    /// * `list_uri` - AT-URI of the moderation list to block
    ///
    /// # Returns
    ///
    /// Returns Ok if successful
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// agent.login("alice.bsky.social", "app-password").await?;
    ///
    /// agent.block_mod_list("at://did:plc:xxx/app.bsky.graph.list/yyy").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn block_mod_list(&self, list_uri: &str) -> Result<(), AgentError> {
        use crate::client::com::atproto::repo::create_record;

        let did = self.assert_did()?;

        // Create a listblock record
        let listblock = serde_json::json!({
            "$type": "app.bsky.graph.listblock",
            "subject": list_uri,
            "createdAt": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        });

        let input = create_record::Input {
            repo: did,
            collection: "app.bsky.graph.listblock".to_string(),
            rkey: None,
            validate: Some(true),
            record: listblock,
            swap_commit: None,
        };

        create_record::create_record(&*self.client, input).await?;

        Ok(())
    }

    /// Unblock a moderation list
    ///
    /// Removes the block record for the specified list.
    ///
    /// # Arguments
    ///
    /// * `list_uri` - AT-URI of the moderation list to unblock
    ///
    /// # Returns
    ///
    /// Returns Ok if successful
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// agent.login("alice.bsky.social", "app-password").await?;
    ///
    /// agent.unblock_mod_list("at://did:plc:xxx/app.bsky.graph.list/yyy").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn unblock_mod_list(&self, list_uri: &str) -> Result<(), AgentError> {
        use crate::client::app::bsky::graph::get_list;
        use crate::client::com::atproto::repo::delete_record;
        use crate::syntax::AtUri;

        let did = self.assert_did()?;

        // Get list info to find the block record
        let params = get_list::QueryParams {
            list: AtUri::try_from(list_uri.to_string())
                .map_err(|e| AgentError::SessionError(format!("Invalid list URI: {}", e)))?,
            limit: Some(1),
            cursor: None,
        };

        let response = get_list::get_list(&*self.client, params).await?;

        // Extract the blocked URI from viewer data
        if let Some(viewer) = response.data.list.get("viewer") {
            if let Some(blocked_uri) = viewer.get("blocked").and_then(|v| v.as_str()) {
                // Parse the blocked URI to get the rkey
                let blocked_at_uri = AtUri::try_from(blocked_uri.to_string())
                    .map_err(|e| AgentError::SessionError(format!("Invalid blocked URI: {}", e)))?;

                // Delete the listblock record
                let input = delete_record::Input {
                    repo: did,
                    collection: "app.bsky.graph.listblock".to_string(),
                    rkey: blocked_at_uri.rkey().ok_or_else(|| {
                        AgentError::SessionError("Blocked URI missing rkey".to_string())
                    })?.to_string(),
                    swap_record: None,
                    swap_commit: None,
                };

                delete_record::delete_record(&*self.client, input).await?;
            } else {
                return Err(AgentError::SessionError("List is not blocked".to_string()));
            }
        } else {
            return Err(AgentError::SessionError("Unable to retrieve list viewer data".to_string()));
        }

        Ok(())
    }

    /// Create a moderation report
    ///
    /// Submit a moderation report regarding an account or record.
    ///
    /// # Arguments
    ///
    /// * `subject` - The subject of the report (account DID or record URI)
    /// * `reason_type` - The type of violation (e.g., "com.atproto.moderation.defs#reasonSpam")
    /// * `reason` - Optional additional context about the violation
    ///
    /// # Returns
    ///
    /// Returns the report ID
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use atproto::agent::Agent;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::new("https://bsky.social".to_string());
    /// agent.login("alice.bsky.social", "app-password").await?;
    ///
    /// let report_id = agent.create_moderation_report(
    ///     serde_json::json!({
    ///         "$type": "com.atproto.admin.defs#repoRef",
    ///         "did": "did:plc:spammer123"
    ///     }),
    ///     "com.atproto.moderation.defs#reasonSpam",
    ///     Some("This account is posting spam".to_string()),
    /// ).await?;
    ///
    /// println!("Report created with ID: {}", report_id);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn create_moderation_report(
        &self,
        subject: serde_json::Value,
        reason_type: &str,
        reason: Option<String>,
    ) -> Result<i64, AgentError> {
        use crate::client::com::atproto::moderation::create_report;

        self.assert_did()?;

        let input = create_report::Input {
            subject,
            reason_type: serde_json::json!(reason_type),
            reason,
            mod_tool: None,
        };

        let response = create_report::create_report(&*self.client, input).await?;

        Ok(response.data.id)
    }

    /// Internal helper to hide/unhide a post
    async fn update_hidden_post(&self, post_uri: &str, hide: bool) -> Result<(), AgentError> {
        self.update_preferences(|mut prefs| {
            let mut found = false;
            for pref in &mut prefs {
                if pref.get("$type").and_then(|v| v.as_str()) == Some("app.bsky.actor.defs#hiddenPostsPref") {
                    if let Some(obj) = pref.as_object_mut() {
                        let items = obj.get("items")
                            .and_then(|v| v.as_array())
                            .cloned()
                            .unwrap_or_default();

                        let mut items_set: std::collections::HashSet<String> = items.into_iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect();

                        if hide {
                            items_set.insert(post_uri.to_string());
                        } else {
                            items_set.remove(post_uri);
                        }

                        let items_vec: Vec<serde_json::Value> = items_set.into_iter()
                            .map(serde_json::Value::String)
                            .collect();

                        obj.insert("items".to_string(), serde_json::Value::Array(items_vec));
                        found = true;
                        break;
                    }
                }
            }

            if !found && hide {
                prefs.push(serde_json::json!({
                    "$type": "app.bsky.actor.defs#hiddenPostsPref",
                    "items": [post_uri]
                }));
            }

            prefs
        }).await?;

        Ok(())
    }

    // ============================================================================
    // Internal Preferences Helpers
    // ============================================================================

    /// Internal: Atomically update preferences
    ///
    /// This acquires a lock to ensure thread-safe preference updates.
    async fn update_preferences<F>(&self, callback: F) -> Result<Vec<serde_json::Value>, AgentError>
    where
        F: FnOnce(Vec<serde_json::Value>) -> Vec<serde_json::Value>,
    {
        use crate::client::app::bsky::actor::{get_preferences, put_preferences};

        let _lock = self.prefs_lock.lock().await;

        // Get current preferences
        let response = get_preferences::get_preferences(&*self.client, get_preferences::QueryParams {}).await?;

        let prefs_vec = if let Some(arr) = response.data.preferences.as_array() {
            arr.clone()
        } else {
            vec![]
        };

        // Apply callback
        let new_prefs = callback(prefs_vec);

        // Save to server
        let input = put_preferences::Input {
            preferences: serde_json::Value::Array(new_prefs.clone()),
        };
        put_preferences::put_preferences(&*self.client, input).await?;

        Ok(new_prefs)
    }
}

// Implement Clone for Agent
impl Clone for Agent {
    fn clone(&self) -> Self {
        self.clone_agent()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_new() {
        let agent = Agent::new("https://bsky.social".to_string());
        assert_eq!(agent.service(), "https://bsky.social");
        assert!(!agent.is_authenticated());
        assert!(agent.did().is_none());
    }

    #[test]
    fn test_agent_assert_did_not_authenticated() {
        let agent = Agent::new("https://bsky.social".to_string());
        let result = agent.assert_did();
        assert!(matches!(result, Err(AgentError::NotAuthenticated)));
    }

    #[test]
    fn test_configure_labelers() {
        let agent = Agent::new("https://bsky.social".to_string());
        agent.configure_labelers(vec!["did:plc:test123".to_string()]);

        let labelers = agent.get_all_labelers();
        assert!(labelers.contains(&"did:plc:test123".to_string()));
    }

    #[test]
    fn test_configure_labelers_filters_invalid() {
        let agent = Agent::new("https://bsky.social".to_string());
        agent.configure_labelers(vec![
            "did:plc:valid123".to_string(),
            "not-a-did".to_string(),
        ]);

        let labelers = agent.labelers.read().unwrap();
        assert_eq!(labelers.len(), 1);
        assert_eq!(labelers[0], "did:plc:valid123");
    }

    #[test]
    fn test_app_labelers_default() {
        let agent = Agent::new("https://bsky.social".to_string());
        let labelers = agent.get_all_labelers();

        // Should include the default Bluesky labeler
        assert!(labelers.contains(&BSKY_LABELER_DID.to_string()));
    }

    #[test]
    fn test_configure_app_labelers() {
        let agent = Agent::new("https://bsky.social".to_string());
        agent.configure_app_labelers(vec!["did:plc:custom".to_string()]);

        let app_labelers = agent.app_labelers.read().unwrap();
        assert_eq!(app_labelers.len(), 1);
        assert_eq!(app_labelers[0], "did:plc:custom");
    }

    #[test]
    fn test_configure_proxy() {
        let agent = Agent::new("https://bsky.social".to_string());
        let did = Did::new("did:plc:test").unwrap();
        let service = AtprotoServiceType::new_unchecked("atproto_labeler");
        let proxy = AtprotoProxy::new(did, service);

        agent.configure_proxy(Some(proxy.clone()));
        assert_eq!(agent.get_proxy(), Some(proxy));
    }

    #[test]
    fn test_configure_proxy_none() {
        let agent = Agent::new("https://bsky.social".to_string());
        agent.configure_proxy(None);
        assert!(agent.get_proxy().is_none());
    }

    #[test]
    fn test_set_header() {
        let agent = Agent::new("https://bsky.social".to_string());
        agent.set_header("X-Custom".to_string(), "value".to_string());

        let headers = agent.get_headers();
        assert_eq!(headers.get("X-Custom"), Some(&"value".to_string()));
    }

    #[test]
    fn test_clear_header() {
        let agent = Agent::new("https://bsky.social".to_string());
        agent.set_header("X-Custom".to_string(), "value".to_string());
        agent.clear_header("X-Custom");

        let headers = agent.get_headers();
        assert!(headers.get("X-Custom").is_none());
    }

    #[test]
    fn test_clear_all_headers() {
        let agent = Agent::new("https://bsky.social".to_string());
        agent.set_header("X-Custom1".to_string(), "value1".to_string());
        agent.set_header("X-Custom2".to_string(), "value2".to_string());

        agent.clear_all_headers();

        let headers = agent.get_headers();
        assert!(headers.is_empty());
    }

    #[test]
    fn test_clone_agent() {
        let agent = Agent::new("https://bsky.social".to_string());
        agent.configure_labelers(vec!["did:plc:test".to_string()]);
        agent.set_header("X-Test".to_string(), "value".to_string());

        let cloned = agent.clone_agent();
        assert_eq!(cloned.service(), agent.service());

        let cloned_headers = cloned.get_headers();
        assert_eq!(cloned_headers.get("X-Test"), Some(&"value".to_string()));
    }

    #[test]
    fn test_with_proxy() {
        let agent = Agent::new("https://bsky.social".to_string());
        let service = AtprotoServiceType::new_unchecked("atproto_labeler");
        let proxied = agent.with_proxy(service, "did:plc:labeler123".to_string());

        assert!(proxied.get_proxy().is_some());
        // Original should not be modified
        assert!(agent.get_proxy().is_none());
    }

    #[test]
    fn test_build_request_headers_with_labelers() {
        let agent = Agent::new("https://bsky.social".to_string());
        agent.configure_labelers(vec!["did:plc:test1".to_string()]);

        let headers = agent.build_request_headers();
        let labelers_header = headers.get("atproto-accept-labelers").unwrap();

        assert!(labelers_header.contains("did:plc:test1;redact"));
    }

    #[test]
    fn test_build_request_headers_with_proxy() {
        let agent = Agent::new("https://bsky.social".to_string());
        let did = Did::new("did:plc:test").unwrap();
        let service = AtprotoServiceType::new_unchecked("atproto_labeler");
        let proxy = AtprotoProxy::new(did, service);
        agent.configure_proxy(Some(proxy));

        let headers = agent.build_request_headers();
        assert!(headers.contains_key("atproto-proxy"));
    }

    #[test]
    fn test_namespace_accessors() {
        let agent = Agent::new("https://bsky.social".to_string());

        // Test that namespace accessors return valid objects
        let _com = agent.com();
        let _app = agent.app();
        let _chat = agent.chat();
        let _tools = agent.tools();
        let _xrpc = agent.xrpc();

        // Verify we can chain namespace calls
        let _server_ns = agent.com().atproto().server();
        let _feed_ns = agent.app().bsky().feed();
        let _convo_ns = agent.chat().bsky().convo();
        let _moderation_ns = agent.tools().ozone().moderation();
    }

    #[test]
    fn test_session_data_management() {
        let agent = Agent::new("https://bsky.social".to_string());

        // Initially no session
        assert!(!agent.is_authenticated());
        assert!(agent.did().is_none());

        // Mock session data
        let session_data = AtpSessionData {
            did: "did:plc:test123".to_string(),
            handle: "test.bsky.social".to_string(),
            email: Some("test@example.com".to_string()),
            email_confirmed: Some(true),
            email_auth_factor: None,
            access_jwt: "access_token".to_string(),
            refresh_jwt: "refresh_token".to_string(),
            active: true,
            status: None,
        };

        // Store session
        {
            let mut session = agent.session_data.write().unwrap();
            *session = Some(session_data);
        }

        // Verify session is accessible
        assert!(agent.is_authenticated());
        assert_eq!(agent.did(), Some("did:plc:test123".to_string()));

        // Clear session
        {
            let mut session = agent.session_data.write().unwrap();
            *session = None;
        }

        // Verify session is cleared
        assert!(!agent.is_authenticated());
        assert!(agent.did().is_none());
    }

    #[test]
    fn test_assert_did_when_authenticated() {
        let agent = Agent::new("https://bsky.social".to_string());

        // Mock session data
        let session_data = AtpSessionData {
            did: "did:plc:test123".to_string(),
            handle: "test.bsky.social".to_string(),
            email: None,
            email_confirmed: None,
            email_auth_factor: None,
            access_jwt: "access_token".to_string(),
            refresh_jwt: "refresh_token".to_string(),
            active: true,
            status: None,
        };

        {
            let mut session = agent.session_data.write().unwrap();
            *session = Some(session_data);
        }

        // assert_did should return DID
        let did = agent.assert_did();
        assert!(did.is_ok());
        assert_eq!(did.unwrap(), "did:plc:test123");
    }

    #[test]
    fn test_clone_agent_preserves_session() {
        let agent = Agent::new("https://bsky.social".to_string());

        // Add custom headers
        agent.set_header("X-Test".to_string(), "value".to_string());

        // Add labelers
        agent.configure_labelers(vec!["did:plc:labeler1".to_string()]);

        // Clone agent
        let cloned = agent.clone_agent();

        // Verify configuration copied
        assert_eq!(cloned.service(), agent.service());
        let headers = cloned.get_headers();
        assert_eq!(headers.get("X-Test"), Some(&"value".to_string()));

        // Note: session_data is not deep copied in clone_agent,
        // which is correct as it's shared via Arc
    }

    #[test]
    fn test_reply_ref_serialization() {
        use crate::client::app::bsky::feed::post::ReplyRef;
        use crate::client::com::atproto::repo::strong_ref::Main as StrongRef;

        let parent_root_ref = StrongRef {
            uri: crate::syntax::AtUri::try_from("at://did:plc:abc123/app.bsky.feed.post/xyz789").unwrap(),
            cid: "bafyreiabc123".to_string(),
        };

        let reply_ref = ReplyRef {
            parent: serde_json::to_value(&parent_root_ref).unwrap(),
            root: serde_json::to_value(&parent_root_ref).unwrap(),
        };

        let json = serde_json::to_value(&reply_ref).unwrap();

        assert_eq!(
            json["parent"]["uri"],
            "at://did:plc:abc123/app.bsky.feed.post/xyz789"
        );
        assert_eq!(json["parent"]["cid"], "bafyreiabc123");
        assert_eq!(
            json["root"]["uri"],
            "at://did:plc:abc123/app.bsky.feed.post/xyz789"
        );
        assert_eq!(json["root"]["cid"], "bafyreiabc123");
    }

    #[test]
    fn test_reply_ref_deserialization() {
        use crate::client::app::bsky::feed::post::ReplyRef;

        let json = serde_json::json!({
            "parent": {
                "uri": "at://did:plc:parent/app.bsky.feed.post/123",
                "cid": "bafyparent"
            },
            "root": {
                "uri": "at://did:plc:root/app.bsky.feed.post/456",
                "cid": "bafyroot"
            }
        });

        let reply_ref: ReplyRef = serde_json::from_value(json).unwrap();

        assert_eq!(
            reply_ref.parent["uri"],
            "at://did:plc:parent/app.bsky.feed.post/123"
        );
        assert_eq!(reply_ref.parent["cid"], "bafyparent");
        assert_eq!(
            reply_ref.root["uri"],
            "at://did:plc:root/app.bsky.feed.post/456"
        );
        assert_eq!(reply_ref.root["cid"], "bafyroot");
    }

    #[test]
    fn test_strong_ref_serialization() {
        use crate::client::com::atproto::repo::strong_ref::Main as StrongRef;

        let strong_ref = StrongRef {
            uri: crate::syntax::AtUri::try_from("at://did:plc:test/app.bsky.feed.post/test123").unwrap(),
            cid: "bafytest456".to_string(),
        };

        let json = serde_json::to_value(&strong_ref).unwrap();

        assert_eq!(json["uri"], "at://did:plc:test/app.bsky.feed.post/test123");
        assert_eq!(json["cid"], "bafytest456");
    }

    #[test]
    fn test_strong_ref_deserialization() {
        use crate::client::com::atproto::repo::strong_ref::Main as StrongRef;

        let json = serde_json::json!({
            "uri": "at://did:plc:example/app.bsky.feed.post/abc",
            "cid": "bafyexample"
        });

        let strong_ref: StrongRef = serde_json::from_value(json).unwrap();

        assert_eq!(strong_ref.uri.as_str(), "at://did:plc:example/app.bsky.feed.post/abc");
        assert_eq!(strong_ref.cid, "bafyexample");
    }

    #[test]
    fn test_reply_ref_different_parent_and_root() {
        use crate::client::app::bsky::feed::post::ReplyRef;
        use crate::client::com::atproto::repo::strong_ref::Main as StrongRef;

        // Test case: replying to a reply (parent != root)
        let parent_ref = StrongRef {
            uri: crate::syntax::AtUri::try_from("at://did:plc:user1/app.bsky.feed.post/reply1").unwrap(),
            cid: "bafyparent".to_string(),
        };
        let root_ref = StrongRef {
            uri: crate::syntax::AtUri::try_from("at://did:plc:user2/app.bsky.feed.post/original").unwrap(),
            cid: "bafyroot".to_string(),
        };

        let reply_ref = ReplyRef {
            parent: serde_json::to_value(&parent_ref).unwrap(),
            root: serde_json::to_value(&root_ref).unwrap(),
        };

        let json = serde_json::to_value(&reply_ref).unwrap();

        assert_eq!(
            json["parent"]["uri"],
            "at://did:plc:user1/app.bsky.feed.post/reply1"
        );
        assert_eq!(
            json["root"]["uri"],
            "at://did:plc:user2/app.bsky.feed.post/original"
        );

        // Verify parent and root are different
        assert_ne!(json["parent"]["uri"], json["root"]["uri"]);
    }

    #[test]
    fn test_external_embed_without_thumb() {
        let external = serde_json::json!({
            "uri": "https://example.com/article",
            "title": "Great Article",
            "description": "An amazing article about Rust",
            "thumb": null,
        });

        assert_eq!(external["uri"], "https://example.com/article");
        assert_eq!(external["title"], "Great Article");
        assert_eq!(external["description"], "An amazing article about Rust");
        assert!(external["thumb"].is_null());
    }

    #[test]
    fn test_external_embed_with_thumb() {
        let thumb_blob = serde_json::json!({
            "$type": "blob",
            "ref": {"$link": "bafyreithumb123"},
            "mimeType": "image/jpeg",
            "size": 50000
        });

        let external = serde_json::json!({
            "uri": "https://blog.rust-lang.org/",
            "title": "Rust Blog",
            "description": "Official Rust blog",
            "thumb": thumb_blob,
        });

        assert_eq!(external["uri"], "https://blog.rust-lang.org/");
        assert_eq!(external["title"], "Rust Blog");
        assert!(!external["thumb"].is_null());
        assert_eq!(external["thumb"]["mimeType"], "image/jpeg");
    }

    #[test]
    fn test_external_embed_structure() {
        let embed = serde_json::json!({
            "$type": "app.bsky.embed.external",
            "external": {
                "uri": "https://rust-lang.org",
                "title": "Rust Programming Language",
                "description": "A language empowering everyone",
                "thumb": null,
            }
        });

        assert_eq!(embed["$type"], "app.bsky.embed.external");
        assert_eq!(embed["external"]["uri"], "https://rust-lang.org");
        assert_eq!(embed["external"]["title"], "Rust Programming Language");
        assert_eq!(
            embed["external"]["description"],
            "A language empowering everyone"
        );
    }

    #[test]
    fn test_external_embed_url_validation() {
        // Test various URL formats
        let urls = vec![
            "https://example.com",
            "http://example.com/path",
            "https://example.com/path?query=value",
            "https://subdomain.example.com:8080/path#fragment",
        ];

        for url in urls {
            let external = serde_json::json!({
                "uri": url,
                "title": "Test",
                "description": "Test description",
            });

            assert_eq!(external["uri"], url);
        }
    }

    #[test]
    fn test_external_embed_long_description() {
        // Test with a longer description
        let long_desc = "This is a very long description that contains a lot of text \
                         explaining the content of the linked page. It should be properly \
                         serialized and deserialized without any truncation or issues.";

        let external = serde_json::json!({
            "uri": "https://example.com",
            "title": "Long Description Test",
            "description": long_desc,
        });

        assert_eq!(external["description"], long_desc);
    }

    #[test]
    fn test_external_embed_special_characters() {
        // Test with special characters in title and description
        let external = serde_json::json!({
            "uri": "https://example.com",
            "title": "Test & Special <Characters> \"Quotes\"",
            "description": "Description with émojis 🦀 and ümlauts",
        });

        assert_eq!(
            external["title"],
            "Test & Special <Characters> \"Quotes\""
        );
        assert_eq!(
            external["description"],
            "Description with émojis 🦀 and ümlauts"
        );
    }
}
