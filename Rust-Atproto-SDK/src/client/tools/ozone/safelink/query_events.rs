//! Generated code for tools.ozone.safelink.queryEvents
//!
//! Query URL safety audit events

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    /// Maximum number of results to return
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    /// Filter by specific URLs or domains
    #[serde(skip_serializing_if = "Option::is_none")]
    pub urls: Option<serde_json::Value>,
    /// Filter by pattern type
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "patternType")]
    pub pattern_type: Option<String>,
    /// Sort direction
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "sortDirection")]
    pub sort_direction: Option<String>,
    /// Cursor for pagination
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    pub events: serde_json::Value,
    /// Next cursor for pagination. Only present if there are more results.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Query URL safety audit events
pub async fn query_events(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("tools.ozone.safelink.queryEvents").data(&input)?;

    client.request(req).await
}
