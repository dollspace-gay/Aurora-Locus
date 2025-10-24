//! Generated code for com.atproto.identity.getRecommendedDidCredentials
//!
//! Describe the credentials that should be included in the DID doc of an account that is migrating to this service.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub services: Option<serde_json::Value>,
    /// Recommended rotation keys for PLC dids. Should be undefined (or ignored) for did:webs.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "rotationKeys")]
    pub rotation_keys: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "alsoKnownAs")]
    pub also_known_as: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "verificationMethods")]
    pub verification_methods: Option<serde_json::Value>,
}

/// Describe the credentials that should be included in the DID doc of an account that is migrating to this service.
pub async fn get_recommended_did_credentials(
    client: &impl XrpcClient,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::query("com.atproto.identity.getRecommendedDidCredentials");

    client.request(req).await
}
