//! Generated code for tools.ozone.team.addMember
//!
//! Add a member to the ozone team. Requires admin role.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub did: crate::types::Did,
    pub role: String,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
}

/// Add a member to the ozone team. Requires admin role.
pub async fn add_member(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("tools.ozone.team.addMember").data(&input)?;

    client.request(req).await
}
