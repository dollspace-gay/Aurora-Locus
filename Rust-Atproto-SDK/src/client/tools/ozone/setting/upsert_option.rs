//! Generated code for tools.ozone.setting.upsertOption
//!
//! Create or update setting option

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub scope: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "managerRole")]
    pub manager_role: Option<String>,
    pub value: serde_json::Value,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    pub option: serde_json::Value,
}

/// Create or update setting option
pub async fn upsert_option(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("tools.ozone.setting.upsertOption").data(&input)?;

    client.request(req).await
}
