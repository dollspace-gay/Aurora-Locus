//! Generated code for app.bsky.notification.putActivitySubscription
//!
//! Puts an activity subscription entry. The key should be omitted for creation and provided for updates. Requires auth.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub subject: crate::types::Did,
    #[serde(rename = "activitySubscription")]
    pub activity_subscription: serde_json::Value,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    pub subject: crate::types::Did,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "activitySubscription")]
    pub activity_subscription: Option<serde_json::Value>,
}

/// Puts an activity subscription entry. The key should be omitted for creation and provided for updates. Requires auth.
pub async fn put_activity_subscription(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("app.bsky.notification.putActivitySubscription").data(&input)?;

    client.request(req).await
}
