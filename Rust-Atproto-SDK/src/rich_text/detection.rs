//! Facet detection for rich text
//!
//! This module provides functionality to detect facets (mentions, links, tags) in text.
//! Facets are structured metadata about text segments, used in ATProto for rich text features.

use serde::{Deserialize, Serialize};

use super::unicode::UnicodeString;
use super::util::{MENTION_REGEX, TAG_REGEX, TRAILING_PUNCTUATION_REGEX, URL_REGEX};

/// A rich text facet representing a mention, link, or tag
///
/// Facets use UTF-8 byte indices to mark text segments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Facet {
    /// Byte range in UTF-8 encoding
    pub index: ByteSlice,

    /// Facet features (mention, link, or tag)
    pub features: Vec<FacetFeature>,
}

/// Byte range for a facet
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ByteSlice {
    /// Start byte index (inclusive)
    pub byte_start: usize,

    /// End byte index (exclusive)
    pub byte_end: usize,
}

/// A facet feature (mention, link, or tag)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "$type")]
pub enum FacetFeature {
    /// A mention of a user (@handle)
    #[serde(rename = "app.bsky.richtext.facet#mention")]
    Mention { did: String },

    /// A clickable link
    #[serde(rename = "app.bsky.richtext.facet#link")]
    Link { uri: String },

    /// A hashtag
    #[serde(rename = "app.bsky.richtext.facet#tag")]
    Tag { tag: String },
}

/// Detects facets (mentions, links, tags) in the given text
///
/// # Arguments
///
/// * `text` - The text to scan for facets
///
/// # Returns
///
/// An optional vector of facets. Returns `None` if no facets are found.
///
/// # Examples
///
/// ```
/// use atproto::rich_text::{UnicodeString, detect_facets};
///
/// let text = UnicodeString::new("Hello @alice.bsky.social check https://example.com #cool");
/// let facets = detect_facets(&text);
/// assert!(facets.is_some());
/// assert_eq!(facets.unwrap().len(), 3); // mention, link, tag
/// ```
pub fn detect_facets(text: &UnicodeString) -> Option<Vec<Facet>> {
    let mut facets = Vec::new();

    // Detect mentions
    for captures in MENTION_REGEX.captures_iter(text.as_str()) {
        if let Some(handle_match) = captures.get(3) {
            let handle = handle_match.as_str();

            // Validate domain (skip invalid handles)
            if !is_valid_domain(handle) && !handle.ends_with(".test") {
                continue;
            }

            // Find the @ symbol position
            let match_start = captures.get(0).unwrap().start();
            let handle_start = text.as_str()[match_start..]
                .find('@')
                .map(|i| match_start + i)
                .unwrap_or(handle_match.start());

            let byte_start = handle_start;
            let byte_end = handle_start + handle.len() + 1; // +1 for @

            facets.push(Facet {
                index: ByteSlice {
                    byte_start,
                    byte_end,
                },
                features: vec![FacetFeature::Mention {
                    did: handle.to_string(), // Must be resolved to actual DID afterwards
                }],
            });
        }
    }

    // Detect links
    for captures in URL_REGEX.captures_iter(text.as_str()) {
        if let Some(url_match) = captures.get(2) {
            let mut uri = url_match.as_str().to_string();

            // Validate domain for bare URLs
            if !uri.starts_with("http") {
                if let Some(domain) = captures.name("domain") {
                    if !is_valid_domain(domain.as_str()) {
                        continue;
                    }
                    uri = format!("https://{}", uri);
                } else {
                    continue;
                }
            }

            let start = url_match.start();
            let mut end = url_match.end();

            // Strip ending punctuation
            let punctuation = ['.', ',', ';', ':', '!', '?'];
            if uri.ends_with(&punctuation[..]) {
                uri = uri[..uri.len() - 1].to_string();
                end -= 1;
            }

            // Strip trailing ) if there's no matching (
            if uri.ends_with(')') && !uri.contains('(') {
                uri = uri[..uri.len() - 1].to_string();
                end -= 1;
            }

            facets.push(Facet {
                index: ByteSlice {
                    byte_start: start,
                    byte_end: end,
                },
                features: vec![FacetFeature::Link { uri }],
            });
        }
    }

    // Detect tags
    for captures in TAG_REGEX.captures_iter(text.as_str()) {
        if let Some(tag_match) = captures.get(2) {
            let leading = captures.get(1).map(|m| m.as_str()).unwrap_or("");
            let mut tag = tag_match.as_str();

            if tag.is_empty() {
                continue;
            }

            // Strip ending punctuation and spaces
            tag = tag.trim();
            let tag_cleaned = TRAILING_PUNCTUATION_REGEX.replace(tag, "");
            let tag = tag_cleaned.as_ref();

            if tag.is_empty() || tag.len() > 64 {
                continue;
            }

            let index = captures.get(0).unwrap().start() + leading.len();

            facets.push(Facet {
                index: ByteSlice {
                    byte_start: index,
                    byte_end: index + 1 + tag.len(), // +1 for # symbol
                },
                features: vec![FacetFeature::Tag {
                    tag: tag.to_string(),
                }],
            });
        }
    }

    if facets.is_empty() {
        None
    } else {
        Some(facets)
    }
}

/// Validates that a string is a valid domain name by checking for known TLDs
///
/// This is a simplified implementation that checks for common TLDs.
/// The TypeScript version uses a comprehensive TLDs package with all known TLDs.
fn is_valid_domain(domain: &str) -> bool {
    // Common TLDs - in a full implementation, this should use a comprehensive list
    const COMMON_TLDS: &[&str] = &[
        "com", "org", "net", "edu", "gov", "mil", "int",
        "io", "co", "uk", "us", "ca", "de", "fr", "jp", "cn", "au", "br", "in",
        "app", "dev", "tech", "ai", "cloud", "online", "site", "blog", "shop",
        "social", "xyz", "me", "tv", "cc", "biz", "info", "name", "pro",
        "test", // for testing
    ];

    COMMON_TLDS.iter().any(|&tld| {
        if let Some(i) = domain.rfind(tld) {
            // Check if this is a proper TLD (preceded by . and at the end)
            i > 0 && domain.as_bytes().get(i - 1) == Some(&b'.') && i + tld.len() == domain.len()
        } else {
            false
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_valid_domain_com() {
        assert!(is_valid_domain("example.com"));
    }

    #[test]
    fn test_is_valid_domain_subdomain() {
        assert!(is_valid_domain("sub.example.com"));
    }

    #[test]
    fn test_is_valid_domain_io() {
        assert!(is_valid_domain("bsky.app"));
    }

    #[test]
    fn test_is_valid_domain_invalid() {
        assert!(!is_valid_domain("example"));
        assert!(!is_valid_domain("example.notarealthing"));
    }

    #[test]
    fn test_is_valid_domain_test() {
        assert!(is_valid_domain("example.test"));
    }

    #[test]
    fn test_detect_facets_mention() {
        let text = UnicodeString::new("Hello @alice.com");
        let facets = detect_facets(&text);
        assert!(facets.is_some());
        let facets = facets.unwrap();
        assert_eq!(facets.len(), 1);

        match &facets[0].features[0] {
            FacetFeature::Mention { did } => assert_eq!(did, "alice.com"),
            _ => panic!("Expected mention facet"),
        }
    }

    #[test]
    fn test_detect_facets_link_https() {
        let text = UnicodeString::new("Check https://example.com");
        let facets = detect_facets(&text);
        assert!(facets.is_some());
        let facets = facets.unwrap();
        assert_eq!(facets.len(), 1);

        match &facets[0].features[0] {
            FacetFeature::Link { uri } => assert_eq!(uri, "https://example.com"),
            _ => panic!("Expected link facet"),
        }
    }

    #[test]
    fn test_detect_facets_link_bare_domain() {
        let text = UnicodeString::new("Visit example.com");
        let facets = detect_facets(&text);
        assert!(facets.is_some());
        let facets = facets.unwrap();
        assert_eq!(facets.len(), 1);

        match &facets[0].features[0] {
            FacetFeature::Link { uri } => assert_eq!(uri, "https://example.com"),
            _ => panic!("Expected link facet"),
        }
    }

    #[test]
    fn test_detect_facets_tag() {
        let text = UnicodeString::new("This is #awesome");
        let facets = detect_facets(&text);
        assert!(facets.is_some());
        let facets = facets.unwrap();
        assert_eq!(facets.len(), 1);

        match &facets[0].features[0] {
            FacetFeature::Tag { tag } => assert_eq!(tag, "awesome"),
            _ => panic!("Expected tag facet"),
        }
    }

    #[test]
    fn test_detect_facets_multiple() {
        let text = UnicodeString::new("@alice.com check https://example.com #cool");
        let facets = detect_facets(&text);
        assert!(facets.is_some());
        let facets = facets.unwrap();
        assert_eq!(facets.len(), 3);
    }

    #[test]
    fn test_detect_facets_none() {
        let text = UnicodeString::new("Just plain text");
        let facets = detect_facets(&text);
        assert!(facets.is_none());
    }

    #[test]
    fn test_detect_facets_invalid_mention() {
        // Mention without valid TLD should be skipped
        let text = UnicodeString::new("Hello @notvalid");
        let facets = detect_facets(&text);
        assert!(facets.is_none());
    }

    #[test]
    fn test_detect_facets_link_with_trailing_punctuation() {
        let text = UnicodeString::new("Check https://example.com.");
        let facets = detect_facets(&text);
        assert!(facets.is_some());
        let facets = facets.unwrap();

        match &facets[0].features[0] {
            FacetFeature::Link { uri } => assert_eq!(uri, "https://example.com"),
            _ => panic!("Expected link facet"),
        }
    }

    #[test]
    fn test_detect_facets_link_with_paren() {
        let text = UnicodeString::new("Check https://example.com)");
        let facets = detect_facets(&text);
        assert!(facets.is_some());
        let facets = facets.unwrap();

        match &facets[0].features[0] {
            FacetFeature::Link { uri } => assert_eq!(uri, "https://example.com"),
            _ => panic!("Expected link facet"),
        }
    }

    #[test]
    fn test_detect_facets_link_with_balanced_parens() {
        let text = UnicodeString::new("Check https://example.com/(foo)");
        let facets = detect_facets(&text);
        assert!(facets.is_some());
        let facets = facets.unwrap();

        match &facets[0].features[0] {
            FacetFeature::Link { uri } => assert_eq!(uri, "https://example.com/(foo)"),
            _ => panic!("Expected link facet"),
        }
    }

    #[test]
    fn test_detect_facets_tag_too_long() {
        let long_tag = "a".repeat(65);
        let text = UnicodeString::new(format!("#{}", long_tag));
        let facets = detect_facets(&text);
        assert!(facets.is_none());
    }

    #[test]
    fn test_detect_facets_tag_with_punctuation() {
        let text = UnicodeString::new("#cool!!!");
        let facets = detect_facets(&text);
        assert!(facets.is_some());
        let facets = facets.unwrap();

        match &facets[0].features[0] {
            FacetFeature::Tag { tag } => assert_eq!(tag, "cool"),
            _ => panic!("Expected tag facet"),
        }
    }

    #[test]
    fn test_facet_serialization() {
        let facet = Facet {
            index: ByteSlice {
                byte_start: 0,
                byte_end: 10,
            },
            features: vec![FacetFeature::Mention {
                did: "alice.com".to_string(),
            }],
        };

        let json = serde_json::to_string(&facet).unwrap();
        assert!(json.contains("byteStart"));
        assert!(json.contains("byteEnd"));
        assert!(json.contains("app.bsky.richtext.facet#mention"));
    }

    #[test]
    fn test_facet_deserialization() {
        let json = r#"{
            "index": {"byteStart": 0, "byteEnd": 10},
            "features": [{
                "$type": "app.bsky.richtext.facet#mention",
                "did": "alice.com"
            }]
        }"#;

        let facet: Facet = serde_json::from_str(json).unwrap();
        assert_eq!(facet.index.byte_start, 0);
        assert_eq!(facet.index.byte_end, 10);
        assert_eq!(facet.features.len(), 1);
    }
}
