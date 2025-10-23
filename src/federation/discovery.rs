/// PDS discovery for finding other instances in the federation
///
/// Enables automatic discovery of PDS instances through:
/// - DNS records
/// - Well-known endpoints
/// - Relay server registries
/// - Manual configuration

use crate::error::{PdsError, PdsResult};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// PDS instance information
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PdsInstance {
    /// PDS service DID
    pub did: String,

    /// Public URL of the PDS
    pub url: String,

    /// Optional display name
    pub name: Option<String>,

    /// Whether this PDS accepts registrations
    pub open_registrations: bool,

    /// Number of users (if public)
    pub user_count: Option<i64>,

    /// Last seen timestamp
    pub last_seen: Option<i64>,

    /// Supported features
    pub features: Vec<String>,
}

/// PDS discovery service
pub struct PdsDiscovery {
    http_client: Client,
    known_instances: Arc<RwLock<HashMap<String, PdsInstance>>>,
    relay_servers: Vec<String>,
}

impl PdsDiscovery {
    /// Create a new PDS discovery service
    pub fn new(relay_servers: Vec<String>) -> Self {
        Self {
            http_client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap(),
            known_instances: Arc::new(RwLock::new(HashMap::new())),
            relay_servers,
        }
    }

    /// Discover PDS instances from relay servers
    pub async fn discover_from_relays(&self) -> PdsResult<Vec<PdsInstance>> {
        let mut all_instances = Vec::new();

        for relay_url in &self.relay_servers {
            match self.fetch_instances_from_relay(relay_url).await {
                Ok(instances) => {
                    info!(
                        "Discovered {} PDS instances from relay {}",
                        instances.len(),
                        relay_url
                    );
                    all_instances.extend(instances);
                }
                Err(e) => {
                    warn!("Failed to discover from relay {}: {}", relay_url, e);
                }
            }
        }

        // Update known instances
        let mut known = self.known_instances.write().await;
        for instance in &all_instances {
            known.insert(instance.did.clone(), instance.clone());
        }

        Ok(all_instances)
    }

    /// Fetch PDS instances from a relay server
    async fn fetch_instances_from_relay(&self, relay_url: &str) -> PdsResult<Vec<PdsInstance>> {
        let url = format!("{}/xrpc/com.atproto.sync.listRepos", relay_url);

        debug!("Fetching PDS list from relay: {}", url);

        let response = self.http_client.get(&url).send().await.map_err(|e| {
            PdsError::Internal(format!("Failed to connect to relay: {}", e))
        })?;

        if !response.status().is_success() {
            return Err(PdsError::Internal(format!(
                "Relay returned error: {}",
                response.status()
            )));
        }

        // Parse response
        let relay_response: RelayResponse = response.json().await.map_err(|e| {
            PdsError::Internal(format!("Failed to parse relay response: {}", e))
        })?;

        // Convert to PdsInstance format
        let instances: Vec<PdsInstance> = relay_response
            .repos
            .into_iter()
            .filter_map(|repo| {
                // Extract PDS URL from repo DID
                // In practice, would resolve DID to get PDS endpoint
                Some(PdsInstance {
                    did: repo.did,
                    url: String::new(), // Would be filled by DID resolution
                    name: None,
                    open_registrations: false,
                    user_count: None,
                    last_seen: Some(chrono::Utc::now().timestamp()),
                    features: vec![],
                })
            })
            .collect();

        Ok(instances)
    }

    /// Discover PDS via well-known endpoint
    pub async fn discover_via_wellknown(&self, domain: &str) -> PdsResult<PdsInstance> {
        let url = format!("https://{}/.well-known/atproto-did", domain);

        debug!("Discovering PDS via well-known: {}", url);

        let response = self.http_client.get(&url).send().await.map_err(|e| {
            PdsError::NotFound(format!("Failed to fetch well-known: {}", e))
        })?;

        let did = response.text().await.map_err(|e| {
            PdsError::Internal(format!("Failed to read response: {}", e))
        })?;

        // Fetch PDS info
        let pds_url = format!("https://{}", domain);
        self.fetch_pds_info(&pds_url, &did).await
    }

    /// Fetch PDS server information
    async fn fetch_pds_info(&self, pds_url: &str, did: &str) -> PdsResult<PdsInstance> {
        let url = format!("{}/xrpc/com.atproto.server.describeServer", pds_url);

        let response = self.http_client.get(&url).send().await.map_err(|e| {
            PdsError::Internal(format!("Failed to fetch PDS info: {}", e))
        })?;

        let info: ServerDescription = response.json().await.map_err(|e| {
            PdsError::Internal(format!("Failed to parse PDS info: {}", e))
        })?;

        Ok(PdsInstance {
            did: did.to_string(),
            url: pds_url.to_string(),
            name: None,
            open_registrations: !info.invite_code_required.unwrap_or(true),
            user_count: None,
            last_seen: Some(chrono::Utc::now().timestamp()),
            features: vec![],
        })
    }

    /// Get all known PDS instances
    pub async fn get_known_instances(&self) -> Vec<PdsInstance> {
        let known = self.known_instances.read().await;
        known.values().cloned().collect()
    }

    /// Add a manually configured PDS instance
    pub async fn add_instance(&self, instance: PdsInstance) {
        let mut known = self.known_instances.write().await;
        known.insert(instance.did.clone(), instance);
    }

    /// Find PDS by DID
    pub async fn find_by_did(&self, did: &str) -> Option<PdsInstance> {
        let known = self.known_instances.read().await;
        known.get(did).cloned()
    }

    /// Find PDS instances that accept registrations
    pub async fn find_open_instances(&self) -> Vec<PdsInstance> {
        let known = self.known_instances.read().await;
        known
            .values()
            .filter(|instance| instance.open_registrations)
            .cloned()
            .collect()
    }

    /// Update instance list (should be called periodically)
    pub async fn refresh_instances(&self) -> PdsResult<()> {
        info!("Refreshing PDS instance list...");

        // Discover from relays
        self.discover_from_relays().await?;

        // Remove stale instances (not seen in 7 days)
        let cutoff = chrono::Utc::now().timestamp() - (7 * 24 * 60 * 60);
        let mut known = self.known_instances.write().await;
        known.retain(|_, instance| {
            instance
                .last_seen
                .map(|ts| ts > cutoff)
                .unwrap_or(true)
        });

        info!("Instance list refreshed: {} known instances", known.len());

        Ok(())
    }
}

/// Response from relay server
#[derive(Debug, Deserialize)]
struct RelayResponse {
    repos: Vec<RepoEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RepoEntry {
    did: String,
}

/// Server description response
#[derive(Debug, Deserialize)]
struct ServerDescription {
    #[serde(rename = "inviteCodeRequired")]
    invite_code_required: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pds_instance_serialization() {
        let instance = PdsInstance {
            did: "did:plc:test123".to_string(),
            url: "https://pds.example.com".to_string(),
            name: Some("Example PDS".to_string()),
            open_registrations: true,
            user_count: Some(100),
            last_seen: Some(1234567890),
            features: vec!["firehose".to_string(), "labels".to_string()],
        };

        let json = serde_json::to_string(&instance).unwrap();
        let deserialized: PdsInstance = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.did, "did:plc:test123");
        assert_eq!(deserialized.open_registrations, true);
        assert_eq!(deserialized.features.len(), 2);
    }

    #[tokio::test]
    async fn test_pds_discovery_creation() {
        let discovery = PdsDiscovery::new(vec![
            "https://relay1.example.com".to_string(),
            "https://relay2.example.com".to_string(),
        ]);

        assert_eq!(discovery.relay_servers.len(), 2);

        let instances = discovery.get_known_instances().await;
        assert_eq!(instances.len(), 0); // No instances discovered yet
    }

    #[tokio::test]
    async fn test_add_and_find_instance() {
        let discovery = PdsDiscovery::new(vec![]);

        let instance = PdsInstance {
            did: "did:plc:test123".to_string(),
            url: "https://pds.example.com".to_string(),
            name: None,
            open_registrations: true,
            user_count: None,
            last_seen: Some(chrono::Utc::now().timestamp()),
            features: vec![],
        };

        discovery.add_instance(instance.clone()).await;

        let found = discovery.find_by_did("did:plc:test123").await;
        assert!(found.is_some());
        assert_eq!(found.unwrap().url, "https://pds.example.com");
    }

    #[tokio::test]
    async fn test_find_open_instances() {
        let discovery = PdsDiscovery::new(vec![]);

        // Add open instance
        discovery
            .add_instance(PdsInstance {
                did: "did:plc:open".to_string(),
                url: "https://open.example.com".to_string(),
                name: None,
                open_registrations: true,
                user_count: None,
                last_seen: Some(chrono::Utc::now().timestamp()),
                features: vec![],
            })
            .await;

        // Add closed instance
        discovery
            .add_instance(PdsInstance {
                did: "did:plc:closed".to_string(),
                url: "https://closed.example.com".to_string(),
                name: None,
                open_registrations: false,
                user_count: None,
                last_seen: Some(chrono::Utc::now().timestamp()),
                features: vec![],
            })
            .await;

        let open_instances = discovery.find_open_instances().await;
        assert_eq!(open_instances.len(), 1);
        assert_eq!(open_instances[0].did, "did:plc:open");
    }
}
