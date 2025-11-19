//! Read-After-Write Consistency Module
//!
//! Ensures users see their own writes immediately, even before AppView indexing completes.
//!
//! ## How It Works
//!
//! 1. User creates/updates records (posts, profile, etc.)
//! 2. Records are stored locally in actor store with monotonic revision
//! 3. User requests feed/profile → PDS proxies to AppView
//! 4. AppView returns data + `atproto-repo-rev` header
//! 5. PDS fetches local records created since that revision
//! 6. PDS merges local records into AppView response
//! 7. User sees their own writes immediately (even if not indexed yet)
//!
//! ## Example
//!
//! ```text
//! User creates post → Actor store (rev=5) → Sequencer → AppView (async indexing)
//! User requests feed → AppView returns feed (up to rev=3) + header: atproto-repo-rev=3
//!                   → PDS fetches local records since rev=3 (finds post at rev=5)
//!                   → PDS inserts post into feed chronologically
//!                   → User sees their post immediately!
//! ```

pub mod cache;
pub mod types;
pub mod viewer;

pub use cache::*;
pub use types::*;
pub use viewer::*;

use crate::error::PdsResult;

/// Extract the `atproto-repo-rev` header from response headers
    #[allow(dead_code)] // Future read-after-write utilities
pub fn get_repo_rev(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get("atproto-repo-rev")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

/// Calculate the lag (in milliseconds) between local writes and AppView indexing
///
/// Returns the age of the oldest local record (how long ago it was created).
    #[allow(dead_code)] // Future lag calculation
/// This indicates how far behind the AppView is.
pub fn get_local_lag(local: &LocalRecords) -> Option<u64> {
    let mut oldest: Option<&str> = None;

    if let Some(profile) = &local.profile {
        oldest = Some(&profile.indexed_at);
    }

    for post in &local.posts {
        if oldest.is_none() || post.indexed_at.as_str() < oldest.unwrap() {
            oldest = Some(&post.indexed_at);
        }
    }

    oldest.map(|timestamp| {
        let indexed_at = chrono::DateTime::parse_from_rfc3339(timestamp)
            .ok()?
            .timestamp_millis();
        let now = chrono::Utc::now().timestamp_millis();
        Some((now - indexed_at) as u64)
    })?
}

/// Merge local records into a feed response
///
/// Inserts local posts into the feed in chronological order.
/// Posts are inserted based on their `indexed_at` timestamp.
pub async fn format_and_insert_posts_in_feed(
    viewer: &LocalViewer,
    mut feed: Vec<serde_json::Value>,
    posts: &[RecordDescript],
) -> PdsResult<Vec<serde_json::Value>> {
    if posts.is_empty() {
        return Ok(feed);
    }

    // Get the timestamp of the last post in the feed
    let last_time = feed
        .last()
        .and_then(|item| item.get("post"))
        .and_then(|post| post.get("indexedAt"))
        .and_then(|v| v.as_str())
        .unwrap_or("1970-01-01T00:00:00Z");

    // Filter posts that are newer than the last post in the feed
    let in_feed: Vec<&RecordDescript> = posts
        .iter()
        .filter(|p| p.indexed_at.as_str() > last_time)
        .collect();

    // Reverse to get newest-to-oldest order
    let mut newest_to_oldest = in_feed;
    newest_to_oldest.reverse();

    // Format each post
    for post_descript in newest_to_oldest {
        if let Some(post_view) = viewer.get_post(post_descript).await? {
            // Find the correct insertion point (chronological order)
            let post_indexed_at = post_view
                .get("indexedAt")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let idx = feed.iter().position(|item| {
                let item_time = item
                    .get("post")
                    .and_then(|p| p.get("indexedAt"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                item_time < post_indexed_at
            });

            let feed_item = serde_json::json!({
                "post": post_view
            });

            if let Some(idx) = idx {
                feed.insert(idx, feed_item);
            } else {
                feed.push(feed_item);
            }
        }
    }

    Ok(feed)
}
