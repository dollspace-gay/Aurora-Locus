//! Generated code for app.bsky.graph.muteThread
//!
//! Mutes a thread preventing notifications from the thread and any of its children. Mutes are private in Bluesky. Requires auth.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub root: crate::syntax::AtUri,
}

/// Mutes a thread preventing notifications from the thread and any of its children. Mutes are private in Bluesky. Requires auth.
pub async fn mute_thread(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("app.bsky.graph.muteThread").data(&input)?;

    client.request(req).await
}
