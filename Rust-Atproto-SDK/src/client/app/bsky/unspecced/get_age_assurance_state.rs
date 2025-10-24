//! Generated code for app.bsky.unspecced.getAgeAssuranceState
//!
//! Returns the current state of the age assurance process for an account. This is used to check if the user has completed age assurance or if further action is required.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
}

/// Returns the current state of the age assurance process for an account. This is used to check if the user has completed age assurance or if further action is required.
pub async fn get_age_assurance_state(
    client: &impl XrpcClient,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::query("app.bsky.unspecced.getAgeAssuranceState");

    client.request(req).await
}
