//! Generated code for com.atproto.server.reserveSigningKey
//!
//! Reserve a repo signing key, for use with account creation. Necessary so that a DID PLC update operation can be constructed during an account migraiton. Public and does not require auth; implemented by PDS. NOTE: this endpoint may change when full account migration is implemented.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    /// The DID to reserve a key for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub did: Option<crate::types::Did>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    /// The public key for the reserved signing key, in did:key serialization.
    #[serde(rename = "signingKey")]
    pub signing_key: String,
}

/// Reserve a repo signing key, for use with account creation. Necessary so that a DID PLC update operation can be constructed during an account migraiton. Public and does not require auth; implemented by PDS. NOTE: this endpoint may change when full account migration is implemented.
pub async fn reserve_signing_key(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.server.reserveSigningKey").data(&input)?;

    client.request(req).await
}
