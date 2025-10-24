//! Generated code for com.atproto.server.deactivateAccount
//!
//! Deactivates a currently active account. Stops serving of repo, and future writes to repo until reactivated. Used to finalize account migration with the old host after the account has been activated on the new host.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    /// A recommendation to server as to how long they should hold onto the deactivated account before deleting.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "deleteAfter")]
    pub delete_after: Option<String>,
}

/// Deactivates a currently active account. Stops serving of repo, and future writes to repo until reactivated. Used to finalize account migration with the old host after the account has been activated on the new host.
pub async fn deactivate_account(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.server.deactivateAccount").data(&input)?;

    client.request(req).await
}
