//! Generated code for com.atproto.server.listAppPasswords
//!
//! List all App Passwords.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    pub passwords: serde_json::Value,
}

/// Error: AccountTakedown
#[derive(Debug, Clone, thiserror::Error)]
#[error("AccountTakedown")]
pub struct AccountTakedownError;

/// List all App Passwords.
pub async fn list_app_passwords(
    client: &impl XrpcClient,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::query("com.atproto.server.listAppPasswords");

    client.request(req).await
}
