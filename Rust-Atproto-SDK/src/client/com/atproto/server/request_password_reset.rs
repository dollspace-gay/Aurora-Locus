//! Generated code for com.atproto.server.requestPasswordReset
//!
//! Initiate a user account password reset via email.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub email: String,
}

/// Initiate a user account password reset via email.
pub async fn request_password_reset(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.server.requestPasswordReset").data(&input)?;

    client.request(req).await
}
