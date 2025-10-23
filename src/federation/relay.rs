/// Relay support for event distribution in the federation
///
/// Relays enable:
/// - Real-time event streaming across PDS instances
/// - Content synchronization
/// - Firehose aggregation
/// - Network-wide event distribution

use crate::error::{PdsError, PdsResult};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};
use futures_util::{SinkExt, StreamExt};

/// Relay configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayConfig {
    /// Relay server URLs
    pub servers: Vec<String>,

    /// Reconnect interval in seconds
    pub reconnect_interval: u64,

    /// Buffer size for event channel
    pub buffer_size: usize,

    /// Enable compression
    pub enable_compression: bool,
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            servers: vec![],
            reconnect_interval: 5,
            buffer_size: 1000,
            enable_compression: true,
        }
    }
}

/// Relay client for connecting to relay servers
pub struct RelayClient {
    config: RelayConfig,
    http_client: Client,
    event_sender: Option<mpsc::Sender<RelayEvent>>,
}

impl RelayClient {
    /// Create a new relay client
    pub fn new(config: RelayConfig) -> Self {
        Self {
            config,
            http_client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap(),
            event_sender: None,
        }
    }

    /// Subscribe to relay firehose
    pub async fn subscribe_firehose(&mut self) -> PdsResult<mpsc::Receiver<RelayEvent>> {
        info!("Subscribing to relay firehose...");

        let (tx, rx) = mpsc::channel(self.config.buffer_size);
        self.event_sender = Some(tx.clone());

        for relay_url in &self.config.servers {
            let relay_url = relay_url.clone();
            let tx = tx.clone();
            let reconnect_interval = self.config.reconnect_interval;

            tokio::spawn(async move {
                Self::connect_to_relay(relay_url, tx, reconnect_interval).await;
            });
        }

        Ok(rx)
    }

    /// Connect to a relay server and stream events
    async fn connect_to_relay(relay_url: String, tx: mpsc::Sender<RelayEvent>, reconnect_interval: u64) {
        loop {
            info!("Connecting to relay: {}", relay_url);

            // Convert HTTP URL to WebSocket URL
            let ws_url = relay_url.replace("https://", "wss://").replace("http://", "ws://");
            let ws_url = format!("{}/xrpc/com.atproto.sync.subscribeRepos", ws_url);

            match connect_async(&ws_url).await {
                Ok((mut ws_stream, _)) => {
                    info!("✓ Connected to relay: {}", relay_url);

                    // Read events from WebSocket
                    while let Some(msg) = ws_stream.next().await {
                        match msg {
                            Ok(Message::Binary(data)) => {
                                // Parse relay event
                                match Self::parse_relay_event(&data) {
                                    Ok(event) => {
                                        if tx.send(event).await.is_err() {
                                            warn!("Event channel closed");
                                            break;
                                        }
                                    }
                                    Err(e) => {
                                        warn!("Failed to parse relay event: {}", e);
                                    }
                                }
                            }
                            Ok(Message::Text(text)) => {
                                debug!("Received text message: {}", text);
                            }
                            Ok(Message::Close(_)) => {
                                info!("Relay closed connection: {}", relay_url);
                                break;
                            }
                            Ok(Message::Ping(data)) => {
                                if let Err(e) = ws_stream.send(Message::Pong(data)).await {
                                    error!("Failed to send pong: {}", e);
                                    break;
                                }
                            }
                            Ok(_) => {}
                            Err(e) => {
                                error!("WebSocket error: {}", e);
                                break;
                            }
                        }
                    }

                    info!("Disconnected from relay: {}", relay_url);
                }
                Err(e) => {
                    error!("Failed to connect to relay {}: {}", relay_url, e);
                }
            }

            // Wait before reconnecting
            info!("Reconnecting in {} seconds...", reconnect_interval);
            tokio::time::sleep(std::time::Duration::from_secs(reconnect_interval)).await;
        }
    }

    /// Parse relay event from binary data
    fn parse_relay_event(data: &[u8]) -> PdsResult<RelayEvent> {
        // In a real implementation, this would parse CAR files and CBOR data
        // For now, we'll create a simple event structure

        // Try to parse as JSON (simplified for this implementation)
        if let Ok(text) = std::str::from_utf8(data) {
            if let Ok(event) = serde_json::from_str::<RelayEvent>(text) {
                return Ok(event);
            }
        }

        // Fallback: create a raw event
        Ok(RelayEvent {
            event_type: "raw".to_string(),
            did: String::new(),
            seq: 0,
            commit: None,
            time: chrono::Utc::now().to_rfc3339(),
        })
    }

    /// Publish event to relay servers
    pub async fn publish_event(&self, event: &RelayEvent) -> PdsResult<()> {
        debug!("Publishing event to {} relay servers", self.config.servers.len());

        for relay_url in &self.config.servers {
            let url = format!("{}/xrpc/com.atproto.repo.uploadBlob", relay_url);

            match self.http_client.post(&url).json(event).send().await {
                Ok(response) => {
                    if response.status().is_success() {
                        debug!("✓ Event published to {}", relay_url);
                    } else {
                        warn!("Relay {} returned error: {}", relay_url, response.status());
                    }
                }
                Err(e) => {
                    warn!("Failed to publish to relay {}: {}", relay_url, e);
                }
            }
        }

        Ok(())
    }

    /// Fetch repository from relay
    pub async fn fetch_repo(&self, did: &str) -> PdsResult<Vec<u8>> {
        info!("Fetching repository from relay: {}", did);

        for relay_url in &self.config.servers {
            let url = format!("{}/xrpc/com.atproto.sync.getRepo?did={}", relay_url, did);

            match self.http_client.get(&url).send().await {
                Ok(response) => {
                    if response.status().is_success() {
                        let data = response.bytes().await.map_err(|e| {
                            PdsError::Internal(format!("Failed to read response: {}", e))
                        })?;
                        return Ok(data.to_vec());
                    }
                }
                Err(e) => {
                    warn!("Failed to fetch from relay {}: {}", relay_url, e);
                }
            }
        }

        Err(PdsError::NotFound("Repository not found on any relay".to_string()))
    }
}

/// Relay event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub did: String,
    pub seq: i64,
    pub commit: Option<serde_json::Value>,
    pub time: String,
}

/// Relay statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayStats {
    pub connected_relays: usize,
    pub events_received: u64,
    pub events_published: u64,
    pub last_event_time: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_relay_config_default() {
        let config = RelayConfig::default();
        assert_eq!(config.reconnect_interval, 5);
        assert_eq!(config.buffer_size, 1000);
        assert_eq!(config.enable_compression, true);
    }

    #[test]
    fn test_relay_event_serialization() {
        let event = RelayEvent {
            event_type: "commit".to_string(),
            did: "did:plc:test123".to_string(),
            seq: 42,
            commit: Some(serde_json::json!({"cid": "bafy..."})),
            time: "2025-01-01T00:00:00Z".to_string(),
        };

        let json = serde_json::to_string(&event).unwrap();
        let deserialized: RelayEvent = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.event_type, "commit");
        assert_eq!(deserialized.seq, 42);
    }

    #[test]
    fn test_relay_client_creation() {
        let config = RelayConfig {
            servers: vec!["https://relay.example.com".to_string()],
            ..Default::default()
        };

        let client = RelayClient::new(config.clone());
        assert_eq!(client.config.servers.len(), 1);
    }
}
