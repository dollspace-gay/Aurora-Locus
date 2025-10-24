//! Generated code for chat.bsky.convo.getConvoAvailability
//!
//! Get whether the requester and the other members can chat. If an existing convo is found for these members, it is returned.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    pub members: serde_json::Value,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    #[serde(rename = "canChat")]
    pub can_chat: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub convo: Option<serde_json::Value>,
}

/// Get whether the requester and the other members can chat. If an existing convo is found for these members, it is returned.
pub async fn get_convo_availability(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let mut req = XrpcRequest::query("chat.bsky.convo.getConvoAvailability");

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
