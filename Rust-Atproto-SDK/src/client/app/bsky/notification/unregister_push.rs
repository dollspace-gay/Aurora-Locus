//! Generated code for app.bsky.notification.unregisterPush
//!
//! The inverse of registerPush - inform a specified service that push notifications should no longer be sent to the given token for the requesting account. Requires auth.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub platform: String,
    #[serde(rename = "appId")]
    pub app_id: String,
    #[serde(rename = "serviceDid")]
    pub service_did: crate::types::Did,
    pub token: String,
}

/// The inverse of registerPush - inform a specified service that push notifications should no longer be sent to the given token for the requesting account. Requires auth.
pub async fn unregister_push(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("app.bsky.notification.unregisterPush").data(&input)?;

    client.request(req).await
}
