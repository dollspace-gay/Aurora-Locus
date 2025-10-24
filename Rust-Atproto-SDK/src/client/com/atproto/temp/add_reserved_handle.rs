//! Generated code for com.atproto.temp.addReservedHandle
//!
//! Add a handle to the set of reserved handles.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub handle: String,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
}

/// Add a handle to the set of reserved handles.
pub async fn add_reserved_handle(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.temp.addReservedHandle").data(&input)?;

    client.request(req).await
}
