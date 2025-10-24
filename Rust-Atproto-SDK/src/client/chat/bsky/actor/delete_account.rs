//! Generated code for chat.bsky.actor.deleteAccount
//!

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
}

/// chat.bsky.actor.deleteAccount
pub async fn delete_account(
    client: &impl XrpcClient,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("chat.bsky.actor.deleteAccount");

    client.request(req).await
}
