//! Generated code for tools.ozone.verification.revokeVerifications
//!
//! Revoke previously granted verifications in batches of up to 100.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    /// Array of verification record uris to revoke
    pub uris: serde_json::Value,
    /// Reason for revoking the verification. This is optional and can be omitted if not needed.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "revokeReason")]
    pub revoke_reason: Option<String>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    /// List of verification uris successfully revoked
    #[serde(rename = "revokedVerifications")]
    pub revoked_verifications: serde_json::Value,
    /// List of verification uris that couldn't be revoked, including failure reasons
    #[serde(rename = "failedRevocations")]
    pub failed_revocations: serde_json::Value,
}

/// Revoke previously granted verifications in batches of up to 100.
pub async fn revoke_verifications(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("tools.ozone.verification.revokeVerifications").data(&input)?;

    client.request(req).await
}
