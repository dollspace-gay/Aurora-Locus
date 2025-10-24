//! Generated code for chat.bsky.convo.acceptConvo
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
    /// Rev when the convo was accepted. If not present, the convo was already accepted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
}

/// chat.bsky.convo.acceptConvo
pub async fn accept_convo(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("chat.bsky.convo.acceptConvo").data(&input)?;

    client.request(req).await
}
