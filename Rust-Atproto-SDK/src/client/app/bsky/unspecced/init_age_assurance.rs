//! Generated code for app.bsky.unspecced.initAgeAssurance
//!
//! Initiate age assurance for an account. This is a one-time action that will start the process of verifying the user's age.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    /// The user's email address to receive assurance instructions.
    pub email: String,
    /// The user's preferred language for communication during the assurance process.
    pub language: String,
    /// An ISO 3166-1 alpha-2 code of the user's location.
    #[serde(rename = "countryCode")]
    pub country_code: String,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
}

/// Initiate age assurance for an account. This is a one-time action that will start the process of verifying the user's age.
pub async fn init_age_assurance(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("app.bsky.unspecced.initAgeAssurance").data(&input)?;

    client.request(req).await
}
