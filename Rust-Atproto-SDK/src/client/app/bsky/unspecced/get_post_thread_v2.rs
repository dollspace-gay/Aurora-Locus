//! Generated code for app.bsky.unspecced.getPostThreadV2
//!
//! (NOTE: this endpoint is under development and WILL change without notice. Don't use it until it is moved out of `unspecced` or your application WILL break) Get posts in a thread. It is based in an anchor post at any depth of the tree, and returns posts above it (recursively resolving the parent, without further branching to their replies) and below it (recursive replies, with branching to their replies). Does not require auth, but additional metadata and filtering will be applied for authed requests.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    /// Sorting for the thread replies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<String>,
    /// Whether to include parents above the anchor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub above: Option<bool>,
    /// Whether to prioritize posts from followed users. It only has effect when the user is authenticated.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "prioritizeFollowedUsers")]
    pub prioritize_followed_users: Option<bool>,
    /// Reference (AT-URI) to post record. This is the anchor post, and the thread will be built around it. It can be any post in the tree, not necessarily a root post.
    pub anchor: crate::syntax::AtUri,
    /// How many levels of replies to include below the anchor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub below: Option<i64>,
    /// Maximum of replies to include at each level of the thread, except for the direct replies to the anchor, which are (NOTE: currently, during unspecced phase) all returned (NOTE: later they might be paginated).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "branchingFactor")]
    pub branching_factor: Option<i64>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    /// A flat list of thread items. The depth of each item is indicated by the depth property inside the item.
    pub thread: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threadgate: Option<serde_json::Value>,
    /// Whether this thread has additional replies. If true, a call can be made to the `getPostThreadOtherV2` endpoint to retrieve them.
    #[serde(rename = "hasOtherReplies")]
    pub has_other_replies: bool,
}

/// (NOTE: this endpoint is under development and WILL change without notice. Don't use it until it is moved out of `unspecced` or your application WILL break) Get posts in a thread. It is based in an anchor post at any depth of the tree, and returns posts above it (recursively resolving the parent, without further branching to their replies) and below it (recursive replies, with branching to their replies). Does not require auth, but additional metadata and filtering will be applied for authed requests.
pub async fn get_post_thread_v2(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let mut req = XrpcRequest::query("app.bsky.unspecced.getPostThreadV2");

    // Add query parameters
    let params_json = serde_json::to_value(&params)
        .map_err(XrpcError::Serialization)?;

    if let Some(obj) = params_json.as_object() {
        for (key, value) in obj {
            if let Some(s) = value.as_str() {
                req.params.insert(key.clone(), s.to_string());
            } else {
                req.params.insert(key.clone(), value.to_string());
            }
        }
    }

    client.request(req).await
}
