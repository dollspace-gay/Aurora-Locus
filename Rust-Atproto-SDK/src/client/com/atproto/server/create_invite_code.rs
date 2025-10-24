//! Generated code for com.atproto.server.createInviteCode
//!
//! Create an invite code.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "forAccount")]
    pub for_account: Option<crate::types::Did>,
    #[serde(rename = "useCount")]
    pub use_count: i64,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    pub code: String,
}

/// Create an invite code.
pub async fn create_invite_code(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.server.createInviteCode").data(&input)?;

    client.request(req).await
}
