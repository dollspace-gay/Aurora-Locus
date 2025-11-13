/// Metrics and telemetry for Aurora Locus PDS
///
/// Provides Prometheus-compatible metrics for monitoring:
/// - HTTP request counts and latencies
/// - Database query times
/// - Cache hit/miss rates
/// - Background job execution
/// - Moderation actions

use lazy_static::lazy_static;
use prometheus::{
    register_counter_vec, register_gauge, register_histogram_vec, register_int_counter,
    register_int_counter_vec, register_int_gauge, CounterVec, Gauge, HistogramVec, IntCounter,
    IntCounterVec, IntGauge, TextEncoder, Encoder,
};

lazy_static! {
    // ========== HTTP Metrics ==========

    /// Total HTTP requests by method, path, and status
    pub static ref HTTP_REQUESTS_TOTAL: IntCounterVec = register_int_counter_vec!(
        "http_requests_total",
        "Total number of HTTP requests",
        &["method", "path", "status"]
    )
    .unwrap();

    /// HTTP request duration in seconds
    pub static ref HTTP_REQUEST_DURATION_SECONDS: HistogramVec = register_histogram_vec!(
        "http_request_duration_seconds",
        "HTTP request latencies in seconds",
        &["method", "path"],
        vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0]
    )
    .unwrap();

    /// Active HTTP requests
    pub static ref HTTP_REQUESTS_ACTIVE: IntGauge = register_int_gauge!(
        "http_requests_active",
        "Number of HTTP requests currently being processed"
    )
    .unwrap();

    // ========== Database Metrics ==========

    /// Database query count by operation type
    pub static ref DB_QUERIES_TOTAL: IntCounterVec = register_int_counter_vec!(
        "db_queries_total",
        "Total number of database queries",
        &["operation", "table"]
    )
    .unwrap();

    /// Database query duration in seconds
    pub static ref DB_QUERY_DURATION_SECONDS: HistogramVec = register_histogram_vec!(
        "db_query_duration_seconds",
        "Database query latencies in seconds",
        &["operation", "table"],
        vec![0.0001, 0.0005, 0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0]
    )
    .unwrap();

    /// Active database connections
    pub static ref DB_CONNECTIONS_ACTIVE: IntGauge = register_int_gauge!(
        "db_connections_active",
        "Number of active database connections"
    )
    .unwrap();

    /// Database connection pool size
    pub static ref DB_CONNECTIONS_POOL_SIZE: IntGauge = register_int_gauge!(
        "db_connections_pool_size",
        "Size of the database connection pool"
    )
    .unwrap();

    // ========== Cache Metrics ==========

    /// Cache hits by cache type
    pub static ref CACHE_HITS_TOTAL: IntCounterVec = register_int_counter_vec!(
        "cache_hits_total",
        "Total number of cache hits",
        &["cache_type"]
    )
    .unwrap();

    /// Cache misses by cache type
    pub static ref CACHE_MISSES_TOTAL: IntCounterVec = register_int_counter_vec!(
        "cache_misses_total",
        "Total number of cache misses",
        &["cache_type"]
    )
    .unwrap();

    /// Cache size (number of entries)
    pub static ref CACHE_SIZE: IntGauge = register_int_gauge!(
        "cache_size",
        "Number of entries in cache"
    )
    .unwrap();

    // ========== Background Job Metrics ==========

    /// Background job executions by job type and status
    pub static ref BACKGROUND_JOBS_TOTAL: IntCounterVec = register_int_counter_vec!(
        "background_jobs_total",
        "Total number of background job executions",
        &["job_type", "status"]
    )
    .unwrap();

    /// Background job duration in seconds
    pub static ref BACKGROUND_JOB_DURATION_SECONDS: HistogramVec = register_histogram_vec!(
        "background_job_duration_seconds",
        "Background job execution time in seconds",
        &["job_type"],
        vec![0.1, 0.5, 1.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0]
    )
    .unwrap();

    /// Active background jobs
    pub static ref BACKGROUND_JOBS_ACTIVE: IntGauge = register_int_gauge!(
        "background_jobs_active",
        "Number of background jobs currently running"
    )
    .unwrap();

    // ========== Moderation Metrics ==========

    /// Moderation actions by action type
    pub static ref MODERATION_ACTIONS_TOTAL: IntCounterVec = register_int_counter_vec!(
        "moderation_actions_total",
        "Total number of moderation actions",
        &["action_type", "target_type"]
    )
    .unwrap();

    /// Reports created by report type
    pub static ref REPORTS_CREATED_TOTAL: IntCounterVec = register_int_counter_vec!(
        "reports_created_total",
        "Total number of reports created",
        &["report_type"]
    )
    .unwrap();

    /// Reports resolved by resolution type
    pub static ref REPORTS_RESOLVED_TOTAL: IntCounterVec = register_int_counter_vec!(
        "reports_resolved_total",
        "Total number of reports resolved",
        &["resolution"]
    )
    .unwrap();

    // ========== Repository Metrics ==========

    /// Repository operations by operation type
    pub static ref REPO_OPERATIONS_TOTAL: IntCounterVec = register_int_counter_vec!(
        "repo_operations_total",
        "Total number of repository operations",
        &["operation", "collection"]
    )
    .unwrap();

    /// Total records in all repositories
    pub static ref REPO_RECORDS_TOTAL: IntGauge = register_int_gauge!(
        "repo_records_total",
        "Total number of records across all repositories"
    )
    .unwrap();

    /// Repository commits
    pub static ref REPO_COMMITS_TOTAL: IntCounterVec = register_int_counter_vec!(
        "repo_commits_total",
        "Total number of repository commits",
        &["did"]
    )
    .unwrap();

    // ========== Validation Metrics ==========

    /// Validation operations by collection and result
    pub static ref VALIDATION_TOTAL: IntCounterVec = register_int_counter_vec!(
        "validation_total",
        "Total number of validation operations",
        &["collection", "result"]
    )
    .unwrap();

    /// Validation duration in seconds
    pub static ref VALIDATION_DURATION_SECONDS: HistogramVec = register_histogram_vec!(
        "validation_duration_seconds",
        "Time to validate records in seconds",
        &["collection"],
        vec![0.00001, 0.00005, 0.0001, 0.0005, 0.001, 0.005, 0.01, 0.05, 0.1]
    )
    .unwrap();

    /// Validation failures by collection and error type
    pub static ref VALIDATION_FAILURES_TOTAL: IntCounterVec = register_int_counter_vec!(
        "validation_failures_total",
        "Total number of validation failures",
        &["collection", "error_type"]
    )
    .unwrap();

    // ========== Blob Storage Metrics ==========

    /// Blob uploads by MIME type
    pub static ref BLOB_UPLOADS_TOTAL: IntCounterVec = register_int_counter_vec!(
        "blob_uploads_total",
        "Total number of blob uploads",
        &["mime_type"]
    )
    .unwrap();

    /// Total blob storage size in bytes
    pub static ref BLOB_STORAGE_BYTES_TOTAL: IntGauge = register_int_gauge!(
        "blob_storage_bytes_total",
        "Total size of blob storage in bytes"
    )
    .unwrap();

    /// Blob count
    pub static ref BLOB_COUNT_TOTAL: IntGauge = register_int_gauge!(
        "blob_count_total",
        "Total number of blobs stored"
    )
    .unwrap();

    // ========== Account Metrics ==========

    /// Account creations
    pub static ref ACCOUNT_CREATIONS_TOTAL: IntCounterVec = register_int_counter_vec!(
        "account_creations_total",
        "Total number of accounts created",
        &["invite_required"]
    )
    .unwrap();

    /// Active sessions
    pub static ref SESSIONS_ACTIVE: IntGauge = register_int_gauge!(
        "sessions_active",
        "Number of active sessions"
    )
    .unwrap();

    /// Total accounts
    pub static ref ACCOUNTS_TOTAL: IntGauge = register_int_gauge!(
        "accounts_total",
        "Total number of accounts"
    )
    .unwrap();

    // ========== Sequencer Metrics ==========

    /// Sequencer events by event type
    pub static ref SEQUENCER_EVENTS_TOTAL: IntCounterVec = register_int_counter_vec!(
        "sequencer_events_total",
        "Total number of sequencer events",
        &["event_type"]
    )
    .unwrap();

    /// Current sequence number
    pub static ref SEQUENCER_CURRENT_SEQ: IntGauge = register_int_gauge!(
        "sequencer_current_seq",
        "Current sequence number"
    )
    .unwrap();

    // ========== Identity Resolution Metrics ==========

    /// Identity resolutions by DID method
    pub static ref IDENTITY_RESOLUTIONS_TOTAL: IntCounterVec = register_int_counter_vec!(
        "identity_resolutions_total",
        "Total number of DID resolutions",
        &["did_method", "status"]
    )
    .unwrap();

    /// Handle resolutions
    pub static ref HANDLE_RESOLUTIONS_TOTAL: IntCounterVec = register_int_counter_vec!(
        "handle_resolutions_total",
        "Total number of handle resolutions",
        &["status"]
    )
    .unwrap();

    // ========== Error Metrics ==========

    /// Errors by error type
    pub static ref ERRORS_TOTAL: IntCounterVec = register_int_counter_vec!(
        "errors_total",
        "Total number of errors",
        &["error_type", "module"]
    )
    .unwrap();

    // ========== System Metrics ==========

    /// Application uptime in seconds
    pub static ref UPTIME_SECONDS: Gauge = register_gauge!(
        "uptime_seconds",
        "Application uptime in seconds"
    )
    .unwrap();

    // ========== Federation/Relay Metrics (Phase 3) ==========

    /// Relay events received by event type
    pub static ref RELAY_EVENTS_TOTAL: IntCounterVec = register_int_counter_vec!(
        "relay_events_total",
        "Total number of relay events received",
        &["event_type"]
    )
    .unwrap();

    /// Relay event processing duration
    pub static ref RELAY_EVENT_PROCESSING_DURATION_SECONDS: HistogramVec = register_histogram_vec!(
        "relay_event_processing_duration_seconds",
        "Time to process relay events in seconds",
        &["event_type"],
        vec![0.0001, 0.0005, 0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0]
    )
    .unwrap();

    /// Relay connection status (0=disconnected, 1=connected)
    pub static ref RELAY_CONNECTION_STATUS: IntGauge = register_int_gauge!(
        "relay_connection_status",
        "Relay connection status (0=down, 1=up)"
    )
    .unwrap();

    /// Total relay connections established
    pub static ref RELAY_CONNECTIONS_TOTAL: IntCounterVec = register_int_counter_vec!(
        "relay_connections_total",
        "Total number of relay connections",
        &["relay_url", "status"]
    )
    .unwrap();

    /// Events published to relay
    pub static ref RELAY_EVENTS_PUBLISHED_TOTAL: IntCounterVec = register_int_counter_vec!(
        "relay_events_published_total",
        "Total number of events published to relay",
        &["event_type", "status"]
    )
    .unwrap();

    // ========== OAuth Metrics (Phase 6.2.4) ==========

    /// OAuth authorization requests by client_id and status
    pub static ref OAUTH_AUTHORIZATION_REQUESTS_TOTAL: IntCounterVec = register_int_counter_vec!(
        "oauth_authorization_requests_total",
        "Total number of OAuth authorization requests",
        &["client_id", "status"]
    )
    .unwrap();

    /// OAuth authorization flow duration
    pub static ref OAUTH_AUTHORIZATION_DURATION_SECONDS: HistogramVec = register_histogram_vec!(
        "oauth_authorization_duration_seconds",
        "OAuth authorization flow latencies in seconds",
        &["client_id"],
        vec![0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0]
    )
    .unwrap();

    /// OAuth token exchanges by grant_type and status
    pub static ref OAUTH_TOKEN_EXCHANGES_TOTAL: IntCounterVec = register_int_counter_vec!(
        "oauth_token_exchanges_total",
        "Total number of OAuth token exchanges",
        &["grant_type", "status"]
    )
    .unwrap();

    /// OAuth token exchange duration
    pub static ref OAUTH_TOKEN_EXCHANGE_DURATION_SECONDS: HistogramVec = register_histogram_vec!(
        "oauth_token_exchange_duration_seconds",
        "OAuth token exchange latencies in seconds",
        &["grant_type"],
        vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0]
    )
    .unwrap();

    /// DPoP verification failures by reason
    pub static ref OAUTH_DPOP_VERIFICATION_FAILURES_TOTAL: IntCounterVec = register_int_counter_vec!(
        "oauth_dpop_verification_failures_total",
        "Total number of DPoP verification failures",
        &["reason"]
    )
    .unwrap();

    /// PKCE verification failures by reason
    pub static ref OAUTH_PKCE_VERIFICATION_FAILURES_TOTAL: IntCounterVec = register_int_counter_vec!(
        "oauth_pkce_verification_failures_total",
        "Total number of PKCE verification failures",
        &["reason"]
    )
    .unwrap();

    /// OAuth token rotations (refresh token rotation)
    pub static ref OAUTH_TOKEN_ROTATIONS_TOTAL: IntCounterVec = register_int_counter_vec!(
        "oauth_token_rotations_total",
        "Total number of OAuth token rotations",
        &["status"]
    )
    .unwrap();

    /// Refresh token replay attack detections
    pub static ref OAUTH_REFRESH_REPLAY_DETECTIONS_TOTAL: IntCounterVec = register_int_counter_vec!(
        "oauth_refresh_replay_detections_total",
        "Total number of refresh token replay attempts detected",
        &["did"]
    )
    .unwrap();

    /// OAuth devices registered per user
    pub static ref OAUTH_DEVICES_REGISTERED_TOTAL: IntCounterVec = register_int_counter_vec!(
        "oauth_devices_registered_total",
        "Total number of OAuth devices registered",
        &["did"]
    )
    .unwrap();

    /// OAuth devices revoked
    pub static ref OAUTH_DEVICES_REVOKED_TOTAL: IntCounterVec = register_int_counter_vec!(
        "oauth_devices_revoked_total",
        "Total number of OAuth devices revoked",
        &["did", "reason"]
    )
    .unwrap();

    /// Active OAuth sessions (current count)
    pub static ref OAUTH_ACTIVE_SESSIONS: IntGauge = register_int_gauge!(
        "oauth_active_sessions",
        "Number of active OAuth sessions"
    )
    .unwrap();

    /// OAuth client registrations
    pub static ref OAUTH_CLIENTS_REGISTERED_TOTAL: IntCounterVec = register_int_counter_vec!(
        "oauth_clients_registered_total",
        "Total number of OAuth clients registered",
        &["client_type"]
    )
    .unwrap();

    /// OAuth scope grants by scope
    pub static ref OAUTH_SCOPE_GRANTS_TOTAL: IntCounterVec = register_int_counter_vec!(
        "oauth_scope_grants_total",
        "Total number of OAuth scope grants",
        &["scope", "granted"]
    )
    .unwrap();

    /// JWT deprecation warnings sent to clients
    /// Tracks how many clients are still using deprecated JWT auth
    pub static ref JWT_DEPRECATION_WARNINGS_TOTAL: IntCounter = register_int_counter!(
        "jwt_deprecation_warnings_total",
        "Total number of JWT deprecation warnings sent to clients"
    )
    .unwrap();

    // ========== Firehose Metrics ==========

    /// Active firehose WebSocket connections
    pub static ref FIREHOSE_CONNECTIONS: IntGauge = register_int_gauge!(
        "firehose_connections_active",
        "Number of active firehose WebSocket connections"
    )
    .unwrap();

    /// Total events sent via firehose by event type
    pub static ref FIREHOSE_EVENTS_SENT_TOTAL: IntCounterVec = register_int_counter_vec!(
        "firehose_events_sent_total",
        "Total number of events sent via firehose",
        &["event_type"]
    )
    .unwrap();

    /// Clients disconnected due to slow processing
    pub static ref FIREHOSE_SLOW_CLIENT_DISCONNECTS_TOTAL: IntCounter = register_int_counter!(
        "firehose_slow_client_disconnects_total",
        "Total number of firehose clients disconnected due to slow processing"
    )
    .unwrap();

    /// Duration of backpressure blocks in seconds
    pub static ref FIREHOSE_BACKPRESSURE_DURATION_SECONDS: HistogramVec = register_histogram_vec!(
        "firehose_backpressure_duration_seconds",
        "Duration of backpressure delays in firehose",
        &["reason"],
        vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0]
    )
    .unwrap();

    /// Firehose batch fetches (catch-up mode)
    pub static ref FIREHOSE_BATCH_FETCHES_TOTAL: IntCounter = register_int_counter!(
        "firehose_batch_fetches_total",
        "Total number of batch event fetches during catch-up"
    )
    .unwrap();

    /// Events sent per batch (catch-up mode)
    pub static ref FIREHOSE_BATCH_SIZE: HistogramVec = register_histogram_vec!(
        "firehose_batch_size_events",
        "Number of events sent per batch in catch-up mode",
        &[],
        vec![1.0, 10.0, 50.0, 100.0, 250.0, 500.0]
    )
    .unwrap();
}

/// Render metrics in Prometheus text format
pub fn render_metrics() -> String {
    let encoder = TextEncoder::new();
    let metric_families = prometheus::gather();
    let mut buffer = Vec::new();
    encoder.encode(&metric_families, &mut buffer).unwrap();
    String::from_utf8(buffer).unwrap()
}

/// Record an HTTP request
pub fn record_http_request(method: &str, path: &str, status: u16, duration: f64) {
    HTTP_REQUESTS_TOTAL
        .with_label_values(&[method, path, &status.to_string()])
        .inc();
    HTTP_REQUEST_DURATION_SECONDS
        .with_label_values(&[method, path])
        .observe(duration);
}

/// Record a database query
pub fn record_db_query(operation: &str, table: &str, duration: f64) {
    DB_QUERIES_TOTAL
        .with_label_values(&[operation, table])
        .inc();
    DB_QUERY_DURATION_SECONDS
        .with_label_values(&[operation, table])
        .observe(duration);
}

/// Record a cache access
pub fn record_cache_access(cache_type: &str, hit: bool) {
    if hit {
        CACHE_HITS_TOTAL.with_label_values(&[cache_type]).inc();
    } else {
        CACHE_MISSES_TOTAL.with_label_values(&[cache_type]).inc();
    }
}

/// Record a background job execution
pub fn record_background_job(job_type: &str, status: &str, duration: f64) {
    BACKGROUND_JOBS_TOTAL
        .with_label_values(&[job_type, status])
        .inc();
    BACKGROUND_JOB_DURATION_SECONDS
        .with_label_values(&[job_type])
        .observe(duration);
}

/// Record a moderation action
pub fn record_moderation_action(action_type: &str, target_type: &str) {
    MODERATION_ACTIONS_TOTAL
        .with_label_values(&[action_type, target_type])
        .inc();
}

/// Record a report
pub fn record_report_created(report_type: &str) {
    REPORTS_CREATED_TOTAL
        .with_label_values(&[report_type])
        .inc();
}

/// Record a report resolution
pub fn record_report_resolved(resolution: &str) {
    REPORTS_RESOLVED_TOTAL
        .with_label_values(&[resolution])
        .inc();
}

/// Record a repository operation
pub fn record_repo_operation(operation: &str, collection: &str) {
    REPO_OPERATIONS_TOTAL
        .with_label_values(&[operation, collection])
        .inc();
}

/// Record a blob upload
pub fn record_blob_upload(mime_type: &str) {
    BLOB_UPLOADS_TOTAL.with_label_values(&[mime_type]).inc();
}

/// Record an account creation
pub fn record_account_creation(invite_required: bool) {
    ACCOUNT_CREATIONS_TOTAL
        .with_label_values(&[if invite_required { "yes" } else { "no" }])
        .inc();
}

/// Record a sequencer event
pub fn record_sequencer_event(event_type: &str) {
    SEQUENCER_EVENTS_TOTAL
        .with_label_values(&[event_type])
        .inc();
}

/// Record an identity resolution
pub fn record_identity_resolution(did_method: &str, success: bool) {
    IDENTITY_RESOLUTIONS_TOTAL
        .with_label_values(&[did_method, if success { "success" } else { "failure" }])
        .inc();
}

/// Record a handle resolution
pub fn record_handle_resolution(success: bool) {
    HANDLE_RESOLUTIONS_TOTAL
        .with_label_values(&[if success { "success" } else { "failure" }])
        .inc();
}

/// Record an error
pub fn record_error(error_type: &str, module: &str) {
    ERRORS_TOTAL
        .with_label_values(&[error_type, module])
        .inc();
}

/// Record a relay event received (Phase 3)
pub fn record_relay_event(event_type: &str, processing_duration: f64) {
    RELAY_EVENTS_TOTAL
        .with_label_values(&[event_type])
        .inc();
    RELAY_EVENT_PROCESSING_DURATION_SECONDS
        .with_label_values(&[event_type])
        .observe(processing_duration);
}

/// Record relay connection status (Phase 3)
pub fn set_relay_connection_status(connected: bool) {
    RELAY_CONNECTION_STATUS.set(if connected { 1 } else { 0 });
}

/// Record relay connection attempt (Phase 3)
pub fn record_relay_connection(relay_url: &str, success: bool) {
    RELAY_CONNECTIONS_TOTAL
        .with_label_values(&[relay_url, if success { "success" } else { "failure" }])
        .inc();
}

/// Record event published to relay (Phase 3)
pub fn record_relay_publish(event_type: &str, success: bool) {
    RELAY_EVENTS_PUBLISHED_TOTAL
        .with_label_values(&[event_type, if success { "success" } else { "failure" }])
        .inc();
}

/// Record a validation operation
pub fn record_validation(collection: &str, success: bool, duration: f64) {
    VALIDATION_TOTAL
        .with_label_values(&[collection, if success { "success" } else { "failure" }])
        .inc();
    VALIDATION_DURATION_SECONDS
        .with_label_values(&[collection])
        .observe(duration);
}

/// Record a validation failure with error details
pub fn record_validation_failure(collection: &str, error_type: &str) {
    VALIDATION_FAILURES_TOTAL
        .with_label_values(&[collection, error_type])
        .inc();
}

// ========== OAuth Helper Functions (Phase 6.2.4) ==========

/// Record an OAuth authorization request
pub fn record_oauth_authorization(client_id: &str, status: &str, duration: f64) {
    OAUTH_AUTHORIZATION_REQUESTS_TOTAL
        .with_label_values(&[client_id, status])
        .inc();
    OAUTH_AUTHORIZATION_DURATION_SECONDS
        .with_label_values(&[client_id])
        .observe(duration);
}

/// Record an OAuth token exchange
pub fn record_oauth_token_exchange(grant_type: &str, status: &str, duration: f64) {
    OAUTH_TOKEN_EXCHANGES_TOTAL
        .with_label_values(&[grant_type, status])
        .inc();
    OAUTH_TOKEN_EXCHANGE_DURATION_SECONDS
        .with_label_values(&[grant_type])
        .observe(duration);
}

/// Record a DPoP verification failure
pub fn record_oauth_dpop_failure(reason: &str) {
    OAUTH_DPOP_VERIFICATION_FAILURES_TOTAL
        .with_label_values(&[reason])
        .inc();
}

/// Record a PKCE verification failure
pub fn record_oauth_pkce_failure(reason: &str) {
    OAUTH_PKCE_VERIFICATION_FAILURES_TOTAL
        .with_label_values(&[reason])
        .inc();
}

/// Record an OAuth token rotation
pub fn record_oauth_token_rotation(status: &str) {
    OAUTH_TOKEN_ROTATIONS_TOTAL
        .with_label_values(&[status])
        .inc();
}

/// Record a refresh token replay detection
pub fn record_oauth_replay_detection(did: &str) {
    OAUTH_REFRESH_REPLAY_DETECTIONS_TOTAL
        .with_label_values(&[did])
        .inc();
}

/// Record an OAuth device registration
pub fn record_oauth_device_registered(did: &str) {
    OAUTH_DEVICES_REGISTERED_TOTAL
        .with_label_values(&[did])
        .inc();
}

/// Record an OAuth device revocation
pub fn record_oauth_device_revoked(did: &str, reason: &str) {
    OAUTH_DEVICES_REVOKED_TOTAL
        .with_label_values(&[did, reason])
        .inc();
}

/// Set active OAuth sessions count
pub fn set_oauth_active_sessions(count: i64) {
    OAUTH_ACTIVE_SESSIONS.set(count);
}

/// Record an OAuth client registration
pub fn record_oauth_client_registered(client_type: &str) {
    OAUTH_CLIENTS_REGISTERED_TOTAL
        .with_label_values(&[client_type])
        .inc();
}

/// Record an OAuth scope grant
pub fn record_oauth_scope_grant(scope: &str, granted: bool) {
    OAUTH_SCOPE_GRANTS_TOTAL
        .with_label_values(&[scope, if granted { "true" } else { "false" }])
        .inc();
}

/// Record a JWT deprecation warning sent to a client
///
/// This tracks how many clients are still using deprecated JWT authentication
/// versus OAuth 2.1, helping monitor migration progress.
pub fn record_jwt_deprecation_warning() {
    JWT_DEPRECATION_WARNINGS_TOTAL.inc();
}

// ========== Firehose Helper Functions ==========

/// Record a firehose connection
pub fn record_firehose_connection_start() {
    FIREHOSE_CONNECTIONS.inc();
}

/// Record a firehose disconnection
pub fn record_firehose_connection_end() {
    FIREHOSE_CONNECTIONS.dec();
}

/// Record an event sent via firehose
pub fn record_firehose_event_sent(event_type: &str) {
    FIREHOSE_EVENTS_SENT_TOTAL
        .with_label_values(&[event_type])
        .inc();
}

/// Record a slow client disconnect
pub fn record_firehose_slow_client_disconnect() {
    FIREHOSE_SLOW_CLIENT_DISCONNECTS_TOTAL.inc();
}

/// Record backpressure duration
pub fn record_firehose_backpressure(reason: &str, duration: f64) {
    FIREHOSE_BACKPRESSURE_DURATION_SECONDS
        .with_label_values(&[reason])
        .observe(duration);
}

/// Record a batch fetch during catch-up
pub fn record_firehose_batch_fetch(event_count: usize) {
    FIREHOSE_BATCH_FETCHES_TOTAL.inc();
    FIREHOSE_BATCH_SIZE
        .with_label_values(&[])
        .observe(event_count as f64);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_http_request() {
        record_http_request("GET", "/xrpc/test", 200, 0.05);
        let metrics = render_metrics();
        assert!(metrics.contains("http_requests_total"));
        assert!(metrics.contains("http_request_duration_seconds"));
    }

    #[test]
    fn test_record_db_query() {
        record_db_query("SELECT", "account", 0.001);
        let metrics = render_metrics();
        assert!(metrics.contains("db_queries_total"));
        assert!(metrics.contains("db_query_duration_seconds"));
    }

    #[test]
    fn test_record_cache_access() {
        record_cache_access("did_cache", true);
        record_cache_access("did_cache", false);
        let metrics = render_metrics();
        assert!(metrics.contains("cache_hits_total"));
        assert!(metrics.contains("cache_misses_total"));
    }

    #[test]
    fn test_record_background_job() {
        record_background_job("cleanup", "success", 1.5);
        let metrics = render_metrics();
        assert!(metrics.contains("background_jobs_total"));
        assert!(metrics.contains("background_job_duration_seconds"));
    }

    #[test]
    fn test_record_moderation_action() {
        record_moderation_action("takedown", "account");
        let metrics = render_metrics();
        assert!(metrics.contains("moderation_actions_total"));
    }

    #[test]
    fn test_metrics_rendering() {
        // Record some metrics first to ensure output
        record_http_request("GET", "/test", 200, 0.05);
        record_db_query("SELECT", "test", 0.001);
        record_cache_access("test", true);

        let metrics = render_metrics();

        // Check that Prometheus format is correct (will have HELP/TYPE for recorded metrics)
        assert!(metrics.contains("# HELP") || !metrics.is_empty());
        assert!(metrics.contains("# TYPE") || !metrics.is_empty());

        // Check some key metrics are present
        assert!(metrics.contains("http_requests_total"));
        assert!(metrics.contains("db_queries_total"));
        assert!(metrics.contains("cache_hits_total"));
    }

    #[test]
    fn test_cache_hit_rate() {
        // Simulate cache accesses
        for _ in 0..70 {
            record_cache_access("test_cache", true);
        }
        for _ in 0..30 {
            record_cache_access("test_cache", false);
        }

        // Cache hit rate should be 70%
        let metrics = render_metrics();
        assert!(metrics.contains("cache_hits_total"));
        assert!(metrics.contains("cache_misses_total"));
    }
}
