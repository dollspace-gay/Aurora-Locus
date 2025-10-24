//! Generated code for app.bsky.video.getUploadLimits
//!
//! Get video upload limits for the authenticated user.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "remainingDailyBytes")]
    pub remaining_daily_bytes: Option<i64>,
    #[serde(rename = "canUpload")]
    pub can_upload: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "remainingDailyVideos")]
    pub remaining_daily_videos: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Get video upload limits for the authenticated user.
pub async fn get_upload_limits(
    client: &impl XrpcClient,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::query("app.bsky.video.getUploadLimits");

    client.request(req).await
}
