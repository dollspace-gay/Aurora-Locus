//! Generated type definitions for app.bsky.unspecced.defs

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkeletonSearchStarterPack {
    pub uri: crate::syntax::AtUri,
}


/// Object used to store age assurance data in stash.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgeAssuranceEvent {
    /// The IP address used when initiating the AA flow.
    /// Object used to store age assurance data in stash.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "initIp")]
    pub init_ip: Option<String>,
    /// The user agent used when initiating the AA flow.
    /// Object used to store age assurance data in stash.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "initUa")]
    pub init_ua: Option<String>,
    /// The IP address used when completing the AA flow.
    /// Object used to store age assurance data in stash.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "completeIp")]
    pub complete_ip: Option<String>,
    /// The status of the age assurance process.
    /// Object used to store age assurance data in stash.
    pub status: String,
    /// The user agent used when completing the AA flow.
    /// Object used to store age assurance data in stash.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "completeUa")]
    pub complete_ua: Option<String>,
    /// The unique identifier for this instance of the age assurance flow, in UUID format.
    /// Object used to store age assurance data in stash.
    #[serde(rename = "attemptId")]
    pub attempt_id: String,
    /// The email used for AA.
    /// Object used to store age assurance data in stash.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// The date and time of this write operation.
    /// Object used to store age assurance data in stash.
    #[serde(rename = "createdAt")]
    pub created_at: String,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadItemPost {
    /// This post has more replies that were not present in the response. This is a numeric value, which is best-effort and might not be accurate.
    #[serde(rename = "moreReplies")]
    pub more_replies: i64,
    /// This is by an account muted by the viewer requesting it.
    #[serde(rename = "mutedByViewer")]
    pub muted_by_viewer: bool,
    /// This post is part of a contiguous thread by the OP from the thread root. Many different OP threads can happen in the same thread.
    #[serde(rename = "opThread")]
    pub op_thread: bool,
    pub post: serde_json::Value,
    /// This post has more parents that were not present in the response. This is just a boolean, without the number of parents.
    #[serde(rename = "moreParents")]
    pub more_parents: bool,
    /// The threadgate created by the author indicates this post as a reply to be hidden for everyone consuming the thread.
    #[serde(rename = "hiddenByThreadgate")]
    pub hidden_by_threadgate: bool,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrendingTopic {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    pub topic: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub link: String,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkeletonTrend {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    pub topic: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    pub link: String,
    #[serde(rename = "startedAt")]
    pub started_at: String,
    #[serde(rename = "postCount")]
    pub post_count: i64,
    pub dids: serde_json::Value,
}


/// The computed state of the age assurance process, returned to the user in question on certain authenticated requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgeAssuranceState {
    /// The timestamp when this state was last updated.
    /// The computed state of the age assurance process, returned to the user in question on certain authenticated requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "lastInitiatedAt")]
    pub last_initiated_at: Option<String>,
    /// The status of the age assurance process.
    /// The computed state of the age assurance process, returned to the user in question on certain authenticated requests.
    pub status: String,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkeletonSearchPost {
    pub uri: crate::syntax::AtUri,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrendView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(rename = "postCount")]
    pub post_count: i64,
    pub topic: String,
    pub link: String,
    pub actors: serde_json::Value,
    #[serde(rename = "startedAt")]
    pub started_at: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadItemNotFound {
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadItemNoUnauthenticated {
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkeletonSearchActor {
    pub did: crate::types::Did,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadItemBlocked {
    pub author: serde_json::Value,
}


