//! Generated code for app.bsky.actor.putPreferences
//!
//! Set the private preferences attached to the account.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub preferences: serde_json::Value,
}

/// Set the private preferences attached to the account.
pub async fn put_preferences(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("app.bsky.actor.putPreferences").data(&input)?;

    client.request(req).await
}
