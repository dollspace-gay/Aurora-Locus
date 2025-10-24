//! Generated code for com.atproto.server.requestEmailUpdate
//!
//! Request a token in order to update email.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    #[serde(rename = "tokenRequired")]
    pub token_required: bool,
}

/// Request a token in order to update email.
pub async fn request_email_update(
    client: &impl XrpcClient,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.server.requestEmailUpdate");

    client.request(req).await
}
