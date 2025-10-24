//! Generated code for com.atproto.server.activateAccount
//!
//! Activates a currently deactivated account. Used to finalize account migration after the account's repo is imported and identity is setup.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};

/// Activates a currently deactivated account. Used to finalize account migration after the account's repo is imported and identity is setup.
pub async fn activate_account(
    client: &impl XrpcClient,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.server.activateAccount");

    client.request(req).await
}
