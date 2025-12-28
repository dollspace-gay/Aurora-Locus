//! AppView proxy endpoints with read-after-write consistency
//!
//! These endpoints proxy requests to the external AppView service (e.g., Bluesky's AppView)
//! and merge in local records that haven't been indexed yet (read-after-write consistency).

use crate::{
    auth::OAuthAuthContext,
    context::AppContext,
    error::{PdsError, PdsResult},
    read_after_write::{self, LocalViewer},
};
use axum::{
    extract::{Query, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Build AppView proxy routes
pub fn routes() -> Router<AppContext> {
    Router::new()
        // Feed endpoints with read-after-write
        .route("/xrpc/app.bsky.feed.getTimeline", get(get_timeline))
        .route("/xrpc/app.bsky.feed.getAuthorFeed", get(get_author_feed))
        // Profile endpoint with read-after-write
        .route("/xrpc/app.bsky.actor.getProfile", get(get_profile))
        // Feed endpoints (simple proxy)
        .route("/xrpc/app.bsky.feed.getPosts", get(get_posts))
        .route("/xrpc/app.bsky.feed.getPostThread", get(get_post_thread))
        .route("/xrpc/app.bsky.feed.getLikes", get(get_likes))
        .route("/xrpc/app.bsky.feed.getRepostedBy", get(get_reposted_by))
        .route("/xrpc/app.bsky.feed.getQuotes", get(get_quotes))
        .route("/xrpc/app.bsky.feed.getFeedGenerator", get(get_feed_generator))
        .route("/xrpc/app.bsky.feed.getFeedGenerators", get(get_feed_generators))
        .route("/xrpc/app.bsky.feed.getActorFeeds", get(get_actor_feeds))
        .route("/xrpc/app.bsky.feed.getSuggestedFeeds", get(get_suggested_feeds))
        .route("/xrpc/app.bsky.feed.getFeed", get(get_feed))
        .route("/xrpc/app.bsky.feed.getListFeed", get(get_list_feed))
        .route("/xrpc/app.bsky.feed.searchPosts", get(search_posts))
        // Actor endpoints (simple proxy)
        .route("/xrpc/app.bsky.actor.getProfiles", get(get_profiles))
        .route("/xrpc/app.bsky.actor.searchActors", get(search_actors))
        .route("/xrpc/app.bsky.actor.searchActorsTypeahead", get(search_actors_typeahead))
        .route("/xrpc/app.bsky.actor.getSuggestions", get(get_suggestions))
        .route("/xrpc/app.bsky.actor.getPreferences", get(get_preferences))
        // Graph endpoints (simple proxy)
        .route("/xrpc/app.bsky.graph.getFollowers", get(get_followers))
        .route("/xrpc/app.bsky.graph.getFollows", get(get_follows))
        .route("/xrpc/app.bsky.graph.getBlocks", get(get_blocks))
        .route("/xrpc/app.bsky.graph.getMutes", get(get_mutes))
        .route("/xrpc/app.bsky.graph.getLists", get(get_lists))
        .route("/xrpc/app.bsky.graph.getList", get(get_list))
        .route("/xrpc/app.bsky.graph.getListMembers", get(get_list_members))
        .route("/xrpc/app.bsky.graph.getListMemberships", get(get_list_memberships))
        // Notification endpoints (simple proxy)
        .route("/xrpc/app.bsky.notification.listNotifications", get(list_notifications))
        .route("/xrpc/app.bsky.notification.getUnreadCount", get(get_unread_count))
        // Labeler endpoints (simple proxy)
        .route("/xrpc/app.bsky.labeler.getServices", get(get_labeler_services))
}

/// Get user's timeline with read-after-write consistency
async fn get_timeline(
    State(ctx): State<AppContext>,
    auth: OAuthAuthContext,
    Query(params): Query<TimelineParams>,
) -> PdsResult<Response> {
    proxy_with_read_after_write(
        &ctx,
        &auth.did,
        "app.bsky.feed.getTimeline",
        params,
        merge_timeline_feed,
    )
    .await
}

/// Get author's feed with read-after-write consistency
async fn get_author_feed(
    State(ctx): State<AppContext>,
    auth: OAuthAuthContext,
    Query(params): Query<AuthorFeedParams>,
) -> PdsResult<Response> {
    // Only apply read-after-write if viewing own feed
    if params.actor == auth.did {
        proxy_with_read_after_write(
            &ctx,
            &auth.did,
            "app.bsky.feed.getAuthorFeed",
            params,
            merge_author_feed,
        )
        .await
    } else {
        // Just proxy without read-after-write for other users
        proxy_to_appview(&ctx, "app.bsky.feed.getAuthorFeed", params).await
    }
}

/// Get profile with read-after-write consistency
async fn get_profile(
    State(ctx): State<AppContext>,
    auth: OAuthAuthContext,
    Query(params): Query<ProfileParams>,
) -> PdsResult<Response> {
    // Only apply read-after-write if viewing own profile
    if params.actor == auth.did {
        proxy_with_read_after_write(
            &ctx,
            &auth.did,
            "app.bsky.actor.getProfile",
            params,
            merge_profile,
        )
        .await
    } else {
        // Just proxy without read-after-write for other users
        proxy_to_appview(&ctx, "app.bsky.actor.getProfile", params).await
    }
}

/// Proxy request to AppView with read-after-write consistency
async fn proxy_with_read_after_write<P, F>(
    ctx: &AppContext,
    user_did: &str,
    method: &str,
    params: P,
    merge_fn: F,
) -> PdsResult<Response>
where
    P: Serialize,
    F: FnOnce(serde_json::Value, &LocalViewer, read_after_write::LocalRecords) -> PdsResult<serde_json::Value>,
{
    // Get AppView URL from config
    let appview_url = ctx
        .config
        .federation
        .appview_url
        .as_ref()
        .ok_or_else(|| PdsError::Internal("AppView URL not configured".to_string()))?;

    // Build request URL
    let url = format!("{}/xrpc/{}", appview_url, method);
    let query_params = serde_json::to_value(&params)
        .map_err(|e| PdsError::Internal(format!("Failed to serialize params: {}", e)))?;

    // Make request to AppView
    let client = reqwest::Client::new();
    let mut req = client.get(&url);

    // Add query parameters
    if let Some(obj) = query_params.as_object() {
        for (key, value) in obj {
            if let Some(v) = value.as_str() {
                req = req.query(&[(key, v)]);
            } else if let Some(v) = value.as_i64() {
                req = req.query(&[(key, v.to_string())]);
            } else if let Some(v) = value.as_bool() {
                req = req.query(&[(key, v.to_string())]);
            }
        }
    }

    let response = req
        .send()
        .await
        .map_err(|e| PdsError::Internal(format!("AppView request failed: {}", e)))?;

    // Extract atproto-repo-rev header
    let repo_rev = response
        .headers()
        .get("atproto-repo-rev")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Get response body
    let status = response.status();
    let headers = response.headers().clone();
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| PdsError::Internal(format!("Failed to parse AppView response: {}", e)))?;

    // If no repo_rev header or request failed, just return AppView response as-is
    if repo_rev.is_none() || !status.is_success() {
        return Ok(build_response(status, headers, body));
    }

    let repo_rev_str = repo_rev.unwrap();

    // Try to get from cache first
    let local_records = if let Some(cached) = ctx.local_records_cache.get(user_did, &repo_rev_str).await {
        tracing::debug!(
            "Cache HIT for read-after-write: did={}, rev={}",
            user_did, repo_rev_str
        );
        (*cached).clone()
    } else {
        // Cache MISS - fetch from database
        tracing::debug!(
            "Cache MISS for read-after-write: did={}, rev={}",
            user_did, repo_rev_str
        );
        let records = ctx
            .actor_store
            .get_records_since_rev(user_did, &repo_rev_str)
            .await?;

        // Cache the result
        ctx.local_records_cache.set(user_did, &repo_rev_str, records.clone()).await;
        records
    };

    // If no local records, just return AppView response as-is
    if local_records.count == 0 {
        return Ok(build_response(status, headers, body));
    }

    // Create LocalViewer for formatting local records
    let viewer = LocalViewer::with_appview(
        user_did.to_string(),
        Arc::clone(&ctx.account_manager),
        Arc::clone(&ctx.actor_store),
        format!("https://{}", ctx.config.service.hostname),
        ctx.config.federation.appview_url.clone(),
    );

    // Merge local records into response
    let merged_body = merge_fn(body, &viewer, local_records)?;

    Ok(build_response(status, headers, merged_body))
}

/// Simple proxy to AppView without read-after-write
async fn proxy_to_appview<P: Serialize>(
    ctx: &AppContext,
    method: &str,
    params: P,
) -> PdsResult<Response> {
    let appview_url = ctx
        .config
        .federation
        .appview_url
        .as_ref()
        .ok_or_else(|| PdsError::Internal("AppView URL not configured".to_string()))?;

    let url = format!("{}/xrpc/{}", appview_url, method);
    let query_params = serde_json::to_value(&params)
        .map_err(|e| PdsError::Internal(format!("Failed to serialize params: {}", e)))?;

    let client = reqwest::Client::new();
    let mut req = client.get(&url);

    if let Some(obj) = query_params.as_object() {
        for (key, value) in obj {
            if let Some(v) = value.as_str() {
                req = req.query(&[(key, v)]);
            } else if let Some(v) = value.as_i64() {
                req = req.query(&[(key, v.to_string())]);
            } else if let Some(v) = value.as_bool() {
                req = req.query(&[(key, v.to_string())]);
            }
        }
    }

    let response = req
        .send()
        .await
        .map_err(|e| PdsError::Internal(format!("AppView request failed: {}", e)))?;

    let status = response.status();
    let headers = response.headers().clone();
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| PdsError::Internal(format!("Failed to parse AppView response: {}", e)))?;

    Ok(build_response(status, headers, body))
}

/// Merge local posts into timeline feed
fn merge_timeline_feed(
    mut body: serde_json::Value,
    viewer: &LocalViewer,
    local: read_after_write::LocalRecords,
) -> PdsResult<serde_json::Value> {
    let feed = body
        .get_mut("feed")
        .and_then(|f| f.as_array_mut())
        .ok_or_else(|| PdsError::Internal("Invalid timeline response format".to_string()))?;

    let feed_vec: Vec<serde_json::Value> = std::mem::take(feed);
    let merged = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(
            read_after_write::format_and_insert_posts_in_feed(viewer, feed_vec, &local.posts),
        )
    })?;

    body["feed"] = serde_json::Value::Array(merged);
    Ok(body)
}

/// Merge local posts into author feed
fn merge_author_feed(
    mut body: serde_json::Value,
    viewer: &LocalViewer,
    local: read_after_write::LocalRecords,
) -> PdsResult<serde_json::Value> {
    let feed = body
        .get_mut("feed")
        .and_then(|f| f.as_array_mut())
        .ok_or_else(|| PdsError::Internal("Invalid author feed response format".to_string()))?;

    let feed_vec: Vec<serde_json::Value> = std::mem::take(feed);
    let merged = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(
            read_after_write::format_and_insert_posts_in_feed(viewer, feed_vec, &local.posts),
        )
    })?;

    body["feed"] = serde_json::Value::Array(merged);
    Ok(body)
}

/// Merge local profile into profile response
fn merge_profile(
    mut body: serde_json::Value,
    viewer: &LocalViewer,
    local: read_after_write::LocalRecords,
) -> PdsResult<serde_json::Value> {
    // If there's a local profile update, merge it into the response
    if let Some(_profile_descript) = local.profile {
        let local_profile = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(viewer.get_profile_basic())
        })?;

        if let Some(profile) = local_profile {
            // Update display name if present
            if let Some(display_name) = profile.display_name {
                body["displayName"] = serde_json::Value::String(display_name);
            }

            // Update avatar if present
            if let Some(avatar) = profile.avatar {
                body["avatar"] = serde_json::Value::String(avatar);
            }
        }
    }

    Ok(body)
}

/// Build HTTP response from status, headers, and body
fn build_response(
    status: StatusCode,
    headers: HeaderMap,
    body: serde_json::Value,
) -> Response {
    let mut response = Json(body).into_response();
    *response.status_mut() = status;

    // Copy relevant headers
    let response_headers = response.headers_mut();
    for (key, value) in headers.iter() {
        if let Ok(header_name) = HeaderName::try_from(key.as_str()) {
            if let Ok(header_value) = HeaderValue::try_from(value.as_bytes()) {
                response_headers.insert(header_name, header_value);
            }
        }
    }

    response
}

// Request parameter types

#[derive(Debug, Deserialize, Serialize)]
pub struct TimelineParams {
    pub algorithm: Option<String>,
    pub limit: Option<i32>,
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AuthorFeedParams {
    pub actor: String,
    pub limit: Option<i32>,
    pub cursor: Option<String>,
    pub filter: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ProfileParams {
    pub actor: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GetPostsParams {
    pub uris: String, // Comma-separated URIs
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PostThreadParams {
    pub uri: String,
    pub depth: Option<i32>,
    pub parent_height: Option<i32>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct UriCursorParams {
    pub uri: String,
    pub limit: Option<i32>,
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ActorCursorParams {
    pub actor: String,
    pub limit: Option<i32>,
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CursorLimitParams {
    pub limit: Option<i32>,
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FeedUriParams {
    pub feed: String,
    pub limit: Option<i32>,
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ListUriParams {
    pub list: String,
    pub limit: Option<i32>,
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SearchParams {
    pub q: String,
    pub limit: Option<i32>,
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ProfilesParams {
    pub actors: String, // Comma-separated actors
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TypeaheadParams {
    pub q: String,
    pub limit: Option<i32>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct LabelerServicesParams {
    pub dids: String, // Comma-separated DIDs
    pub detailed: Option<bool>,
}

// ============================================================================
// Simple Proxy Handlers (no read-after-write needed)
// ============================================================================

/// Get posts by URI
async fn get_posts(
    State(ctx): State<AppContext>,
    _auth: OAuthAuthContext,
    Query(params): Query<GetPostsParams>,
) -> PdsResult<Response> {
    proxy_to_appview(&ctx, "app.bsky.feed.getPosts", params).await
}

/// Get post thread
async fn get_post_thread(
    State(ctx): State<AppContext>,
    _auth: OAuthAuthContext,
    Query(params): Query<PostThreadParams>,
) -> PdsResult<Response> {
    proxy_to_appview(&ctx, "app.bsky.feed.getPostThread", params).await
}

/// Get users who liked a post
async fn get_likes(
    State(ctx): State<AppContext>,
    _auth: OAuthAuthContext,
    Query(params): Query<UriCursorParams>,
) -> PdsResult<Response> {
    proxy_to_appview(&ctx, "app.bsky.feed.getLikes", params).await
}

/// Get users who reposted a post
async fn get_reposted_by(
    State(ctx): State<AppContext>,
    _auth: OAuthAuthContext,
    Query(params): Query<UriCursorParams>,
) -> PdsResult<Response> {
    proxy_to_appview(&ctx, "app.bsky.feed.getRepostedBy", params).await
}

/// Get quotes of a post
async fn get_quotes(
    State(ctx): State<AppContext>,
    _auth: OAuthAuthContext,
    Query(params): Query<UriCursorParams>,
) -> PdsResult<Response> {
    proxy_to_appview(&ctx, "app.bsky.feed.getQuotes", params).await
}

/// Get feed generator info
async fn get_feed_generator(
    State(ctx): State<AppContext>,
    _auth: OAuthAuthContext,
    Query(params): Query<FeedUriParams>,
) -> PdsResult<Response> {
    proxy_to_appview(&ctx, "app.bsky.feed.getFeedGenerator", params).await
}

/// Get multiple feed generators
async fn get_feed_generators(
    State(ctx): State<AppContext>,
    _auth: OAuthAuthContext,
    Query(params): Query<serde_json::Value>,
) -> PdsResult<Response> {
    proxy_to_appview(&ctx, "app.bsky.feed.getFeedGenerators", params).await
}

/// Get feeds created by an actor
async fn get_actor_feeds(
    State(ctx): State<AppContext>,
    _auth: OAuthAuthContext,
    Query(params): Query<ActorCursorParams>,
) -> PdsResult<Response> {
    proxy_to_appview(&ctx, "app.bsky.feed.getActorFeeds", params).await
}

/// Get suggested feeds
async fn get_suggested_feeds(
    State(ctx): State<AppContext>,
    _auth: OAuthAuthContext,
    Query(params): Query<CursorLimitParams>,
) -> PdsResult<Response> {
    proxy_to_appview(&ctx, "app.bsky.feed.getSuggestedFeeds", params).await
}

/// Get a custom feed
async fn get_feed(
    State(ctx): State<AppContext>,
    _auth: OAuthAuthContext,
    Query(params): Query<FeedUriParams>,
) -> PdsResult<Response> {
    proxy_to_appview(&ctx, "app.bsky.feed.getFeed", params).await
}

/// Get feed for a list
async fn get_list_feed(
    State(ctx): State<AppContext>,
    _auth: OAuthAuthContext,
    Query(params): Query<ListUriParams>,
) -> PdsResult<Response> {
    proxy_to_appview(&ctx, "app.bsky.feed.getListFeed", params).await
}

/// Search posts
async fn search_posts(
    State(ctx): State<AppContext>,
    _auth: OAuthAuthContext,
    Query(params): Query<SearchParams>,
) -> PdsResult<Response> {
    proxy_to_appview(&ctx, "app.bsky.feed.searchPosts", params).await
}

/// Get multiple profiles
async fn get_profiles(
    State(ctx): State<AppContext>,
    _auth: OAuthAuthContext,
    Query(params): Query<ProfilesParams>,
) -> PdsResult<Response> {
    proxy_to_appview(&ctx, "app.bsky.actor.getProfiles", params).await
}

/// Search actors
async fn search_actors(
    State(ctx): State<AppContext>,
    _auth: OAuthAuthContext,
    Query(params): Query<SearchParams>,
) -> PdsResult<Response> {
    proxy_to_appview(&ctx, "app.bsky.actor.searchActors", params).await
}

/// Search actors typeahead
async fn search_actors_typeahead(
    State(ctx): State<AppContext>,
    _auth: OAuthAuthContext,
    Query(params): Query<TypeaheadParams>,
) -> PdsResult<Response> {
    proxy_to_appview(&ctx, "app.bsky.actor.searchActorsTypeahead", params).await
}

/// Get suggested accounts to follow
async fn get_suggestions(
    State(ctx): State<AppContext>,
    _auth: OAuthAuthContext,
    Query(params): Query<CursorLimitParams>,
) -> PdsResult<Response> {
    proxy_to_appview(&ctx, "app.bsky.actor.getSuggestions", params).await
}

/// Get user preferences
async fn get_preferences(
    State(ctx): State<AppContext>,
    _auth: OAuthAuthContext,
) -> PdsResult<Response> {
    proxy_to_appview(&ctx, "app.bsky.actor.getPreferences", serde_json::json!({})).await
}

/// Get followers
async fn get_followers(
    State(ctx): State<AppContext>,
    _auth: OAuthAuthContext,
    Query(params): Query<ActorCursorParams>,
) -> PdsResult<Response> {
    proxy_to_appview(&ctx, "app.bsky.graph.getFollowers", params).await
}

/// Get follows
async fn get_follows(
    State(ctx): State<AppContext>,
    _auth: OAuthAuthContext,
    Query(params): Query<ActorCursorParams>,
) -> PdsResult<Response> {
    proxy_to_appview(&ctx, "app.bsky.graph.getFollows", params).await
}

/// Get blocked accounts
async fn get_blocks(
    State(ctx): State<AppContext>,
    _auth: OAuthAuthContext,
    Query(params): Query<CursorLimitParams>,
) -> PdsResult<Response> {
    proxy_to_appview(&ctx, "app.bsky.graph.getBlocks", params).await
}

/// Get muted accounts
async fn get_mutes(
    State(ctx): State<AppContext>,
    _auth: OAuthAuthContext,
    Query(params): Query<CursorLimitParams>,
) -> PdsResult<Response> {
    proxy_to_appview(&ctx, "app.bsky.graph.getMutes", params).await
}

/// Get lists created by an actor
async fn get_lists(
    State(ctx): State<AppContext>,
    _auth: OAuthAuthContext,
    Query(params): Query<ActorCursorParams>,
) -> PdsResult<Response> {
    proxy_to_appview(&ctx, "app.bsky.graph.getLists", params).await
}

/// Get list info
async fn get_list(
    State(ctx): State<AppContext>,
    _auth: OAuthAuthContext,
    Query(params): Query<ListUriParams>,
) -> PdsResult<Response> {
    proxy_to_appview(&ctx, "app.bsky.graph.getList", params).await
}

/// Get list members
async fn get_list_members(
    State(ctx): State<AppContext>,
    _auth: OAuthAuthContext,
    Query(params): Query<ListUriParams>,
) -> PdsResult<Response> {
    proxy_to_appview(&ctx, "app.bsky.graph.getListMembers", params).await
}

/// Get lists an actor is a member of
async fn get_list_memberships(
    State(ctx): State<AppContext>,
    _auth: OAuthAuthContext,
    Query(params): Query<ActorCursorParams>,
) -> PdsResult<Response> {
    proxy_to_appview(&ctx, "app.bsky.graph.getListMemberships", params).await
}

/// List notifications
async fn list_notifications(
    State(ctx): State<AppContext>,
    _auth: OAuthAuthContext,
    Query(params): Query<CursorLimitParams>,
) -> PdsResult<Response> {
    proxy_to_appview(&ctx, "app.bsky.notification.listNotifications", params).await
}

/// Get unread notification count
async fn get_unread_count(
    State(ctx): State<AppContext>,
    _auth: OAuthAuthContext,
) -> PdsResult<Response> {
    proxy_to_appview(&ctx, "app.bsky.notification.getUnreadCount", serde_json::json!({})).await
}

/// Get labeler services
async fn get_labeler_services(
    State(ctx): State<AppContext>,
    _auth: OAuthAuthContext,
    Query(params): Query<LabelerServicesParams>,
) -> PdsResult<Response> {
    proxy_to_appview(&ctx, "app.bsky.labeler.getServices", params).await
}
