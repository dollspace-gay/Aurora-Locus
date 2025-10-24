//! Generated code for app.bsky.feed.sendInteractions
//!
//! Send information about interactions with feed items back to the feed generator that served them.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub interactions: serde_json::Value,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
}

/// Send information about interactions with feed items back to the feed generator that served them.
pub async fn send_interactions(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("app.bsky.feed.sendInteractions").data(&input)?;

    client.request(req).await
}
