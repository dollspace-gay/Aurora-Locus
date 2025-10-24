//! Generated code for com.atproto.server.confirmEmail
//!
//! Confirm an email using a token from com.atproto.server.requestEmailConfirmation.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub token: String,
    pub email: String,
}

/// Confirm an email using a token from com.atproto.server.requestEmailConfirmation.
pub async fn confirm_email(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.server.confirmEmail").data(&input)?;

    client.request(req).await
}
