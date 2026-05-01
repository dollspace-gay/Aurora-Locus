//! Federated search across multiple PDS instances.
//!
//! Enables searching for content, users, and posts across the entire
//! federation. The implementation is staged but not yet wired to public
//! routes — see `src/api/federation.rs` for the deferral note. Allow
//! dead_code at the module level so the staged code doesn't drown the
//! lint output.
#![allow(dead_code)]

use crate::error::{PdsError, PdsResult};
use crate::federation::discovery::{PdsDiscovery, PdsInstance};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio::task::JoinSet;
use tracing::{debug, warn};

/// Circuit breaker state for a PDS instance
#[derive(Clone)]
struct CircuitState {
    failure_count: u32,
    last_failure: Instant,
    open_until: Option<Instant>,
}

/// Federated search client with circuit breaker pattern
pub struct FederatedSearch {
    http_client: Client,
    discovery: Arc<PdsDiscovery>,
    max_concurrent: usize,
    #[allow(dead_code)] // Configured into http_client during construction
    timeout_secs: u64,
    /// Circuit breaker state per PDS instance (DID -> state)
    circuit_breakers: Arc<RwLock<HashMap<String, CircuitState>>>,
    /// Maximum failures before opening circuit (default: 3)
    failure_threshold: u32,
    /// Duration to keep circuit open (default: 60 seconds)
    cooldown_duration: Duration,
}

impl FederatedSearch {
    /// Create a new federated search client with circuit breaker
    pub fn new(discovery: Arc<PdsDiscovery>, max_concurrent: usize, timeout_secs: u64) -> Self {
        Self {
            http_client: Client::builder()
                .timeout(Duration::from_secs(timeout_secs))
                .build()
                .unwrap(),
            discovery,
            max_concurrent,
            timeout_secs,
            circuit_breakers: Arc::new(RwLock::new(HashMap::new())),
            failure_threshold: 3,
            cooldown_duration: Duration::from_secs(60),
        }
    }

    /// Check if circuit breaker is open for a PDS instance
    async fn is_circuit_open(&self, pds_did: &str) -> bool {
        let breakers = self.circuit_breakers.read().await;
        if let Some(state) = breakers.get(pds_did) {
            if let Some(open_until) = state.open_until {
                if Instant::now() < open_until {
                    debug!("Circuit breaker open for PDS: {}", pds_did);
                    return true;
                }
            }
        }
        false
    }

    /// Record a failure for a PDS instance
    async fn record_failure(&self, pds_did: &str) {
        let mut breakers = self.circuit_breakers.write().await;
        let state = breakers.entry(pds_did.to_string()).or_insert(CircuitState {
            failure_count: 0,
            last_failure: Instant::now(),
            open_until: None,
        });

        state.failure_count += 1;
        state.last_failure = Instant::now();

        // Open circuit if threshold exceeded
        if state.failure_count >= self.failure_threshold {
            state.open_until = Some(Instant::now() + self.cooldown_duration);
            warn!(
                "Circuit breaker opened for PDS {} after {} failures (cooldown: {}s)",
                pds_did,
                state.failure_count,
                self.cooldown_duration.as_secs()
            );
        }
    }

    /// Record a success for a PDS instance (resets circuit breaker)
    async fn record_success(&self, pds_did: &str) {
        let mut breakers = self.circuit_breakers.write().await;
        if let Some(state) = breakers.get_mut(pds_did) {
            if state.failure_count > 0 {
                debug!("Circuit breaker reset for PDS: {}", pds_did);
            }
            state.failure_count = 0;
            state.open_until = None;
        }
    }

    /// Search for actors (users) across all known PDS instances
    pub async fn search_actors(&self, query: &str, limit: usize) -> PdsResult<Vec<ActorResult>> {
        debug!("Federated actor search: query='{}', limit={}", query, limit);

        let instances = self.discovery.get_known_instances().await;

        // Filter out instances with open circuit breakers
        let mut available_instances = Vec::new();
        for instance in instances {
            if !self.is_circuit_open(&instance.did).await {
                available_instances.push(instance);
            }
        }

        if available_instances.is_empty() {
            warn!("No available PDS instances (all circuit breakers open)");
            return Ok(Vec::new());
        }

        debug!("Searching {} PDS instance(s)", available_instances.len());
        let mut results = Vec::new();

        // Create tasks for parallel searching
        let mut tasks = JoinSet::new();
        let instances_to_search = available_instances.iter().take(self.max_concurrent);
        let search_limit = (limit as f64 / self.max_concurrent as f64).ceil() as usize;

        for instance in instances_to_search {
            let client = self.http_client.clone();
            let instance = instance.clone();
            let query = query.to_string();

            tasks.spawn(async move {
                let result =
                    Self::search_actors_on_instance(&client, &instance, &query, search_limit).await;
                (instance.did.clone(), result)
            });
        }

        // Collect results and track success/failure
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok((pds_did, Ok(actors))) => {
                    self.record_success(&pds_did).await;
                    results.extend(actors);
                }
                Ok((pds_did, Err(e))) => {
                    warn!("PDS search failed for {}: {}", pds_did, e);
                    self.record_failure(&pds_did).await;
                }
                Err(e) => warn!("Task join error: {}", e),
            }
        }

        // Deduplicate actors by DID
        let mut seen_dids = HashSet::new();
        let mut deduplicated = Vec::new();
        for actor in results {
            if seen_dids.insert(actor.did.clone()) {
                deduplicated.push(actor);
            }
        }

        // Sort by relevance (highest followers first) and limit.
        deduplicated.sort_by_key(|actor| std::cmp::Reverse(actor.followers_count));
        deduplicated.truncate(limit);

        debug!(
            "Federated search returned {} unique actors from {} PDS(s)",
            deduplicated.len(),
            available_instances.len()
        );

        Ok(deduplicated)
    }

    /// Search for posts across all known PDS instances
    pub async fn search_posts(&self, query: &str, limit: usize) -> PdsResult<Vec<PostResult>> {
        debug!("Federated post search: query='{}', limit={}", query, limit);

        let instances = self.discovery.get_known_instances().await;

        // Filter out instances with open circuit breakers
        let mut available_instances = Vec::new();
        for instance in instances {
            if !self.is_circuit_open(&instance.did).await {
                available_instances.push(instance);
            }
        }

        if available_instances.is_empty() {
            warn!("No available PDS instances (all circuit breakers open)");
            return Ok(Vec::new());
        }

        debug!("Searching {} PDS instance(s)", available_instances.len());
        let mut results = Vec::new();

        let mut tasks = JoinSet::new();
        let instances_to_search = available_instances.iter().take(self.max_concurrent);
        let search_limit = (limit as f64 / self.max_concurrent as f64).ceil() as usize;

        for instance in instances_to_search {
            let client = self.http_client.clone();
            let instance = instance.clone();
            let query = query.to_string();

            tasks.spawn(async move {
                let result =
                    Self::search_posts_on_instance(&client, &instance, &query, search_limit).await;
                (instance.did.clone(), result)
            });
        }

        while let Some(result) = tasks.join_next().await {
            match result {
                Ok((pds_did, Ok(posts))) => {
                    self.record_success(&pds_did).await;
                    results.extend(posts);
                }
                Ok((pds_did, Err(e))) => {
                    warn!("PDS search failed for {}: {}", pds_did, e);
                    self.record_failure(&pds_did).await;
                }
                Err(e) => warn!("Task join error: {}", e),
            }
        }

        // Deduplicate posts by URI
        let mut seen_uris = HashSet::new();
        let mut deduplicated = Vec::new();
        for post in results {
            if seen_uris.insert(post.uri.clone()) {
                deduplicated.push(post);
            }
        }

        // Sort by recency
        deduplicated.sort_by(|a, b| b.indexed_at.cmp(&a.indexed_at));
        deduplicated.truncate(limit);

        debug!(
            "Federated search returned {} unique posts from {} PDS(s)",
            deduplicated.len(),
            available_instances.len()
        );

        Ok(deduplicated)
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

        let search_response: ActorSearchResponse = response
            .json()
            .await
            .map_err(|e| PdsError::Internal(format!("Failed to parse search response: {}", e)))?;

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

        let search_response: PostSearchResponse = response
            .json()
            .await
            .map_err(|e| PdsError::Internal(format!("Failed to parse search response: {}", e)))?;

        Ok(search_response.posts)
    }

    /// Aggregate timeline from multiple PDS instances
    pub async fn aggregate_timeline(
        &self,
        dids: Vec<String>,
        limit: usize,
    ) -> PdsResult<Vec<PostResult>> {
        debug!("Aggregating timeline from {} DIDs", dids.len());

        let mut results = Vec::new();
        let mut tasks = JoinSet::new();

        for did in dids {
            let client = self.http_client.clone();
            let discovery = self.discovery.clone();
            let circuit_breakers = self.circuit_breakers.clone();

            tasks.spawn(async move {
                // Find PDS for this DID
                if let Some(instance) = discovery.find_by_did(&did).await {
                    // Check circuit breaker
                    let breakers = circuit_breakers.read().await;
                    let is_open = if let Some(state) = breakers.get(&instance.did) {
                        if let Some(open_until) = state.open_until {
                            Instant::now() < open_until
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                    drop(breakers);

                    if is_open {
                        debug!(
                            "Skipping feed fetch from {} (circuit breaker open)",
                            instance.did
                        );
                        return (Some(instance.did.clone()), Ok(Vec::new()));
                    }

                    let result = Self::fetch_author_feed(&client, &instance, &did, 20).await;
                    (Some(instance.did.clone()), result)
                } else {
                    (None, Ok(Vec::new()))
                }
            });
        }

        while let Some(result) = tasks.join_next().await {
            match result {
                Ok((Some(pds_did), Ok(posts))) => {
                    self.record_success(&pds_did).await;
                    results.extend(posts);
                }
                Ok((Some(pds_did), Err(e))) => {
                    warn!("Feed fetch failed for {}: {}", pds_did, e);
                    self.record_failure(&pds_did).await;
                }
                Ok((None, _)) => {
                    // No instance found for DID, skip
                }
                Err(e) => warn!("Task join error: {}", e),
            }
        }

        // Deduplicate posts by URI
        let mut seen_uris = HashSet::new();
        let mut deduplicated = Vec::new();
        for post in results {
            if seen_uris.insert(post.uri.clone()) {
                deduplicated.push(post);
            }
        }

        // Sort by recency
        deduplicated.sort_by(|a, b| b.indexed_at.cmp(&a.indexed_at));
        deduplicated.truncate(limit);

        debug!(
            "Aggregated {} unique posts from timeline",
            deduplicated.len()
        );

        Ok(deduplicated)
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

        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| PdsError::Internal(format!("Failed to fetch feed: {}", e)))?;

        if !response.status().is_success() {
            return Ok(Vec::new());
        }

        let feed_response: FeedResponse = response
            .json()
            .await
            .map_err(|e| PdsError::Internal(format!("Failed to parse feed response: {}", e)))?;

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
