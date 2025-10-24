//! Generated code for app.bsky.graph.unmuteActorList
//!
//! Unmutes the specified list of accounts. Requires auth.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub list: crate::syntax::AtUri,
}

/// Unmutes the specified list of accounts. Requires auth.
pub async fn unmute_actor_list(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("app.bsky.graph.unmuteActorList").data(&input)?;

    client.request(req).await
}
