//! Generated code for app.bsky.feed.repost
//!
//! Record representing a 'repost' of an existing Bluesky post.

use serde::{Deserialize, Serialize};

/// Record representing a 'repost' of an existing Bluesky post.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repost {
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<serde_json::Value>,
    pub subject: serde_json::Value,
}

