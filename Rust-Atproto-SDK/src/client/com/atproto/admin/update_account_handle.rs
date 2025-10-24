//! Generated code for com.atproto.admin.updateAccountHandle
//!
//! Administrative action to update an account's handle.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub handle: String,
    pub did: crate::types::Did,
}

/// Administrative action to update an account's handle.
pub async fn update_account_handle(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.admin.updateAccountHandle").data(&input)?;

    client.request(req).await
}
