//! Generated code for chat.bsky.convo.sendMessage
//!

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub message: serde_json::Value,
    #[serde(rename = "convoId")]
    pub convo_id: String,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
}

/// chat.bsky.convo.sendMessage
pub async fn send_message(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("chat.bsky.convo.sendMessage").data(&input)?;

    client.request(req).await
}
