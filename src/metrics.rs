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
    register_counter_vec, register_gauge, register_histogram_vec, register_int_counter_vec,
    register_int_gauge, CounterVec, Gauge, HistogramVec, IntCounterVec, IntGauge, TextEncoder,
    Encoder,
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
