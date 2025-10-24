//! Generated code for app.bsky.feed.post
//!
//! Record containing a Bluesky post.

use serde::{Deserialize, Serialize};

/// Record containing a Bluesky post.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Post {
    /// Annotations of text (mentions, URLs, hashtags, etc)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub facets: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embed: Option<serde_json::Value>,
    /// Additional hashtags, in addition to any included in post text and facets.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<serde_json::Value>,
    /// Indicates human language of post primary text content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub langs: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply: Option<serde_json::Value>,
    /// The primary post content. May be an empty string, if there are embeds.
    pub text: String,
    /// DEPRECATED: replaced by app.bsky.richtext.facet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entities: Option<serde_json::Value>,
    /// Self-label values for this post. Effectively content warnings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<serde_json::Value>,
    /// Client-declared timestamp when this post was originally created.
    #[serde(rename = "createdAt")]
    pub created_at: String,
}

/// Deprecated. Use app.bsky.richtext instead -- A text segment. Start is inclusive, end is exclusive. Indices are for utf16-encoded strings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextSlice {
    /// Deprecated. Use app.bsky.richtext instead -- A text segment. Start is inclusive, end is exclusive. Indices are for utf16-encoded strings.
    pub start: i64,
    /// Deprecated. Use app.bsky.richtext instead -- A text segment. Start is inclusive, end is exclusive. Indices are for utf16-encoded strings.
    pub end: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplyRef {
    pub parent: serde_json::Value,
    pub root: serde_json::Value,
}

/// Deprecated: use facets instead.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    /// Deprecated: use facets instead.
    pub index: serde_json::Value,
    /// Expected values are 'mention' and 'link'.
    /// Deprecated: use facets instead.
    #[serde(rename = "type")]
    pub r#type: String,
    /// Deprecated: use facets instead.
    pub value: String,
}

