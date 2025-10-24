//! Generated code for com.atproto.temp.requestPhoneVerification
//!
//! Request a verification code to be sent to the supplied phone number

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    #[serde(rename = "phoneNumber")]
    pub phone_number: String,
}

/// Request a verification code to be sent to the supplied phone number
pub async fn request_phone_verification(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.temp.requestPhoneVerification").data(&input)?;

    client.request(req).await
}
