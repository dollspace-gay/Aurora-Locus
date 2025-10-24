//! Generated code for app.bsky.notification.registerPush
//!
//! Register to receive push notifications, via a specified service, for the requesting account. Requires auth.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    /// Set to true when the actor is age restricted
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "ageRestricted")]
    pub age_restricted: Option<bool>,
    #[serde(rename = "serviceDid")]
    pub service_did: crate::types::Did,
    pub token: String,
    #[serde(rename = "appId")]
    pub app_id: String,
    pub platform: String,
}

/// Register to receive push notifications, via a specified service, for the requesting account. Requires auth.
pub async fn register_push(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("app.bsky.notification.registerPush").data(&input)?;

    client.request(req).await
}
