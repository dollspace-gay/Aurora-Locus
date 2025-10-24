//! Handle resolution for ATProto
//!
//! This module provides functionality to resolve ATProto handles to DIDs.
//! Handles are human-readable identifiers (like alice.bsky.social) that map to DIDs.
//!
//! ## Resolution Methods
//!
//! ATProto supports two handle resolution methods:
//!
//! 1. **DNS TXT Record** (Preferred)
//!    - Query `_atproto.{handle}` for a TXT record
//!    - Record value should be `did={did}`
//!
//! 2. **HTTPS Well-Known**
//!    - Fetch `https://{handle}/.well-known/atproto-did`
//!    - Response should contain the DID as plain text
//!
//! The DNS method is preferred. If both methods return different DIDs, DNS takes precedence.
//!
//! ## Example
//!
//! ```no_run
//! use atproto::handle::HandleResolver;
//!
//! #[tokio::main]
//! async fn main() {
//!     let resolver = HandleResolver::new();
//!     let did = resolver.resolve("alice.bsky.social").await.unwrap();
//!     println!("DID: {}", did);
//! }
//! ```

use crate::types::Did;

/// Error types for handle resolution
#[derive(Debug, thiserror::Error)]
pub enum HandleError {
    #[error("Invalid handle: {0}")]
    InvalidHandle(String),

    #[error("Handle not found: {0}")]
    HandleNotFound(String),

    #[error("DNS resolution error: {0}")]
    DnsError(String),

    #[error("HTTP error: {0}")]
    HttpError(#[from] reqwest::Error),

    #[error("Invalid DID returned: {0}")]
    InvalidDid(String),

    #[error("No valid resolution method succeeded")]
    ResolutionFailed,
}

/// Handle resolver for ATProto
///
/// Resolves handles to DIDs using DNS TXT records and/or HTTPS well-known.
pub struct HandleResolver {
    /// HTTP client for HTTPS resolution
    client: reqwest::Client,

    /// Whether to attempt DNS resolution (default: true)
    enable_dns: bool,

    /// Whether to attempt HTTPS resolution (default: true)
    enable_https: bool,
}

impl HandleResolver {
    /// Create a new handle resolver with default settings
    ///
    /// # Examples
    ///
    /// ```
    /// use atproto::handle::HandleResolver;
    ///
    /// let resolver = HandleResolver::new();
    /// ```
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            enable_dns: true,
            enable_https: true,
        }
    }

    /// Create a resolver with only DNS resolution enabled
    pub fn dns_only() -> Self {
        Self {
            client: reqwest::Client::new(),
            enable_dns: true,
            enable_https: false,
        }
    }

    /// Create a resolver with only HTTPS resolution enabled
    pub fn https_only() -> Self {
        Self {
            client: reqwest::Client::new(),
            enable_dns: false,
            enable_https: true,
        }
    }

    /// Resolve a handle to a DID
    ///
    /// Tries DNS TXT record first (if enabled), then falls back to HTTPS well-known (if enabled).
    /// If both methods are enabled and return different results, DNS takes precedence.
    ///
    /// # Arguments
    ///
    /// * `handle` - The handle to resolve (e.g., "alice.bsky.social")
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The handle format is invalid
    /// - The handle cannot be resolved via any enabled method
    /// - The resolved value is not a valid DID
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use atproto::handle::HandleResolver;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let resolver = HandleResolver::new();
    /// let did = resolver.resolve("alice.bsky.social").await?;
    /// println!("Resolved to: {}", did);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn resolve(&self, handle: &str) -> Result<Did, HandleError> {
        // Validate and normalize the handle
        let normalized = normalize_handle(handle)?;

        // Try DNS first (preferred method)
        if self.enable_dns {
            if let Ok(did) = self.resolve_dns(&normalized).await {
                return Ok(did);
            }
        }

        // Fall back to HTTPS
        if self.enable_https {
            if let Ok(did) = self.resolve_https(&normalized).await {
                return Ok(did);
            }
        }

        Err(HandleError::HandleNotFound(handle.to_string()))
    }

    /// Resolve a handle via DNS TXT record
    ///
    /// Queries `_atproto.{handle}` for a TXT record with format `did={did}`.
    async fn resolve_dns(&self, _handle: &str) -> Result<Did, HandleError> {
        // For now, we'll use the trust-dns-resolver crate or similar
        // Since this requires additional dependencies, we'll implement a basic version
        // that could be enhanced with actual DNS resolution

        // DNS resolution requires platform-specific libraries
        // For a minimal implementation, we'll return an error
        // In a full implementation, we would use trust-dns-resolver or hickory-dns

        Err(HandleError::DnsError(
            "DNS resolution not yet implemented - use HTTPS resolution".to_string(),
        ))
    }

    /// Resolve a handle via HTTPS well-known
    ///
    /// Fetches `https://{handle}/.well-known/atproto-did` and expects the DID as plain text.
    async fn resolve_https(&self, handle: &str) -> Result<Did, HandleError> {
        let url = format!("https://{}/.well-known/atproto-did", handle);

        let response = self.client.get(&url).send().await?;

        if !response.status().is_success() {
            return Err(HandleError::HandleNotFound(handle.to_string()));
        }

        let did_str = response.text().await?.trim().to_string();

        // Validate it's a proper DID
        Did::new(&did_str).map_err(|_| HandleError::InvalidDid(did_str))
    }
}

impl Default for HandleResolver {
    fn default() -> Self {
        Self::new()
    }
}

/// Validate and normalize a handle
///
/// Handles must:
/// - Be valid domain names
/// - Be lowercase
/// - Not exceed 253 characters (DNS limit)
/// - Contain only allowed characters (a-z, 0-9, -, .)
///
/// # Arguments
///
/// * `handle` - The handle to validate and normalize
///
/// # Errors
///
/// Returns an error if the handle is invalid.
///
/// # Examples
///
/// ```
/// use atproto::handle::normalize_handle;
///
/// let normalized = normalize_handle("Alice.Bsky.Social").unwrap();
/// assert_eq!(normalized, "alice.bsky.social");
/// ```
pub fn normalize_handle(handle: &str) -> Result<String, HandleError> {
    // Convert to lowercase
    let normalized = handle.to_lowercase();

    // Check length (DNS limit is 253 characters)
    if normalized.is_empty() {
        return Err(HandleError::InvalidHandle(
            "Handle cannot be empty".to_string(),
        ));
    }

    if normalized.len() > 253 {
        return Err(HandleError::InvalidHandle(format!(
            "Handle too long: {} characters (max 253)",
            normalized.len()
        )));
    }

    // Validate characters and structure
    if !is_valid_handle(&normalized) {
        return Err(HandleError::InvalidHandle(format!(
            "Invalid handle format: {}",
            handle
        )));
    }

    Ok(normalized)
}

/// Check if a handle is valid according to ATProto rules
///
/// A valid handle:
/// - Contains only lowercase letters, digits, hyphens, and periods
/// - Does not start or end with a hyphen or period
/// - Does not contain consecutive periods
/// - Has at least one period (must be a domain)
/// - Each label (segment between periods) is valid
///
/// # Arguments
///
/// * `handle` - The handle to validate (should already be lowercase)
///
/// # Examples
///
/// ```
/// use atproto::handle::is_valid_handle;
///
/// assert!(is_valid_handle("alice.bsky.social"));
/// assert!(is_valid_handle("bob-123.example.com"));
/// assert!(!is_valid_handle("invalid..handle.com"));
/// assert!(!is_valid_handle("-invalid.com"));
/// ```
pub fn is_valid_handle(handle: &str) -> bool {
    // Must contain at least one period (domain requirement)
    if !handle.contains('.') {
        return false;
    }

    // Cannot start or end with hyphen or period
    if handle.starts_with('-')
        || handle.starts_with('.')
        || handle.ends_with('-')
        || handle.ends_with('.')
    {
        return false;
    }

    // Cannot contain consecutive periods
    if handle.contains("..") {
        return false;
    }

    // Split into labels and validate each
    let labels: Vec<&str> = handle.split('.').collect();

    for label in labels {
        if !is_valid_label(label) {
            return false;
        }
    }

    true
}

/// Check if a domain label is valid
///
/// A valid label:
/// - Is not empty
/// - Is not longer than 63 characters
/// - Contains only lowercase letters, digits, and hyphens
/// - Does not start or end with a hyphen
fn is_valid_label(label: &str) -> bool {
    // Labels must not be empty
    if label.is_empty() {
        return false;
    }

    // Labels must not exceed 63 characters (DNS limit)
    if label.len() > 63 {
        return false;
    }

    // Labels cannot start or end with hyphen
    if label.starts_with('-') || label.ends_with('-') {
        return false;
    }

    // Labels must contain only lowercase letters, digits, and hyphens
    label
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_handle_lowercase() {
        assert_eq!(
            normalize_handle("Alice.Bsky.Social").unwrap(),
            "alice.bsky.social"
        );
    }

    #[test]
    fn test_normalize_handle_already_lowercase() {
        assert_eq!(
            normalize_handle("alice.bsky.social").unwrap(),
            "alice.bsky.social"
        );
    }

    #[test]
    fn test_normalize_handle_empty() {
        assert!(normalize_handle("").is_err());
    }

    #[test]
    fn test_normalize_handle_too_long() {
        let long_handle = format!("{}.com", "a".repeat(250));
        assert!(normalize_handle(&long_handle).is_err());
    }

    #[test]
    fn test_is_valid_handle_simple() {
        assert!(is_valid_handle("alice.com"));
    }

    #[test]
    fn test_is_valid_handle_subdomain() {
        assert!(is_valid_handle("alice.bsky.social"));
    }

    #[test]
    fn test_is_valid_handle_with_hyphen() {
        assert!(is_valid_handle("alice-123.bsky.social"));
    }

    #[test]
    fn test_is_valid_handle_with_numbers() {
        assert!(is_valid_handle("user123.example.com"));
    }

    #[test]
    fn test_is_valid_handle_no_period() {
        assert!(!is_valid_handle("nodomain"));
    }

    #[test]
    fn test_is_valid_handle_consecutive_periods() {
        assert!(!is_valid_handle("alice..bsky.social"));
    }

    #[test]
    fn test_is_valid_handle_starts_with_period() {
        assert!(!is_valid_handle(".alice.com"));
    }

    #[test]
    fn test_is_valid_handle_ends_with_period() {
        assert!(!is_valid_handle("alice.com."));
    }

    #[test]
    fn test_is_valid_handle_starts_with_hyphen() {
        assert!(!is_valid_handle("-alice.com"));
    }

    #[test]
    fn test_is_valid_handle_ends_with_hyphen() {
        assert!(!is_valid_handle("alice.com-"));
    }

    #[test]
    fn test_is_valid_handle_uppercase() {
        // is_valid_handle expects lowercase input
        assert!(!is_valid_handle("Alice.Com"));
    }

    #[test]
    fn test_is_valid_label_simple() {
        assert!(is_valid_label("alice"));
    }

    #[test]
    fn test_is_valid_label_with_hyphen() {
        assert!(is_valid_label("alice-123"));
    }

    #[test]
    fn test_is_valid_label_with_numbers() {
        assert!(is_valid_label("user123"));
    }

    #[test]
    fn test_is_valid_label_empty() {
        assert!(!is_valid_label(""));
    }

    #[test]
    fn test_is_valid_label_too_long() {
        let long_label = "a".repeat(64);
        assert!(!is_valid_label(&long_label));
    }

    #[test]
    fn test_is_valid_label_max_length() {
        let max_label = "a".repeat(63);
        assert!(is_valid_label(&max_label));
    }

    #[test]
    fn test_is_valid_label_starts_with_hyphen() {
        assert!(!is_valid_label("-alice"));
    }

    #[test]
    fn test_is_valid_label_ends_with_hyphen() {
        assert!(!is_valid_label("alice-"));
    }

    #[test]
    fn test_is_valid_label_uppercase() {
        assert!(!is_valid_label("Alice"));
    }

    #[test]
    fn test_is_valid_label_special_chars() {
        assert!(!is_valid_label("alice_123"));
        assert!(!is_valid_label("alice.123"));
    }

    #[test]
    fn test_handle_resolver_new() {
        let resolver = HandleResolver::new();
        assert!(resolver.enable_dns);
        assert!(resolver.enable_https);
    }

    #[test]
    fn test_handle_resolver_dns_only() {
        let resolver = HandleResolver::dns_only();
        assert!(resolver.enable_dns);
        assert!(!resolver.enable_https);
    }

    #[test]
    fn test_handle_resolver_https_only() {
        let resolver = HandleResolver::https_only();
        assert!(!resolver.enable_dns);
        assert!(resolver.enable_https);
    }

    #[test]
    fn test_handle_resolver_default() {
        let resolver = HandleResolver::default();
        assert!(resolver.enable_dns);
        assert!(resolver.enable_https);
    }

    #[tokio::test]
    async fn test_resolve_invalid_handle() {
        let resolver = HandleResolver::new();
        let result = resolver.resolve("invalid..handle").await;
        assert!(result.is_err());
    }

    // Note: Real resolution tests would require either:
    // 1. A mock DNS/HTTP server
    // 2. Integration tests with real handles
    // 3. DNS resolution library integration
}
