//! Generated type definitions for app.bsky.embed.record

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Main {
    pub record: serde_json::Value,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct View {
    pub record: serde_json::Value,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewRecord {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "repostCount")]
    pub repost_count: Option<i64>,
    /// The record data itself.
    pub value: serde_json::Value,
    pub cid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "quoteCount")]
    pub quote_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<serde_json::Value>,
    pub uri: crate::syntax::AtUri,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "replyCount")]
    pub reply_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embeds: Option<serde_json::Value>,
    #[serde(rename = "indexedAt")]
    pub indexed_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "likeCount")]
    pub like_count: Option<i64>,
    pub author: serde_json::Value,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewNotFound {
    pub uri: crate::syntax::AtUri,
    #[serde(rename = "notFound")]
    pub not_found: bool,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewBlocked {
    pub author: serde_json::Value,
    pub uri: crate::syntax::AtUri,
    pub blocked: bool,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewDetached {
    pub detached: bool,
    pub uri: crate::syntax::AtUri,
}


