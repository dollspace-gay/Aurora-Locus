//! Identity Resolution System
//!
//! Handles DID document resolution, handle resolution, and caching
//! for efficient cross-server identity lookups.

pub mod cache;
pub mod handle_validation;
pub mod reserved_handles;
pub mod resolver;

pub use cache::DidCache;
pub use handle_validation::validate_handle;
pub use resolver::{IdentityResolver, IdentityResolverConfig};

use chrono::{DateTime, Utc};
use serde::Serialize;

/// Cached DID document entry with stale flag
///
/// The `stale` flag indicates whether this cached data is past its fresh TTL
/// but still within max TTL. Stale data can be used as fallback during outages.
#[derive(Debug, Clone)]
pub struct CachedDidDoc {
    #[allow(dead_code)] // Future DID tracking
    pub did: String,
    pub doc: String,  // JSON-encoded DID document
    #[allow(dead_code)] // Future cache timestamp tracking
    pub updated_at: DateTime<Utc>,
    #[allow(dead_code)] // Future cache timestamp tracking
    pub cached_at: DateTime<Utc>,
    /// True if cached_at is past stale_ttl but within max_ttl
    pub stale: bool,
}

/// Cached handle mapping entry with stale flag
#[derive(Debug, Clone)]
pub struct CachedHandle {
    #[allow(dead_code)] // Future handle tracking
    pub handle: String,
    pub did: String,
    #[allow(dead_code)] // Future timestamp tracking
    pub declared_at: Option<DateTime<Utc>>,
    #[allow(dead_code)] // Future timestamp tracking
    pub updated_at: DateTime<Utc>,
    /// True if updated_at is past stale_ttl but within max_ttl
    pub stale: bool,
}

/// Handle resolution result
    #[allow(dead_code)] // Future handle resolution result
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HandleResolutionResult {
    pub did: String,
}
    #[allow(dead_code)] // Future DID doc resolution result

/// DID document resolution result
#[derive(Debug, Clone, Serialize)]
pub struct DidDocResolutionResult {
    pub did: String,
    pub doc: serde_json::Value,
}
