//! Generated code for com.atproto.temp.fetchLabels
//!
//! DEPRECATED: use queryLabels or subscribeLabels instead -- Fetch all labels from a labeler created after a certain date.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    pub labels: serde_json::Value,
}

/// DEPRECATED: use queryLabels or subscribeLabels instead -- Fetch all labels from a labeler created after a certain date.
pub async fn fetch_labels(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let mut req = XrpcRequest::query("com.atproto.temp.fetchLabels");

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
