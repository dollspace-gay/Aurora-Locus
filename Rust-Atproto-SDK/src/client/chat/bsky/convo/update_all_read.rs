//! Generated code for chat.bsky.convo.updateAllRead
//!

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    /// The count of updated convos.
    #[serde(rename = "updatedCount")]
    pub updated_count: i64,
}

/// chat.bsky.convo.updateAllRead
pub async fn update_all_read(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("chat.bsky.convo.updateAllRead").data(&input)?;

    client.request(req).await
}
