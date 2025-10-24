//! Generated code for app.bsky.feed.postgate
//!
//! Record defining interaction rules for a post. The record key (rkey) of the postgate record must match the record key of the post, and that record must be in the same repository.

use serde::{Deserialize, Serialize};

/// Record defining interaction rules for a post. The record key (rkey) of the postgate record must match the record key of the post, and that record must be in the same repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Postgate {
    /// List of rules defining who can embed this post. If value is an empty array or is undefined, no particular rules apply and anyone can embed.
    #[serde(rename = "embeddingRules")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding_rules: Option<serde_json::Value>,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    /// Reference (AT-URI) to the post record.
    pub post: crate::syntax::AtUri,
    /// List of AT-URIs embedding this post that the author has detached from.
    #[serde(rename = "detachedEmbeddingUris")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detached_embedding_uris: Option<serde_json::Value>,
}

/// Disables embedding of this post.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisableRule {
}

