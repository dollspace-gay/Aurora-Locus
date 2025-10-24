//! Generated code for com.atproto.server.createAppPassword
//!
//! Create an App Password.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    /// A short name for the App Password, to help distinguish them.
    pub name: String,
    /// If an app password has 'privileged' access to possibly sensitive account state. Meant for use with trusted clients.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub privileged: Option<bool>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
}

/// Create an App Password.
pub async fn create_app_password(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.server.createAppPassword").data(&input)?;

    client.request(req).await
}
