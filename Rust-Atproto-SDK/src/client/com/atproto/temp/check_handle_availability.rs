//! Generated code for com.atproto.temp.checkHandleAvailability
//!
//! Checks whether the provided handle is available. If the handle is not available, available suggestions will be returned. Optional inputs will be used to generate suggestions.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    /// Tentative handle. Will be checked for availability or used to build handle suggestions.
    pub handle: String,
    /// User-provided email. Might be used to build handle suggestions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// User-provided birth date. Might be used to build handle suggestions.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "birthDate")]
    pub birth_date: Option<String>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    pub result: serde_json::Value,
    /// Echo of the input handle.
    pub handle: String,
}

/// Error: InvalidEmail
/// An invalid email was provided.
#[derive(Debug, Clone, thiserror::Error)]
#[error("InvalidEmail")]
pub struct InvalidEmailError;

/// Checks whether the provided handle is available. If the handle is not available, available suggestions will be returned. Optional inputs will be used to generate suggestions.
pub async fn check_handle_availability(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let mut req = XrpcRequest::query("com.atproto.temp.checkHandleAvailability");

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
