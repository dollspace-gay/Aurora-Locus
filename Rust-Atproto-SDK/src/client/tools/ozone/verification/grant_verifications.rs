//! Generated code for tools.ozone.verification.grantVerifications
//!
//! Grant verifications to multiple subjects. Allows batch processing of up to 100 verifications at once.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    /// Array of verification requests to process
    pub verifications: serde_json::Value,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    #[serde(rename = "failedVerifications")]
    pub failed_verifications: serde_json::Value,
    pub verifications: serde_json::Value,
}

/// Grant verifications to multiple subjects. Allows batch processing of up to 100 verifications at once.
pub async fn grant_verifications(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("tools.ozone.verification.grantVerifications").data(&input)?;

    client.request(req).await
}
