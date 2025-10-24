//! Generated code for com.atproto.server.deleteSession
//!
//! Delete the current session. Requires auth.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};

/// Delete the current session. Requires auth.
pub async fn delete_session(
    client: &impl XrpcClient,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.server.deleteSession");

    client.request(req).await
}
