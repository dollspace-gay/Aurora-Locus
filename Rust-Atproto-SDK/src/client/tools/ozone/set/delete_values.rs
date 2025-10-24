//! Generated code for tools.ozone.set.deleteValues
//!
//! Delete values from a specific set. Attempting to delete values that are not in the set will not result in an error

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    /// Name of the set to delete values from
    pub name: String,
    /// Array of string values to delete from the set
    pub values: serde_json::Value,
}

/// Delete values from a specific set. Attempting to delete values that are not in the set will not result in an error
pub async fn delete_values(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("tools.ozone.set.deleteValues").data(&input)?;

    client.request(req).await
}
