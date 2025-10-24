//! Generated code for chat.bsky.moderation.updateActorAccess
//!

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "ref")]
    pub r#ref: Option<String>,
    pub actor: crate::types::Did,
    #[serde(rename = "allowAccess")]
    pub allow_access: bool,
}

/// chat.bsky.moderation.updateActorAccess
pub async fn update_actor_access(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("chat.bsky.moderation.updateActorAccess").data(&input)?;

    client.request(req).await
}
