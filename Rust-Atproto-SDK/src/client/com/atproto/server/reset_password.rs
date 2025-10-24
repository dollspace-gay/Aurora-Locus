//! Generated code for com.atproto.server.resetPassword
//!
//! Reset a user account password using a token.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub token: String,
    pub password: String,
}

/// Reset a user account password using a token.
pub async fn reset_password(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.server.resetPassword").data(&input)?;

    client.request(req).await
}
