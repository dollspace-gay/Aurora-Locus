//! Generated code for com.atproto.admin.disableAccountInvites
//!
//! Disable an account from receiving new invite codes, but does not invalidate existing codes.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub account: crate::types::Did,
    /// Optional reason for disabled invites.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Disable an account from receiving new invite codes, but does not invalidate existing codes.
pub async fn disable_account_invites(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.admin.disableAccountInvites").data(&input)?;

    client.request(req).await
}
