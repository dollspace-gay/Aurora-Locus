/// Federation support for multi-PDS ecosystem
///
/// This module enables Aurora Locus PDS to participate in a federated network:
/// - Cross-PDS authentication via DID resolution
/// - Discovery of other PDS instances
/// - Federated content aggregation
/// - Relay support for event distribution

pub mod authentication;
pub mod discovery;
pub mod relay;
pub mod search;

pub use authentication::FederationAuthenticator;
pub use discovery::{PdsDiscovery, PdsInstance};
pub use relay::{RelayClient, RelayConfig};
pub use search::FederatedSearch;

use serde::{Deserialize, Serialize};

/// Federation configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationConfig {
    /// Enable federation features
    pub enabled: bool,

    /// This PDS instance's DID
    pub service_did: String,

    /// This PDS instance's public URL
    pub service_url: String,

    /// List of trusted relay servers
    pub relay_servers: Vec<String>,

    /// Enable federated search
    pub enable_search: bool,

    /// Maximum number of concurrent federated requests
    pub max_concurrent_requests: usize,

    /// Timeout for federated requests in seconds
    pub request_timeout: u64,

    /// List of known PDS instances (optional, for manual configuration)
    pub known_instances: Vec<PdsInstance>,
}

impl Default for FederationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            service_did: String::new(),
            service_url: String::new(),
            relay_servers: vec![],
            enable_search: false,
            max_concurrent_requests: 10,
            request_timeout: 30,
            known_instances: vec![],
        }
    }
}

impl FederationConfig {
    /// Load from environment variables
    pub fn from_env() -> Self {
        let relay_servers = std::env::var("FEDERATION_RELAY_SERVERS")
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.trim().to_string())
            .collect();

        Self {
            enabled: std::env::var("FEDERATION_ENABLED")
                .unwrap_or_else(|_| "false".to_string())
                .parse()
                .unwrap_or(false),
            service_did: std::env::var("PDS_SERVICE_DID").unwrap_or_default(),
            service_url: std::env::var("PDS_SERVICE_URL").unwrap_or_default(),
            relay_servers,
            enable_search: std::env::var("FEDERATION_ENABLE_SEARCH")
                .unwrap_or_else(|_| "false".to_string())
                .parse()
                .unwrap_or(false),
            max_concurrent_requests: std::env::var("FEDERATION_MAX_CONCURRENT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(10),
            request_timeout: std::env::var("FEDERATION_TIMEOUT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(30),
            known_instances: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_federation_config_default() {
        let config = FederationConfig::default();
        assert_eq!(config.enabled, false);
        assert_eq!(config.max_concurrent_requests, 10);
        assert_eq!(config.request_timeout, 30);
    }

    #[test]
    fn test_federation_config_from_env() {
        std::env::set_var("FEDERATION_ENABLED", "true");
        std::env::set_var("PDS_SERVICE_DID", "did:plc:test123");
        std::env::set_var("FEDERATION_RELAY_SERVERS", "https://relay1.com,https://relay2.com");

        let config = FederationConfig::from_env();
        assert_eq!(config.enabled, true);
        assert_eq!(config.service_did, "did:plc:test123");
        assert_eq!(config.relay_servers.len(), 2);

        // Cleanup
        std::env::remove_var("FEDERATION_ENABLED");
        std::env::remove_var("PDS_SERVICE_DID");
        std::env::remove_var("FEDERATION_RELAY_SERVERS");
    }
}
