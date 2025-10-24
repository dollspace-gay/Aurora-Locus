//! Generated code for app.bsky.feed.like
//!
//! Record declaring a 'like' of a piece of subject content.

use serde::{Deserialize, Serialize};

/// Record declaring a 'like' of a piece of subject content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Like {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<serde_json::Value>,
    pub subject: serde_json::Value,
    #[serde(rename = "createdAt")]
    pub created_at: String,
}

