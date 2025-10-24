//! Generated code for com.atproto.identity.signPlcOperation
//!
//! Signs a PLC operation to update some value(s) in the requesting DID's document.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub services: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "verificationMethods")]
    pub verification_methods: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "rotationKeys")]
    pub rotation_keys: Option<serde_json::Value>,
    /// A token received through com.atproto.identity.requestPlcOperationSignature
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "alsoKnownAs")]
    pub also_known_as: Option<serde_json::Value>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    /// A signed DID PLC operation.
    pub operation: serde_json::Value,
}

/// Signs a PLC operation to update some value(s) in the requesting DID's document.
pub async fn sign_plc_operation(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.identity.signPlcOperation").data(&input)?;

    client.request(req).await
}
