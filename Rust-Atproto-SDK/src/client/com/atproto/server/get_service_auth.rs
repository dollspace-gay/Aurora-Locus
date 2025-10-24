//! Generated code for com.atproto.server.getServiceAuth
//!
//! Get a signed token on behalf of the requesting DID for the requested service.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    /// Lexicon (XRPC) method to bind the requested token to
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lxm: Option<String>,
    /// The time in Unix Epoch seconds that the JWT expires. Defaults to 60 seconds in the future. The service may enforce certain time bounds on tokens depending on the requested scope.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exp: Option<i64>,
    /// The DID of the service that the token will be used to authenticate with
    pub aud: crate::types::Did,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    pub token: String,
}

/// Error: BadExpiration
/// Indicates that the requested expiration date is not a valid. May be in the past or may be reliant on the requested scopes.
#[derive(Debug, Clone, thiserror::Error)]
#[error("BadExpiration")]
pub struct BadExpirationError;

/// Get a signed token on behalf of the requesting DID for the requested service.
pub async fn get_service_auth(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let mut req = XrpcRequest::query("com.atproto.server.getServiceAuth");

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
