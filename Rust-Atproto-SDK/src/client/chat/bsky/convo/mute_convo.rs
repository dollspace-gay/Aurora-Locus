//! Generated code for chat.bsky.convo.muteConvo
//!

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    #[serde(rename = "convoId")]
    pub convo_id: String,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    pub convo: serde_json::Value,
}

/// chat.bsky.convo.muteConvo
pub async fn mute_convo(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("chat.bsky.convo.muteConvo").data(&input)?;

    client.request(req).await
}
