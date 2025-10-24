//! Generated code for tools.ozone.setting.removeOptions
//!
//! Delete settings by key

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub keys: serde_json::Value,
    pub scope: String,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
}

/// Delete settings by key
pub async fn remove_options(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("tools.ozone.setting.removeOptions").data(&input)?;

    client.request(req).await
}
