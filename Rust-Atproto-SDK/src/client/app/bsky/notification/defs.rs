//! Generated type definitions for app.bsky.notification.defs

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordDeleted {
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterablePreference {
    pub push: bool,
    pub include: String,
    pub list: bool,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatPreference {
    pub push: bool,
    pub include: String,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preferences {
    pub repost: serde_json::Value,
    pub unverified: serde_json::Value,
    pub mention: serde_json::Value,
    pub like: serde_json::Value,
    pub verified: serde_json::Value,
    pub chat: serde_json::Value,
    #[serde(rename = "likeViaRepost")]
    pub like_via_repost: serde_json::Value,
    pub follow: serde_json::Value,
    pub reply: serde_json::Value,
    #[serde(rename = "repostViaRepost")]
    pub repost_via_repost: serde_json::Value,
    #[serde(rename = "subscribedPost")]
    pub subscribed_post: serde_json::Value,
    #[serde(rename = "starterpackJoined")]
    pub starterpack_joined: serde_json::Value,
    pub quote: serde_json::Value,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preference {
    pub list: bool,
    pub push: bool,
}


/// Object used to store activity subscription data in stash.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubjectActivitySubscription {
    /// Object used to store activity subscription data in stash.
    pub subject: crate::types::Did,
    /// Object used to store activity subscription data in stash.
    #[serde(rename = "activitySubscription")]
    pub activity_subscription: serde_json::Value,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivitySubscription {
    pub reply: bool,
    pub post: bool,
}


