//! Generated code for com.atproto.repo.uploadBlob
//!
//! Upload a new blob, to be referenced from a repository record. The blob will be deleted if it is not referenced within a time window (eg, minutes). Blob restrictions (mimetype, size, etc) are enforced when the reference is created. Requires auth, implemented by PDS.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input (binary data)
pub type Input = Vec<u8>;

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    pub blob: serde_json::Value,
}

/// Upload a new blob, to be referenced from a repository record. The blob will be deleted if it is not referenced within a time window (eg, minutes). Blob restrictions (mimetype, size, etc) are enforced when the reference is created. Requires auth, implemented by PDS.
pub async fn upload_blob(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.repo.uploadBlob").data(&input)?;

    client.request(req).await
}
