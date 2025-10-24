//! Generated code for com.atproto.admin.deleteAccount
//!
//! Delete a user account as an administrator.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub did: crate::types::Did,
}

/// Delete a user account as an administrator.
pub async fn delete_account(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.admin.deleteAccount").data(&input)?;

    client.request(req).await
}
