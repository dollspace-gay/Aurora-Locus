//! Generated code for app.bsky.feed.getFeedGenerator
//!
//! Get information about a feed generator. Implemented by AppView.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    /// AT-URI of the feed generator record.
    pub feed: crate::syntax::AtUri,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    /// Indicates whether the feed generator service is compatible with the record declaration.
    #[serde(rename = "isValid")]
    pub is_valid: bool,
    pub view: serde_json::Value,
    /// Indicates whether the feed generator service has been online recently, or else seems to be inactive.
    #[serde(rename = "isOnline")]
    pub is_online: bool,
}

/// Get information about a feed generator. Implemented by AppView.
pub async fn get_feed_generator(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let mut req = XrpcRequest::query("app.bsky.feed.getFeedGenerator");

    // Add query parameters
    let params_json = serde_json::to_value(&params)
        .map_err(XrpcError::Serialization)?;

    if let Some(obj) = params_json.as_object() {
        for (key, value) in obj {
            if let Some(s) = value.as_str() {
                req.params.insert(key.clone(), s.to_string());
            } else {
                req.params.insert(key.clone(), value.to_string());
            }
        }
    }

    client.request(req).await
}
