//! Generated code for app.bsky.graph.getStarterPack
//!
//! Gets a view of a starter pack.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    /// Reference (AT-URI) of the starter pack record.
    #[serde(rename = "starterPack")]
    pub starter_pack: crate::syntax::AtUri,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    #[serde(rename = "starterPack")]
    pub starter_pack: serde_json::Value,
}

/// Gets a view of a starter pack.
pub async fn get_starter_pack(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let mut req = XrpcRequest::query("app.bsky.graph.getStarterPack");

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
