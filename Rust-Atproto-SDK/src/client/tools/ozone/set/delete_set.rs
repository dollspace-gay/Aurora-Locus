//! Generated code for tools.ozone.set.deleteSet
//!
//! Delete an entire set. Attempting to delete a set that does not exist will result in an error.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    /// Name of the set to delete
    pub name: String,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
}

/// Delete an entire set. Attempting to delete a set that does not exist will result in an error.
pub async fn delete_set(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("tools.ozone.set.deleteSet").data(&input)?;

    client.request(req).await
}
