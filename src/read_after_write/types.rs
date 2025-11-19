// Allow dead_code - read-after-write types for future use
#![allow(dead_code)]

//! Types for read-after-write consistency

use serde::{Deserialize, Serialize};

/// Local records that haven't been indexed by AppView yet
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalRecords {
    /// Total count of local records
    pub count: usize,

    /// User's profile record (if updated locally)
    pub profile: Option<RecordDescript>,

    /// User's post records created locally
    pub posts: Vec<RecordDescript>,
}

impl LocalRecords {
    pub fn empty() -> Self {
        Self {
            count: 0,
            profile: None,
            posts: Vec::new(),
        }
    }
}

/// Description of a record stored locally
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordDescript {
    /// AT-URI of the record (at://did:plc:abc/app.bsky.feed.post/123)
    pub uri: String,

    /// CID of the record
    pub cid: String,

    /// Timestamp when indexed locally
    pub indexed_at: String,

    /// The record data (JSON value)
    pub record: serde_json::Value,
}

/// Profile view (basic)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileViewBasic {
    pub did: String,
    pub handle: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
}

/// Post view (formatted for feeds)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostView {
    pub uri: String,
    pub cid: String,
    pub author: ProfileViewBasic,
    pub record: serde_json::Value,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub embed: Option<serde_json::Value>,

    pub indexed_at: String,

    // Counts (presumed to be 0 for new posts)
    #[serde(default)]
    pub like_count: u64,

    #[serde(default)]
    pub reply_count: u64,

    #[serde(default)]
    pub repost_count: u64,

    #[serde(default)]
    pub quote_count: u64,
}
