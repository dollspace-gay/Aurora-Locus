//! Generated code for com.atproto.server.createAccount
//!
//! Create an account. Implemented by PDS.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "inviteCode")]
    pub invite_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Pre-existing atproto DID, being imported to a new account.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub did: Option<crate::types::Did>,
    /// Requested handle for the account.
    pub handle: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "verificationCode")]
    pub verification_code: Option<String>,
    /// A signed DID PLC operation to be submitted as part of importing an existing account to this instance. NOTE: this optional field may be updated when full account migration is implemented.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "plcOp")]
    pub plc_op: Option<serde_json::Value>,
    /// Initial account password. May need to meet instance-specific password strength requirements.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// DID PLC rotation key (aka, recovery key) to be included in PLC creation operation.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "recoveryKey")]
    pub recovery_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "verificationPhone")]
    pub verification_phone: Option<String>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    pub handle: String,
    /// The DID of the new account.
    pub did: crate::types::Did,
    /// Complete DID document.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "didDoc")]
    pub did_doc: Option<serde_json::Value>,
    #[serde(rename = "accessJwt")]
    pub access_jwt: String,
    #[serde(rename = "refreshJwt")]
    pub refresh_jwt: String,
}

/// Create an account. Implemented by PDS.
pub async fn create_account(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.server.createAccount").data(&input)?;

    client.request(req).await
}
