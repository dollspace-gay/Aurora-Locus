//! Generated code for chat.bsky.actor.exportAccountData
//!

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};

/// chat.bsky.actor.exportAccountData
pub async fn export_account_data(
    client: &impl XrpcClient,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::query("chat.bsky.actor.exportAccountData");

    client.request(req).await
}
