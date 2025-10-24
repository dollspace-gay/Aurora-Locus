//! Generated code for app.bsky.notification.putPreferences
//!
//! Set notification-related preferences for an account. Requires auth.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub priority: bool,
}

/// Set notification-related preferences for an account. Requires auth.
pub async fn put_preferences(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("app.bsky.notification.putPreferences").data(&input)?;

    client.request(req).await
}
