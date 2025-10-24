//! Generated code for app.bsky.video.uploadVideo
//!
//! Upload a video to be processed then stored on the PDS.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input (binary data)
pub type Input = Vec<u8>;

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    #[serde(rename = "jobStatus")]
    pub job_status: serde_json::Value,
}

/// Upload a video to be processed then stored on the PDS.
pub async fn upload_video(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("app.bsky.video.uploadVideo").data(&input)?;

    client.request(req).await
}
