//! Generated code for app.bsky.feed.describeFeedGenerator
//!
//! Get information about a feed generator, including policies and offered feed URIs. Does not require auth; implemented by Feed Generator services (not App View).

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub links: Option<serde_json::Value>,
    pub feeds: serde_json::Value,
    pub did: crate::types::Did,
}

/// Get information about a feed generator, including policies and offered feed URIs. Does not require auth; implemented by Feed Generator services (not App View).
pub async fn describe_feed_generator(
    client: &impl XrpcClient,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::query("app.bsky.feed.describeFeedGenerator");

    client.request(req).await
}
