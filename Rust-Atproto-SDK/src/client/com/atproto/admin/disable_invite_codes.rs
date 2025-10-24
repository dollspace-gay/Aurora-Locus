//! Generated code for com.atproto.admin.disableInviteCodes
//!
//! Disable some set of codes and/or all codes associated with a set of users.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codes: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accounts: Option<serde_json::Value>,
}

/// Disable some set of codes and/or all codes associated with a set of users.
pub async fn disable_invite_codes(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.admin.disableInviteCodes").data(&input)?;

    client.request(req).await
}
