//! Generated code for tools.ozone.verification.listVerifications
//!
//! List verifications

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    /// Sort direction for creation date
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "sortDirection")]
    pub sort_direction: Option<String>,
    /// Filter to verifications from specific issuers
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issuers: Option<serde_json::Value>,
    /// Filter to specific verified DIDs
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subjects: Option<serde_json::Value>,
    /// Filter to verifications that are revoked or not. By default, includes both.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "isRevoked")]
    pub is_revoked: Option<bool>,
    /// Pagination cursor
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Maximum number of results to return
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    /// Filter to verifications created before this timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "createdBefore")]
    pub created_before: Option<String>,
    /// Filter to verifications created after this timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "createdAfter")]
    pub created_after: Option<String>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    pub verifications: serde_json::Value,
}

/// List verifications
pub async fn list_verifications(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let mut req = XrpcRequest::query("tools.ozone.verification.listVerifications");

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
