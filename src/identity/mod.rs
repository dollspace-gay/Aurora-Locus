//! Identity Resolution System
//!
//! Handles DID document resolution, handle resolution, and caching
//! for efficient cross-server identity lookups.

pub mod cache;
pub mod clock;
pub mod did_document;
pub mod handle_validation;
pub mod reserved_handles;
pub mod resolver;

pub use cache::DidCache;
pub use handle_validation::validate_handle;
pub use resolver::{IdentityResolver, IdentityResolverApi, IdentityResolverConfig};

use chrono::{DateTime, Utc};

/// Cached DID document entry with stale flag
///
/// The `stale` flag indicates whether this cached data is past its fresh TTL
/// but still within max TTL. Stale data can be used as fallback during outages.
#[derive(Debug, Clone)]
pub struct CachedDidDoc {
    /// The DID this document belongs to
    pub did: String,
    /// JSON-encoded DID document
    pub doc: String,
    /// When the DID document was last updated at source
    pub updated_at: DateTime<Utc>,
    /// When this entry was cached locally
    pub cached_at: DateTime<Utc>,
    /// True if cached_at is past stale_ttl but within max_ttl
    pub stale: bool,
}

/// Cached handle mapping entry with stale flag
#[derive(Debug, Clone)]
pub struct CachedHandle {
    /// The handle that was resolved
    pub handle: String,
    /// The DID the handle resolves to
    pub did: String,
    /// When the handle was declared in the DID document (if available)
    pub declared_at: Option<DateTime<Utc>>,
    /// When this cache entry was last updated
    pub updated_at: DateTime<Utc>,
    /// True if updated_at is past stale_ttl but within max_ttl
    pub stale: bool,
}
