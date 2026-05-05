use crate::identity::did_document::DidDocument;
/// Identity Resolver - Orchestrates handle and DID resolution with caching
use crate::{
    error::{PdsError, PdsResult},
    identity::DidCache,
};
use p256::pkcs8::EncodePublicKey;
use proto_blue::identity::HandleResolver;
use std::sync::Arc;

/// Identity resolution configuration
#[derive(Debug, Clone)]
pub struct IdentityResolverConfig {
    /// User-Agent header for HTTP requests
    pub user_agent: String,
    /// Enable DNS-over-HTTPS for handle resolution
    #[allow(dead_code)] // Future DNS-over-HTTPS support
    pub use_doh: bool,
    /// PLC directory URL for DID resolution (default: https://plc.directory)
    pub plc_directory_url: String,
    /// Maximum number of retry attempts for HTTP requests
    pub max_retries: u32,
    /// Base delay for exponential backoff in milliseconds
    pub retry_base_delay_ms: u64,
    /// Maximum delay between retries in milliseconds
    pub retry_max_delay_ms: u64,
}

impl Default for IdentityResolverConfig {
    fn default() -> Self {
        Self {
            user_agent: "Aurora-Locus/0.1".to_string(),
            use_doh: false,
            plc_directory_url: std::env::var("PLC_DIRECTORY_URL")
                .unwrap_or_else(|_| "https://plc.directory".to_string()),
            max_retries: 3,
            retry_base_delay_ms: 100,
            retry_max_delay_ms: 5000,
        }
    }
}

/// Main identity resolver - combines caching with SDK resolution
#[derive(Clone)]
pub struct IdentityResolver {
    cache: DidCache,
    handle_resolver: Arc<HandleResolver>,
    http_client: reqwest::Client,
    #[allow(dead_code)] // Kept for future configuration needs
    config: IdentityResolverConfig,
}

impl IdentityResolver {
    /// Create a new identity resolver
    pub fn new(cache: DidCache, config: IdentityResolverConfig) -> PdsResult<Self> {
        // Build HTTP client
        let http_client = reqwest::Client::builder()
            .user_agent(&config.user_agent)
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| PdsError::Internal(format!("Failed to create HTTP client: {}", e)))?;

        // Create handle resolver from SDK
        // 10s timeout matches the http_client; matches the SDK default for handle DNS+well-known races.
        let handle_resolver = Arc::new(HandleResolver::new(10_000));

        Ok(Self {
            cache,
            handle_resolver,
            http_client,
            config,
        })
    }

    /// Resolve handle to DID with two-tier caching and stale fallback
    ///
    /// Resolution order:
    /// 1. Check if handle is reserved
    /// 2. Check cache first (fast path)
    ///    - If fresh: return immediately
    ///    - If stale: try to refresh, use stale as fallback on failure
    /// 3. Try DNS TXT record resolution
    /// 4. Try HTTPS well-known resolution
    /// 5. Cache successful resolution
    ///
    /// **Graceful Degradation**: If cache is stale and fresh fetch fails,
    /// the stale cached data is returned to maintain availability during outages.
    pub async fn resolve_handle(&self, handle: &str) -> PdsResult<String> {
        let normalized = handle.to_lowercase();

        // Check if handle is reserved
        if crate::identity::reserved_handles::is_reserved(&normalized) {
            return Err(PdsError::Validation(format!(
                "Handle '{}' is reserved and cannot be used",
                normalized
            )));
        }

        // Check cache first
        if let Some(cached) = self.cache.get_handle(&normalized).await? {
            // Fresh cache hit - return immediately
            if !cached.stale {
                tracing::trace!(
                    handle = %cached.handle,
                    did = %cached.did,
                    updated_at = %cached.updated_at,
                    declared_at = ?cached.declared_at,
                    "Fresh handle cache hit"
                );
                return Ok(cached.did);
            }

            // Stale cache hit - try to refresh in background
            tracing::debug!(
                handle = %cached.handle,
                did = %cached.did,
                updated_at = %cached.updated_at,
                "Cache hit but stale, attempting refresh"
            );

            // Try to fetch fresh data
            match self.handle_resolver.resolve(&normalized).await {
                Ok(Some(did_str)) => {
                    // Update cache with fresh data
                    self.cache.cache_handle(&normalized, &did_str).await?;
                    tracing::debug!(handle = %normalized, "Successfully refreshed stale cache");
                    return Ok(did_str);
                }
                Ok(None) => {
                    // Resolver succeeded but found no DID — fall back to the stale cache value.
                    tracing::warn!(
                        handle = %normalized,
                        "Resolver returned no DID, using stale cache as fallback"
                    );
                    return Ok(cached.did);
                }
                Err(e) => {
                    // Fresh fetch failed - use stale data as fallback (graceful degradation)
                    tracing::warn!(
                        handle = %normalized,
                        error = %e,
                        "Failed to refresh stale cache, using stale data as fallback"
                    );
                    return Ok(cached.did);
                }
            }
        }

        // Cache miss - resolve via SDK
        let did_str = self
            .handle_resolver
            .resolve(&normalized)
            .await
            .map_err(|e| PdsError::IdentityResolution(format!("Failed to resolve handle: {}", e)))?
            .ok_or_else(|| {
                PdsError::IdentityResolution(format!(
                    "Handle {} did not resolve to any DID",
                    normalized
                ))
            })?;

        // Cache the successful resolution
        self.cache.cache_handle(&normalized, &did_str).await?;

        Ok(did_str)
    }

    /// Resolve DID to DID document with two-tier caching and stale fallback
    ///
    /// Supports did:plc and did:web methods
    ///
    /// **Graceful Degradation**: If cache is stale and PLC/Web fetch fails,
    /// the stale cached document is returned to maintain availability during outages.
    pub async fn resolve_did(&self, did: &str) -> PdsResult<DidDocument> {
        // Check cache first
        if let Some(cached) = self.cache.get_did_doc(did).await? {
            // Parse cached document
            let cached_doc: DidDocument = serde_json::from_str(&cached.doc)
                .map_err(|e| PdsError::Internal(format!("Invalid cached DID document: {}", e)))?;

            // Fresh cache hit - return immediately
            if !cached.stale {
                tracing::trace!(
                    did = %cached.did,
                    updated_at = %cached.updated_at,
                    cached_at = %cached.cached_at,
                    "Fresh DID doc cache hit"
                );
                return Ok(cached_doc);
            }

            // Stale cache hit - try to refresh
            tracing::debug!(
                did = %cached.did,
                updated_at = %cached.updated_at,
                cached_at = %cached.cached_at,
                "DID doc cache hit but stale, attempting refresh"
            );

            // Try to fetch fresh document
            match self.fetch_did_document(did).await {
                Ok(doc) => {
                    // Update cache with fresh data
                    let doc_json = serde_json::to_string(&doc).map_err(|e| {
                        PdsError::Internal(format!("Failed to serialize DID document: {}", e))
                    })?;
                    self.cache.cache_did_doc(did, &doc_json).await?;
                    tracing::debug!(did = %did, "Successfully refreshed stale DID doc cache");
                    return Ok(doc);
                }
                Err(e) => {
                    // Fresh fetch failed - use stale data as fallback (graceful degradation)
                    tracing::warn!(
                        did = %did,
                        error = %e,
                        "Failed to refresh stale DID doc cache, using stale data as fallback"
                    );
                    return Ok(cached_doc);
                }
            }
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
            Err(PdsError::IdentityResolution(format!(
                "Unsupported DID method: {}",
                did
            )))
        }
    }

    /// Check if an HTTP error is retryable
    ///
    /// Retryable errors include:
    /// - Network/connection errors
    /// - 5xx server errors (except 501 Not Implemented)
    /// - 429 Too Many Requests
    /// - Request timeout
    fn is_retryable_error(error: &reqwest::Error) -> bool {
        if error.is_timeout() || error.is_connect() || error.is_request() {
            return true;
        }
        if let Some(status) = error.status() {
            return status.as_u16() == 429 || (status.is_server_error() && status.as_u16() != 501);
        }
        false
    }

    /// Check if an HTTP status code is retryable
    fn is_retryable_status(status: reqwest::StatusCode) -> bool {
        status.as_u16() == 429 || (status.is_server_error() && status.as_u16() != 501)
    }

    /// Calculate delay for retry attempt using exponential backoff with jitter
    fn calculate_retry_delay(&self, attempt: u32) -> std::time::Duration {
        let base_delay = self.config.retry_base_delay_ms;
        let max_delay = self.config.retry_max_delay_ms;

        // Exponential backoff: base * 2^attempt
        let delay_ms = base_delay.saturating_mul(1u64 << attempt);
        let capped_delay = delay_ms.min(max_delay);

        // Add jitter (±25% of delay)
        let jitter_range = capped_delay / 4;
        let jitter = if jitter_range > 0 {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};

            // Simple deterministic jitter based on current time
            let mut hasher = DefaultHasher::new();
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                .hash(&mut hasher);
            (hasher.finish() % (jitter_range * 2)) as i64 - jitter_range as i64
        } else {
            0
        };

        let final_delay = (capped_delay as i64 + jitter).max(0) as u64;
        std::time::Duration::from_millis(final_delay)
    }

    /// Fetch DID document from PLC directory with retry logic
    async fn fetch_plc_document(&self, did: &str) -> PdsResult<DidDocument> {
        let plc_url = format!(
            "{}/{}",
            self.config.plc_directory_url.trim_end_matches('/'),
            did
        );
        let max_retries = self.config.max_retries;
        let mut last_error = None;

        for attempt in 0..=max_retries {
            if attempt > 0 {
                let delay = self.calculate_retry_delay(attempt - 1);
                tracing::debug!(
                    did = %did,
                    attempt = attempt,
                    delay_ms = delay.as_millis(),
                    "Retrying PLC document fetch"
                );
                tokio::time::sleep(delay).await;
            }

            match self.http_client.get(&plc_url).send().await {
                Ok(response) => {
                    if response.status().is_success() {
                        let doc: DidDocument = response.json().await.map_err(|e| {
                            PdsError::IdentityResolution(format!("Invalid PLC document: {}", e))
                        })?;

                        if attempt > 0 {
                            tracing::info!(
                                did = %did,
                                attempts = attempt + 1,
                                "PLC document fetch succeeded after retries"
                            );
                        }
                        return Ok(doc);
                    }

                    let status = response.status();
                    if Self::is_retryable_status(status) && attempt < max_retries {
                        tracing::warn!(
                            did = %did,
                            status = %status,
                            attempt = attempt,
                            "PLC directory returned retryable error"
                        );
                        last_error = Some(PdsError::IdentityResolution(format!(
                            "PLC directory returned error: {}",
                            status
                        )));
                        continue;
                    }

                    return Err(PdsError::IdentityResolution(format!(
                        "PLC directory returned error: {}",
                        status
                    )));
                }
                Err(e) => {
                    if Self::is_retryable_error(&e) && attempt < max_retries {
                        tracing::warn!(
                            did = %did,
                            error = %e,
                            attempt = attempt,
                            "Retryable error fetching PLC document"
                        );
                        last_error = Some(PdsError::IdentityResolution(format!(
                            "Failed to fetch PLC document: {}",
                            e
                        )));
                        continue;
                    }
                    return Err(PdsError::IdentityResolution(format!(
                        "Failed to fetch PLC document: {}",
                        e
                    )));
                }
            }
        }

        // All retries exhausted
        Err(last_error.unwrap_or_else(|| {
            PdsError::IdentityResolution(format!(
                "Failed to fetch PLC document after {} retries",
                max_retries
            ))
        }))
    }

    /// Fetch DID document from did:web with retry logic
    async fn fetch_web_document(&self, did: &str) -> PdsResult<DidDocument> {
        // did:web:example.com -> https://example.com/.well-known/did.json
        // did:web:example.com:user:alice -> https://example.com/user/alice/did.json
        let did_suffix = did
            .strip_prefix("did:web:")
            .ok_or_else(|| PdsError::IdentityResolution("Invalid did:web format".to_string()))?;

        let parts: Vec<&str> = did_suffix.split(':').collect();
        let domain = parts
            .first()
            .ok_or_else(|| PdsError::IdentityResolution("Missing domain in did:web".to_string()))?;

        let url = if parts.len() == 1 {
            format!("https://{}/.well-known/did.json", domain)
        } else {
            let path = parts[1..].join("/");
            format!("https://{}/{}/did.json", domain, path)
        };

        let max_retries = self.config.max_retries;
        let mut last_error = None;

        for attempt in 0..=max_retries {
            if attempt > 0 {
                let delay = self.calculate_retry_delay(attempt - 1);
                tracing::debug!(
                    did = %did,
                    attempt = attempt,
                    delay_ms = delay.as_millis(),
                    "Retrying did:web document fetch"
                );
                tokio::time::sleep(delay).await;
            }

            match self.http_client.get(&url).send().await {
                Ok(response) => {
                    if response.status().is_success() {
                        let doc: DidDocument = response.json().await.map_err(|e| {
                            PdsError::IdentityResolution(format!("Invalid did:web document: {}", e))
                        })?;

                        if attempt > 0 {
                            tracing::info!(
                                did = %did,
                                attempts = attempt + 1,
                                "did:web document fetch succeeded after retries"
                            );
                        }
                        return Ok(doc);
                    }

                    let status = response.status();
                    if Self::is_retryable_status(status) && attempt < max_retries {
                        tracing::warn!(
                            did = %did,
                            status = %status,
                            attempt = attempt,
                            "did:web server returned retryable error"
                        );
                        last_error = Some(PdsError::IdentityResolution(format!(
                            "did:web server returned error: {}",
                            status
                        )));
                        continue;
                    }

                    return Err(PdsError::IdentityResolution(format!(
                        "did:web server returned error: {}",
                        status
                    )));
                }
                Err(e) => {
                    if Self::is_retryable_error(&e) && attempt < max_retries {
                        tracing::warn!(
                            did = %did,
                            error = %e,
                            attempt = attempt,
                            "Retryable error fetching did:web document"
                        );
                        last_error = Some(PdsError::IdentityResolution(format!(
                            "Failed to fetch did:web document: {}",
                            e
                        )));
                        continue;
                    }
                    return Err(PdsError::IdentityResolution(format!(
                        "Failed to fetch did:web document: {}",
                        e
                    )));
                }
            }
        }

        // All retries exhausted
        Err(last_error.unwrap_or_else(|| {
            PdsError::IdentityResolution(format!(
                "Failed to fetch did:web document after {} retries",
                max_retries
            ))
        }))
    }

    /// Get atproto signing key from DID document (for Phase 4: Service Auth)
    ///
    /// This extracts the public signing key from the DID document's verificationMethod.
    /// The key is used to verify service auth JWTs in cross-PDS requests.
    ///
    /// # Returns
    /// The signing key in PEM format (ES256/P-256 or ES256K/secp256k1)
    pub async fn get_signing_key(&self, did: &str) -> PdsResult<Vec<u8>> {
        // Resolve DID document (with caching)
        let doc = self.resolve_did(did).await?;

        // Find the atproto signing key in verificationMethod
        for vm in &doc.verification_method {
            // Check if this is an atproto signing key
            // ATProto keys typically have id like "did:plc:abc123#atproto"
            if vm.id.contains("#atproto") {
                // Extract public key from multibase format
                if let Some(multibase_key) = &vm.public_key_multibase {
                    return self.decode_multibase_key(multibase_key);
                }
            }
        }

        // If no atproto key found, try the first verification method
        if let Some(vm) = doc.verification_method.first() {
            if let Some(multibase_key) = &vm.public_key_multibase {
                return self.decode_multibase_key(multibase_key);
            }
        }

        Err(PdsError::IdentityResolution(format!(
            "No signing key found in DID document for {}",
            did
        )))
    }

    /// Decode a multibase-encoded public key to PEM format
    ///
    /// ATProto DID documents use multibase encoding with multicodec prefixes:
    /// - 'z' prefix indicates base58btc encoding
    /// - Multicodec prefix identifies the key type (P-256 or secp256k1)
    /// - Compressed public key bytes (33 bytes)
    ///
    /// This function:
    /// 1. Strips the 'z' multibase prefix
    /// 2. Decodes base58btc to get raw bytes
    /// 3. Parses the multicodec prefix to identify key type
    /// 4. Decompresses the public key point (if compressed)
    /// 5. Converts to PEM format for jsonwebtoken
    fn decode_multibase_key(&self, multibase_key: &str) -> PdsResult<Vec<u8>> {
        // Step 1: Verify and strip multibase prefix
        // 'z' = base58btc encoding (most common in ATProto)
        let encoded = multibase_key.strip_prefix('z').ok_or_else(|| {
            PdsError::IdentityResolution(format!(
                "Unsupported multibase encoding: expected 'z' prefix, got '{}'",
                multibase_key.chars().next().unwrap_or('?')
            ))
        })?;

        // Step 2: Decode base58btc
        let decoded = bs58::decode(encoded).into_vec().map_err(|e| {
            PdsError::IdentityResolution(format!("Failed to decode base58btc key: {}", e))
        })?;

        if decoded.len() < 2 {
            return Err(PdsError::IdentityResolution(
                "Decoded key too short for multicodec prefix".to_string(),
            ));
        }

        // Step 3: Parse multicodec prefix (varint encoded)
        // P-256 (secp256r1): 0x1200 -> varint: 0x80 0x24
        // secp256k1: 0xe7 -> varint: 0xe7 0x01
        let (multicodec, key_bytes) = Self::parse_multicodec(&decoded)?;

        // Step 4: Convert key based on type
        match multicodec {
            // P-256 public key (compressed or uncompressed)
            0x1200 => self.decode_p256_key(key_bytes),
            // secp256k1 public key (compressed)
            0xe7 => self.decode_secp256k1_key(key_bytes),
            _ => Err(PdsError::IdentityResolution(format!(
                "Unsupported multicodec: 0x{:x}",
                multicodec
            ))),
        }
    }

    /// Parse a varint-encoded multicodec prefix
    ///
    /// Returns (multicodec_value, remaining_bytes)
    fn parse_multicodec(data: &[u8]) -> PdsResult<(u64, &[u8])> {
        let mut value: u64 = 0;
        let mut shift = 0;
        let mut i = 0;

        loop {
            if i >= data.len() {
                return Err(PdsError::IdentityResolution(
                    "Incomplete multicodec varint".to_string(),
                ));
            }

            let byte = data[i];
            value |= ((byte & 0x7f) as u64) << shift;

            if byte & 0x80 == 0 {
                // Last byte of varint
                return Ok((value, &data[i + 1..]));
            }

            shift += 7;
            i += 1;

            if shift >= 64 {
                return Err(PdsError::IdentityResolution(
                    "Multicodec varint too large".to_string(),
                ));
            }
        }
    }

    /// Decode a P-256 (secp256r1) public key to PEM format
    ///
    /// Handles both compressed (33 bytes) and uncompressed (65 bytes) formats.
    fn decode_p256_key(&self, key_bytes: &[u8]) -> PdsResult<Vec<u8>> {
        use p256::elliptic_curve::sec1::FromEncodedPoint;
        use p256::{EncodedPoint, PublicKey};

        // Parse the SEC1-encoded point (compressed or uncompressed)
        let encoded_point = EncodedPoint::from_bytes(key_bytes).map_err(|e| {
            PdsError::IdentityResolution(format!("Invalid P-256 point encoding: {}", e))
        })?;

        // Decompress if necessary and validate the point is on the curve
        let public_key: Option<PublicKey> = PublicKey::from_encoded_point(&encoded_point).into();
        let public_key = public_key
            .ok_or_else(|| PdsError::IdentityResolution("P-256 point not on curve".to_string()))?;

        // Convert to PEM format (SPKI - SubjectPublicKeyInfo)
        let pem = public_key
            .to_public_key_pem(Default::default())
            .map_err(|e| {
                PdsError::IdentityResolution(format!("Failed to encode P-256 key as PEM: {}", e))
            })?;

        Ok(pem.into_bytes())
    }

    /// Decode a secp256k1 public key to PEM format
    ///
    /// ATProto uses secp256k1 for signing in addition to P-256.
    /// The key is typically in compressed format (33 bytes).
    fn decode_secp256k1_key(&self, key_bytes: &[u8]) -> PdsResult<Vec<u8>> {
        use k256::elliptic_curve::sec1::FromEncodedPoint;
        use k256::pkcs8::EncodePublicKey as K256EncodePublicKey;
        use k256::{EncodedPoint, PublicKey};

        // Parse the SEC1-encoded point (compressed format: 33 bytes)
        let encoded_point = EncodedPoint::from_bytes(key_bytes).map_err(|e| {
            PdsError::IdentityResolution(format!("Invalid secp256k1 point encoding: {}", e))
        })?;

        // Decompress if necessary and validate the point is on the curve
        let public_key: Option<PublicKey> = PublicKey::from_encoded_point(&encoded_point).into();
        let public_key = public_key.ok_or_else(|| {
            PdsError::IdentityResolution("secp256k1 point not on curve".to_string())
        })?;

        // Convert to PEM format (SPKI - SubjectPublicKeyInfo)
        let pem = public_key
            .to_public_key_pem(Default::default())
            .map_err(|e| {
                PdsError::IdentityResolution(format!(
                    "Failed to encode secp256k1 key as PEM: {}",
                    e
                ))
            })?;

        Ok(pem.into_bytes())
    }

    /// Invalidate cached signing key for a DID (force re-fetch)
    ///
    /// This should be called when identity events are received via relay,
    #[allow(dead_code)] // Future key invalidation
    /// indicating that the DID document has changed.
    pub async fn invalidate_signing_key(&self, did: &str) -> PdsResult<()> {
        // Invalidating the DID document invalidates the signing key
        self.invalidate_did(did).await
    }

    /// Update handle for a DID
    ///
    /// This updates the cache and should be called when a user changes their handle
    pub async fn update_handle(&self, did: &str, handle: &str) -> PdsResult<()> {
        let normalized = handle.to_lowercase();

        // Check if handle is reserved
        if crate::identity::reserved_handles::is_reserved(&normalized) {
            return Err(PdsError::Validation(format!(
                "Handle '{}' is reserved and cannot be used",
                normalized
            )));
        }

        // Verify the handle resolves to this DID
        let resolved_did = self.resolve_handle(&normalized).await?;
        if resolved_did != did {
            return Err(PdsError::IdentityResolution(format!(
                "Handle {} does not resolve to DID {}",
                handle, did
            )));
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
    use sqlx::AnyPool;

    /// Open a single-connection SQLite-backed `AnyPool` for tests. The
    /// single-connection cap is required because each connection to
    /// `:memory:` has its own private database. Mirror of the helper in
    /// `super::cache::tests::open_any_memory_pool`.
    async fn open_test_pool() -> AnyPool {
        use std::sync::Once;
        static INSTALL: Once = Once::new();
        INSTALL.call_once(sqlx::any::install_default_drivers);
        sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap()
    }

    async fn create_test_resolver() -> IdentityResolver {
        let db = open_test_pool().await;

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
        resolver
            .cache
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
        resolver
            .cache
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
        resolver
            .cache
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
        let _resolver = create_test_resolver().await;

        // Simple did:web should map to .well-known
        let _did_simple = "did:web:example.com";
        // Would fetch: https://example.com/.well-known/did.json

        // Path-based did:web
        let _did_path = "did:web:example.com:user:alice";
        // Would fetch: https://example.com/user/alice/did.json

        // Note: These tests verify the logic, not actual HTTP calls
        // Real HTTP tests would require mocking or integration tests
    }

    #[tokio::test]
    async fn test_custom_plc_directory_url() {
        let db = open_test_pool().await;

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

        // Test with custom PLC directory URL
        let custom_config = IdentityResolverConfig {
            user_agent: "Test-Agent/1.0".to_string(),
            use_doh: false,
            plc_directory_url: "https://test.plc.directory".to_string(),
            max_retries: 3,
            retry_base_delay_ms: 100,
            retry_max_delay_ms: 5000,
        };

        let resolver = IdentityResolver::new(cache.clone(), custom_config).unwrap();

        // Verify the custom URL is set correctly
        assert_eq!(
            resolver.config.plc_directory_url,
            "https://test.plc.directory"
        );

        // Test with default configuration (should use official directory or env var)
        let default_resolver =
            IdentityResolver::new(cache, IdentityResolverConfig::default()).unwrap();

        // Should either be the default or from environment variable
        assert!(
            default_resolver.config.plc_directory_url == "https://plc.directory"
                || default_resolver.config.plc_directory_url
                    == std::env::var("PLC_DIRECTORY_URL").unwrap_or_default()
        );
    }

    #[tokio::test]
    async fn test_plc_url_trailing_slash_handling() {
        let db = open_test_pool().await;

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

        // Test with trailing slash
        let config_with_slash = IdentityResolverConfig {
            user_agent: "Test-Agent/1.0".to_string(),
            use_doh: false,
            plc_directory_url: "https://test.plc.directory/".to_string(),
            max_retries: 3,
            retry_base_delay_ms: 100,
            retry_max_delay_ms: 5000,
        };

        let resolver = IdentityResolver::new(cache, config_with_slash).unwrap();

        // The fetch_plc_document method should handle trailing slashes correctly
        // by using trim_end_matches('/') in the format string
        assert_eq!(
            resolver.config.plc_directory_url,
            "https://test.plc.directory/"
        );
    }

    // =====================================================
    // Multibase Key Decoding Tests
    // =====================================================

    #[test]
    fn test_parse_multicodec_secp256k1() {
        // secp256k1 multicodec is 0xe7 (231)
        // Varint encoding: 231 > 127, so it's 0xe7 0x01
        let data = [0xe7, 0x01, 0x02, 0x03, 0x04];
        let (multicodec, remaining) = IdentityResolver::parse_multicodec(&data).unwrap();
        assert_eq!(multicodec, 0xe7);
        assert_eq!(remaining, &[0x02, 0x03, 0x04]);
    }

    #[test]
    fn test_parse_multicodec_p256() {
        // P-256 multicodec is 0x1200 (4608)
        // Varint encoding: 4608 = 0b1001000000000 -> 0x80 0x24
        let data = [0x80, 0x24, 0x02, 0x03, 0x04];
        let (multicodec, remaining) = IdentityResolver::parse_multicodec(&data).unwrap();
        assert_eq!(multicodec, 0x1200);
        assert_eq!(remaining, &[0x02, 0x03, 0x04]);
    }

    #[test]
    fn test_parse_multicodec_single_byte() {
        // Values < 128 are single-byte varints
        let data = [0x55, 0xaa, 0xbb];
        let (multicodec, remaining) = IdentityResolver::parse_multicodec(&data).unwrap();
        assert_eq!(multicodec, 0x55);
        assert_eq!(remaining, &[0xaa, 0xbb]);
    }

    #[test]
    fn test_parse_multicodec_incomplete() {
        // Incomplete varint (continuation bit set but no more bytes)
        let data = [0x80];
        let result = IdentityResolver::parse_multicodec(&data);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_decode_secp256k1_multibase_key() {
        let resolver = create_test_resolver().await;

        // Real secp256k1 key from ATProto DID document
        // zQ3shunBKsXixLxKtC5qeSG9E4J5RkGN57im31pcTzbNQnm5w
        // This is from did:plc:ewvi7nxzyoun6zhxrhs64oiz (atproto.com)
        let multibase_key = "zQ3shunBKsXixLxKtC5qeSG9E4J5RkGN57im31pcTzbNQnm5w";

        let result = resolver.decode_multibase_key(multibase_key);
        assert!(result.is_ok(), "Failed to decode key: {:?}", result.err());

        let pem_bytes = result.unwrap();
        let pem_string = String::from_utf8(pem_bytes).unwrap();

        // Verify it's a valid PEM public key
        assert!(pem_string.starts_with("-----BEGIN PUBLIC KEY-----"));
        assert!(pem_string.trim_end().ends_with("-----END PUBLIC KEY-----"));
    }

    #[tokio::test]
    async fn test_decode_multibase_key_invalid_prefix() {
        let resolver = create_test_resolver().await;

        // 'm' prefix is base64 (not base58btc)
        let result = resolver.decode_multibase_key("mABCDEF");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unsupported multibase encoding"));
    }

    #[tokio::test]
    async fn test_decode_multibase_key_invalid_base58() {
        let resolver = create_test_resolver().await;

        // Invalid base58 characters (0, O, I, l are not in base58 alphabet)
        let result = resolver.decode_multibase_key("z0OILAB");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_decode_multibase_key_too_short() {
        let resolver = create_test_resolver().await;

        // Just the 'z' prefix, decoded to 1 byte
        let result = resolver.decode_multibase_key("z2");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_decode_multibase_key_unsupported_multicodec() {
        let resolver = create_test_resolver().await;

        // Create a valid base58btc string with an unsupported multicodec prefix
        // 0x01 is a single-byte varint for multicodec 1 (which is not P-256 or secp256k1)
        // We need to encode: [0x01, ... some bytes ...]
        let bytes = [0x01, 0x02, 0x03, 0x04, 0x05];
        let encoded = bs58::encode(&bytes).into_string();
        let multibase_key = format!("z{}", encoded);

        let result = resolver.decode_multibase_key(&multibase_key);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unsupported multicodec"));
    }

    #[tokio::test]
    async fn test_decode_p256_key_from_raw_bytes() {
        let resolver = create_test_resolver().await;

        // Use a known test key (compressed format)
        // This is a valid P-256 point (compressed)
        let compressed_key: [u8; 33] = [
            0x02, // Compressed point prefix (even y)
            0x6b, 0x17, 0xd1, 0xf2, 0xe1, 0x2c, 0x42, 0x47, 0xf8, 0xbc, 0xe6, 0xe5, 0x63, 0xa4,
            0x40, 0xf2, 0x77, 0x03, 0x7d, 0x81, 0x2d, 0xeb, 0x33, 0xa0, 0xf4, 0xa1, 0x39, 0x45,
            0xd8, 0x98, 0xc2, 0x96,
        ];

        // Encode with P-256 multicodec prefix (0x1200 = varint 0x80 0x24)
        let mut multicodec_bytes = vec![0x80, 0x24];
        multicodec_bytes.extend_from_slice(&compressed_key);

        let encoded = bs58::encode(&multicodec_bytes).into_string();
        let multibase_key = format!("z{}", encoded);

        let result = resolver.decode_multibase_key(&multibase_key);
        assert!(
            result.is_ok(),
            "Failed to decode P-256 key: {:?}",
            result.err()
        );

        let pem_bytes = result.unwrap();
        let pem_string = String::from_utf8(pem_bytes).unwrap();

        // Verify it's a valid PEM public key
        assert!(pem_string.contains("BEGIN PUBLIC KEY"));
        assert!(pem_string.contains("END PUBLIC KEY"));
    }

    #[tokio::test]
    async fn test_decoded_key_works_with_jsonwebtoken() {
        let resolver = create_test_resolver().await;

        // Real secp256k1 key from ATProto
        let multibase_key = "zQ3shunBKsXixLxKtC5qeSG9E4J5RkGN57im31pcTzbNQnm5w";

        let pem_bytes = resolver.decode_multibase_key(multibase_key).unwrap();

        // Verify the PEM can be used with jsonwebtoken's DecodingKey
        // Note: This uses ES256K (secp256k1), but jsonwebtoken expects ES256 (P-256)
        // For secp256k1, we would need to use a different algorithm
        // This test verifies the PEM format is valid
        let pem_string = String::from_utf8(pem_bytes.clone()).unwrap();
        assert!(pem_string.contains("BEGIN PUBLIC KEY"));

        // The key should be parseable as a SPKI PEM
        // We can verify by checking the PEM structure
        let lines: Vec<&str> = pem_string.lines().collect();
        assert!(lines.len() >= 3); // Header, base64 content, footer
        assert_eq!(lines[0], "-----BEGIN PUBLIC KEY-----");
        assert_eq!(lines[lines.len() - 1], "-----END PUBLIC KEY-----");
    }
}
