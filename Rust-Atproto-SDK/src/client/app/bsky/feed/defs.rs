//! Generated type definitions for app.bsky.feed.defs

use serde::{Deserialize, Serialize};

/// User reposted the feed item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteractionRepost;

/// Declares the feed generator returns posts containing app.bsky.embed.video embeds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentModeVideo;

/// User clicked through to the reposter of the feed item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClickthroughReposter;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratorView {
    #[serde(rename = "indexedAt")]
    pub indexed_at: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    pub cid: String,
    pub creator: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "descriptionFacets")]
    pub description_facets: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "acceptsInteractions")]
    pub accepts_interactions: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub viewer: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "likeCount")]
    pub like_count: Option<i64>,
    pub uri: crate::syntax::AtUri,
    pub did: crate::types::Did,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "contentMode")]
    pub content_mode: Option<String>,
}


/// User clicked through to the feed item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClickthroughItem;

/// Declares the feed generator returns any types of posts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentModeUnspecified;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockedAuthor {
    pub did: crate::types::Did,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub viewer: Option<serde_json::Value>,
}


/// Request that more content like the given feed item be shown in the feed
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestMore;

/// User replied to the feed item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteractionReply;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkeletonReasonPin {
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonRepost {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<crate::syntax::AtUri>,
    pub by: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cid: Option<String>,
    #[serde(rename = "indexedAt")]
    pub indexed_at: String,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadgateView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<crate::syntax::AtUri>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lists: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<serde_json::Value>,
}


/// User clicked through to the author of the feed item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClickthroughAuthor;

/// Feed item was seen by user
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteractionSeen;

/// Metadata about the requesting account's relationship with the subject content. Only has meaningful content for authed requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewerState {
    /// Metadata about the requesting account's relationship with the subject content. Only has meaningful content for authed requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub like: Option<crate::syntax::AtUri>,
    /// Metadata about the requesting account's relationship with the subject content. Only has meaningful content for authed requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
    /// Metadata about the requesting account's relationship with the subject content. Only has meaningful content for authed requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "threadMuted")]
    pub thread_muted: Option<bool>,
    /// Metadata about the requesting account's relationship with the subject content. Only has meaningful content for authed requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "embeddingDisabled")]
    pub embedding_disabled: Option<bool>,
    /// Metadata about the requesting account's relationship with the subject content. Only has meaningful content for authed requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bookmarked: Option<bool>,
    /// Metadata about the requesting account's relationship with the subject content. Only has meaningful content for authed requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "replyDisabled")]
    pub reply_disabled: Option<bool>,
    /// Metadata about the requesting account's relationship with the subject content. Only has meaningful content for authed requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repost: Option<crate::syntax::AtUri>,
}


/// Metadata about this post within the context of the thread it is in.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadContext {
    /// Metadata about this post within the context of the thread it is in.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "rootAuthorLike")]
    pub root_author_like: Option<crate::syntax::AtUri>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadViewPost {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "threadContext")]
    pub thread_context: Option<serde_json::Value>,
    pub post: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replies: Option<serde_json::Value>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostView {
    pub uri: crate::syntax::AtUri,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "bookmarkCount")]
    pub bookmark_count: Option<i64>,
    pub cid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embed: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "replyCount")]
    pub reply_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "repostCount")]
    pub repost_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "quoteCount")]
    pub quote_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub viewer: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "likeCount")]
    pub like_count: Option<i64>,
    pub record: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threadgate: Option<serde_json::Value>,
    #[serde(rename = "indexedAt")]
    pub indexed_at: String,
    pub author: serde_json::Value,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Interaction {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item: Option<crate::syntax::AtUri>,
    /// Context on a feed item that was originally supplied by the feed generator on getFeedSkeleton.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "feedContext")]
    pub feed_context: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    /// Unique identifier per request that may be passed back alongside interactions.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "reqId")]
    pub req_id: Option<String>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotFoundPost {
    #[serde(rename = "notFound")]
    pub not_found: bool,
    pub uri: crate::syntax::AtUri,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratorViewerState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub like: Option<crate::syntax::AtUri>,
}


/// User clicked through to the embedded content of the feed item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClickthroughEmbed;

/// User quoted the feed item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteractionQuote;

/// User liked the feed item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteractionLike;

/// Request that less content like the given feed item be shown in the feed
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestLess;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockedPost {
    pub author: serde_json::Value,
    pub blocked: bool,
    pub uri: crate::syntax::AtUri,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkeletonFeedPost {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<serde_json::Value>,
    /// Context that will be passed through to client and may be passed to feed generator back alongside interactions.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "feedContext")]
    pub feed_context: Option<String>,
    pub post: crate::syntax::AtUri,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkeletonReasonRepost {
    pub repost: crate::syntax::AtUri,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedViewPost {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<serde_json::Value>,
    /// Context provided by feed generator that may be passed back alongside interactions.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "feedContext")]
    pub feed_context: Option<String>,
    pub post: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply: Option<serde_json::Value>,
    /// Unique identifier per request that may be passed back alongside interactions.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "reqId")]
    pub req_id: Option<String>,
}


/// User shared the feed item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteractionShare;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonPin {
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplyRef {
    /// When parent is a reply to another post, this is the author of that post.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "grandparentAuthor")]
    pub grandparent_author: Option<serde_json::Value>,
    pub parent: serde_json::Value,
    pub root: serde_json::Value,
}


