//! Rust SDK for the AT Protocol (ATProto)
//!
//! This is a complete Rust implementation of the ATProto SDK, translated from the
//! official TypeScript SDK maintained by Bluesky.
//!
//! # Features
//!
//! - Type-safe DID and session handling
//! - Async/await support with Tokio
//! - Complete API client for ATProto services
//! - Rich text processing
//! - Moderation system
//!
//! # Example
//!
//! ```rust,no_run
//! use atproto::types::Did;
//! use atproto::consts::BSKY_LABELER_DID;
//!
//! let did = Did::new("did:plc:z72i7hdynmk6r22z27h6tvur").unwrap();
//! println!("DID: {}", did);
//! println!("Bluesky Labeler: {}", BSKY_LABELER_DID);
//! ```

pub mod agent;
pub mod blob;
pub mod car;
pub mod client;
pub mod consts;
pub mod did_doc;
pub mod handle;
pub mod moderation;
pub mod mst;
pub mod namespaces;
pub mod oauth;
pub mod preferences;
pub mod repo;
pub mod rich_text;
pub mod server_auth;
pub mod session_manager;
pub mod syntax;
pub mod tid;
pub mod types;
pub mod util;
pub mod validation;
pub mod xrpc;
pub mod xrpc_subscription;

// Re-export commonly used types
pub use types::{
    AtpSessionData, AtpSessionEvent, BskyPreferences, Did, AtprotoProxy,
    AtprotoServiceType, is_did, is_atproto_proxy,
};

// Re-export constants
pub use consts::BSKY_LABELER_DID;

// Re-export commonly used utilities
pub use util::{
    sanitize_muted_word_value, saved_feeds_to_uri_arrays, get_saved_feed_type,
    validate_saved_feed, validate_nux, SavedFeed, SavedFeedType, Nux,
};

// Re-export session manager
pub use session_manager::{SessionManager, UnauthenticatedSessionManager, PersistentSessionManager, SessionCallback};

// Re-export XRPC types
pub use xrpc::{XrpcClient, XrpcClientImpl, XrpcRequest, XrpcResponse, XrpcMethod, XrpcError, RetryConfig};

// Re-export syntax types
pub use syntax::{AtUri, SyntaxError, ensure_valid_did};

// Re-export rich text types
pub use rich_text::UnicodeString;

// Re-export handle resolution types
pub use handle::{HandleResolver, HandleError, normalize_handle, is_valid_handle};

// Re-export blob utilities
pub use blob::{BlobRef, detect_mime_type, detect_mime_type_from_data, validate_blob_size};

// Re-export preferences types
pub use preferences::{
    BskyFeedViewPreference, BskyThreadViewPreference, BskyInterestsPreference,
    ModerationPrefs, ModerationPrefsLabeler, BskyAppState, LabelPreference,
};

// Re-export validation types
pub use validation::{LexiconSchema, ValidationError, ValidationResult, StringFormat};

// Re-export server auth types
pub use server_auth::{
    AuthManager, AuthError, Account, AccountCreate, AccountCreated,
    SessionCreate, Session, PasswordHasher, TokenManager, SessionStore,
    AccountStatus, TokenClaims,
};
