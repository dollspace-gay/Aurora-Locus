//! AT Protocol syntax utilities
//!
//! This module provides parsing and validation for AT Protocol syntax elements:
//! - AT URIs (at://...)
//! - DID validation enhancements
//! - NSID (Namespaced Identifier) handling

use crate::types::Did;
use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;

/// Error types for syntax parsing
#[derive(Error, Debug, Clone, PartialEq)]
pub enum SyntaxError {
    #[error("Invalid AT URI: {0}")]
    InvalidAtUri(String),

    #[error("Invalid DID: {0}")]
    InvalidDid(String),

    #[error("Invalid NSID: {0}")]
    InvalidNsid(String),

    #[error("Missing required component: {0}")]
    MissingComponent(String),

    #[error("Invalid format: {0}")]
    InvalidFormat(String),
}

/// Result type for syntax operations
pub type Result<T> = std::result::Result<T, SyntaxError>;

/// AT URI parser and representation
///
/// AT URIs follow the format: `at://{authority}/{collection}/{rkey}`
///
/// Where:
/// - `authority` is typically a DID (e.g., `did:plc:abc123`)
/// - `collection` is an NSID (e.g., `app.bsky.feed.post`)
/// - `rkey` is a record key (optional)
///
/// # Examples
///
/// ```
/// use atproto::syntax::AtUri;
///
/// // Full URI with record key
/// let uri = AtUri::new("at://did:plc:abc123/app.bsky.feed.post/3k2akljdf").unwrap();
/// assert_eq!(uri.hostname(), "did:plc:abc123");
/// assert_eq!(uri.collection(), "app.bsky.feed.post");
/// assert_eq!(uri.rkey(), Some("3k2akljdf"));
///
/// // URI without record key
/// let uri = AtUri::new("at://did:plc:abc123/app.bsky.feed.post").unwrap();
/// assert_eq!(uri.rkey(), None);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AtUri {
    /// The authority (typically a DID)
    hostname: String,

    /// The collection (NSID)
    collection: String,

    /// The record key (optional)
    rkey: Option<String>,
}

impl AtUri {
    /// Parses an AT URI string
    ///
    /// # Format
    ///
    /// `at://{hostname}/{collection}/{rkey}`
    ///
    /// The `rkey` component is optional.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - URI doesn't start with "at://"
    /// - Hostname is missing or invalid
    /// - Collection is missing or invalid
    /// - Format is otherwise malformed
    pub fn new(uri: impl AsRef<str>) -> Result<Self> {
        let uri = uri.as_ref();

        // Must start with at://
        if !uri.starts_with("at://") {
            return Err(SyntaxError::InvalidAtUri(format!(
                "URI must start with 'at://': {}",
                uri
            )));
        }

        // Remove the at:// prefix
        let path = &uri[5..];

        if path.is_empty() {
            return Err(SyntaxError::MissingComponent(
                "hostname is required".to_string(),
            ));
        }

        // Split by '/'
        let parts: Vec<&str> = path.split('/').collect();

        if parts.is_empty() {
            return Err(SyntaxError::MissingComponent(
                "hostname is required".to_string(),
            ));
        }

        let hostname = parts[0].to_string();

        if hostname.is_empty() {
            return Err(SyntaxError::MissingComponent(
                "hostname cannot be empty".to_string(),
            ));
        }

        // Validate hostname (should be a DID or handle)
        if !Self::is_valid_hostname(&hostname) {
            return Err(SyntaxError::InvalidAtUri(format!(
                "Invalid hostname: {}",
                hostname
            )));
        }

        // Collection is required
        if parts.len() < 2 {
            return Err(SyntaxError::MissingComponent(
                "collection is required".to_string(),
            ));
        }

        let collection = parts[1].to_string();

        if collection.is_empty() {
            return Err(SyntaxError::MissingComponent(
                "collection cannot be empty".to_string(),
            ));
        }

        // Validate collection (should be an NSID)
        if !Self::is_valid_nsid(&collection) {
            return Err(SyntaxError::InvalidNsid(collection));
        }

        // Record key is optional
        let rkey = if parts.len() >= 3 {
            let rkey = parts[2].to_string();
            if rkey.is_empty() {
                None
            } else {
                Some(rkey)
            }
        } else {
            None
        };

        Ok(AtUri {
            hostname,
            collection,
            rkey,
        })
    }

    /// Creates an AT URI from components
    pub fn from_parts(
        hostname: impl Into<String>,
        collection: impl Into<String>,
        rkey: Option<impl Into<String>>,
    ) -> Result<Self> {
        let hostname = hostname.into();
        let collection = collection.into();
        let rkey = rkey.map(|r| r.into());

        if !Self::is_valid_hostname(&hostname) {
            return Err(SyntaxError::InvalidAtUri(format!(
                "Invalid hostname: {}",
                hostname
            )));
        }

        if !Self::is_valid_nsid(&collection) {
            return Err(SyntaxError::InvalidNsid(collection));
        }

        Ok(AtUri {
            hostname,
            collection,
            rkey,
        })
    }

    /// Returns the hostname (authority) component
    pub fn hostname(&self) -> &str {
        &self.hostname
    }

    /// Returns the collection (NSID) component
    pub fn collection(&self) -> &str {
        &self.collection
    }

    /// Returns the record key component, if present
    pub fn rkey(&self) -> Option<&str> {
        self.rkey.as_deref()
    }


    /// Validates a hostname (DID or handle)
    fn is_valid_hostname(hostname: &str) -> bool {
        // Could be a DID or a handle
        // For now, basic validation
        if hostname.starts_with("did:") {
            // Use DID validation from types module
            crate::types::is_did(hostname)
        } else {
            // Handle format: letters, numbers, dots, hyphens
            // Basic validation - should match domain-like format
            !hostname.is_empty()
                && hostname.len() <= 253
                && hostname
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '.' || c == '-')
        }
    }

    /// Validates an NSID (Namespaced Identifier)
    ///
    /// NSIDs follow the format: {authority}.{name}.{name}
    /// Example: app.bsky.feed.post, com.atproto.repo.getRecord
    pub fn is_valid_nsid(nsid: &str) -> bool {
        if nsid.is_empty() || nsid.len() > 317 {
            return false;
        }

        // Must contain at least one dot
        if !nsid.contains('.') {
            return false;
        }

        // Split by dots
        let segments: Vec<&str> = nsid.split('.').collect();

        if segments.len() < 2 {
            return false;
        }

        // Each segment must be valid
        for segment in segments {
            if segment.is_empty() || segment.len() > 63 {
                return false;
            }

            // Must start with a letter (lowercase or uppercase)
            if !segment
                .chars()
                .next()
                .map(|c| c.is_ascii_alphabetic())
                .unwrap_or(false)
            {
                return false;
            }

            // Can only contain letters, digits, and hyphens
            if !segment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
            {
                return false;
            }
        }

        true
    }
}

impl fmt::Display for AtUri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ref rkey) = self.rkey {
            write!(f, "at://{}/{}/{}", self.hostname, self.collection, rkey)
        } else {
            write!(f, "at://{}/{}", self.hostname, self.collection)
        }
    }
}

impl TryFrom<String> for AtUri {
    type Error = SyntaxError;

    fn try_from(value: String) -> Result<Self> {
        AtUri::new(value)
    }
}

impl TryFrom<&str> for AtUri {
    type Error = SyntaxError;

    fn try_from(value: &str) -> Result<Self> {
        AtUri::new(value)
    }
}

/// Ensures a string is a valid DID, returning it or an error
///
/// This is a convenience function that validates a DID and returns
/// a Did type or an error.
///
/// # Examples
///
/// ```
/// use atproto::syntax::ensure_valid_did;
///
/// let did = ensure_valid_did("did:plc:abc123").unwrap();
/// assert_eq!(did.as_str(), "did:plc:abc123");
///
/// assert!(ensure_valid_did("not-a-did").is_err());
/// ```
pub fn ensure_valid_did(input: impl AsRef<str>) -> Result<Did> {
    Did::new(input.as_ref()).map_err(|e| SyntaxError::InvalidDid(e.to_string()))
}

/// Validates an NSID (Namespaced Identifier)
///
/// NSIDs follow the format: {authority}.{name}.{name}
/// Example: app.bsky.feed.post, com.atproto.repo.getRecord
///
/// # Examples
///
/// ```
/// use atproto::syntax::is_valid_nsid;
///
/// assert!(is_valid_nsid("app.bsky.feed.post"));
/// assert!(is_valid_nsid("com.atproto.repo.getRecord"));
/// assert!(!is_valid_nsid("invalid"));
/// ```
pub fn is_valid_nsid(nsid: &str) -> bool {
    AtUri::is_valid_nsid(nsid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_at_uri_full() {
        let uri = AtUri::new("at://did:plc:abc123/app.bsky.feed.post/3k2akljdf").unwrap();
        assert_eq!(uri.hostname(), "did:plc:abc123");
        assert_eq!(uri.collection(), "app.bsky.feed.post");
        assert_eq!(uri.rkey(), Some("3k2akljdf"));
    }

    #[test]
    fn test_at_uri_without_rkey() {
        let uri = AtUri::new("at://did:plc:abc123/app.bsky.feed.post").unwrap();
        assert_eq!(uri.hostname(), "did:plc:abc123");
        assert_eq!(uri.collection(), "app.bsky.feed.post");
        assert_eq!(uri.rkey(), None);
    }

    #[test]
    fn test_at_uri_with_handle() {
        let uri = AtUri::new("at://alice.bsky.social/app.bsky.feed.post/123").unwrap();
        assert_eq!(uri.hostname(), "alice.bsky.social");
        assert_eq!(uri.collection(), "app.bsky.feed.post");
        assert_eq!(uri.rkey(), Some("123"));
    }

    #[test]
    fn test_at_uri_invalid_prefix() {
        let result = AtUri::new("http://example.com/collection/rkey");
        assert!(result.is_err());
    }

    #[test]
    fn test_at_uri_missing_hostname() {
        let result = AtUri::new("at:///app.bsky.feed.post");
        assert!(result.is_err());
    }

    #[test]
    fn test_at_uri_missing_collection() {
        let result = AtUri::new("at://did:plc:abc123");
        assert!(result.is_err());
    }

    #[test]
    fn test_at_uri_empty_collection() {
        let result = AtUri::new("at://did:plc:abc123//rkey");
        assert!(result.is_err());
    }

    #[test]
    fn test_at_uri_to_string_with_rkey() {
        let uri = AtUri::new("at://did:plc:abc123/app.bsky.feed.post/123").unwrap();
        assert_eq!(
            uri.to_string(),
            "at://did:plc:abc123/app.bsky.feed.post/123"
        );
    }

    #[test]
    fn test_at_uri_to_string_without_rkey() {
        let uri = AtUri::new("at://did:plc:abc123/app.bsky.feed.post").unwrap();
        assert_eq!(uri.to_string(), "at://did:plc:abc123/app.bsky.feed.post");
    }

    #[test]
    fn test_at_uri_display() {
        let uri = AtUri::new("at://did:plc:abc123/app.bsky.feed.post/123").unwrap();
        assert_eq!(
            format!("{}", uri),
            "at://did:plc:abc123/app.bsky.feed.post/123"
        );
    }

    #[test]
    fn test_at_uri_from_parts() {
        let uri = AtUri::from_parts(
            "did:plc:abc123",
            "app.bsky.feed.post",
            Some("123"),
        )
        .unwrap();

        assert_eq!(uri.hostname(), "did:plc:abc123");
        assert_eq!(uri.collection(), "app.bsky.feed.post");
        assert_eq!(uri.rkey(), Some("123"));
    }

    #[test]
    fn test_at_uri_from_parts_no_rkey() {
        let uri = AtUri::from_parts("did:plc:abc123", "app.bsky.feed.post", None::<String>).unwrap();

        assert_eq!(uri.hostname(), "did:plc:abc123");
        assert_eq!(uri.collection(), "app.bsky.feed.post");
        assert_eq!(uri.rkey(), None);
    }

    #[test]
    fn test_at_uri_try_from_string() {
        let uri: AtUri = "at://did:plc:abc123/app.bsky.feed.post/123"
            .try_into()
            .unwrap();
        assert_eq!(uri.hostname(), "did:plc:abc123");
    }

    #[test]
    fn test_at_uri_equality() {
        let uri1 = AtUri::new("at://did:plc:abc123/app.bsky.feed.post/123").unwrap();
        let uri2 = AtUri::new("at://did:plc:abc123/app.bsky.feed.post/123").unwrap();
        let uri3 = AtUri::new("at://did:plc:abc123/app.bsky.feed.post/456").unwrap();

        assert_eq!(uri1, uri2);
        assert_ne!(uri1, uri3);
    }

    #[test]
    fn test_is_valid_hostname_did() {
        assert!(AtUri::is_valid_hostname("did:plc:abc123"));
        assert!(AtUri::is_valid_hostname("did:web:example.com"));
        assert!(!AtUri::is_valid_hostname("did:invalid"));
    }

    #[test]
    fn test_is_valid_hostname_handle() {
        assert!(AtUri::is_valid_hostname("alice.bsky.social"));
        assert!(AtUri::is_valid_hostname("bob.example.com"));
        assert!(AtUri::is_valid_hostname("user-123.test"));
        assert!(!AtUri::is_valid_hostname(""));
    }

    #[test]
    fn test_is_valid_nsid() {
        assert!(AtUri::is_valid_nsid("app.bsky.feed.post"));
        assert!(AtUri::is_valid_nsid("com.atproto.repo.getRecord"));
        assert!(AtUri::is_valid_nsid("io.example.my-collection"));

        // Invalid NSIDs
        assert!(!AtUri::is_valid_nsid("")); // Empty
        assert!(!AtUri::is_valid_nsid("noperiod")); // No period
        assert!(!AtUri::is_valid_nsid("app.bsky.")); // Empty segment
        assert!(!AtUri::is_valid_nsid(".app.bsky")); // Empty first segment
        assert!(!AtUri::is_valid_nsid("app.bsky.feed.post.".repeat(50).as_str())); // Too long

        // Capital letters are valid in NSIDs
        assert!(AtUri::is_valid_nsid("App.bsky.feed.post"));
        assert!(AtUri::is_valid_nsid("com.atproto.repo.getRecord"));
    }

    #[test]
    fn test_is_valid_nsid_segments() {
        // Segment must start with letter (not digit or hyphen)
        assert!(!AtUri::is_valid_nsid("app.123bsky.feed.post"));
        assert!(!AtUri::is_valid_nsid("app.-bsky.feed.post"));

        // Valid with hyphens in middle
        assert!(AtUri::is_valid_nsid("app.bsky-social.feed.post"));

        // Valid with mixed case
        assert!(AtUri::is_valid_nsid("app.BskySocial.feed.Post"));
    }

    #[test]
    fn test_ensure_valid_did() {
        let did = ensure_valid_did("did:plc:abc123").unwrap();
        assert_eq!(did.as_str(), "did:plc:abc123");

        let result = ensure_valid_did("not-a-did");
        assert!(result.is_err());
    }

    #[test]
    fn test_syntax_error_display() {
        let err = SyntaxError::InvalidAtUri("bad uri".to_string());
        assert_eq!(err.to_string(), "Invalid AT URI: bad uri");

        let err = SyntaxError::MissingComponent("hostname".to_string());
        assert_eq!(err.to_string(), "Missing required component: hostname");
    }

    #[test]
    fn test_at_uri_clone() {
        let uri1 = AtUri::new("at://did:plc:abc123/app.bsky.feed.post/123").unwrap();
        let uri2 = uri1.clone();
        assert_eq!(uri1, uri2);
    }

    #[test]
    fn test_at_uri_hash() {
        use std::collections::HashMap;

        let uri = AtUri::new("at://did:plc:abc123/app.bsky.feed.post/123").unwrap();
        let mut map = HashMap::new();
        map.insert(uri.clone(), "value");

        assert_eq!(map.get(&uri), Some(&"value"));
    }

    #[test]
    fn test_at_uri_serialization() {
        let uri = AtUri::new("at://did:plc:abc123/app.bsky.feed.post/123").unwrap();
        let json = serde_json::to_string(&uri).unwrap();
        assert!(json.contains("did:plc:abc123"));
        assert!(json.contains("app.bsky.feed.post"));
        assert!(json.contains("123"));
    }

    #[test]
    fn test_at_uri_deserialization() {
        let json = r#"{"hostname":"did:plc:abc123","collection":"app.bsky.feed.post","rkey":"123"}"#;
        let uri: AtUri = serde_json::from_str(json).unwrap();
        assert_eq!(uri.hostname(), "did:plc:abc123");
        assert_eq!(uri.collection(), "app.bsky.feed.post");
        assert_eq!(uri.rkey(), Some("123"));
    }
}
