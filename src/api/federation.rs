/// Federation management API endpoints (Phase 1 & 2)
use crate::{
    auth::AdminAuthContext,
    context::AppContext,
    error::{PdsError, PdsResult},
    federation::search::{ActorResult, PostResult},
};
use axum::{
    extract::{Query, State},
    response::Json,
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};

/// Build federation routes
pub fn routes() -> Router<AppContext> {
    Router::new()
        // Phase 1: Admin & status endpoints
        .route("/xrpc/com.aurora.federation.status", get(federation_status))
        .route(
            "/xrpc/com.aurora.federation.listInstances",
            get(list_instances),
        )
        .route(
            "/xrpc/com.aurora.federation.refreshDiscovery",
            post(refresh_discovery),
        )
        // Phase 2: Federated search (dead code, see commit message)
        // Routes deferred to avoid collision with appview proxy.
        .route(
            "/xrpc/com.aurora.federation.aggregateTimeline",
            get(aggregate_timeline),
        )
        // Phase 4: DPoP support
        .route(
            "/xrpc/com.aurora.dpop.getNonce",
            get(get_dpop_nonce),
        )
}

/// Get federation status (public endpoint)
async fn federation_status(
    State(ctx): State<AppContext>,
) -> Json<FederationStatusResponse> {
    let enabled = ctx.config.federation.enabled;
    let relay_connected = ctx.relay_client.is_some();
    let discovery_active = ctx.pds_discovery.is_some();
    let auth_active = ctx.federation_auth.is_some();

    let known_instances = if let Some(discovery) = &ctx.pds_discovery {
        discovery.get_known_instances().await.len()
    } else {
        0
    };

    Json(FederationStatusResponse {
        enabled,
        relay_connected,
        discovery_active,
        auth_active,
        known_instances,
        relay_urls: ctx.config.federation.relay_urls.clone(),
    })
}

/// List known PDS instances (admin only)
async fn list_instances(
    State(ctx): State<AppContext>,
    _admin: AdminAuthContext,
) -> PdsResult<Json<ListInstancesResponse>> {
    let instances = if let Some(discovery) = &ctx.pds_discovery {
        discovery.get_known_instances().await
    } else {
        vec![]
    };

    Ok(Json(ListInstancesResponse { instances }))
}

/// Manually trigger discovery refresh (admin only)
async fn refresh_discovery(
    State(ctx): State<AppContext>,
    _admin: AdminAuthContext,
) -> PdsResult<Json<RefreshResponse>> {
    if let Some(discovery) = &ctx.pds_discovery {
        discovery.refresh_instances().await?;
        let count = discovery.get_known_instances().await.len();
        Ok(Json(RefreshResponse {
            success: true,
            instances_found: count,
        }))
    } else {
        Ok(Json(RefreshResponse {
            success: false,
            instances_found: 0,
        }))
    }
}

// Response types

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FederationStatusResponse {
    pub enabled: bool,
    pub relay_connected: bool,
    pub discovery_active: bool,
    pub auth_active: bool,
    pub known_instances: usize,
    pub relay_urls: Vec<String>,
}

#[derive(Serialize)]
pub struct ListInstancesResponse {
    pub instances: Vec<crate::federation::discovery::PdsInstance>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshResponse {
    pub success: bool,
    pub instances_found: usize,
}

// Phase 2: Federated Search Endpoints

/// Search for actors across federated PDS instances
async fn search_actors(
    State(ctx): State<AppContext>,
    Query(params): Query<SearchQuery>,
) -> PdsResult<Json<SearchActorsResponse>> {
    // Validate query
    if params.q.trim().is_empty() {
        return Err(PdsError::Validation("Query cannot be empty".to_string()));
    }

    // Check if federation is enabled
    let search = ctx.federated_search.as_ref().ok_or_else(|| {
        PdsError::Validation("Federation is not enabled".to_string())
    })?;

    // Rate limiting is handled by the rate_limiter middleware
    let limit = params.limit.unwrap_or(25).min(100); // Cap at 100 results

    let actors = search.search_actors(&params.q, limit).await?;

    Ok(Json(SearchActorsResponse { actors }))
}

/// Search for posts across federated PDS instances
async fn search_posts(
    State(ctx): State<AppContext>,
    Query(params): Query<SearchQuery>,
) -> PdsResult<Json<SearchPostsResponse>> {
    // Validate query
    if params.q.trim().is_empty() {
        return Err(PdsError::Validation("Query cannot be empty".to_string()));
    }

    // Check if federation is enabled
    let search = ctx.federated_search.as_ref().ok_or_else(|| {
        PdsError::Validation("Federation is not enabled".to_string())
    })?;

    let limit = params.limit.unwrap_or(25).min(100);

    let posts = search.search_posts(&params.q, limit).await?;

    Ok(Json(SearchPostsResponse { posts }))
}

/// Aggregate timeline from multiple users across federated PDS instances
async fn aggregate_timeline(
    State(ctx): State<AppContext>,
    Query(params): Query<TimelineQuery>,
) -> PdsResult<Json<TimelineResponse>> {
    // Validate DIDs
    if params.dids.is_empty() {
        return Err(PdsError::Validation("At least one DID is required".to_string()));
    }

    if params.dids.len() > 50 {
        return Err(PdsError::Validation("Maximum 50 DIDs allowed".to_string()));
    }

    // Check if federation is enabled
    let search = ctx.federated_search.as_ref().ok_or_else(|| {
        PdsError::Validation("Federation is not enabled".to_string())
    })?;

    let limit = params.limit.unwrap_or(50).min(200);

    let posts = search.aggregate_timeline(params.dids, limit).await?;

    Ok(Json(TimelineResponse { feed: posts }))
}

// Query parameter types

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: String,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct TimelineQuery {
    #[serde(default, deserialize_with = "deserialize_string_list")]
    pub dids: Vec<String>,
    pub limit: Option<usize>,
}

/// Custom deserializer for comma-separated string lists
fn deserialize_string_list<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    Ok(s.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect())
}

// Response types for Phase 2

#[derive(Serialize)]
pub struct SearchActorsResponse {
    pub actors: Vec<ActorResult>,
}

#[derive(Serialize)]
pub struct SearchPostsResponse {
    pub posts: Vec<PostResult>,
}

#[derive(Serialize)]
pub struct TimelineResponse {
    pub feed: Vec<PostResult>,
}

/// Get DPoP nonce (Phase 4)
///
/// Returns a fresh nonce for clients to use in DPoP proof JWTs.
/// Clients should call this endpoint before making DPoP-protected requests.
///
/// Reference: https://datatracker.ietf.org/doc/html/rfc9449#section-8
async fn get_dpop_nonce(
    State(ctx): State<AppContext>,
) -> PdsResult<Json<DPopNonceResponse>> {
    let dpop_nonce_store = ctx
        .dpop_nonce_store
        .as_ref()
        .ok_or_else(|| PdsError::Internal("DPoP support not enabled".to_string()))?;

    let nonce = dpop_nonce_store.generate_nonce().await;

    Ok(Json(DPopNonceResponse { nonce }))
}

#[derive(Serialize)]
pub struct DPopNonceResponse {
    pub nonce: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_routes_compile() {
        // Ensure routes() compiles correctly
        let _router = routes();
    }
}
