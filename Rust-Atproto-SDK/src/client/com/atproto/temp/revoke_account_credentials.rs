//! Generated code for com.atproto.temp.revokeAccountCredentials
//!
//! Revoke sessions, password, and app passwords associated with account. May be resolved by a password reset.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub account: String,
}

/// Revoke sessions, password, and app passwords associated with account. May be resolved by a password reset.
pub async fn revoke_account_credentials(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.temp.revokeAccountCredentials").data(&input)?;

    client.request(req).await
}
