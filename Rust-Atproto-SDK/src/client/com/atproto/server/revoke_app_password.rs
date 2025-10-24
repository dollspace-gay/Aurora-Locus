//! Generated code for com.atproto.server.revokeAppPassword
//!
//! Revoke an App Password by name.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub name: String,
}

/// Revoke an App Password by name.
pub async fn revoke_app_password(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.server.revokeAppPassword").data(&input)?;

    client.request(req).await
}
