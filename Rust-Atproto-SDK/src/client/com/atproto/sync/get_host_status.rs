//! Generated code for com.atproto.sync.getHostStatus
//!
//! Returns information about a specified upstream host, as consumed by the server. Implemented by relays.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    /// Hostname of the host (eg, PDS or relay) being queried.
    pub hostname: String,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    /// Number of accounts on the server which are associated with the upstream host. Note that the upstream may actually have more accounts.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "accountCount")]
    pub account_count: Option<i64>,
    pub hostname: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<serde_json::Value>,
    /// Recent repo stream event sequence number. May be delayed from actual stream processing (eg, persisted cursor not in-memory cursor).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seq: Option<i64>,
}

/// Error: HostNotFound
#[derive(Debug, Clone, thiserror::Error)]
#[error("HostNotFound")]
pub struct HostNotFoundError;

/// Returns information about a specified upstream host, as consumed by the server. Implemented by relays.
pub async fn get_host_status(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let mut req = XrpcRequest::query("com.atproto.sync.getHostStatus");

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
