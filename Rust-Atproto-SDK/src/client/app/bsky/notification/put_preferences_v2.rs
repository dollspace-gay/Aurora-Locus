//! Generated code for app.bsky.notification.putPreferencesV2
//!
//! Set notification-related preferences for an account. Requires auth.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "repostViaRepost")]
    pub repost_via_repost: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub like: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "likeViaRepost")]
    pub like_via_repost: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub follow: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "starterpackJoined")]
    pub starterpack_joined: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "subscribedPost")]
    pub subscribed_post: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mention: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unverified: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repost: Option<serde_json::Value>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    pub preferences: serde_json::Value,
}

/// Set notification-related preferences for an account. Requires auth.
pub async fn put_preferences_v2(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("app.bsky.notification.putPreferencesV2").data(&input)?;

    client.request(req).await
}
