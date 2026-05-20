//! Configuration Validation CLI Command
//!
//! Provides comprehensive configuration validation with security and production readiness checks.
//!
//! Last audited for staleness: 2026-05-13 (Arc 10 Step 3 / chainlink
//! #57). Step 3 added the four `validate_gc_sweep_config` warnings
//! covering risky operator opt-ins for the scheduled GC sweep; all
//! previously emitted warnings classified as still valid. Re-audit
//! when major auth, federation, or storage features change.

use crate::config::{BlobstoreConfig, ServerConfig};
use crate::error::PdsResult;
use std::path::Path;

/// Validation issue severity
#[derive(Debug, Clone, PartialEq)]
enum Severity {
    Error,
    Warning,
    Info,
}

/// A validation issue found during config validation
#[derive(Debug, Clone)]
struct ValidationIssue {
    severity: Severity,
    category: String,
    message: String,
}

impl ValidationIssue {
    fn error(category: &str, message: String) -> Self {
        Self {
            severity: Severity::Error,
            category: category.to_string(),
            message,
        }
    }

    fn warning(category: &str, message: String) -> Self {
        Self {
            severity: Severity::Warning,
            category: category.to_string(),
            message,
        }
    }

    fn info(category: &str, message: String) -> Self {
        Self {
            severity: Severity::Info,
            category: category.to_string(),
            message,
        }
    }
}

/// Validate server configuration
pub fn validate_config(config: &ServerConfig) -> PdsResult<()> {
    println!("════════════════════════════════════════════════════════");
    println!("  Configuration Validation");
    println!("════════════════════════════════════════════════════════\n");

    let mut issues = Vec::new();

    // Run all validation checks
    validate_service_config(config, &mut issues);
    validate_storage_config(config, &mut issues);
    validate_auth_config(config, &mut issues);
    validate_identity_config(config, &mut issues);
    validate_email_config(config, &mut issues);
    validate_rate_limit_config(config, &mut issues);
    validate_federation_config(config, &mut issues);
    validate_gc_sweep_config(config, &mut issues);
    check_production_readiness(config, &mut issues);

    // Categorize and display issues
    let errors: Vec<_> = issues
        .iter()
        .filter(|i| i.severity == Severity::Error)
        .collect();
    let warnings: Vec<_> = issues
        .iter()
        .filter(|i| i.severity == Severity::Warning)
        .collect();
    let infos: Vec<_> = issues
        .iter()
        .filter(|i| i.severity == Severity::Info)
        .collect();

    // Display results
    if !errors.is_empty() {
        println!("❌ ERRORS ({}):", errors.len());
        println!("────────────────────────────────────────────────────────");
        for issue in &errors {
            println!("  [{}] {}", issue.category, issue.message);
        }
        println!();
    }

    if !warnings.is_empty() {
        println!("⚠️  WARNINGS ({}):", warnings.len());
        println!("────────────────────────────────────────────────────────");
        for issue in &warnings {
            println!("  [{}] {}", issue.category, issue.message);
        }
        println!();
    }

    if !infos.is_empty() {
        println!("ℹ️  INFO ({}):", infos.len());
        println!("────────────────────────────────────────────────────────");
        for issue in &infos {
            println!("  [{}] {}", issue.category, issue.message);
        }
        println!();
    }

    println!("════════════════════════════════════════════════════════");
    if errors.is_empty() && warnings.is_empty() && infos.is_empty() {
        println!("✅ Configuration is valid!");
        println!("   No issues found.");
    } else if errors.is_empty() {
        println!("✅ Configuration is valid!");
        println!(
            "   {} warnings, {} info messages",
            warnings.len(),
            infos.len()
        );
    } else {
        println!("❌ Configuration has {} errors", errors.len());
        println!("   Please fix the errors above before starting the server.");
        std::process::exit(1);
    }
    println!("════════════════════════════════════════════════════════\n");

    Ok(())
}

/// Validate service configuration
fn validate_service_config(config: &ServerConfig, issues: &mut Vec<ValidationIssue>) {
    // Hostname validation
    if config.service.hostname.is_empty() {
        issues.push(ValidationIssue::error(
            "Service",
            "Hostname cannot be empty".to_string(),
        ));
    } else if config.service.hostname == "localhost" || config.service.hostname == "127.0.0.1" {
        issues.push(ValidationIssue::warning(
            "Service",
            "Hostname is set to localhost - not suitable for production".to_string(),
        ));
    }

    // Port validation
    if config.service.port == 0 {
        issues.push(ValidationIssue::error(
            "Service",
            "Port cannot be 0".to_string(),
        ));
    } else if config.service.port < 1024 {
        issues.push(ValidationIssue::warning(
            "Service",
            format!("Port {} requires elevated privileges", config.service.port),
        ));
    }

    // Service DID validation
    if !config.service.service_did.starts_with("did:") {
        issues.push(ValidationIssue::error(
            "Service",
            "Service DID must start with 'did:'".to_string(),
        ));
    }

    // Blob upload limit validation
    if config.service.blob_upload_limit == 0 {
        issues.push(ValidationIssue::warning(
            "Service",
            "Blob upload limit is 0 - uploads will be disabled".to_string(),
        ));
    } else if config.service.blob_upload_limit > 52428800 {
        // 50 MB
        issues.push(ValidationIssue::warning(
            "Service",
            format!(
                "Blob upload limit is {} bytes (>50MB) - may cause memory issues",
                config.service.blob_upload_limit
            ),
        ));
    }
}

/// Validate storage configuration
fn validate_storage_config(config: &ServerConfig, issues: &mut Vec<ValidationIssue>) {
    // Check data directory
    let data_dir = &config.storage.data_directory;
    if !data_dir.exists() {
        issues.push(ValidationIssue::warning(
            "Storage",
            format!("Data directory does not exist: {}", data_dir.display()),
        ));
    } else if !data_dir.is_dir() {
        issues.push(ValidationIssue::error(
            "Storage",
            format!(
                "Data directory path is not a directory: {}",
                data_dir.display()
            ),
        ));
    }

    // Check database file paths
    check_db_path(&config.storage.account_db, "Account DB", issues);
    check_db_path(&config.storage.sequencer_db, "Sequencer DB", issues);
    check_db_path(&config.storage.did_cache_db, "DID Cache DB", issues);

    // Check actor store directory
    let actor_dir = &config.storage.actor_store_directory;
    if !actor_dir.exists() {
        issues.push(ValidationIssue::info(
            "Storage",
            format!(
                "Actor store directory will be created: {}",
                actor_dir.display()
            ),
        ));
    }

    // Check blobstore configuration
    match &config.storage.blobstore {
        BlobstoreConfig::Disk {
            location,
            tmp_location,
        } => {
            if !location.exists() {
                issues.push(ValidationIssue::info(
                    "Blobstore",
                    format!(
                        "Blob storage directory will be created: {}",
                        location.display()
                    ),
                ));
            }
            if !tmp_location.exists() {
                issues.push(ValidationIssue::info(
                    "Blobstore",
                    format!(
                        "Temp storage directory will be created: {}",
                        tmp_location.display()
                    ),
                ));
            }
        }
        BlobstoreConfig::S3 {
            bucket,
            region,
            access_key_id,
            secret_access_key,
            endpoint,
            ..
        } => {
            if bucket.is_empty() {
                issues.push(ValidationIssue::error(
                    "Blobstore",
                    "S3 bucket name cannot be empty".to_string(),
                ));
            }
            if region.is_empty() {
                issues.push(ValidationIssue::error(
                    "Blobstore",
                    "S3 region cannot be empty".to_string(),
                ));
            }
            if access_key_id.is_empty() {
                issues.push(ValidationIssue::error(
                    "Blobstore",
                    "S3 access key ID cannot be empty".to_string(),
                ));
            }
            if secret_access_key.is_empty() {
                issues.push(ValidationIssue::error(
                    "Blobstore",
                    "S3 secret access key cannot be empty".to_string(),
                ));
            }
            if let Some(ep) = endpoint {
                if !ep.starts_with("http://") && !ep.starts_with("https://") {
                    issues.push(ValidationIssue::warning(
                        "Blobstore",
                        "S3 endpoint should start with http:// or https://".to_string(),
                    ));
                }
            }
        }
    }
}

/// Check database file path
fn check_db_path(path: &Path, name: &str, issues: &mut Vec<ValidationIssue>) {
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            issues.push(ValidationIssue::warning(
                "Storage",
                format!(
                    "{} parent directory does not exist: {}",
                    name,
                    parent.display()
                ),
            ));
        }
    }

    if path.exists() && !path.is_file() {
        issues.push(ValidationIssue::error(
            "Storage",
            format!("{} path exists but is not a file: {}", name, path.display()),
        ));
    }
}

/// Validate authentication configuration
fn validate_auth_config(config: &ServerConfig, issues: &mut Vec<ValidationIssue>) {
    // JWT secret validation
    if config.authentication.jwt_secret.is_empty() {
        issues.push(ValidationIssue::error(
            "Auth",
            "JWT secret cannot be empty".to_string(),
        ));
    } else if config.authentication.jwt_secret.len() < 32 {
        issues.push(ValidationIssue::error(
            "Auth",
            format!(
                "JWT secret is too short ({} chars) - must be at least 32 characters",
                config.authentication.jwt_secret.len()
            ),
        ));
    } else if config.authentication.jwt_secret.len() < 64 {
        issues.push(ValidationIssue::warning(
            "Auth",
            format!(
                "JWT secret is short ({} chars) - recommend at least 64 characters",
                config.authentication.jwt_secret.len()
            ),
        ));
    }

    // Check for weak/default secrets
    let weak_secrets = [
        "change_me",
        "secret",
        "password",
        "12345678901234567890123456789012",
    ];
    if weak_secrets
        .iter()
        .any(|&s| config.authentication.jwt_secret.contains(s))
    {
        issues.push(ValidationIssue::error(
            "Auth",
            "JWT secret appears to be a default/weak value - please change it".to_string(),
        ));
    }

    // Repo signing key validation (should be 64 hex chars for 32 bytes)
    if config.authentication.repo_signing_key.is_empty() {
        issues.push(ValidationIssue::error(
            "Auth",
            "Repo signing key cannot be empty".to_string(),
        ));
    } else {
        match hex::decode(&config.authentication.repo_signing_key) {
            Ok(bytes) => {
                if bytes.len() != 32 {
                    issues.push(ValidationIssue::error(
                        "Auth",
                        format!(
                            "Repo signing key must be 32 bytes (64 hex chars), got {} bytes",
                            bytes.len()
                        ),
                    ));
                }
            }
            Err(_) => {
                issues.push(ValidationIssue::error(
                    "Auth",
                    "Repo signing key is not valid hex".to_string(),
                ));
            }
        }
    }

    // PLC rotation key validation
    if config.authentication.plc_rotation_key.is_empty() {
        issues.push(ValidationIssue::error(
            "Auth",
            "PLC rotation key cannot be empty".to_string(),
        ));
    } else {
        match hex::decode(&config.authentication.plc_rotation_key) {
            Ok(bytes) => {
                if bytes.len() != 32 {
                    issues.push(ValidationIssue::error(
                        "Auth",
                        format!(
                            "PLC rotation key must be 32 bytes (64 hex chars), got {} bytes",
                            bytes.len()
                        ),
                    ));
                }
            }
            Err(_) => {
                issues.push(ValidationIssue::error(
                    "Auth",
                    "PLC rotation key is not valid hex".to_string(),
                ));
            }
        }
    }

    // OAuth configuration validation
    if !config.authentication.oauth.client_id.starts_with("http://")
        && !config
            .authentication
            .oauth
            .client_id
            .starts_with("https://")
    {
        issues.push(ValidationIssue::warning(
            "OAuth",
            "OAuth client ID should be a URL to client metadata".to_string(),
        ));
    }

    if !config
        .authentication
        .oauth
        .redirect_uri
        .starts_with("http://")
        && !config
            .authentication
            .oauth
            .redirect_uri
            .starts_with("https://")
    {
        issues.push(ValidationIssue::error(
            "OAuth",
            "OAuth redirect URI must be a valid URL".to_string(),
        ));
    }

    if config
        .authentication
        .oauth
        .redirect_uri
        .starts_with("http://")
    {
        issues.push(ValidationIssue::warning(
            "OAuth",
            "OAuth redirect URI uses HTTP - should use HTTPS in production".to_string(),
        ));
    }
}

/// Validate identity configuration
fn validate_identity_config(config: &ServerConfig, issues: &mut Vec<ValidationIssue>) {
    // DID PLC URL validation
    if !config.identity.did_plc_url.starts_with("http://")
        && !config.identity.did_plc_url.starts_with("https://")
    {
        issues.push(ValidationIssue::error(
            "Identity",
            "DID PLC URL must be a valid URL".to_string(),
        ));
    }

    // Service handle domains validation
    if config.identity.service_handle_domains.is_empty() {
        issues.push(ValidationIssue::warning(
            "Identity",
            "No service handle domains configured".to_string(),
        ));
    }

    // Cache TTL validation
    if config.identity.did_cache_stale_ttl == 0 {
        issues.push(ValidationIssue::warning(
            "Identity",
            "DID cache stale TTL is 0 - caching is effectively disabled".to_string(),
        ));
    }

    if config.identity.did_cache_max_ttl < config.identity.did_cache_stale_ttl {
        issues.push(ValidationIssue::error(
            "Identity",
            "DID cache max TTL cannot be less than stale TTL".to_string(),
        ));
    }
}

/// Validate email configuration
fn validate_email_config(config: &ServerConfig, issues: &mut Vec<ValidationIssue>) {
    if let Some(email) = &config.email {
        if !email.smtp_url.starts_with("smtp://") && !email.smtp_url.starts_with("smtps://") {
            issues.push(ValidationIssue::warning(
                "Email",
                "SMTP URL should start with smtp:// or smtps://".to_string(),
            ));
        }

        if !email.from_address.contains('@') {
            issues.push(ValidationIssue::error(
                "Email",
                "Email from address is not valid".to_string(),
            ));
        }
    } else {
        issues.push(ValidationIssue::info(
            "Email",
            "Email is not configured - email verification will be disabled".to_string(),
        ));
    }
}

/// Validate rate limiting configuration
fn validate_rate_limit_config(config: &ServerConfig, issues: &mut Vec<ValidationIssue>) {
    if !config.rate_limit.enabled {
        issues.push(ValidationIssue::warning(
            "Rate Limit",
            "Rate limiting is disabled - server may be vulnerable to abuse".to_string(),
        ));
    } else if config.rate_limit.global_requests_per_minute == 0 {
        issues.push(ValidationIssue::warning(
            "Rate Limit",
            "Global rate limit is 0 - all requests will be rejected".to_string(),
        ));
    } else if config.rate_limit.global_requests_per_minute < 100 {
        issues.push(ValidationIssue::warning(
            "Rate Limit",
            format!(
                "Global rate limit is very low ({} req/min) - may affect usability",
                config.rate_limit.global_requests_per_minute
            ),
        ));
    }
}

/// Validate federation configuration
fn validate_federation_config(config: &ServerConfig, issues: &mut Vec<ValidationIssue>) {
    if config.federation.enabled {
        // Check relay URLs
        if config.federation.relay_urls.is_empty() {
            issues.push(ValidationIssue::error(
                "Federation",
                "Federation is enabled but no relay URLs configured".to_string(),
            ));
        } else {
            for url in &config.federation.relay_urls {
                if !url.starts_with("http://") && !url.starts_with("https://") {
                    issues.push(ValidationIssue::error(
                        "Federation",
                        format!("Relay URL '{}' is not a valid URL", url),
                    ));
                }
            }
        }

        // Check public URL
        if (config.federation.crawl_enabled || config.federation.firehose_enabled)
            && config.federation.public_url.is_none()
        {
            issues.push(ValidationIssue::error(
                "Federation",
                "Crawl or firehose enabled but no public URL configured".to_string(),
            ));
        }

        // Check AppView URL if provided
        if let Some(appview) = &config.federation.appview_url {
            if !appview.starts_with("http://") && !appview.starts_with("https://") {
                issues.push(ValidationIssue::error(
                    "Federation",
                    "AppView URL is not a valid URL".to_string(),
                ));
            }
        }
    } else {
        issues.push(ValidationIssue::info(
            "Federation",
            "Federation is disabled - server will not connect to Bluesky network".to_string(),
        ));
    }
}

/// Validate GC sweep configuration (Arc 10 Step 3, V04_DESIGN.md
/// §9.4.3). Off-by-default; warnings only fire when the operator
/// has opted in via `PDS_GC_SWEEP_ENABLED=true`. Each warning
/// targets a specific operator-misconfiguration mode the v0.4
/// design flagged as worth surfacing at validate-time rather than
/// at sweep-time.
fn validate_gc_sweep_config(config: &ServerConfig, issues: &mut Vec<ValidationIssue>) {
    if !config.gc_sweep.enabled {
        // Sweep disabled — config knobs irrelevant. No warnings.
        return;
    }

    if !config.gc_sweep.dry_run {
        issues.push(ValidationIssue::warning(
            "GcSweep",
            "dry_run is false - sweep will perform real deletes. \
             Recommend running with dry_run=true for at least 7 days \
             before enabling destructive mode to verify classification \
             accuracy on this deployment's workload."
                .to_string(),
        ));

        if config.gc_sweep.max_deletes_per_run > 100_000 {
            issues.push(ValidationIssue::warning(
                "GcSweep",
                format!(
                    "max_deletes_per_run is {} (>100,000) and dry_run is \
                     false - a single misclassification could delete many \
                     blobs. Consider a lower cap until operational data \
                     confirms classification accuracy.",
                    config.gc_sweep.max_deletes_per_run
                ),
            ));
        }
    }

    if config.gc_sweep.freshness_threshold_secs < 600 {
        issues.push(ValidationIssue::warning(
            "GcSweep",
            format!(
                "freshness_threshold_secs is {} (<10 minutes) - increases \
                 risk of classifying genuine in-flight uploads as orphans \
                 if the upload's `temp_blob_metadata` row hasn't committed \
                 by sweep time. Recommend >=3600 (1 hour) unless operational \
                 data justifies tightening.",
                config.gc_sweep.freshness_threshold_secs
            ),
        ));
    }

    if config.gc_sweep.interval_secs < 3600 {
        issues.push(ValidationIssue::warning(
            "GcSweep",
            format!(
                "interval_secs is {} (<1 hour) - sweep cadence may exceed \
                 throughput on large stores. Recommend >=21600 (6 hours) \
                 unless operational data justifies tightening.",
                config.gc_sweep.interval_secs
            ),
        ));
    }

    // Arc 16d §9.4.3.6 TTL-vs-interval warning: rows are eligible
    // for sweep when `created_at < now - untethered_ttl_seconds`. If
    // the TTL is <= the cadence between sweep runs, a row may live
    // past TTL but never be observed by a sweep cycle (the previous
    // cycle didn't see it, the next cycle's snapshot may also miss
    // it if a refresh fires).
    if config.gc_sweep.untethered_ttl_seconds <= config.gc_sweep.interval_secs {
        issues.push(ValidationIssue::warning(
            "GcSweep",
            format!(
                "untethered_ttl_seconds ({}) <= interval_secs ({}) - the \
                 row-walker may observe untethered rows past TTL only in \
                 the next sweep cycle, effectively doubling the reclamation \
                 floor. Recommend untethered_ttl_seconds > interval_secs.",
                config.gc_sweep.untethered_ttl_seconds,
                config.gc_sweep.interval_secs,
            ),
        ));
    }

    // Arc 16d §9.4.3.6 / Step 1.6 + §9.4.4 round-4 F9 closure:
    // case-insensitive Postgres-conditional warning. SQLite-only
    // deployments don't trip the warning. Comparison is against
    // "read committed" — any other value (READ COMMITTED variants
    // included) is accepted as the operator's choice but warned on,
    // since §9.4.3.4's race analysis is keyed to READ COMMITTED.
    if config.database.backend == crate::config::DatabaseBackend::Postgres
        && config.database.pg_transaction_isolation.to_ascii_lowercase()
            != "read committed"
    {
        issues.push(ValidationIssue::warning(
            "GcSweep",
            format!(
                "database.pg_transaction_isolation is {:?} - Arc 16d's \
                 sweep-vs-STRICT predicate-disjointness race analysis \
                 (V05_DESIGN.md §9.4.3.4) is keyed to Postgres READ \
                 COMMITTED. Higher isolation (REPEATABLE READ / \
                 SERIALIZABLE) produces 40001 serialization-failure errors \
                 on the per-row autocommit DELETE that v0.5's sweep does \
                 NOT retry-classify (deferred to v0.6+ per §9.4.1.2). \
                 Either set pg_transaction_isolation back to \"read \
                 committed\" or accept elevated db_error_skip_count from \
                 the sweep job.",
                config.database.pg_transaction_isolation
            ),
        ));
    }
}

/// Check production readiness
fn check_production_readiness(config: &ServerConfig, issues: &mut Vec<ValidationIssue>) {
    // Check for localhost/development indicators
    if config.service.hostname.contains("localhost")
        || config.service.hostname.contains("127.0.0.1")
        || config.service.hostname.contains("0.0.0.0")
    {
        issues.push(ValidationIssue::warning(
            "Production",
            "Server hostname indicates development environment".to_string(),
        ));
    }

    // Check logging level
    if config.logging.level.to_lowercase() == "trace"
        || config.logging.level.to_lowercase() == "debug"
    {
        issues.push(ValidationIssue::warning(
            "Production",
            format!(
                "Logging level is '{}' - may impact performance in production",
                config.logging.level
            ),
        ));
    }

    // Check validation mode
    match config.validation_mode {
        crate::validation::ValidationMode::None => {
            issues.push(ValidationIssue::warning(
                "Production",
                "Validation mode is 'none' - records will not be validated".to_string(),
            ));
        }
        crate::validation::ValidationMode::Optimistic => {
            issues.push(ValidationIssue::info(
                "Production",
                "Validation mode is 'optimistic' - good for production".to_string(),
            ));
        }
        crate::validation::ValidationMode::Required => {
            issues.push(ValidationIssue::info(
                "Production",
                "Validation mode is 'required' - strict validation enabled".to_string(),
            ));
        }
    }

    // Check if using disk blobstore in production
    if let BlobstoreConfig::Disk { .. } = config.storage.blobstore {
        issues.push(ValidationIssue::info(
            "Production",
            "Using disk-based blob storage - consider S3 for production scalability".to_string(),
        ));
    }
}
