/// Identity Resolution System
///
/// Handles DID document resolution, handle resolution, and caching
/// for efficient cross-server identity lookups.

pub mod cache;
pub mod handle_validation;
pub mod reserved_handles;
pub mod resolver;

pub use cache::DidCache;
pub use handle_validation::{validate_handle, normalize_handle};
pub use reserved_handles::{is_reserved, check_reserved};
pub use resolver::{IdentityResolver, IdentityResolverConfig};

use chrono::{DateTime, Utc};
use serde::Serialize;

/// Cached DID document entry with stale flag
///
/// The `stale` flag indicates whether this cached data is past its fresh TTL
/// but still within max TTL. Stale data can be used as fallback during outages.
#[derive(Debug, Clone)]
pub struct CachedDidDoc {
    pub did: String,
    pub doc: String,  // JSON-encoded DID document
    pub updated_at: DateTime<Utc>,
    pub cached_at: DateTime<Utc>,
    /// True if cached_at is past stale_ttl but within max_ttl
    pub stale: bool,
}

/// Cached handle mapping entry with stale flag
#[derive(Debug, Clone)]
pub struct CachedHandle {
    pub handle: String,
    pub did: String,
    pub declared_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
    /// True if updated_at is past stale_ttl but within max_ttl
    pub stale: bool,
}

/// Handle resolution result
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HandleResolutionResult {
    pub did: String,
}

/// DID document resolution result
#[derive(Debug, Clone, Serialize)]
pub struct DidDocResolutionResult {
    pub did: String,
    pub doc: serde_json::Value,
}
