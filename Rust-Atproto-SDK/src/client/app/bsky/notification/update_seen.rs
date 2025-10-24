//! Generated code for app.bsky.notification.updateSeen
//!
//! Notify server that the requesting account has seen notifications. Requires auth.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    #[serde(rename = "seenAt")]
    pub seen_at: String,
}

/// Notify server that the requesting account has seen notifications. Requires auth.
pub async fn update_seen(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("app.bsky.notification.updateSeen").data(&input)?;

    client.request(req).await
}
