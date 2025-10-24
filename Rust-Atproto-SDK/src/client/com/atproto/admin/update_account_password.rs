//! Generated code for com.atproto.admin.updateAccountPassword
//!
//! Update the password for a user account as an administrator.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub did: crate::types::Did,
    pub password: String,
}

/// Update the password for a user account as an administrator.
pub async fn update_account_password(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.admin.updateAccountPassword").data(&input)?;

    client.request(req).await
}
