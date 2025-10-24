//! OAuth 2.0 with PKCE (Proof Key for Code Exchange) implementation
//!
//! This module implements OAuth 2.0 authorization code flow with PKCE (RFC 7636)
//! and DPoP (RFC 9449) for secure authentication with ATProto services.
//!
//! ## Flow Overview
//!
//! 1. **Generate PKCE parameters**: code_verifier and code_challenge
//! 2. **Build authorization URL**: Redirect user to authorization server
//! 3. **Handle callback**: Receive authorization code
//! 4. **Exchange code for tokens**: Use code + verifier to get access/refresh tokens
//! 5. **Use tokens**: Attach tokens to API requests
//! 6. **Refresh tokens**: Automatically refresh when expired
//!
//! ## Example
//!
//! ```no_run
//! use atproto::oauth::{OAuthClient, PkceParams};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Create OAuth client
//!     let client = OAuthClient::new(
//!         "https://example.com/client-metadata.json".to_string(),
//!         "https://example.com/callback".to_string(),
//!     );
//!
//!     // Generate PKCE parameters (store code_verifier securely!)
//!     let pkce = PkceParams::generate();
//!
//!     // Build authorization URL
//!     let auth_url = client.build_authorization_url(
//!         "https://bsky.social",
//!         "user.bsky.social",
//!         &pkce,
//!     ).await?;
//!
//!     // Redirect user to auth_url...
//!     // User authorizes and gets redirected back with code
//!
//!     // Exchange authorization code for tokens
//!     let session = client.exchange_code(
//!         "authorization_code_from_callback",
//!         &pkce.code_verifier,
//!     ).await?;
//!
//!     println!("Authenticated as: {}", session.did);
//!     Ok(())
//! }
//! ```

pub mod dpop;
pub mod pkce;
pub mod client;
pub mod types;
pub mod state;
pub mod callback;

pub use client::OAuthClient;
pub use pkce::PkceParams;
pub use types::{OAuthSession, OAuthError, ClientMetadata};
pub use dpop::DPopManager;
pub use state::{OAuthState, StateManager};
pub use callback::{CallbackParser, CallbackResult};
