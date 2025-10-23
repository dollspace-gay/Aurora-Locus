/// Federated search across multiple PDS instances
///
/// Enables searching for content, users, and posts across the entire federation

use crate::error::{PdsError, PdsResult};
use crate::federation::discovery::{PdsDiscovery, PdsInstance};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::task::JoinSet;
use tracing::{debug, warn};

/// Federated search client
pub struct FederatedSearch {
    http_client: Client,
    discovery: Arc<PdsDiscovery>,
    max_concurrent: usize,
    timeout_secs: u64,
}

impl FederatedSearch {
    /// Create a new federated search client
    pub fn new(discovery: Arc<PdsDiscovery>, max_concurrent: usize, timeout_secs: u64) -> Self {
        Self {
            http_client: Client::builder()
                .timeout(std::time::Duration::from_secs(timeout_secs))
                .build()
                .unwrap(),
            discovery,
            max_concurrent,
            timeout_secs,
        }
    }

    /// Search for actors (users) across all known PDS instances
    pub async fn search_actors(&self, query: &str, limit: usize) -> PdsResult<Vec<ActorResult>> {
        debug!("Federated actor search: query='{}', limit={}", query, limit);

        let instances = self.discovery.get_known_instances().await;
        let mut results = Vec::new();

        // Create tasks for parallel searching
        let mut tasks = JoinSet::new();

        for instance in instances.iter().take(self.max_concurrent) {
            let client = self.http_client.clone();
            let instance = instance.clone();
            let query = query.to_string();
            let search_limit = (limit as f64 / self.max_concurrent as f64).ceil() as usize;

            tasks.spawn(async move {
                Self::search_actors_on_instance(&client, &instance, &query, search_limit).await
            });
        }

        // Collect results
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(Ok(actors)) => results.extend(actors),
                Ok(Err(e)) => warn!("PDS search failed: {}", e),
                Err(e) => warn!("Task join error: {}", e),
            }
        }

        // Sort by relevance and limit
        results.sort_by(|a, b| b.followers_count.cmp(&a.followers_count));
        results.truncate(limit);

        debug!("Federated search returned {} actors", results.len());

        Ok(results)
    }

    /// Search for posts across all known PDS instances
    pub async fn search_posts(&self, query: &str, limit: usize) -> PdsResult<Vec<PostResult>> {
        debug!("Federated post search: query='{}', limit={}", query, limit);

        let instances = self.discovery.get_known_instances().await;
        let mut results = Vec::new();

        let mut tasks = JoinSet::new();

        for instance in instances.iter().take(self.max_concurrent) {
            let client = self.http_client.clone();
            let instance = instance.clone();
            let query = query.to_string();
            let search_limit = (limit as f64 / self.max_concurrent as f64).ceil() as usize;

            tasks.spawn(async move {
                Self::search_posts_on_instance(&client, &instance, &query, search_limit).await
            });
        }

        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(Ok(posts)) => results.extend(posts),
                Ok(Err(e)) => warn!("PDS search failed: {}", e),
                Err(e) => warn!("Task join error: {}", e),
            }
        }

        // Sort by recency
        results.sort_by(|a, b| b.indexed_at.cmp(&a.indexed_at));
        results.truncate(limit);

        debug!("Federated search returned {} posts", results.len());

        Ok(results)
    }

    /// Search actors on a specific PDS instance
    async fn search_actors_on_instance(
        client: &Client,
        instance: &PdsInstance,
        query: &str,
        limit: usize,
    ) -> PdsResult<Vec<ActorResult>> {
        let url = format!(
            "{}/xrpc/app.bsky.actor.searchActors?q={}&limit={}",
            instance.url,
            urlencoding::encode(query),
            limit
        );

        let response = client.get(&url).send().await.map_err(|e| {
            PdsError::Internal(format!("Failed to search PDS {}: {}", instance.did, e))
        })?;

        if !response.status().is_success() {
            return Ok(Vec::new()); // Silently skip failed instances
        }

        let search_response: ActorSearchResponse = response.json().await.map_err(|e| {
            PdsError::Internal(format!("Failed to parse search response: {}", e))
        })?;

        Ok(search_response.actors)
    }

    /// Search posts on a specific PDS instance
    async fn search_posts_on_instance(
        client: &Client,
        instance: &PdsInstance,
        query: &str,
        limit: usize,
    ) -> PdsResult<Vec<PostResult>> {
        let url = format!(
            "{}/xrpc/app.bsky.feed.searchPosts?q={}&limit={}",
            instance.url,
            urlencoding::encode(query),
            limit
        );

        let response = client.get(&url).send().await.map_err(|e| {
            PdsError::Internal(format!("Failed to search PDS {}: {}", instance.did, e))
        })?;

        if !response.status().is_success() {
            return Ok(Vec::new());
        }

        let search_response: PostSearchResponse = response.json().await.map_err(|e| {
            PdsError::Internal(format!("Failed to parse search response: {}", e))
        })?;

        Ok(search_response.posts)
    }

    /// Aggregate timeline from multiple PDS instances
    pub async fn aggregate_timeline(&self, dids: Vec<String>, limit: usize) -> PdsResult<Vec<PostResult>> {
        debug!("Aggregating timeline from {} DIDs", dids.len());

        let mut results = Vec::new();
        let mut tasks = JoinSet::new();

        for did in dids {
            let client = self.http_client.clone();
            let discovery = self.discovery.clone();

            tasks.spawn(async move {
                // Find PDS for this DID
                if let Some(instance) = discovery.find_by_did(&did).await {
                    Self::fetch_author_feed(&client, &instance, &did, 20).await
                } else {
                    Ok(Vec::new())
                }
            });
        }

        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(Ok(posts)) => results.extend(posts),
                Ok(Err(e)) => warn!("Feed fetch failed: {}", e),
                Err(e) => warn!("Task join error: {}", e),
            }
        }

        // Sort by recency
        results.sort_by(|a, b| b.indexed_at.cmp(&a.indexed_at));
        results.truncate(limit);

        Ok(results)
    }

    /// Fetch author feed from a PDS
    async fn fetch_author_feed(
        client: &Client,
        instance: &PdsInstance,
        did: &str,
        limit: usize,
    ) -> PdsResult<Vec<PostResult>> {
        let url = format!(
            "{}/xrpc/app.bsky.feed.getAuthorFeed?actor={}&limit={}",
            instance.url, did, limit
        );

        let response = client.get(&url).send().await.map_err(|e| {
            PdsError::Internal(format!("Failed to fetch feed: {}", e))
        })?;

        if !response.status().is_success() {
            return Ok(Vec::new());
        }

        let feed_response: FeedResponse = response.json().await.map_err(|e| {
            PdsError::Internal(format!("Failed to parse feed response: {}", e))
        })?;

        Ok(feed_response
            .feed
            .into_iter()
            .map(|item| item.post)
            .collect())
    }
}

/// Actor search response
#[derive(Debug, Deserialize)]
struct ActorSearchResponse {
    actors: Vec<ActorResult>,
}

/// Actor search result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActorResult {
    pub did: String,
    pub handle: String,
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub avatar: Option<String>,
    #[serde(rename = "followersCount", default)]
    pub followers_count: i64,
    #[serde(rename = "followsCount", default)]
    pub follows_count: i64,
    #[serde(rename = "postsCount", default)]
    pub posts_count: i64,
}

/// Post search response
#[derive(Debug, Deserialize)]
struct PostSearchResponse {
    posts: Vec<PostResult>,
}

/// Post search result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostResult {
    pub uri: String,
    pub cid: String,
    pub author: ActorResult,
    pub record: serde_json::Value,
    #[serde(rename = "indexedAt")]
    pub indexed_at: String,
    #[serde(rename = "replyCount", default)]
    pub reply_count: i64,
    #[serde(rename = "repostCount", default)]
    pub repost_count: i64,
    #[serde(rename = "likeCount", default)]
    pub like_count: i64,
}

/// Feed response
#[derive(Debug, Deserialize)]
struct FeedResponse {
    feed: Vec<FeedItem>,
}

#[derive(Debug, Deserialize)]
struct FeedItem {
    post: PostResult,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_actor_result_serialization() {
        let actor = ActorResult {
            did: "did:plc:test123".to_string(),
            handle: "alice.bsky.social".to_string(),
            display_name: Some("Alice".to_string()),
            description: Some("Test user".to_string()),
            avatar: None,
            followers_count: 100,
            follows_count: 50,
            posts_count: 200,
        };

        let json = serde_json::to_string(&actor).unwrap();
        let deserialized: ActorResult = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.did, "did:plc:test123");
        assert_eq!(deserialized.followers_count, 100);
    }
}
