//! Generated code for app.bsky.graph.unmuteActor
//!
//! Unmutes the specified account. Requires auth.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub actor: String,
}

/// Unmutes the specified account. Requires auth.
pub async fn unmute_actor(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("app.bsky.graph.unmuteActor").data(&input)?;

    client.request(req).await
}
