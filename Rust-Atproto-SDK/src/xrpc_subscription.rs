//! WebSocket XRPC Subscription Client
//!
//! Provides WebSocket-based subscription support for ATProto event streams.

use futures::{stream::Stream, StreamExt};
use std::pin::Pin;
use std::time::Duration;
use tokio::time::sleep;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::xrpc::{XrpcError, XrpcRequest};

/// Result type for subscription operations
pub type SubscriptionResult<T> = Result<T, XrpcError>;

/// Frame header for subscription messages
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct FrameHeader {
    #[serde(rename = "op")]
    pub operation: i32,

    #[serde(rename = "t", skip_serializing_if = "Option::is_none")]
    pub message_type: Option<String>,
}

/// Subscription event types
#[derive(Debug, Clone)]
pub enum SubscriptionEvent {
    /// Message event with type and body
    Message {
        message_type: String,
        body: Vec<u8>
    },
    /// Error event
    Error {
        error: String,
        message: Option<String>
    },
    /// Connection closed
    Closed,
}

/// Configuration for subscription reconnection
#[derive(Debug, Clone)]
pub struct ReconnectConfig {
    /// Maximum number of reconnection attempts (0 = infinite)
    pub max_attempts: u32,

    /// Initial delay between reconnection attempts
    pub initial_delay: Duration,

    /// Maximum delay between reconnection attempts
    pub max_delay: Duration,

    /// Backoff multiplier for exponential backoff
    pub backoff_multiplier: f64,
}

impl Default for ReconnectConfig {
    fn default() -> Self {
        Self {
            max_attempts: 0, // Infinite retries
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            backoff_multiplier: 2.0,
        }
    }
}

impl ReconnectConfig {
    /// Calculate delay for a given attempt
    pub fn calculate_delay(&self, attempt: u32) -> Duration {
        let delay_ms = (self.initial_delay.as_millis() as f64)
            * self.backoff_multiplier.powi(attempt as i32);

        let delay = Duration::from_millis(delay_ms as u64);

        if delay > self.max_delay {
            self.max_delay
        } else {
            delay
        }
    }
}

/// WebSocket subscription client
pub struct SubscriptionClient {
    /// Base URL for WebSocket connections
    base_url: String,

    /// Reconnection configuration
    reconnect_config: ReconnectConfig,
}

impl SubscriptionClient {
    /// Create a new subscription client
    ///
    /// # Arguments
    ///
    /// * `base_url` - Base URL (e.g., "wss://bsky.network")
    pub fn new(base_url: String) -> Self {
        Self {
            base_url,
            reconnect_config: ReconnectConfig::default(),
        }
    }

    /// Set reconnection configuration
    pub fn with_reconnect_config(mut self, config: ReconnectConfig) -> Self {
        self.reconnect_config = config;
        self
    }

    /// Subscribe to an XRPC endpoint
    ///
    /// # Arguments
    ///
    /// * `request` - XRPC request for the subscription
    ///
    /// # Returns
    ///
    /// Returns a stream of subscription events
    pub async fn subscribe(
        &self,
        request: XrpcRequest,
    ) -> SubscriptionResult<Pin<Box<dyn Stream<Item = SubscriptionResult<SubscriptionEvent>> + Send>>> {
        let url = self.build_websocket_url(&request)?;

        let stream = self.connect_with_retry(url).await?;

        Ok(Box::pin(stream))
    }

    /// Build WebSocket URL from XRPC request
    fn build_websocket_url(&self, request: &XrpcRequest) -> SubscriptionResult<String> {
        let mut url = self.base_url.clone();

        // Convert https:// to wss:// or http:// to ws://
        if url.starts_with("https://") {
            url = url.replace("https://", "wss://");
        } else if url.starts_with("http://") {
            url = url.replace("http://", "ws://");
        } else if !url.starts_with("ws://") && !url.starts_with("wss://") {
            url = format!("wss://{}", url);
        }

        // Add XRPC path
        url = format!("{}/xrpc/{}", url, request.nsid);

        // Add query parameters
        if !request.params.is_empty() {
            let params: Vec<String> = request.params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect();

            url = format!("{}?{}", url, params.join("&"));
        }

        Ok(url)
    }

    /// Connect to WebSocket with automatic retry
    async fn connect_with_retry(
        &self,
        url: String,
    ) -> SubscriptionResult<impl Stream<Item = SubscriptionResult<SubscriptionEvent>>> {
        let mut attempt = 0;

        loop {
            match connect_async(&url).await {
                Ok((ws_stream, _)) => {
                    return Ok(Self::process_stream(ws_stream));
                }
                Err(e) if self.should_retry(attempt) => {
                    let delay = self.reconnect_config.calculate_delay(attempt);
                    eprintln!("WebSocket connection failed (attempt {}): {}. Retrying in {:?}...",
                              attempt + 1, e, delay);
                    sleep(delay).await;
                    attempt += 1;
                }
                Err(e) => {
                    return Err(XrpcError::Network(format!("Failed to connect to WebSocket: {}", e)));
                }
            }
        }
    }

    /// Check if should retry connection
    fn should_retry(&self, attempt: u32) -> bool {
        self.reconnect_config.max_attempts == 0 || attempt < self.reconnect_config.max_attempts
    }

    /// Process WebSocket stream into subscription events
    fn process_stream(
        ws_stream: tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    ) -> impl Stream<Item = SubscriptionResult<SubscriptionEvent>> {
        ws_stream.filter_map(|msg_result| async move {
            match msg_result {
                Ok(Message::Binary(data)) => {
                    Some(Self::parse_frame(&data))
                }
                Ok(Message::Close(_)) => {
                    Some(Ok(SubscriptionEvent::Closed))
                }
                Ok(_) => None, // Ignore text, ping, pong
                Err(e) => {
                    Some(Err(XrpcError::Network(format!("WebSocket error: {}", e))))
                }
            }
        })
    }

    /// Parse a subscription frame
    fn parse_frame(data: &[u8]) -> SubscriptionResult<SubscriptionEvent> {
        if data.is_empty() {
            return Err(XrpcError::Deserialization(
                "Empty frame data".to_string()
            ));
        }

        // Read operation code (first byte)
        let op = data[0] as i32;

        match op {
            1 => {
                // Message frame
                // Format: [op:1][type_len:varint][type:string][body:bytes]

                if data.len() < 2 {
                    return Err(XrpcError::Deserialization(
                        "Invalid message frame: too short".to_string()
                    ));
                }

                // Read type length (simple varint - just use u8 for now)
                let type_len = data[1] as usize;

                if data.len() < 2 + type_len {
                    return Err(XrpcError::Deserialization(
                        "Invalid message frame: type too short".to_string()
                    ));
                }

                // Read message type
                let message_type = String::from_utf8(data[2..2 + type_len].to_vec())
                    .map_err(|e| XrpcError::Deserialization(format!("Invalid UTF-8 in message type: {}", e)))?;

                // Body is the rest
                let body = data[2 + type_len..].to_vec();

                Ok(SubscriptionEvent::Message {
                    message_type,
                    body
                })
            }
            -1 => {
                // Error frame
                // Try to parse as JSON
                let error_data: serde_json::Value = serde_json::from_slice(&data[1..])
                    .map_err(|e| XrpcError::Deserialization(format!("Failed to parse error frame: {}", e)))?;

                let error = error_data.get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown error")
                    .to_string();

                let message = error_data.get("message")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                Ok(SubscriptionEvent::Error { error, message })
            }
            _ => {
                Err(XrpcError::Deserialization(
                    format!("Unknown operation code: {}", op)
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reconnect_config_delay() {
        let config = ReconnectConfig::default();

        assert_eq!(config.calculate_delay(0), Duration::from_secs(1));
        assert_eq!(config.calculate_delay(1), Duration::from_secs(2));
        assert_eq!(config.calculate_delay(2), Duration::from_secs(4));
        assert_eq!(config.calculate_delay(3), Duration::from_secs(8));
    }

    #[test]
    fn test_reconnect_config_max_delay() {
        let config = ReconnectConfig {
            max_attempts: 0,
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(10),
            backoff_multiplier: 2.0,
        };

        // Should cap at max_delay
        assert_eq!(config.calculate_delay(10), Duration::from_secs(10));
    }

    #[test]
    fn test_build_websocket_url() {
        let client = SubscriptionClient::new("https://bsky.network".to_string());
        let request = XrpcRequest::query("com.atproto.sync.subscribeRepos");

        let url = client.build_websocket_url(&request).unwrap();
        assert!(url.starts_with("wss://"));
        assert!(url.contains("xrpc/com.atproto.sync.subscribeRepos"));
    }

    #[test]
    fn test_build_websocket_url_with_params() {
        let client = SubscriptionClient::new("https://bsky.network".to_string());
        let mut request = XrpcRequest::query("com.atproto.sync.subscribeRepos");
        request.params.insert("cursor".to_string(), "123".to_string());

        let url = client.build_websocket_url(&request).unwrap();
        assert!(url.contains("?cursor=123"));
    }
}
