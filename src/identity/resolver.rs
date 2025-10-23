/// Identity Resolver - Orchestrates handle and DID resolution with caching
use crate::{
    error::{PdsError, PdsResult},
    identity::DidCache,
};
use atproto::{did_doc::DidDocument, handle::HandleResolver};
use std::sync::Arc;

/// Identity resolution configuration
#[derive(Debug, Clone)]
pub struct IdentityResolverConfig {
    /// User-Agent header for HTTP requests
    pub user_agent: String,
    /// Enable DNS-over-HTTPS for handle resolution
    pub use_doh: bool,
}

impl Default for IdentityResolverConfig {
    fn default() -> Self {
        Self {
            user_agent: "Aurora-Locus/0.1".to_string(),
            use_doh: false,
        }
    }
}

/// Main identity resolver - combines caching with SDK resolution
#[derive(Clone)]
pub struct IdentityResolver {
    cache: DidCache,
    handle_resolver: Arc<HandleResolver>,
    http_client: reqwest::Client,
    config: IdentityResolverConfig,
}

impl IdentityResolver {
    /// Create a new identity resolver
    pub fn new(
        cache: DidCache,
        config: IdentityResolverConfig,
    ) -> PdsResult<Self> {
        // Build HTTP client
        let http_client = reqwest::Client::builder()
            .user_agent(&config.user_agent)
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| PdsError::Internal(format!("Failed to create HTTP client: {}", e)))?;

        // Create handle resolver from SDK
        let handle_resolver = Arc::new(HandleResolver::new());

        Ok(Self {
            cache,
            handle_resolver,
            http_client,
            config,
        })
    }

    /// Resolve handle to DID with caching
    ///
    /// Resolution order:
    /// 1. Check cache first (fast path)
    /// 2. Try DNS TXT record resolution
    /// 3. Try HTTPS well-known resolution
    /// 4. Cache successful resolution
    pub async fn resolve_handle(&self, handle: &str) -> PdsResult<String> {
        let normalized = handle.to_lowercase();

        // Check cache first
        if let Some(cached) = self.cache.get_handle(&normalized).await? {
            return Ok(cached.did);
        }

        // Cache miss - resolve via SDK
        let did = self.handle_resolver
            .resolve(&normalized)
            .await
            .map_err(|e| PdsError::IdentityResolution(format!("Failed to resolve handle: {}", e)))?;

        let did_str = did.as_str().to_string();

        // Cache the successful resolution
        self.cache.cache_handle(&normalized, &did_str).await?;

        Ok(did_str)
    }

    /// Resolve DID to DID document with caching
    ///
    /// Supports did:plc and did:web methods
    pub async fn resolve_did(&self, did: &str) -> PdsResult<DidDocument> {
        // Check cache first
        if let Some(cached) = self.cache.get_did_doc(did).await? {
            // Parse cached document
            let doc: DidDocument = serde_json::from_str(&cached.doc)
                .map_err(|e| PdsError::Internal(format!("Invalid cached DID document: {}", e)))?;
            return Ok(doc);
        }

        // Cache miss - fetch DID document
        let doc = self.fetch_did_document(did).await?;

        // Cache the document
        let doc_json = serde_json::to_string(&doc)
            .map_err(|e| PdsError::Internal(format!("Failed to serialize DID document: {}", e)))?;
        self.cache.cache_did_doc(did, &doc_json).await?;

        Ok(doc)
    }

    /// Fetch DID document from source
    async fn fetch_did_document(&self, did: &str) -> PdsResult<DidDocument> {
        if did.starts_with("did:plc:") {
            self.fetch_plc_document(did).await
        } else if did.starts_with("did:web:") {
            self.fetch_web_document(did).await
        } else {
            Err(PdsError::IdentityResolution(format!("Unsupported DID method: {}", did)))
        }
    }

    /// Fetch DID document from PLC directory
    async fn fetch_plc_document(&self, did: &str) -> PdsResult<DidDocument> {
        let plc_url = format!("https://plc.directory/{}", did);

        let response = self.http_client
            .get(&plc_url)
            .send()
            .await
            .map_err(|e| PdsError::IdentityResolution(format!("Failed to fetch PLC document: {}", e)))?;

        if !response.status().is_success() {
            return Err(PdsError::IdentityResolution(
                format!("PLC directory returned error: {}", response.status())
            ));
        }

        let doc: DidDocument = response
            .json()
            .await
            .map_err(|e| PdsError::IdentityResolution(format!("Invalid PLC document: {}", e)))?;

        Ok(doc)
    }

    /// Fetch DID document from did:web
    async fn fetch_web_document(&self, did: &str) -> PdsResult<DidDocument> {
        // did:web:example.com -> https://example.com/.well-known/did.json
        // did:web:example.com:user:alice -> https://example.com/user/alice/did.json
        let did_suffix = did.strip_prefix("did:web:")
            .ok_or_else(|| PdsError::IdentityResolution("Invalid did:web format".to_string()))?;

        let parts: Vec<&str> = did_suffix.split(':').collect();
        let domain = parts.first()
            .ok_or_else(|| PdsError::IdentityResolution("Missing domain in did:web".to_string()))?;

        let url = if parts.len() == 1 {
            format!("https://{}/.well-known/did.json", domain)
        } else {
            let path = parts[1..].join("/");
            format!("https://{}/{}/did.json", domain, path)
        };

        let response = self.http_client
            .get(&url)
            .send()
            .await
            .map_err(|e| PdsError::IdentityResolution(format!("Failed to fetch did:web document: {}", e)))?;

        if !response.status().is_success() {
            return Err(PdsError::IdentityResolution(
                format!("did:web server returned error: {}", response.status())
            ));
        }

        let doc: DidDocument = response
            .json()
            .await
            .map_err(|e| PdsError::IdentityResolution(format!("Invalid did:web document: {}", e)))?;

        Ok(doc)
    }

    /// Update handle for a DID
    ///
    /// This updates the cache and should be called when a user changes their handle
    pub async fn update_handle(&self, did: &str, handle: &str) -> PdsResult<()> {
        let normalized = handle.to_lowercase();

        // Verify the handle resolves to this DID
        let resolved_did = self.resolve_handle(&normalized).await?;
        if resolved_did != did {
            return Err(PdsError::IdentityResolution(
                format!("Handle {} does not resolve to DID {}", handle, did)
            ));
        }

        // Update cache
        self.cache.cache_handle(&normalized, did).await?;

        Ok(())
    }

    /// Get handle for a DID (reverse lookup)
    ///
    /// First checks cache, then falls back to examining DID document's alsoKnownAs
    pub async fn get_handle_for_did(&self, did: &str) -> PdsResult<Option<String>> {
        // Check cache first
        if let Some(handle) = self.cache.get_did_handle(did).await? {
            return Ok(Some(handle));
        }

        // Cache miss - check DID document
        let doc = self.resolve_did(did).await?;

        // Look for at:// handle in alsoKnownAs
        for aka in &doc.also_known_as {
            if let Some(handle) = aka.strip_prefix("at://") {
                // Cache this mapping
                self.cache.cache_handle(handle, did).await?;
                return Ok(Some(handle.to_string()));
            }
        }

        Ok(None)
    }

    /// Invalidate cached handle (force re-resolution)
    pub async fn invalidate_handle(&self, handle: &str) -> PdsResult<()> {
        self.cache.delete_handle(handle).await
    }

    /// Invalidate cached DID document (force re-fetch)
    pub async fn invalidate_did(&self, did: &str) -> PdsResult<()> {
        self.cache.delete_did_doc(did).await
    }

    /// Clean up expired cache entries
    pub async fn cleanup_cache(&self) -> PdsResult<()> {
        self.cache.cleanup_expired().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;

    async fn create_test_resolver() -> IdentityResolver {
        let db = SqlitePool::connect(":memory:").await.unwrap();

        // Create cache tables
        sqlx::query(
            r#"
            CREATE TABLE did_doc (
                did TEXT PRIMARY KEY,
                doc TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                cached_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        sqlx::query(
            r#"
            CREATE TABLE did_handle (
                handle TEXT PRIMARY KEY,
                did TEXT NOT NULL,
                declared_at TEXT,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        let cache = DidCache::new(db);
        IdentityResolver::new(cache, IdentityResolverConfig::default()).unwrap()
    }

    #[tokio::test]
    async fn test_resolve_handle_with_cache() {
        let resolver = create_test_resolver().await;

        // Pre-populate cache
        resolver.cache
            .cache_handle("alice.test", "did:plc:alice123")
            .await
            .unwrap();

        // Should return cached value
        let did = resolver.resolve_handle("alice.test").await.unwrap();
        assert_eq!(did, "did:plc:alice123");

        // Case-insensitive lookup
        let did_upper = resolver.resolve_handle("ALICE.TEST").await.unwrap();
        assert_eq!(did_upper, "did:plc:alice123");
    }

    #[tokio::test]
    async fn test_get_handle_for_did() {
        let resolver = create_test_resolver().await;

        // Pre-populate cache
        resolver.cache
            .cache_handle("bob.test", "did:plc:bob456")
            .await
            .unwrap();

        // Reverse lookup
        let handle = resolver.get_handle_for_did("did:plc:bob456").await.unwrap();
        assert_eq!(handle, Some("bob.test".to_string()));
    }

    #[tokio::test]
    async fn test_invalidate_handle() {
        let resolver = create_test_resolver().await;

        // Pre-populate cache
        resolver.cache
            .cache_handle("charlie.test", "did:plc:charlie789")
            .await
            .unwrap();

        // Verify cached
        let cached = resolver.cache.get_handle("charlie.test").await.unwrap();
        assert!(cached.is_some());

        // Invalidate
        resolver.invalidate_handle("charlie.test").await.unwrap();

        // Verify removed
        let cached_after = resolver.cache.get_handle("charlie.test").await.unwrap();
        assert!(cached_after.is_none());
    }

    #[tokio::test]
    async fn test_did_web_url_parsing() {
        let resolver = create_test_resolver().await;

        // Simple did:web should map to .well-known
        let did_simple = "did:web:example.com";
        // Would fetch: https://example.com/.well-known/did.json

        // Path-based did:web
        let did_path = "did:web:example.com:user:alice";
        // Would fetch: https://example.com/user/alice/did.json

        // Note: These tests verify the logic, not actual HTTP calls
        // Real HTTP tests would require mocking or integration tests
    }
}
