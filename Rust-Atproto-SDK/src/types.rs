//! Core types for the ATProto SDK
//!
//! This module contains foundational types used throughout the SDK including:
//! - DID (Decentralized Identifier) types and validation
//! - Session management types
//! - User preferences
//! - Service type definitions

use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;

/// Error types for type validation
#[derive(Error, Debug)]
pub enum TypeError {
    #[error("Invalid DID: {0}")]
    InvalidDid(String),

    #[error("Invalid DID reference: {0} (must be of the form did:example:alice#service)")]
    InvalidAtprotoProxy(String),
}

/// Result type for type operations
pub type Result<T> = std::result::Result<T, TypeError>;

/// Service type identifier for ATProto services
///
/// Can be either 'atproto_labeler' or any other valid service identifier
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AtprotoServiceType(String);

impl AtprotoServiceType {
    /// Validates and creates a new AtprotoServiceType
    pub fn new(input: impl Into<String>) -> Option<Self> {
        let s = input.into();
        if is_atproto_service_type(&s) {
            Some(AtprotoServiceType(s))
        } else {
            None
        }
    }

    /// Creates an AtprotoServiceType without validation (use with caution)
    pub fn new_unchecked(input: impl Into<String>) -> Self {
        AtprotoServiceType(input.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AtprotoServiceType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Checks if a string is a valid ATProto service type
pub fn is_atproto_service_type(input: &str) -> bool {
    !input.contains(' ') && !input.contains('#')
}

/// Decentralized Identifier (DID)
///
/// Format: `did:{method}:{identifier}`
/// Example: `did:plc:z72i7hdynmk6r22z27h6tvur`
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Did(String);

impl Did {
    /// Validates and creates a new DID
    pub fn new(input: impl Into<String>) -> Result<Self> {
        let s = input.into();
        if is_did(&s) {
            Ok(Did(s))
        } else {
            Err(TypeError::InvalidDid(s))
        }
    }

    /// Creates a DID without validation (use with caution)
    pub fn new_unchecked(input: impl Into<String>) -> Self {
        Did(input.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Did {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl TryFrom<String> for Did {
    type Error = TypeError;

    fn try_from(value: String) -> Result<Self> {
        Did::new(value)
    }
}

impl TryFrom<&str> for Did {
    type Error = TypeError;

    fn try_from(value: &str) -> Result<Self> {
        Did::new(value.to_string())
    }
}

/// Validates if a string is a valid DID
///
/// Checks:
/// - Starts with "did:"
/// - Length between 8 and 2048 characters
/// - Contains method and identifier separated by ":"
pub fn is_did(input: &str) -> bool {
    if !input.starts_with("did:") {
        return false;
    }
    if input.len() < 8 {
        return false;
    }
    if input.len() > 2048 {
        return false;
    }

    // Find the method separator (second colon)
    if let Some(msidx) = input[4..].find(':') {
        let msidx = msidx + 4; // Adjust for the offset
        msidx > 4 && msidx < input.len() - 1
    } else {
        false
    }
}

/// Asserts that a string is a valid DID, returning it or an error
pub fn as_did(input: impl Into<String>) -> Result<Did> {
    Did::new(input)
}

/// ATProto Proxy - A DID reference with a service type
///
/// Format: `{did}#{service}`
/// Example: `did:plc:z72i7hdynmk6r22z27h6tvur#atproto_labeler`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AtprotoProxy {
    pub did: Did,
    pub service: AtprotoServiceType,
}

impl AtprotoProxy {
    /// Creates a new AtprotoProxy from a DID and service type
    pub fn new(did: Did, service: AtprotoServiceType) -> Self {
        AtprotoProxy { did, service }
    }

    /// Parses an AtprotoProxy from a string
    pub fn parse(input: &str) -> Result<Self> {
        let parts: Vec<&str> = input.split('#').collect();

        if parts.len() != 2 {
            return Err(TypeError::InvalidAtprotoProxy(input.to_string()));
        }

        let did = Did::new(parts[0])?;

        if !is_atproto_service_type(parts[1]) {
            return Err(TypeError::InvalidAtprotoProxy(input.to_string()));
        }

        let service = AtprotoServiceType::new_unchecked(parts[1]);

        Ok(AtprotoProxy { did, service })
    }

}

impl fmt::Display for AtprotoProxy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{}", self.did, self.service)
    }
}

impl TryFrom<String> for AtprotoProxy {
    type Error = TypeError;

    fn try_from(value: String) -> Result<Self> {
        AtprotoProxy::parse(&value)
    }
}

impl TryFrom<&str> for AtprotoProxy {
    type Error = TypeError;

    fn try_from(value: &str) -> Result<Self> {
        AtprotoProxy::parse(value)
    }
}

/// Validates if a string is a valid AtprotoProxy
pub fn is_atproto_proxy(input: &str) -> bool {
    AtprotoProxy::parse(input).is_ok()
}

/// Asserts that a string is a valid AtprotoProxy, returning it or an error
pub fn as_atproto_proxy(input: impl AsRef<str>) -> Result<AtprotoProxy> {
    AtprotoProxy::parse(input.as_ref())
}

/// Events that occur during session lifecycle
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AtpSessionEvent {
    Create,
    CreateFailed,
    Update,
    Expired,
    Delete,
    NetworkError,
}

/// Session data stored by the AtpAgent
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AtpSessionData {
    pub refresh_jwt: String,
    pub access_jwt: String,
    pub handle: String,
    pub did: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email_confirmed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email_auth_factor: Option<bool>,
    pub active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// Handler function type for persisting session data
///
/// This is called whenever a session event occurs (create, update, expire, etc.)
pub type AtpPersistSessionHandler = Box<
    dyn Fn(AtpSessionEvent, Option<AtpSessionData>) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

/// Options for AtpAgent login
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AtpAgentLoginOpts {
    pub identifier: String,
    pub password: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_factor_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_takendown: Option<bool>,
}

/// Global configuration options for AtpAgent
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AtpAgentGlobalOpts {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_labelers: Option<Vec<String>>,
}

/// Bluesky feed view preferences
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BskyFeedViewPreference {
    pub hide_replies: bool,
    pub hide_replies_by_unfollowed: bool,
    pub hide_replies_by_like_count: i32,
    pub hide_reposts: bool,
    pub hide_quote_posts: bool,
    /// Additional fields not explicitly defined
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

impl Default for BskyFeedViewPreference {
    fn default() -> Self {
        BskyFeedViewPreference {
            hide_replies: false,
            hide_replies_by_unfollowed: false,
            hide_replies_by_like_count: 0,
            hide_reposts: false,
            hide_quote_posts: false,
            extra: serde_json::Value::Null,
        }
    }
}

/// Bluesky thread view preferences
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BskyThreadViewPreference {
    pub sort: String,
    pub prioritize_followed_users: bool,
    /// Additional fields not explicitly defined
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

impl Default for BskyThreadViewPreference {
    fn default() -> Self {
        BskyThreadViewPreference {
            sort: "oldest".to_string(),
            prioritize_followed_users: true,
            extra: serde_json::Value::Null,
        }
    }
}

/// Bluesky interests preferences
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BskyInterestsPreference {
    pub tags: Vec<String>,
    /// Additional fields not explicitly defined
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

/// Legacy feeds configuration (deprecated, use saved_feeds instead)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LegacyFeeds {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned: Option<Vec<String>>,
}

/// Bluesky app state
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BskyAppState {
    pub queued_nudges: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_progress_guide: Option<serde_json::Value>, // Will be properly typed when we translate client types
    pub nuxs: Vec<serde_json::Value>, // Will be properly typed when we translate client types
}

/// Complete Bluesky preferences
///
/// Note: Some fields reference types from the client module that haven't been translated yet.
/// They're temporarily represented as serde_json::Value
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BskyPreferences {
    /// @deprecated use saved_feeds
    pub feeds: LegacyFeeds,
    pub saved_feeds: Vec<serde_json::Value>, // TODO: AppBskyActorDefs.SavedFeed when client is translated
    pub feed_view_prefs: std::collections::HashMap<String, BskyFeedViewPreference>,
    pub thread_view_prefs: BskyThreadViewPreference,
    pub moderation_prefs: serde_json::Value, // TODO: ModerationPrefs when moderation module is translated
    #[serde(skip_serializing_if = "Option::is_none")]
    pub birth_date: Option<chrono::NaiveDate>,
    pub interests: BskyInterestsPreference,
    pub bsky_app_state: BskyAppState,
    pub post_interaction_settings: serde_json::Value, // TODO: AppBskyActorDefs.PostInteractionSettingsPref
    pub verification_prefs: serde_json::Value, // TODO: AppBskyActorDefs.VerificationPrefs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_atproto_service_type() {
        assert!(is_atproto_service_type("atproto_labeler"));
        assert!(is_atproto_service_type("custom_service"));
        assert!(!is_atproto_service_type("has space"));
        assert!(!is_atproto_service_type("has#hash"));
    }

    #[test]
    fn test_is_did() {
        assert!(is_did("did:plc:z72i7hdynmk6r22z27h6tvur"));
        assert!(is_did("did:web:example.com"));
        assert!(!is_did("not:a:did"));
        assert!(!is_did("did:"));
        assert!(!is_did("did:x"));
        assert!(!is_did("did:method"));
    }

    #[test]
    fn test_did_creation() {
        let did = Did::new("did:plc:z72i7hdynmk6r22z27h6tvur").unwrap();
        assert_eq!(did.as_str(), "did:plc:z72i7hdynmk6r22z27h6tvur");

        let invalid = Did::new("invalid");
        assert!(invalid.is_err());
    }

    #[test]
    fn test_atproto_proxy() {
        let proxy = AtprotoProxy::parse("did:plc:z72i7hdynmk6r22z27h6tvur#atproto_labeler").unwrap();
        assert_eq!(proxy.did.as_str(), "did:plc:z72i7hdynmk6r22z27h6tvur");
        assert_eq!(proxy.service.as_str(), "atproto_labeler");
        assert_eq!(proxy.to_string(), "did:plc:z72i7hdynmk6r22z27h6tvur#atproto_labeler");

        let invalid = AtprotoProxy::parse("not-a-proxy");
        assert!(invalid.is_err());

        let invalid_service = AtprotoProxy::parse("did:plc:abc#has space");
        assert!(invalid_service.is_err());
    }

    #[test]
    fn test_session_event_serialization() {
        let event = AtpSessionEvent::Create;
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(json, "\"create\"");

        let event = AtpSessionEvent::NetworkError;
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(json, "\"network-error\"");
    }

    #[test]
    fn test_session_event_deserialization() {
        let event: AtpSessionEvent = serde_json::from_str("\"create\"").unwrap();
        assert_eq!(event, AtpSessionEvent::Create);

        let event: AtpSessionEvent = serde_json::from_str("\"create-failed\"").unwrap();
        assert_eq!(event, AtpSessionEvent::CreateFailed);

        let event: AtpSessionEvent = serde_json::from_str("\"update\"").unwrap();
        assert_eq!(event, AtpSessionEvent::Update);

        let event: AtpSessionEvent = serde_json::from_str("\"expired\"").unwrap();
        assert_eq!(event, AtpSessionEvent::Expired);

        let event: AtpSessionEvent = serde_json::from_str("\"network-error\"").unwrap();
        assert_eq!(event, AtpSessionEvent::NetworkError);
    }

    #[test]
    fn test_did_length_validation() {
        // Too short
        assert!(!is_did("did:x:y"));

        // Valid minimum length
        assert!(is_did("did:a:bc"));

        // Too long (over 2048 chars)
        let long_did = format!("did:plc:{}", "a".repeat(2048));
        assert!(!is_did(&long_did));

        // Valid long DID (under limit)
        let valid_long = format!("did:plc:{}", "a".repeat(1000));
        assert!(is_did(&valid_long));
    }

    #[test]
    fn test_did_format_validation() {
        // Valid DIDs
        assert!(is_did("did:plc:z72i7hdynmk6r22z27h6tvur"));
        assert!(is_did("did:web:example.com"));
        assert!(is_did("did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK"));

        // Invalid - missing parts
        assert!(!is_did("did:"));
        assert!(!is_did("did:plc"));
        assert!(!is_did("did:plc:"));

        // Invalid - wrong prefix
        assert!(!is_did("DID:plc:abc"));
        assert!(!is_did("not:plc:abc"));

        // Invalid - no second colon
        assert!(!is_did("did:method"));
    }

    #[test]
    fn test_did_try_from() {
        let did: Did = "did:plc:test".try_into().unwrap();
        assert_eq!(did.as_str(), "did:plc:test");

        let result: Result<Did> = "invalid".try_into();
        assert!(result.is_err());

        let did: Did = String::from("did:web:test.com").try_into().unwrap();
        assert_eq!(did.as_str(), "did:web:test.com");
    }

    #[test]
    fn test_did_display() {
        let did = Did::new("did:plc:example").unwrap();
        assert_eq!(format!("{}", did), "did:plc:example");
    }

    #[test]
    fn test_atproto_service_type_creation() {
        let service = AtprotoServiceType::new("atproto_labeler").unwrap();
        assert_eq!(service.as_str(), "atproto_labeler");

        let custom = AtprotoServiceType::new("custom_service").unwrap();
        assert_eq!(custom.as_str(), "custom_service");

        // Invalid - contains space
        let invalid = AtprotoServiceType::new("has space");
        assert!(invalid.is_none());

        // Invalid - contains hash
        let invalid = AtprotoServiceType::new("has#hash");
        assert!(invalid.is_none());
    }

    #[test]
    fn test_atproto_service_type_display() {
        let service = AtprotoServiceType::new_unchecked("my_service");
        assert_eq!(format!("{}", service), "my_service");
    }

    #[test]
    fn test_atproto_proxy_creation() {
        let did = Did::new("did:plc:abc123").unwrap();
        let service = AtprotoServiceType::new("atproto_labeler").unwrap();
        let proxy = AtprotoProxy::new(did, service);

        assert_eq!(proxy.did.as_str(), "did:plc:abc123");
        assert_eq!(proxy.service.as_str(), "atproto_labeler");
    }

    #[test]
    fn test_atproto_proxy_parse_valid() {
        let proxy = AtprotoProxy::parse("did:plc:test#atproto_labeler").unwrap();
        assert_eq!(proxy.did.as_str(), "did:plc:test");
        assert_eq!(proxy.service.as_str(), "atproto_labeler");

        let proxy = AtprotoProxy::parse("did:web:example.com#custom_service").unwrap();
        assert_eq!(proxy.did.as_str(), "did:web:example.com");
        assert_eq!(proxy.service.as_str(), "custom_service");
    }

    #[test]
    fn test_atproto_proxy_parse_invalid() {
        // No hash separator
        assert!(AtprotoProxy::parse("did:plc:test").is_err());

        // Multiple hashes
        assert!(AtprotoProxy::parse("did:plc:test#service#extra").is_err());

        // Invalid DID
        assert!(AtprotoProxy::parse("invalid#service").is_err());

        // Invalid service (contains space)
        assert!(AtprotoProxy::parse("did:plc:test#bad service").is_err());

        // Invalid service (contains hash)
        assert!(AtprotoProxy::parse("did:plc:test#bad#service").is_err());

        // Empty string
        assert!(AtprotoProxy::parse("").is_err());

        // Just hash
        assert!(AtprotoProxy::parse("#").is_err());
    }

    #[test]
    fn test_atproto_proxy_to_string() {
        let proxy = AtprotoProxy::parse("did:plc:example#my_service").unwrap();
        assert_eq!(proxy.to_string(), "did:plc:example#my_service");
    }

    #[test]
    fn test_atproto_proxy_display() {
        let proxy = AtprotoProxy::parse("did:plc:example#service").unwrap();
        assert_eq!(format!("{}", proxy), "did:plc:example#service");
    }

    #[test]
    fn test_atproto_proxy_try_from() {
        let proxy: AtprotoProxy = "did:plc:test#service".try_into().unwrap();
        assert_eq!(proxy.to_string(), "did:plc:test#service");

        let result: Result<AtprotoProxy> = "invalid".try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_atp_session_data_serialization() {
        let session = AtpSessionData {
            refresh_jwt: "refresh_token".to_string(),
            access_jwt: "access_token".to_string(),
            handle: "user.bsky.social".to_string(),
            did: "did:plc:test".to_string(),
            email: Some("user@example.com".to_string()),
            email_confirmed: Some(true),
            email_auth_factor: Some(false),
            active: true,
            status: Some("active".to_string()),
        };

        let json = serde_json::to_string(&session).unwrap();
        assert!(json.contains("\"refreshJwt\""));
        assert!(json.contains("\"accessJwt\""));
        assert!(json.contains("\"handle\""));
        assert!(json.contains("\"did\""));
        assert!(json.contains("\"email\""));
        assert!(json.contains("\"emailConfirmed\""));
        assert!(json.contains("\"emailAuthFactor\""));
        assert!(json.contains("\"active\""));
        assert!(json.contains("\"status\""));
    }

    #[test]
    fn test_atp_session_data_deserialization() {
        let json = r#"{
            "refreshJwt": "refresh_token",
            "accessJwt": "access_token",
            "handle": "user.bsky.social",
            "did": "did:plc:test",
            "email": "user@example.com",
            "emailConfirmed": true,
            "emailAuthFactor": false,
            "active": true,
            "status": "active"
        }"#;

        let session: AtpSessionData = serde_json::from_str(json).unwrap();
        assert_eq!(session.refresh_jwt, "refresh_token");
        assert_eq!(session.access_jwt, "access_token");
        assert_eq!(session.handle, "user.bsky.social");
        assert_eq!(session.did, "did:plc:test");
        assert_eq!(session.email, Some("user@example.com".to_string()));
        assert_eq!(session.email_confirmed, Some(true));
        assert_eq!(session.email_auth_factor, Some(false));
        assert_eq!(session.active, true);
        assert_eq!(session.status, Some("active".to_string()));
    }

    #[test]
    fn test_atp_session_data_optional_fields() {
        let session = AtpSessionData {
            refresh_jwt: "refresh_token".to_string(),
            access_jwt: "access_token".to_string(),
            handle: "user.bsky.social".to_string(),
            did: "did:plc:test".to_string(),
            email: None,
            email_confirmed: None,
            email_auth_factor: None,
            active: true,
            status: None,
        };

        let json = serde_json::to_string(&session).unwrap();
        // Optional None fields should be omitted
        assert!(!json.contains("\"email\""));
        assert!(!json.contains("\"emailConfirmed\""));
        assert!(!json.contains("\"emailAuthFactor\""));
        assert!(!json.contains("\"status\""));
    }

    #[test]
    fn test_bsky_feed_view_preference_defaults() {
        let pref = BskyFeedViewPreference::default();
        assert_eq!(pref.hide_replies, false);
        assert_eq!(pref.hide_replies_by_unfollowed, false);
        assert_eq!(pref.hide_replies_by_like_count, 0);
        assert_eq!(pref.hide_reposts, false);
        assert_eq!(pref.hide_quote_posts, false);
    }

    #[test]
    fn test_bsky_feed_view_preference_serialization() {
        let pref = BskyFeedViewPreference {
            hide_replies: true,
            hide_replies_by_unfollowed: true,
            hide_replies_by_like_count: 5,
            hide_reposts: false,
            hide_quote_posts: true,
            extra: serde_json::json!({"customField": "customValue"}),
        };

        let json = serde_json::to_string(&pref).unwrap();
        assert!(json.contains("\"hideReplies\":true"));
        assert!(json.contains("\"hideRepliesByUnfollowed\":true"));
        assert!(json.contains("\"hideRepliesByLikeCount\":5"));
        assert!(json.contains("\"hideReposts\":false"));
        assert!(json.contains("\"hideQuotePosts\":true"));
        assert!(json.contains("\"customField\""));
    }

    #[test]
    fn test_bsky_thread_view_preference_defaults() {
        let pref = BskyThreadViewPreference::default();
        assert_eq!(pref.sort, "oldest");
        assert_eq!(pref.prioritize_followed_users, true);
    }

    #[test]
    fn test_bsky_thread_view_preference_serialization() {
        let pref = BskyThreadViewPreference {
            sort: "newest".to_string(),
            prioritize_followed_users: false,
            extra: serde_json::json!({"custom": "value"}),
        };

        let json = serde_json::to_string(&pref).unwrap();
        assert!(json.contains("\"sort\":\"newest\""));
        assert!(json.contains("\"prioritizeFollowedUsers\":false"));
        assert!(json.contains("\"custom\""));
    }

    #[test]
    fn test_bsky_interests_preference() {
        let pref = BskyInterestsPreference {
            tags: vec!["tech".to_string(), "science".to_string()],
            extra: serde_json::Value::Null,
        };

        let json = serde_json::to_string(&pref).unwrap();
        assert!(json.contains("\"tags\""));
        assert!(json.contains("\"tech\""));
        assert!(json.contains("\"science\""));
    }

    #[test]
    fn test_atp_agent_login_opts() {
        let opts = AtpAgentLoginOpts {
            identifier: "user.bsky.social".to_string(),
            password: "password123".to_string(),
            auth_factor_token: Some("token".to_string()),
            allow_takendown: Some(false),
        };

        let json = serde_json::to_string(&opts).unwrap();
        assert!(json.contains("\"identifier\""));
        assert!(json.contains("\"password\""));
        assert!(json.contains("\"authFactorToken\""));
        assert!(json.contains("\"allowTakendown\""));
    }

    #[test]
    fn test_atp_agent_global_opts() {
        let opts = AtpAgentGlobalOpts {
            app_labelers: Some(vec!["did:plc:labeler1".to_string()]),
        };

        let json = serde_json::to_string(&opts).unwrap();
        assert!(json.contains("\"appLabelers\""));

        // Test default
        let default_opts = AtpAgentGlobalOpts::default();
        assert!(default_opts.app_labelers.is_none());
    }

    #[test]
    fn test_legacy_feeds() {
        let feeds = LegacyFeeds {
            saved: Some(vec!["feed1".to_string(), "feed2".to_string()]),
            pinned: Some(vec!["feed1".to_string()]),
        };

        let json = serde_json::to_string(&feeds).unwrap();
        assert!(json.contains("\"saved\""));
        assert!(json.contains("\"pinned\""));
    }

    #[test]
    fn test_bsky_app_state() {
        let state = BskyAppState {
            queued_nudges: vec!["nudge1".to_string()],
            active_progress_guide: Some(serde_json::json!({"step": 1})),
            nuxs: vec![serde_json::json!({"id": "nux1"})],
        };

        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("\"queuedNudges\""));
        assert!(json.contains("\"activeProgressGuide\""));
        assert!(json.contains("\"nuxs\""));
    }

    #[test]
    fn test_did_equality() {
        let did1 = Did::new("did:plc:test").unwrap();
        let did2 = Did::new("did:plc:test").unwrap();
        let did3 = Did::new("did:plc:other").unwrap();

        assert_eq!(did1, did2);
        assert_ne!(did1, did3);
    }

    #[test]
    fn test_did_hash() {
        use std::collections::HashMap;

        let mut map = HashMap::new();
        let did = Did::new("did:plc:test").unwrap();
        map.insert(did.clone(), "value");

        assert_eq!(map.get(&did), Some(&"value"));
    }

    #[test]
    fn test_atproto_proxy_equality() {
        let proxy1 = AtprotoProxy::parse("did:plc:test#service").unwrap();
        let proxy2 = AtprotoProxy::parse("did:plc:test#service").unwrap();
        let proxy3 = AtprotoProxy::parse("did:plc:test#other").unwrap();

        assert_eq!(proxy1, proxy2);
        assert_ne!(proxy1, proxy3);
    }

    #[test]
    fn test_is_atproto_proxy_helper() {
        assert!(is_atproto_proxy("did:plc:test#service"));
        assert!(!is_atproto_proxy("did:plc:test"));
        assert!(!is_atproto_proxy("invalid#service"));
    }

    #[test]
    fn test_as_atproto_proxy_helper() {
        let proxy = as_atproto_proxy("did:plc:test#service").unwrap();
        assert_eq!(proxy.to_string(), "did:plc:test#service");

        let result = as_atproto_proxy("invalid");
        assert!(result.is_err());
    }

    #[test]
    fn test_as_did_helper() {
        let did = as_did("did:plc:test").unwrap();
        assert_eq!(did.as_str(), "did:plc:test");

        let result = as_did("invalid");
        assert!(result.is_err());
    }
}
