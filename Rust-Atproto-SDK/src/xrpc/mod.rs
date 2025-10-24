//! XRPC (Cross-organizational RPC) client implementation for ATProto
//!
//! This module provides HTTP client functionality for making requests to
//! ATProto services using the XRPC protocol.
//!
//! # Features
//!
//! - Request/response handling with proper typing
//! - Header management
//! - Exponential backoff retry logic
//! - Schema validation support
//! - Integration with SessionManager

use async_trait::async_trait;
use reqwest::{header::HeaderMap, Method, Request, Response};
use serde::{de::DeserializeOwned, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;

/// XRPC error types
#[derive(Error, Debug)]
pub enum XrpcError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("Server error ({status}): {message}")]
    ServerError { status: u16, message: String },

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Deserialization error: {0}")]
    Deserialization(String),

    #[error("Network error: {0}")]
    Network(String),

    #[error("XRPC error ({error}): {message}")]
    Xrpc { error: String, message: String },
}

/// Result type for XRPC operations
pub type Result<T> = std::result::Result<T, XrpcError>;

/// Retry configuration for XRPC requests
///
/// Provides exponential backoff retry logic for transient failures.
///
/// # Examples
///
/// ```
/// use atproto::xrpc::RetryConfig;
///
/// // Default retry config: 3 attempts, 1s initial delay, 2x multiplier
/// let config = RetryConfig::default();
///
/// // Custom retry config
/// let config = RetryConfig::new(5, 500, 2.0);
///
/// // No retries
/// let config = RetryConfig::none();
/// ```
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts (0 = no retries)
    pub max_attempts: u32,

    /// Initial delay in milliseconds
    pub initial_delay_ms: u64,

    /// Multiplier for exponential backoff (typically 2.0)
    pub backoff_multiplier: f64,

    /// Maximum delay in milliseconds (prevents excessive waits)
    pub max_delay_ms: u64,
}

impl RetryConfig {
    /// Create a new retry configuration
    ///
    /// # Arguments
    ///
    /// * `max_attempts` - Maximum retry attempts (0 = no retries)
    /// * `initial_delay_ms` - Initial delay between retries in milliseconds
    /// * `backoff_multiplier` - Multiplier for exponential backoff (e.g., 2.0)
    ///
    /// # Examples
    ///
    /// ```
    /// use atproto::xrpc::RetryConfig;
    ///
    /// // Retry up to 5 times, starting with 500ms delay, doubling each time
    /// let config = RetryConfig::new(5, 500, 2.0);
    /// ```
    pub fn new(max_attempts: u32, initial_delay_ms: u64, backoff_multiplier: f64) -> Self {
        Self {
            max_attempts,
            initial_delay_ms,
            backoff_multiplier,
            max_delay_ms: 30_000, // Default max 30 seconds
        }
    }

    /// Create a retry config with no retries
    ///
    /// # Examples
    ///
    /// ```
    /// use atproto::xrpc::RetryConfig;
    ///
    /// let config = RetryConfig::none();
    /// assert_eq!(config.max_attempts, 0);
    /// ```
    pub fn none() -> Self {
        Self {
            max_attempts: 0,
            initial_delay_ms: 0,
            backoff_multiplier: 1.0,
            max_delay_ms: 0,
        }
    }

    /// Set the maximum delay in milliseconds
    ///
    /// # Examples
    ///
    /// ```
    /// use atproto::xrpc::RetryConfig;
    ///
    /// let config = RetryConfig::default().with_max_delay_ms(60_000); // 60 seconds max
    /// ```
    pub fn with_max_delay_ms(mut self, max_delay_ms: u64) -> Self {
        self.max_delay_ms = max_delay_ms;
        self
    }

    /// Calculate the delay for a given attempt number
    ///
    /// Uses exponential backoff: delay = initial_delay * (multiplier ^ attempt)
    /// Capped at max_delay_ms.
    ///
    /// # Arguments
    ///
    /// * `attempt` - The attempt number (0-indexed)
    fn calculate_delay(&self, attempt: u32) -> Duration {
        if attempt == 0 {
            return Duration::from_millis(self.initial_delay_ms);
        }

        let delay_ms = (self.initial_delay_ms as f64)
            * self.backoff_multiplier.powi(attempt as i32);

        let capped_delay_ms = delay_ms.min(self.max_delay_ms as f64) as u64;

        Duration::from_millis(capped_delay_ms)
    }
}

impl Default for RetryConfig {
    /// Default retry configuration:
    /// - 3 retry attempts
    /// - 1 second initial delay
    /// - 2.0x exponential backoff
    /// - 30 second maximum delay
    ///
    /// # Examples
    ///
    /// ```
    /// use atproto::xrpc::RetryConfig;
    ///
    /// let config = RetryConfig::default();
    /// assert_eq!(config.max_attempts, 3);
    /// assert_eq!(config.initial_delay_ms, 1000);
    /// ```
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_delay_ms: 1000,
            backoff_multiplier: 2.0,
            max_delay_ms: 30_000,
        }
    }
}

/// XRPC method type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XrpcMethod {
    Query,
    Procedure,
}

/// XRPC request parameters
#[derive(Debug, Clone)]
pub struct XrpcRequest {
    pub method: XrpcMethod,
    pub nsid: String,
    pub params: HashMap<String, String>,
    pub data: Option<serde_json::Value>,
    pub binary_data: Option<Vec<u8>>,
    pub headers: HeaderMap,
}

impl XrpcRequest {
    /// Creates a new XRPC query request
    pub fn query(nsid: impl Into<String>) -> Self {
        Self {
            method: XrpcMethod::Query,
            nsid: nsid.into(),
            params: HashMap::new(),
            data: None,
            binary_data: None,
            headers: HeaderMap::new(),
        }
    }

    /// Creates a new XRPC procedure request
    pub fn procedure(nsid: impl Into<String>) -> Self {
        Self {
            method: XrpcMethod::Procedure,
            nsid: nsid.into(),
            params: HashMap::new(),
            data: None,
            binary_data: None,
            headers: HeaderMap::new(),
        }
    }

    /// Adds a query parameter
    pub fn param(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.params.insert(key.into(), value.into());
        self
    }

    /// Sets the request body data
    pub fn data<T: Serialize>(mut self, data: &T) -> Result<Self> {
        self.data = Some(serde_json::to_value(data)?);
        Ok(self)
    }

    /// Sets binary data with a content type
    ///
    /// # Examples
    ///
    /// ```
    /// use atproto::xrpc::XrpcRequest;
    ///
    /// let image_data = vec![0xFF, 0xD8, 0xFF]; // JPEG magic bytes
    /// let req = XrpcRequest::procedure("com.atproto.repo.uploadBlob")
    ///     .binary(image_data, "image/jpeg");
    /// ```
    pub fn binary(mut self, data: Vec<u8>, content_type: impl AsRef<str>) -> Self {
        self.binary_data = Some(data);
        self.header("Content-Type", content_type)
    }

    /// Adds a header
    pub fn header(mut self, key: impl AsRef<str>, value: impl AsRef<str>) -> Self {
        if let Ok(name) = reqwest::header::HeaderName::from_bytes(key.as_ref().as_bytes()) {
            if let Ok(val) = reqwest::header::HeaderValue::from_str(value.as_ref()) {
                self.headers.insert(name, val);
            }
        }
        self
    }
}

/// XRPC response
#[derive(Debug)]
pub struct XrpcResponse<T> {
    pub data: T,
    pub headers: HeaderMap,
}

impl<T> XrpcResponse<T> {
    pub fn new(data: T, headers: HeaderMap) -> Self {
        Self { data, headers }
    }
}

/// Trait for XRPC client implementations
#[async_trait]
pub trait XrpcClient: Send + Sync {
    /// Executes an XRPC request
    async fn request<T: DeserializeOwned>(
        &self,
        req: XrpcRequest,
    ) -> Result<XrpcResponse<T>>;

    /// Executes an XRPC query
    async fn query<T: DeserializeOwned>(
        &self,
        nsid: impl Into<String> + Send,
        params: HashMap<String, String>,
    ) -> Result<XrpcResponse<T>> {
        let mut req = XrpcRequest::query(nsid);
        req.params = params;
        self.request(req).await
    }

    /// Executes an XRPC procedure
    async fn procedure<I: Serialize + Send + Sync, O: DeserializeOwned>(
        &self,
        nsid: impl Into<String> + Send,
        input: &I,
    ) -> Result<XrpcResponse<O>> {
        let req = XrpcRequest::procedure(nsid).data(input)?;
        self.request(req).await
    }
}

/// Basic XRPC client implementation
pub struct XrpcClientImpl {
    base_url: String,
    client: reqwest::Client,
    headers: Arc<parking_lot::RwLock<HeaderMap>>,
    retry_config: RetryConfig,
}

impl XrpcClientImpl {
    /// Creates a new XRPC client with default retry configuration
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            client: reqwest::Client::new(),
            headers: Arc::new(parking_lot::RwLock::new(HeaderMap::new())),
            retry_config: RetryConfig::default(),
        }
    }

    /// Creates a new XRPC client with a custom HTTP client
    pub fn with_client(base_url: impl Into<String>, client: reqwest::Client) -> Self {
        Self {
            base_url: base_url.into(),
            client,
            headers: Arc::new(parking_lot::RwLock::new(HeaderMap::new())),
            retry_config: RetryConfig::default(),
        }
    }

    /// Creates a new XRPC client with custom retry configuration
    ///
    /// # Examples
    ///
    /// ```
    /// use atproto::xrpc::{XrpcClientImpl, RetryConfig};
    ///
    /// let retry_config = RetryConfig::new(5, 500, 2.0);
    /// let client = XrpcClientImpl::with_retry_config("https://bsky.social", retry_config);
    /// ```
    pub fn with_retry_config(base_url: impl Into<String>, retry_config: RetryConfig) -> Self {
        Self {
            base_url: base_url.into(),
            client: reqwest::Client::new(),
            headers: Arc::new(parking_lot::RwLock::new(HeaderMap::new())),
            retry_config,
        }
    }

    /// Set the retry configuration
    ///
    /// # Examples
    ///
    /// ```
    /// use atproto::xrpc::{XrpcClientImpl, RetryConfig};
    ///
    /// let mut client = XrpcClientImpl::new("https://bsky.social");
    /// client.set_retry_config(RetryConfig::none()); // Disable retries
    /// ```
    pub fn set_retry_config(&mut self, retry_config: RetryConfig) {
        self.retry_config = retry_config;
    }

    /// Get the current retry configuration
    pub fn retry_config(&self) -> &RetryConfig {
        &self.retry_config
    }

    /// Sets a header for all requests
    pub fn set_header(&self, key: impl AsRef<str>, value: impl AsRef<str>) {
        if let Ok(name) = reqwest::header::HeaderName::from_bytes(key.as_ref().as_bytes()) {
            if let Ok(val) = reqwest::header::HeaderValue::from_str(value.as_ref()) {
                self.headers.write().insert(name, val);
            }
        }
    }

    /// Removes a header
    pub fn remove_header(&self, key: impl AsRef<str>) {
        if let Ok(name) = reqwest::header::HeaderName::from_bytes(key.as_ref().as_bytes()) {
            self.headers.write().remove(&name);
        }
    }

    /// Clears all headers
    pub fn clear_headers(&self) {
        self.headers.write().clear();
    }

    /// Gets all current headers
    pub fn headers(&self) -> HeaderMap {
        self.headers.read().clone()
    }

    /// Builds the URL for an XRPC request
    fn build_url(&self, nsid: &str, params: &HashMap<String, String>) -> String {
        let mut url = format!("{}/xrpc/{}", self.base_url.trim_end_matches('/'), nsid);

        if !params.is_empty() {
            url.push('?');
            let param_str: Vec<String> = params
                .iter()
                .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
                .collect();
            url.push_str(&param_str.join("&"));
        }

        url
    }

    /// Builds an HTTP request from an XRPC request
    fn build_request(&self, xrpc_req: &XrpcRequest) -> Result<Request> {
        let url = self.build_url(&xrpc_req.nsid, &xrpc_req.params);

        let method = match xrpc_req.method {
            XrpcMethod::Query => Method::GET,
            XrpcMethod::Procedure => Method::POST,
        };

        let mut req = self.client.request(method, &url);

        // Add global headers
        let global_headers = self.headers.read();
        for (key, value) in global_headers.iter() {
            req = req.header(key.clone(), value.clone());
        }

        // Add request-specific headers
        for (key, value) in xrpc_req.headers.iter() {
            req = req.header(key.clone(), value.clone());
        }

        // Add body for procedures
        if let Some(ref binary) = xrpc_req.binary_data {
            // Binary data takes precedence over JSON data
            req = req.body(binary.clone());
        } else if let Some(ref data) = xrpc_req.data {
            req = req.json(data);
        }

        req.build().map_err(XrpcError::Http)
    }

    /// Parses an HTTP response into an XRPC response
    async fn parse_response<T: DeserializeOwned>(&self, resp: Response) -> Result<XrpcResponse<T>> {
        let headers = resp.headers().clone();
        let status = resp.status();

        if status.is_success() {
            let data = resp.json::<T>().await?;
            Ok(XrpcResponse::new(data, headers))
        } else {
            // Try to parse XRPC error
            if let Ok(error_body) = resp.json::<serde_json::Value>().await {
                if let Some(error) = error_body.get("error").and_then(|e| e.as_str()) {
                    let message = error_body
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("Unknown error");

                    return Err(XrpcError::Xrpc {
                        error: error.to_string(),
                        message: message.to_string(),
                    });
                }
            }

            Err(XrpcError::ServerError {
                status: status.as_u16(),
                message: format!("HTTP {}", status),
            })
        }
    }

    /// Determines if an error should trigger a retry
    ///
    /// Retries are triggered for:
    /// - Network errors (connection failures, timeouts)
    /// - HTTP 429 (Too Many Requests / Rate Limit)
    /// - HTTP 500, 502, 503, 504 (Server errors)
    ///
    /// Retries are NOT triggered for:
    /// - HTTP 4xx client errors (except 429)
    /// - Application-level XRPC errors
    /// - Serialization errors
    fn is_retryable_error(&self, error: &XrpcError) -> bool {
        match error {
            // Network errors are retryable
            XrpcError::Http(reqwest_err) => {
                // Connection errors, timeouts, etc.
                reqwest_err.is_timeout()
                    || reqwest_err.is_connect()
                    || reqwest_err.is_request()
            }

            // Server errors are retryable
            XrpcError::ServerError { status, .. } => {
                // 429 Too Many Requests
                // 500 Internal Server Error
                // 502 Bad Gateway
                // 503 Service Unavailable
                // 504 Gateway Timeout
                matches!(status, 429 | 500 | 502 | 503 | 504)
            }

            // Client errors and XRPC errors are not retryable
            XrpcError::Xrpc { .. } => false,
            XrpcError::InvalidRequest(_) => false,
            XrpcError::Serialization(_) => false,
            XrpcError::Deserialization(_) => false,

            // Network errors are retryable
            XrpcError::Network(_) => true,
        }
    }

    /// Execute a request with retry logic
    async fn execute_with_retry<T: DeserializeOwned>(
        &self,
        req: &XrpcRequest,
    ) -> Result<XrpcResponse<T>> {
        let mut last_error = None;

        for attempt in 0..=self.retry_config.max_attempts {
            // Build a fresh request for each attempt
            let http_req = self.build_request(req)?;

            // Execute the request
            match self.client.execute(http_req).await {
                Ok(resp) => {
                    // Try to parse the response
                    match self.parse_response(resp).await {
                        Ok(result) => return Ok(result),
                        Err(err) => {
                            // Check if we should retry this error
                            if !self.is_retryable_error(&err) || attempt == self.retry_config.max_attempts {
                                return Err(err);
                            }
                            last_error = Some(err);
                        }
                    }
                }
                Err(err) => {
                    let xrpc_err = XrpcError::Http(err);

                    // Check if we should retry this error
                    if !self.is_retryable_error(&xrpc_err) || attempt == self.retry_config.max_attempts {
                        return Err(xrpc_err);
                    }
                    last_error = Some(xrpc_err);
                }
            }

            // Calculate delay and wait before retry
            if attempt < self.retry_config.max_attempts {
                let delay = self.retry_config.calculate_delay(attempt);
                tokio::time::sleep(delay).await;
            }
        }

        // This should never be reached, but return the last error if it happens
        Err(last_error.unwrap_or_else(|| {
            XrpcError::InvalidRequest("Max retries reached without error".to_string())
        }))
    }
}

#[async_trait]
impl XrpcClient for XrpcClientImpl {
    async fn request<T: DeserializeOwned>(
        &self,
        req: XrpcRequest,
    ) -> Result<XrpcResponse<T>> {
        self.execute_with_retry(&req).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct TestData {
        message: String,
        count: i32,
    }

    #[test]
    fn test_xrpc_request_query() {
        let req = XrpcRequest::query("com.atproto.test.query");
        assert_eq!(req.method, XrpcMethod::Query);
        assert_eq!(req.nsid, "com.atproto.test.query");
        assert!(req.data.is_none());
    }

    #[test]
    fn test_xrpc_request_procedure() {
        let req = XrpcRequest::procedure("com.atproto.test.procedure");
        assert_eq!(req.method, XrpcMethod::Procedure);
        assert_eq!(req.nsid, "com.atproto.test.procedure");
    }

    #[test]
    fn test_xrpc_request_with_params() {
        let req = XrpcRequest::query("com.atproto.test.query")
            .param("limit", "10")
            .param("cursor", "abc123");

        assert_eq!(req.params.len(), 2);
        assert_eq!(req.params.get("limit"), Some(&"10".to_string()));
        assert_eq!(req.params.get("cursor"), Some(&"abc123".to_string()));
    }

    #[test]
    fn test_xrpc_request_with_data() {
        let data = TestData {
            message: "hello".to_string(),
            count: 42,
        };

        let req = XrpcRequest::procedure("com.atproto.test.procedure")
            .data(&data)
            .unwrap();

        assert!(req.data.is_some());
        let json_data = req.data.unwrap();
        assert_eq!(json_data["message"], "hello");
        assert_eq!(json_data["count"], 42);
    }

    #[test]
    fn test_xrpc_client_new() {
        let client = XrpcClientImpl::new("https://bsky.social");
        assert_eq!(client.base_url, "https://bsky.social");
    }

    #[test]
    fn test_xrpc_client_headers() {
        let client = XrpcClientImpl::new("https://bsky.social");

        client.set_header("X-Custom-Header", "test-value");
        let headers = client.headers();
        assert_eq!(
            headers.get("X-Custom-Header").unwrap(),
            "test-value"
        );

        client.remove_header("X-Custom-Header");
        let headers = client.headers();
        assert!(headers.get("X-Custom-Header").is_none());
    }

    #[test]
    fn test_xrpc_client_clear_headers() {
        let client = XrpcClientImpl::new("https://bsky.social");

        client.set_header("Header1", "value1");
        client.set_header("Header2", "value2");
        assert_eq!(client.headers().len(), 2);

        client.clear_headers();
        assert_eq!(client.headers().len(), 0);
    }

    #[test]
    fn test_build_url_no_params() {
        let client = XrpcClientImpl::new("https://bsky.social");
        let url = client.build_url("com.atproto.repo.getRecord", &HashMap::new());
        assert_eq!(url, "https://bsky.social/xrpc/com.atproto.repo.getRecord");
    }

    #[test]
    fn test_build_url_with_params() {
        let client = XrpcClientImpl::new("https://bsky.social");
        let mut params = HashMap::new();
        params.insert("repo".to_string(), "did:plc:test".to_string());
        params.insert("collection".to_string(), "app.bsky.feed.post".to_string());

        let url = client.build_url("com.atproto.repo.getRecord", &params);

        assert!(url.starts_with("https://bsky.social/xrpc/com.atproto.repo.getRecord?"));
        assert!(url.contains("repo=did%3Aplc%3Atest"));
        assert!(url.contains("collection=app.bsky.feed.post"));
    }

    #[test]
    fn test_build_url_trailing_slash() {
        let client = XrpcClientImpl::new("https://bsky.social/");
        let url = client.build_url("com.atproto.repo.getRecord", &HashMap::new());
        assert_eq!(url, "https://bsky.social/xrpc/com.atproto.repo.getRecord");
    }

    #[test]
    fn test_xrpc_response() {
        let data = TestData {
            message: "test".to_string(),
            count: 5,
        };
        let headers = HeaderMap::new();
        let response = XrpcResponse::new(data.clone(), headers);

        assert_eq!(response.data.message, "test");
        assert_eq!(response.data.count, 5);
    }

    #[test]
    fn test_xrpc_error_display() {
        let err = XrpcError::InvalidRequest("bad request".to_string());
        assert_eq!(err.to_string(), "Invalid request: bad request");

        let err = XrpcError::ServerError {
            status: 500,
            message: "Internal Server Error".to_string(),
        };
        assert_eq!(err.to_string(), "Server error (500): Internal Server Error");

        let err = XrpcError::Xrpc {
            error: "InvalidRequest".to_string(),
            message: "Missing required parameter".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "XRPC error (InvalidRequest): Missing required parameter"
        );
    }

    #[test]
    fn test_xrpc_method_equality() {
        assert_eq!(XrpcMethod::Query, XrpcMethod::Query);
        assert_eq!(XrpcMethod::Procedure, XrpcMethod::Procedure);
        assert_ne!(XrpcMethod::Query, XrpcMethod::Procedure);
    }

    #[test]
    fn test_retry_config_default() {
        let config = RetryConfig::default();
        assert_eq!(config.max_attempts, 3);
        assert_eq!(config.initial_delay_ms, 1000);
        assert_eq!(config.backoff_multiplier, 2.0);
        assert_eq!(config.max_delay_ms, 30_000);
    }

    #[test]
    fn test_retry_config_new() {
        let config = RetryConfig::new(5, 500, 2.0);
        assert_eq!(config.max_attempts, 5);
        assert_eq!(config.initial_delay_ms, 500);
        assert_eq!(config.backoff_multiplier, 2.0);
    }

    #[test]
    fn test_retry_config_none() {
        let config = RetryConfig::none();
        assert_eq!(config.max_attempts, 0);
        assert_eq!(config.initial_delay_ms, 0);
    }

    #[test]
    fn test_retry_config_with_max_delay() {
        let config = RetryConfig::default().with_max_delay_ms(60_000);
        assert_eq!(config.max_delay_ms, 60_000);
    }

    #[test]
    fn test_retry_config_calculate_delay() {
        let config = RetryConfig::new(5, 1000, 2.0);

        // First attempt: 1000ms
        assert_eq!(config.calculate_delay(0).as_millis(), 1000);

        // Second attempt: 1000 * 2^1 = 2000ms
        assert_eq!(config.calculate_delay(1).as_millis(), 2000);

        // Third attempt: 1000 * 2^2 = 4000ms
        assert_eq!(config.calculate_delay(2).as_millis(), 4000);

        // Fourth attempt: 1000 * 2^3 = 8000ms
        assert_eq!(config.calculate_delay(3).as_millis(), 8000);
    }

    #[test]
    fn test_retry_config_max_delay_cap() {
        let config = RetryConfig::new(10, 1000, 2.0).with_max_delay_ms(5000);

        // Should be capped at 5000ms
        assert_eq!(config.calculate_delay(10).as_millis(), 5000);
        assert_eq!(config.calculate_delay(20).as_millis(), 5000);
    }

    #[test]
    fn test_xrpc_client_with_retry_config() {
        let retry_config = RetryConfig::new(5, 500, 2.0);
        let client = XrpcClientImpl::with_retry_config("https://bsky.social", retry_config.clone());

        assert_eq!(client.retry_config().max_attempts, 5);
        assert_eq!(client.retry_config().initial_delay_ms, 500);
    }

    #[test]
    fn test_xrpc_client_set_retry_config() {
        let mut client = XrpcClientImpl::new("https://bsky.social");
        assert_eq!(client.retry_config().max_attempts, 3); // Default

        client.set_retry_config(RetryConfig::none());
        assert_eq!(client.retry_config().max_attempts, 0);
    }

    #[test]
    fn test_is_retryable_error_server_errors() {
        let client = XrpcClientImpl::new("https://bsky.social");

        // Server errors should be retryable
        assert!(client.is_retryable_error(&XrpcError::ServerError {
            status: 500,
            message: "Internal Server Error".to_string()
        }));
        assert!(client.is_retryable_error(&XrpcError::ServerError {
            status: 502,
            message: "Bad Gateway".to_string()
        }));
        assert!(client.is_retryable_error(&XrpcError::ServerError {
            status: 503,
            message: "Service Unavailable".to_string()
        }));
        assert!(client.is_retryable_error(&XrpcError::ServerError {
            status: 504,
            message: "Gateway Timeout".to_string()
        }));
        assert!(client.is_retryable_error(&XrpcError::ServerError {
            status: 429,
            message: "Too Many Requests".to_string()
        }));
    }

    #[test]
    fn test_is_retryable_error_client_errors() {
        let client = XrpcClientImpl::new("https://bsky.social");

        // Client errors should NOT be retryable (except 429)
        assert!(!client.is_retryable_error(&XrpcError::ServerError {
            status: 400,
            message: "Bad Request".to_string()
        }));
        assert!(!client.is_retryable_error(&XrpcError::ServerError {
            status: 401,
            message: "Unauthorized".to_string()
        }));
        assert!(!client.is_retryable_error(&XrpcError::ServerError {
            status: 404,
            message: "Not Found".to_string()
        }));
    }

    #[test]
    fn test_is_retryable_error_xrpc_errors() {
        let client = XrpcClientImpl::new("https://bsky.social");

        // Application-level errors should NOT be retryable
        assert!(!client.is_retryable_error(&XrpcError::Xrpc {
            error: "InvalidRequest".to_string(),
            message: "Missing parameter".to_string()
        }));
        assert!(!client.is_retryable_error(&XrpcError::InvalidRequest(
            "Bad request".to_string()
        )));
    }
}
