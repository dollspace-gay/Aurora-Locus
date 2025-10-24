//! Generated code for com.atproto.server.requestEmailConfirmation
//!
//! Request an email with a code to confirm ownership of email.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};

/// Request an email with a code to confirm ownership of email.
pub async fn request_email_confirmation(
    client: &impl XrpcClient,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.server.requestEmailConfirmation");

    client.request(req).await
}
