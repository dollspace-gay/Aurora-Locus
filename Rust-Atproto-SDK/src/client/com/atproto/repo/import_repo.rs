//! Generated code for com.atproto.repo.importRepo
//!
//! Import a repo in the form of a CAR file. Requires Content-Length HTTP header to be set.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};

/// Request input (binary data)
pub type Input = Vec<u8>;

/// Import a repo in the form of a CAR file. Requires Content-Length HTTP header to be set.
pub async fn import_repo(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.repo.importRepo").data(&input)?;

    client.request(req).await
}
