//! Generated code for app.bsky.feed.threadgate
//!
//! Record defining interaction gating rules for a thread (aka, reply controls). The record key (rkey) of the threadgate record must match the record key of the thread's root post, and that record must be in the same repository.

use serde::{Deserialize, Serialize};

/// Record defining interaction gating rules for a thread (aka, reply controls). The record key (rkey) of the threadgate record must match the record key of the thread's root post, and that record must be in the same repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Threadgate {
    /// List of rules defining who can reply to this post. If value is an empty array, no one can reply. If value is undefined, anyone can reply.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow: Option<serde_json::Value>,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    /// List of hidden reply URIs.
    #[serde(rename = "hiddenReplies")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hidden_replies: Option<serde_json::Value>,
    /// Reference (AT-URI) to the post record.
    pub post: crate::syntax::AtUri,
}

/// Allow replies from actors who follow you.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FollowerRule {
}

/// Allow replies from actors you follow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FollowingRule {
}

/// Allow replies from actors mentioned in your post.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MentionRule {
}

/// Allow replies from actors on a list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListRule {
    /// Allow replies from actors on a list.
    pub list: crate::syntax::AtUri,
}

