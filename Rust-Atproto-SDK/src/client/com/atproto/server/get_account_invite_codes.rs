//! Generated code for com.atproto.server.getAccountInviteCodes
//!
//! Get all invite codes for the current account. Requires auth.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "includeUsed")]
    pub include_used: Option<bool>,
    /// Controls whether any new 'earned' but not 'created' invites should be created.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "createAvailable")]
    pub create_available: Option<bool>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    pub codes: serde_json::Value,
}

/// Error: DuplicateCreate
#[derive(Debug, Clone, thiserror::Error)]
#[error("DuplicateCreate")]
pub struct DuplicateCreateError;

/// Get all invite codes for the current account. Requires auth.
pub async fn get_account_invite_codes(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let mut req = XrpcRequest::query("com.atproto.server.getAccountInviteCodes");

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
