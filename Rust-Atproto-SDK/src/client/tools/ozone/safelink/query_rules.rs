//! Generated code for tools.ozone.safelink.queryRules
//!
//! Query URL safety rules

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    /// Filter by rule creator
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "createdBy")]
    pub created_by: Option<crate::types::Did>,
    /// Filter by reason type
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Maximum number of results to return
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    /// Cursor for pagination
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Filter by pattern type
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "patternType")]
    pub pattern_type: Option<String>,
    /// Filter by action types
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actions: Option<serde_json::Value>,
    /// Filter by specific URLs or domains
    #[serde(skip_serializing_if = "Option::is_none")]
    pub urls: Option<serde_json::Value>,
    /// Sort direction
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "sortDirection")]
    pub sort_direction: Option<String>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    pub rules: serde_json::Value,
    /// Next cursor for pagination. Only present if there are more results.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Query URL safety rules
pub async fn query_rules(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("tools.ozone.safelink.queryRules").data(&input)?;

    client.request(req).await
}
