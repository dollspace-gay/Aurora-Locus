//! Generated code for com.atproto.temp.checkSignupQueue
//!
//! Check accounts location in signup queue.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "placeInQueue")]
    pub place_in_queue: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "estimatedTimeMs")]
    pub estimated_time_ms: Option<i64>,
    pub activated: bool,
}

/// Check accounts location in signup queue.
pub async fn check_signup_queue(
    client: &impl XrpcClient,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::query("com.atproto.temp.checkSignupQueue");

    client.request(req).await
}
