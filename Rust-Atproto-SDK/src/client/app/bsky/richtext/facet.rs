//! Generated type definitions for app.bsky.richtext.facet

use serde::{Deserialize, Serialize};

/// Facet feature for a URL. The text URL may have been simplified or truncated, but the facet reference should be a complete URL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Link {
    /// Facet feature for a URL. The text URL may have been simplified or truncated, but the facet reference should be a complete URL.
    pub uri: String,
}


/// Facet feature for a hashtag. The text usually includes a '#' prefix, but the facet reference should not (except in the case of 'double hash tags').
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tag {
    /// Facet feature for a hashtag. The text usually includes a '#' prefix, but the facet reference should not (except in the case of 'double hash tags').
    pub tag: String,
}


/// Specifies the sub-string range a facet feature applies to. Start index is inclusive, end index is exclusive. Indices are zero-indexed, counting bytes of the UTF-8 encoded text. NOTE: some languages, like Javascript, use UTF-16 or Unicode codepoints for string slice indexing; in these languages, convert to byte arrays before working with facets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ByteSlice {
    /// Specifies the sub-string range a facet feature applies to. Start index is inclusive, end index is exclusive. Indices are zero-indexed, counting bytes of the UTF-8 encoded text. NOTE: some languages, like Javascript, use UTF-16 or Unicode codepoints for string slice indexing; in these languages, convert to byte arrays before working with facets.
    #[serde(rename = "byteEnd")]
    pub byte_end: i64,
    /// Specifies the sub-string range a facet feature applies to. Start index is inclusive, end index is exclusive. Indices are zero-indexed, counting bytes of the UTF-8 encoded text. NOTE: some languages, like Javascript, use UTF-16 or Unicode codepoints for string slice indexing; in these languages, convert to byte arrays before working with facets.
    #[serde(rename = "byteStart")]
    pub byte_start: i64,
}


/// Annotation of a sub-string within rich text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Main {
    /// Annotation of a sub-string within rich text.
    pub features: serde_json::Value,
    /// Annotation of a sub-string within rich text.
    pub index: serde_json::Value,
}


/// Facet feature for mention of another account. The text is usually a handle, including a '@' prefix, but the facet reference is a DID.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mention {
    /// Facet feature for mention of another account. The text is usually a handle, including a '@' prefix, but the facet reference is a DID.
    pub did: crate::types::Did,
}


