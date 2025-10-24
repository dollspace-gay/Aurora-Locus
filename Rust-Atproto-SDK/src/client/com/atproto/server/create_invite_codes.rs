//! Generated code for com.atproto.server.createInviteCodes
//!
//! Create invite codes.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    #[serde(rename = "codeCount")]
    pub code_count: i64,
    #[serde(rename = "useCount")]
    pub use_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "forAccounts")]
    pub for_accounts: Option<serde_json::Value>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    pub codes: serde_json::Value,
}

/// Create invite codes.
pub async fn create_invite_codes(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.server.createInviteCodes").data(&input)?;

    client.request(req).await
}
