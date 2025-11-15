/// AppView proxy endpoints with read-after-write consistency
///
/// These endpoints proxy requests to the external AppView service (e.g., Bluesky's AppView)
/// and merge in local records that haven't been indexed yet (read-after-write consistency).

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
        .route("/xrpc/app.bsky.feed.getTimeline", get(get_timeline))
        .route("/xrpc/app.bsky.feed.getAuthorFeed", get(get_author_feed))
        .route("/xrpc/app.bsky.actor.getProfile", get(get_profile))
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

    let feed_vec: Vec<serde_json::Value> = feed.drain(..).collect();
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

    let feed_vec: Vec<serde_json::Value> = feed.drain(..).collect();
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
