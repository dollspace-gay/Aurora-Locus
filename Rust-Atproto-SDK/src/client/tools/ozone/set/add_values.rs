//! Generated code for tools.ozone.set.addValues
//!
//! Add values to a specific set. Attempting to add values to a set that does not exist will result in an error.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    /// Name of the set to add values to
    pub name: String,
    /// Array of string values to add to the set
    pub values: serde_json::Value,
}

/// Add values to a specific set. Attempting to add values to a set that does not exist will result in an error.
pub async fn add_values(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("tools.ozone.set.addValues").data(&input)?;

    client.request(req).await
}
