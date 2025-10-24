//! Generated code for com.atproto.server.deleteAccount
//!
//! Delete an actor's account with a token and password. Can only be called after requesting a deletion token. Requires auth.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub token: String,
    pub did: crate::types::Did,
    pub password: String,
}

/// Delete an actor's account with a token and password. Can only be called after requesting a deletion token. Requires auth.
pub async fn delete_account(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.server.deleteAccount").data(&input)?;

    client.request(req).await
}
