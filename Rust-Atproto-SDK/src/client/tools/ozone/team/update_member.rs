//! Generated code for tools.ozone.team.updateMember
//!
//! Update a member in the ozone service. Requires admin role.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub did: crate::types::Did,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
}

/// Update a member in the ozone service. Requires admin role.
pub async fn update_member(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("tools.ozone.team.updateMember").data(&input)?;

    client.request(req).await
}
