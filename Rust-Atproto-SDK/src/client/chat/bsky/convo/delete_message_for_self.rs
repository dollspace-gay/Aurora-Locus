//! Generated code for chat.bsky.convo.deleteMessageForSelf
//!

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    #[serde(rename = "messageId")]
    pub message_id: String,
    #[serde(rename = "convoId")]
    pub convo_id: String,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
}

/// chat.bsky.convo.deleteMessageForSelf
pub async fn delete_message_for_self(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("chat.bsky.convo.deleteMessageForSelf").data(&input)?;

    client.request(req).await
}
