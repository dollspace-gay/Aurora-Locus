//! Generated code for com.atproto.identity.requestPlcOperationSignature
//!
//! Request an email with a code to in order to request a signed PLC operation. Requires Auth.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};

/// Request an email with a code to in order to request a signed PLC operation. Requires Auth.
pub async fn request_plc_operation_signature(
    client: &impl XrpcClient,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.identity.requestPlcOperationSignature");

    client.request(req).await
}
