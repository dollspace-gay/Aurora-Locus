//! Utility functions for the ATProto SDK
//!
//! This module contains helper functions for:
//! - Text sanitization (muted words)
//! - Feed management and conversion
//! - Validation of feeds and NUX data

use lazy_static::lazy_static;
use regex::Regex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Error types for utility operations
#[derive(Error, Debug)]
pub enum UtilError {
    #[error("Saved feed must have an `id` - use a TID")]
    MissingFeedId,

    #[error("Saved feed of type 'feed' must be a feed, got {0}")]
    InvalidFeedType(String),

    #[error("Saved feed of type 'list' must be a list, got {0}")]
    InvalidListType(String),

    #[error("Invalid AT URI: {0}")]
    InvalidAtUri(String),

    #[error("Nux validation error: {0}")]
    NuxValidationError(String),
}

/// Result type for utility operations
pub type Result<T> = std::result::Result<T, UtilError>;

lazy_static! {
    /// Regex to match various Unicode control characters and whitespace
    /// Includes: \r, \n, soft hyphen, word joiner, zero-width joiner, zero-width non-joiner, zero-width space
    static ref CONTROL_CHARS: Regex = Regex::new(r"[\r\n\u{00AD}\u{2060}\u{200D}\u{200C}\u{200B}]+").unwrap();
}

/// Sanitizes a muted word value by removing unwanted characters
///
/// This function:
/// 1. Trims whitespace
/// 2. Removes leading hashtag (unless followed by emoji variation selector U+FE0F)
/// 3. Removes control characters (newlines, zero-width characters, etc.)
///
/// # Examples
///
/// ```
/// use atproto::util::sanitize_muted_word_value;
///
/// assert_eq!(sanitize_muted_word_value("  #word  "), "word");
/// assert_eq!(sanitize_muted_word_value("word\ntest"), "wordtest");
/// assert_eq!(sanitize_muted_word_value("#test"), "test");
/// ```
pub fn sanitize_muted_word_value(value: &str) -> String {
    let trimmed = value.trim();

    // Remove leading hashtag unless followed by emoji variation selector (U+FE0F)
    // Since Rust regex doesn't support lookahead, we check manually
    let without_hash = if let Some(after_hash) = trimmed.strip_prefix('#') {
        // Don't remove # if followed by U+FE0F (emoji variation selector)
        if after_hash.starts_with('\u{FE0F}') {
            trimmed
        } else {
            after_hash
        }
    } else {
        trimmed
    };

    // Remove control characters
    let without_control = CONTROL_CHARS.replace_all(without_hash, "");
    without_control.to_string()
}

/// Represents a saved feed in the Bluesky preferences
///
/// Note: This is a temporary simplified version. Will be replaced with the
/// proper AppBskyActorDefs.SavedFeed when the client module is translated.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedFeed {
    pub id: String,
    #[serde(rename = "type")]
    pub feed_type: SavedFeedType,
    pub value: String,
    pub pinned: bool,
}

/// Type of saved feed
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SavedFeedType {
    Feed,
    List,
    Unknown,
}

/// Result of converting saved feeds to URI arrays
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedFeedUriArrays {
    pub pinned: Vec<String>,
    pub saved: Vec<String>,
}

/// Converts an array of saved feeds to separate pinned and saved URI arrays
///
/// The `saved` array includes all feeds (both pinned and unpinned), maintaining
/// backwards compatibility with v1 behavior.
///
/// # Examples
///
/// ```
/// use atproto::util::{SavedFeed, SavedFeedType, saved_feeds_to_uri_arrays};
///
/// let feeds = vec![
///     SavedFeed {
///         id: "1".to_string(),
///         feed_type: SavedFeedType::Feed,
///         value: "at://did:plc:test/app.bsky.feed.generator/feed1".to_string(),
///         pinned: true,
///     },
///     SavedFeed {
///         id: "2".to_string(),
///         feed_type: SavedFeedType::Feed,
///         value: "at://did:plc:test/app.bsky.feed.generator/feed2".to_string(),
///         pinned: false,
///     },
/// ];
///
/// let result = saved_feeds_to_uri_arrays(&feeds);
/// assert_eq!(result.pinned.len(), 1);
/// assert_eq!(result.saved.len(), 2); // saved includes pinned
/// ```
pub fn saved_feeds_to_uri_arrays(saved_feeds: &[SavedFeed]) -> SavedFeedUriArrays {
    let mut pinned = Vec::new();
    let mut saved = Vec::new();

    for feed in saved_feeds {
        if feed.pinned {
            pinned.push(feed.value.clone());
            // saved in v1 includes pinned
            saved.push(feed.value.clone());
        } else {
            saved.push(feed.value.clone());
        }
    }

    SavedFeedUriArrays { pinned, saved }
}

// Re-export AtUri from syntax module for convenience
pub use crate::syntax::AtUri;

/// Get the type of a saved feed based on its AT URI
///
/// **Deprecated**: Used by deprecated methods for backwards compatibility.
/// Should not be used moving forward.
///
/// # Errors
///
/// Returns an error if the URI is invalid.
///
/// # Examples
///
/// ```
/// use atproto::util::{get_saved_feed_type, SavedFeedType};
///
/// let feed_type = get_saved_feed_type("at://did:plc:test/app.bsky.feed.generator/feed1").unwrap();
/// assert_eq!(feed_type, SavedFeedType::Feed);
///
/// let list_type = get_saved_feed_type("at://did:plc:test/app.bsky.graph.list/list1").unwrap();
/// assert_eq!(list_type, SavedFeedType::List);
/// ```
pub fn get_saved_feed_type(uri: &str) -> Result<SavedFeedType> {
    let uri_parsed = AtUri::new(uri).map_err(|e| UtilError::InvalidAtUri(e.to_string()))?;

    match uri_parsed.collection() {
        "app.bsky.feed.generator" => Ok(SavedFeedType::Feed),
        "app.bsky.graph.list" => Ok(SavedFeedType::List),
        _ => Ok(SavedFeedType::Unknown),
    }
}

/// Validates a saved feed
///
/// Ensures that:
/// 1. The feed has an ID
/// 2. If the type is 'feed' or 'list', the URI matches the expected collection
///
/// # Errors
///
/// Returns an error if validation fails.
///
/// # Examples
///
/// ```
/// use atproto::util::{SavedFeed, SavedFeedType, validate_saved_feed};
///
/// let feed = SavedFeed {
///     id: "123".to_string(),
///     feed_type: SavedFeedType::Feed,
///     value: "at://did:plc:test/app.bsky.feed.generator/feed1".to_string(),
///     pinned: false,
/// };
///
/// assert!(validate_saved_feed(&feed).is_ok());
/// ```
pub fn validate_saved_feed(saved_feed: &SavedFeed) -> Result<()> {
    if saved_feed.id.is_empty() {
        return Err(UtilError::MissingFeedId);
    }

    if saved_feed.feed_type == SavedFeedType::Feed
        || saved_feed.feed_type == SavedFeedType::List
    {
        let uri = AtUri::new(&saved_feed.value).map_err(|e| UtilError::InvalidAtUri(e.to_string()))?;
        let is_feed = uri.collection() == "app.bsky.feed.generator";
        let is_list = uri.collection() == "app.bsky.graph.list";

        if saved_feed.feed_type == SavedFeedType::Feed && !is_feed {
            return Err(UtilError::InvalidFeedType(uri.collection().to_string()));
        }

        if saved_feed.feed_type == SavedFeedType::List && !is_list {
            return Err(UtilError::InvalidListType(uri.collection().to_string()));
        }
    }

    Ok(())
}

/// Represents a NUX (New User Experience) item
///
/// Note: This is a temporary simplified version. Will be replaced with the
/// proper client type when the client module is translated.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Nux {
    pub id: String,
    pub completed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "expiresAt")]
    pub expires_at: Option<String>,
}

/// Validates a NUX item
///
/// Ensures that:
/// 1. ID is not empty and <= 64 characters
/// 2. Data (if present) is <= 300 characters
/// 3. ExpiresAt (if present) is a valid ISO 8601 datetime string
///
/// # Errors
///
/// Returns an error if validation fails.
///
/// # Examples
///
/// ```
/// use atproto::util::{Nux, validate_nux};
///
/// let nux = Nux {
///     id: "test-nux".to_string(),
///     completed: false,
///     data: Some("test data".to_string()),
///     expires_at: Some("2024-01-01T00:00:00Z".to_string()),
/// };
///
/// assert!(validate_nux(&nux).is_ok());
/// ```
pub fn validate_nux(nux: &Nux) -> Result<()> {
    if nux.id.is_empty() {
        return Err(UtilError::NuxValidationError(
            "id must not be empty".to_string(),
        ));
    }

    if nux.id.len() > 64 {
        return Err(UtilError::NuxValidationError(format!(
            "id must be <= 64 characters, got {}",
            nux.id.len()
        )));
    }

    if let Some(ref data) = nux.data {
        if data.len() > 300 {
            return Err(UtilError::NuxValidationError(format!(
                "data must be <= 300 characters, got {}",
                data.len()
            )));
        }
    }

    if let Some(ref expires_at) = nux.expires_at {
        // Validate ISO 8601 datetime format
        if chrono::DateTime::parse_from_rfc3339(expires_at).is_err() {
            return Err(UtilError::NuxValidationError(format!(
                "expiresAt must be a valid ISO 8601 datetime, got '{}'",
                expires_at
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_muted_word_value_basic() {
        assert_eq!(sanitize_muted_word_value("word"), "word");
        assert_eq!(sanitize_muted_word_value("  word  "), "word");
        assert_eq!(sanitize_muted_word_value("#word"), "word");
        assert_eq!(sanitize_muted_word_value("  #word  "), "word");
    }

    #[test]
    fn test_sanitize_muted_word_value_newlines() {
        assert_eq!(sanitize_muted_word_value("word\ntest"), "wordtest");
        assert_eq!(sanitize_muted_word_value("word\rtest"), "wordtest");
        assert_eq!(sanitize_muted_word_value("word\r\ntest"), "wordtest");
    }

    #[test]
    fn test_sanitize_muted_word_value_unicode_control() {
        // Soft hyphen (U+00AD)
        assert_eq!(sanitize_muted_word_value("word\u{00AD}test"), "wordtest");
        // Word joiner (U+2060)
        assert_eq!(sanitize_muted_word_value("word\u{2060}test"), "wordtest");
        // Zero-width joiner (U+200D)
        assert_eq!(sanitize_muted_word_value("word\u{200D}test"), "wordtest");
        // Zero-width non-joiner (U+200C)
        assert_eq!(sanitize_muted_word_value("word\u{200C}test"), "wordtest");
        // Zero-width space (U+200B)
        assert_eq!(sanitize_muted_word_value("word\u{200B}test"), "wordtest");
    }

    #[test]
    fn test_sanitize_muted_word_value_multiple_control_chars() {
        assert_eq!(
            sanitize_muted_word_value("word\n\r\u{200B}\u{00AD}test"),
            "wordtest"
        );
    }

    #[test]
    fn test_sanitize_muted_word_value_emoji_variation_selector() {
        // Hashtag followed by emoji variation selector should NOT be removed
        assert_eq!(sanitize_muted_word_value("#\u{FE0F}test"), "#\u{FE0F}test");

        // Regular hashtag should be removed
        assert_eq!(sanitize_muted_word_value("#test"), "test");
    }

    #[test]
    fn test_saved_feeds_to_uri_arrays_empty() {
        let feeds = vec![];
        let result = saved_feeds_to_uri_arrays(&feeds);
        assert_eq!(result.pinned.len(), 0);
        assert_eq!(result.saved.len(), 0);
    }

    #[test]
    fn test_saved_feeds_to_uri_arrays_no_pinned() {
        let feeds = vec![
            SavedFeed {
                id: "1".to_string(),
                feed_type: SavedFeedType::Feed,
                value: "at://did:plc:test/app.bsky.feed.generator/feed1".to_string(),
                pinned: false,
            },
            SavedFeed {
                id: "2".to_string(),
                feed_type: SavedFeedType::Feed,
                value: "at://did:plc:test/app.bsky.feed.generator/feed2".to_string(),
                pinned: false,
            },
        ];

        let result = saved_feeds_to_uri_arrays(&feeds);
        assert_eq!(result.pinned.len(), 0);
        assert_eq!(result.saved.len(), 2);
        assert_eq!(
            result.saved[0],
            "at://did:plc:test/app.bsky.feed.generator/feed1"
        );
    }

    #[test]
    fn test_saved_feeds_to_uri_arrays_with_pinned() {
        let feeds = vec![
            SavedFeed {
                id: "1".to_string(),
                feed_type: SavedFeedType::Feed,
                value: "at://did:plc:test/app.bsky.feed.generator/feed1".to_string(),
                pinned: true,
            },
            SavedFeed {
                id: "2".to_string(),
                feed_type: SavedFeedType::Feed,
                value: "at://did:plc:test/app.bsky.feed.generator/feed2".to_string(),
                pinned: false,
            },
            SavedFeed {
                id: "3".to_string(),
                feed_type: SavedFeedType::Feed,
                value: "at://did:plc:test/app.bsky.feed.generator/feed3".to_string(),
                pinned: true,
            },
        ];

        let result = saved_feeds_to_uri_arrays(&feeds);
        assert_eq!(result.pinned.len(), 2);
        assert_eq!(result.saved.len(), 3); // saved includes all feeds

        assert_eq!(
            result.pinned[0],
            "at://did:plc:test/app.bsky.feed.generator/feed1"
        );
        assert_eq!(
            result.pinned[1],
            "at://did:plc:test/app.bsky.feed.generator/feed3"
        );
    }

    #[test]
    fn test_at_uri_parse_valid() {
        let uri = AtUri::new("at://did:plc:test/app.bsky.feed.generator/feed1").unwrap();
        assert_eq!(uri.collection(), "app.bsky.feed.generator");

        let uri = AtUri::new("at://did:plc:test/app.bsky.graph.list/list1").unwrap();
        assert_eq!(uri.collection(), "app.bsky.graph.list");
    }

    #[test]
    fn test_at_uri_parse_invalid() {
        assert!(AtUri::new("http://example.com").is_err());
        assert!(AtUri::new("at://").is_err());
        assert!(AtUri::new("at://did:plc:test").is_err());
        assert!(AtUri::new("not-a-uri").is_err());
    }

    #[test]
    fn test_get_saved_feed_type_feed() {
        let feed_type =
            get_saved_feed_type("at://did:plc:test/app.bsky.feed.generator/feed1").unwrap();
        assert_eq!(feed_type, SavedFeedType::Feed);
    }

    #[test]
    fn test_get_saved_feed_type_list() {
        let feed_type =
            get_saved_feed_type("at://did:plc:test/app.bsky.graph.list/list1").unwrap();
        assert_eq!(feed_type, SavedFeedType::List);
    }

    #[test]
    fn test_get_saved_feed_type_unknown() {
        let feed_type =
            get_saved_feed_type("at://did:plc:test/app.bsky.other.collection/item1").unwrap();
        assert_eq!(feed_type, SavedFeedType::Unknown);
    }

    #[test]
    fn test_get_saved_feed_type_invalid_uri() {
        assert!(get_saved_feed_type("not-a-uri").is_err());
    }

    #[test]
    fn test_validate_saved_feed_valid() {
        let feed = SavedFeed {
            id: "123".to_string(),
            feed_type: SavedFeedType::Feed,
            value: "at://did:plc:test/app.bsky.feed.generator/feed1".to_string(),
            pinned: false,
        };

        assert!(validate_saved_feed(&feed).is_ok());
    }

    #[test]
    fn test_validate_saved_feed_missing_id() {
        let feed = SavedFeed {
            id: "".to_string(),
            feed_type: SavedFeedType::Feed,
            value: "at://did:plc:test/app.bsky.feed.generator/feed1".to_string(),
            pinned: false,
        };

        assert!(validate_saved_feed(&feed).is_err());
    }

    #[test]
    fn test_validate_saved_feed_type_mismatch() {
        // Says it's a feed but URI is for a list
        let feed = SavedFeed {
            id: "123".to_string(),
            feed_type: SavedFeedType::Feed,
            value: "at://did:plc:test/app.bsky.graph.list/list1".to_string(),
            pinned: false,
        };

        assert!(validate_saved_feed(&feed).is_err());

        // Says it's a list but URI is for a feed
        let feed = SavedFeed {
            id: "123".to_string(),
            feed_type: SavedFeedType::List,
            value: "at://did:plc:test/app.bsky.feed.generator/feed1".to_string(),
            pinned: false,
        };

        assert!(validate_saved_feed(&feed).is_err());
    }

    #[test]
    fn test_validate_saved_feed_unknown_type() {
        // Unknown type doesn't validate URI
        let feed = SavedFeed {
            id: "123".to_string(),
            feed_type: SavedFeedType::Unknown,
            value: "at://did:plc:test/app.bsky.other.collection/item1".to_string(),
            pinned: false,
        };

        assert!(validate_saved_feed(&feed).is_ok());
    }

    #[test]
    fn test_validate_nux_valid() {
        let nux = Nux {
            id: "test-nux".to_string(),
            completed: false,
            data: Some("test data".to_string()),
            expires_at: Some("2024-01-01T00:00:00Z".to_string()),
        };

        assert!(validate_nux(&nux).is_ok());
    }

    #[test]
    fn test_validate_nux_minimal() {
        let nux = Nux {
            id: "test".to_string(),
            completed: true,
            data: None,
            expires_at: None,
        };

        assert!(validate_nux(&nux).is_ok());
    }

    #[test]
    fn test_validate_nux_empty_id() {
        let nux = Nux {
            id: "".to_string(),
            completed: false,
            data: None,
            expires_at: None,
        };

        assert!(validate_nux(&nux).is_err());
    }

    #[test]
    fn test_validate_nux_id_too_long() {
        let nux = Nux {
            id: "a".repeat(65), // 65 characters, max is 64
            completed: false,
            data: None,
            expires_at: None,
        };

        assert!(validate_nux(&nux).is_err());
    }

    #[test]
    fn test_validate_nux_id_max_length() {
        let nux = Nux {
            id: "a".repeat(64), // exactly 64 characters
            completed: false,
            data: None,
            expires_at: None,
        };

        assert!(validate_nux(&nux).is_ok());
    }

    #[test]
    fn test_validate_nux_data_too_long() {
        let nux = Nux {
            id: "test".to_string(),
            completed: false,
            data: Some("a".repeat(301)), // 301 characters, max is 300
            expires_at: None,
        };

        assert!(validate_nux(&nux).is_err());
    }

    #[test]
    fn test_validate_nux_data_max_length() {
        let nux = Nux {
            id: "test".to_string(),
            completed: false,
            data: Some("a".repeat(300)), // exactly 300 characters
            expires_at: None,
        };

        assert!(validate_nux(&nux).is_ok());
    }

    #[test]
    fn test_validate_nux_invalid_datetime() {
        let nux = Nux {
            id: "test".to_string(),
            completed: false,
            data: None,
            expires_at: Some("not-a-date".to_string()),
        };

        assert!(validate_nux(&nux).is_err());
    }

    #[test]
    fn test_validate_nux_valid_datetime_formats() {
        // RFC3339 format
        let nux = Nux {
            id: "test".to_string(),
            completed: false,
            data: None,
            expires_at: Some("2024-01-01T00:00:00Z".to_string()),
        };
        assert!(validate_nux(&nux).is_ok());

        // With timezone offset
        let nux = Nux {
            id: "test".to_string(),
            completed: false,
            data: None,
            expires_at: Some("2024-01-01T00:00:00+05:00".to_string()),
        };
        assert!(validate_nux(&nux).is_ok());
    }
}
