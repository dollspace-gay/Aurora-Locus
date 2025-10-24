//! Generated code for tools.ozone.communication.deleteTemplate
//!
//! Delete a communication template.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub id: String,
}

/// Delete a communication template.
pub async fn delete_template(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("tools.ozone.communication.deleteTemplate").data(&input)?;

    client.request(req).await
}
