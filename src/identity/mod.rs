/// Identity Resolution System
///
/// Handles DID document resolution, handle resolution, and caching
/// for efficient cross-server identity lookups.

pub mod cache;
pub mod resolver;

pub use cache::DidCache;
pub use resolver::{IdentityResolver, IdentityResolverConfig};

use chrono::{DateTime, Utc};
use serde::Serialize;

/// Cached DID document entry
#[derive(Debug, Clone)]
pub struct CachedDidDoc {
    pub did: String,
    pub doc: String,  // JSON-encoded DID document
    pub updated_at: DateTime<Utc>,
    pub cached_at: DateTime<Utc>,
}

/// Cached handle mapping entry
#[derive(Debug, Clone)]
pub struct CachedHandle {
    pub handle: String,
    pub did: String,
    pub declared_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
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
