//! Generated code for com.atproto.identity.refreshIdentity
//!
//! Request that the server re-resolve an identity (DID and handle). The server may ignore this request, or require authentication, depending on the role, implementation, and policy of the server.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub identifier: String,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
}

/// Request that the server re-resolve an identity (DID and handle). The server may ignore this request, or require authentication, depending on the role, implementation, and policy of the server.
pub async fn refresh_identity(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.identity.refreshIdentity").data(&input)?;

    client.request(req).await
}
