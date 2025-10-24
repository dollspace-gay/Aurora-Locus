//! Generated code for com.atproto.server.requestAccountDelete
//!
//! Initiate a user account deletion via email.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};

/// Initiate a user account deletion via email.
pub async fn request_account_delete(
    client: &impl XrpcClient,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.server.requestAccountDelete");

    client.request(req).await
}
