//! Generated code for tools.ozone.team.deleteMember
//!
//! Delete a member from ozone team. Requires admin role.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub did: crate::types::Did,
}

/// Delete a member from ozone team. Requires admin role.
pub async fn delete_member(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("tools.ozone.team.deleteMember").data(&input)?;

    client.request(req).await
}
