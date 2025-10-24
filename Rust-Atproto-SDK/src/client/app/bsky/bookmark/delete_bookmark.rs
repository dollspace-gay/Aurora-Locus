//! Generated code for app.bsky.bookmark.deleteBookmark
//!
//! Deletes a private bookmark for the specified record. Currently, only `app.bsky.feed.post` records are supported. Requires authentication.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub uri: crate::syntax::AtUri,
}

/// Deletes a private bookmark for the specified record. Currently, only `app.bsky.feed.post` records are supported. Requires authentication.
pub async fn delete_bookmark(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<()>, XrpcError> {
    let req = XrpcRequest::procedure("app.bsky.bookmark.deleteBookmark").data(&input)?;

    client.request(req).await
}
