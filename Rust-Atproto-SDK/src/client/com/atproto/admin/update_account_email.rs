//! Generated code for com.atproto.admin.updateAccountEmail
//!
//! Administrative action to update an account's email.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub email: String,
    /// The handle or DID of the repo.
    pub account: String,
}

/// Administrative action to update an account's email.
pub async fn update_account_email(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.admin.updateAccountEmail").data(&input)?;

    client.request(req).await
}
