//! Generated code for com.atproto.admin.updateAccountSigningKey
//!
//! Administrative action to update an account's signing key in their Did document.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub did: crate::types::Did,
    /// Did-key formatted public key
    #[serde(rename = "signingKey")]
    pub signing_key: crate::types::Did,
}

/// Administrative action to update an account's signing key in their Did document.
pub async fn update_account_signing_key(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.admin.updateAccountSigningKey").data(&input)?;

    client.request(req).await
}
