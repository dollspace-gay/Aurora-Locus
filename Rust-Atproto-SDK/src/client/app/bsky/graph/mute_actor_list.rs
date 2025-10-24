//! Generated code for app.bsky.graph.muteActorList
//!
//! Creates a mute relationship for the specified list of accounts. Mutes are private in Bluesky. Requires auth.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub list: crate::syntax::AtUri,
}

/// Creates a mute relationship for the specified list of accounts. Mutes are private in Bluesky. Requires auth.
pub async fn mute_actor_list(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("app.bsky.graph.muteActorList").data(&input)?;

    client.request(req).await
}
