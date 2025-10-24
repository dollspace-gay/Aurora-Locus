//! Generated code for app.bsky.graph.muteActor
//!
//! Creates a mute relationship for the specified account. Mutes are private in Bluesky. Requires auth.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub actor: String,
}

/// Creates a mute relationship for the specified account. Mutes are private in Bluesky. Requires auth.
pub async fn mute_actor(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("app.bsky.graph.muteActor").data(&input)?;

    client.request(req).await
}
