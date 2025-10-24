//! Generated code for com.atproto.admin.enableAccountInvites
//!
//! Re-enable an account's ability to receive invite codes.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub account: crate::types::Did,
    /// Optional reason for enabled invites.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Re-enable an account's ability to receive invite codes.
pub async fn enable_account_invites(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.admin.enableAccountInvites").data(&input)?;

    client.request(req).await
}
