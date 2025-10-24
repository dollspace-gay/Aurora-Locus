//! Generated code for app.bsky.graph.unmuteThread
//!
//! Unmutes the specified thread. Requires auth.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub root: crate::syntax::AtUri,
}

/// Unmutes the specified thread. Requires auth.
pub async fn unmute_thread(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("app.bsky.graph.unmuteThread").data(&input)?;

    client.request(req).await
}
