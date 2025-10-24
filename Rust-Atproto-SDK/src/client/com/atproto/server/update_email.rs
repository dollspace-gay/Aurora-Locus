//! Generated code for com.atproto.server.updateEmail
//!
//! Update an account's email.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "emailAuthFactor")]
    pub email_auth_factor: Option<bool>,
    /// Requires a token from com.atproto.sever.requestEmailUpdate if the account's email has been confirmed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    pub email: String,
}

/// Update an account's email.
pub async fn update_email(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.server.updateEmail").data(&input)?;

    client.request(req).await
}
