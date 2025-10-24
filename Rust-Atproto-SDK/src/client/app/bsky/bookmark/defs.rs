//! Generated type definitions for app.bsky.bookmark.defs

use serde::{Deserialize, Serialize};

/// Object used to store bookmark data in stash.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bookmark {
    /// A strong ref to the record to be bookmarked. Currently, only `app.bsky.feed.post` records are supported.
    /// Object used to store bookmark data in stash.
    pub subject: serde_json::Value,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookmarkView {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "createdAt")]
    pub created_at: Option<String>,
    /// A strong ref to the bookmarked record.
    pub subject: serde_json::Value,
    pub item: serde_json::Value,
}


