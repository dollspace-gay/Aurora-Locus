/// Configuration management for Aurora Locus PDS
use crate::error::{PdsError, PdsResult};
use crate::validation::ValidationMode;
use serde::{Deserialize, Serialize};
use std::env;
use std::path::{Path, PathBuf};

/// Main server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub service: ServiceConfig,
    pub storage: StorageConfig,
    /// Shared-database backend selection (per
    /// POSTGRES_BACKEND_ASSESSMENT.md §6 Phase 2 / chainlink #75).
    /// Per-actor `ActorStore` always uses SQLite; this only controls
    /// `account_db` and `did_cache_db`.
    #[serde(default)]
    pub database: DatabaseConfig,
    pub authentication: AuthConfig,
    pub identity: IdentityConfig,
    pub email: Option<EmailConfig>,
    pub invites: InviteConfig,
    pub rate_limit: RateLimitConfig,
    pub logging: LoggingConfig,
    pub federation: FederationConfig,
    pub validation_mode: ValidationMode,
}

/// Shared-database backend selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseBackend {
    /// SQLite (default; suitable for hobbyist and development deployments).
    #[default]
    Sqlite,
    /// PostgreSQL (production deployments requiring better concurrency).
    Postgres,
}

/// Database configuration shared by `account_db` and `did_cache_db`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    #[serde(default)]
    pub backend: DatabaseBackend,
    /// Connection URL: file path for SQLite, `postgres://` URL for Postgres.
    /// When unset for SQLite, the per-database file paths in
    /// `StorageConfig` are used (preserving the legacy default).
    pub url: Option<String>,
    pub max_connections: u32,
    pub min_connections: u32,
    pub acquire_timeout_secs: u64,
    pub idle_timeout_secs: Option<u64>,
    pub max_lifetime_secs: Option<u64>,
    /// Standby retry interval for the sequencer leader-election loop
    /// (Phase 4.2 / chainlink #89). Postgres-only — SQLite deployments
    /// skip leader election entirely. See
    /// docs/POSTGRES_PHASE_4_DESIGN.md §3.2.
    pub leader_retry_interval_ms: u64,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            backend: DatabaseBackend::Sqlite,
            url: None,
            // Defaults per POSTGRES_BACKEND_ASSESSMENT.md §9.3.
            max_connections: 25,
            min_connections: 5,
            acquire_timeout_secs: 30,
            idle_timeout_secs: None,
            max_lifetime_secs: None,
            leader_retry_interval_ms: 2000,
        }
    }
}

impl DatabaseConfig {
    /// Construct from explicit option-typed env-var values. Pure function
    /// over `Option<String>` inputs so tests can exercise validation
    /// without manipulating process-global env. Mirrors the pattern used
    /// by `BlobstoreConfig::from_env_values`.
    #[allow(clippy::too_many_arguments)]
    pub fn from_env_values(
        backend: Option<String>,
        url: Option<String>,
        max_connections: Option<String>,
        min_connections: Option<String>,
        acquire_timeout_secs: Option<String>,
        idle_timeout_secs: Option<String>,
        max_lifetime_secs: Option<String>,
        leader_retry_interval_ms: Option<String>,
    ) -> PdsResult<Self> {
        let backend = match backend.as_deref().map(str::to_ascii_lowercase) {
            None => DatabaseBackend::Sqlite,
            Some(s) if s == "sqlite" => DatabaseBackend::Sqlite,
            Some(s) if s == "postgres" || s == "postgresql" => DatabaseBackend::Postgres,
            Some(other) => {
                return Err(PdsError::Validation(format!(
                    "PDS_DB_BACKEND must be 'sqlite' or 'postgres' (got: {:?})",
                    other
                )));
            }
        };

        if backend == DatabaseBackend::Postgres {
            let u = url.as_deref().unwrap_or("");
            if u.is_empty() {
                return Err(PdsError::Validation(
                    "PDS_DB_URL is required when PDS_DB_BACKEND=postgres".to_string(),
                ));
            }
            if !u.starts_with("postgres://") && !u.starts_with("postgresql://") {
                return Err(PdsError::Validation(
                    "PDS_DB_URL must start with 'postgres://' or 'postgresql://' \
                     when PDS_DB_BACKEND=postgres"
                        .to_string(),
                ));
            }
        }

        let max_connections = parse_u32_env("PDS_DB_MAX_CONNECTIONS", max_connections, 25)?;
        let min_connections = parse_u32_env("PDS_DB_MIN_CONNECTIONS", min_connections, 5)?;
        let acquire_timeout_secs =
            parse_u64_env("PDS_DB_ACQUIRE_TIMEOUT_SECS", acquire_timeout_secs, 30)?;
        let idle_timeout_secs = parse_u64_env_opt("PDS_DB_IDLE_TIMEOUT_SECS", idle_timeout_secs)?;
        let max_lifetime_secs = parse_u64_env_opt("PDS_DB_MAX_LIFETIME_SECS", max_lifetime_secs)?;

        if max_connections == 0 {
            return Err(PdsError::Validation(
                "PDS_DB_MAX_CONNECTIONS must be greater than 0".to_string(),
            ));
        }
        if min_connections > max_connections {
            return Err(PdsError::Validation(format!(
                "PDS_DB_MIN_CONNECTIONS ({}) must not exceed PDS_DB_MAX_CONNECTIONS ({})",
                min_connections, max_connections
            )));
        }
        if acquire_timeout_secs == 0 {
            return Err(PdsError::Validation(
                "PDS_DB_ACQUIRE_TIMEOUT_SECS must be greater than 0".to_string(),
            ));
        }

        let leader_retry_interval_ms = parse_u64_env(
            "PDS_SEQUENCER_LEADER_RETRY_MS",
            leader_retry_interval_ms,
            2000,
        )?;
        if !(500..=30_000).contains(&leader_retry_interval_ms) {
            return Err(PdsError::Validation(format!(
                "PDS_SEQUENCER_LEADER_RETRY_MS ({}) must be between 500 and 30000",
                leader_retry_interval_ms
            )));
        }

        Ok(Self {
            backend,
            url,
            max_connections,
            min_connections,
            acquire_timeout_secs,
            idle_timeout_secs,
            max_lifetime_secs,
            leader_retry_interval_ms,
        })
    }
}

fn parse_u32_env(name: &str, raw: Option<String>, default: u32) -> PdsResult<u32> {
    match raw {
        None => Ok(default),
        Some(v) => v.parse::<u32>().map_err(|_| {
            PdsError::Validation(format!(
                "{name} must be a non-negative integer (got: {:?})",
                v
            ))
        }),
    }
}

fn parse_u64_env(name: &str, raw: Option<String>, default: u64) -> PdsResult<u64> {
    match raw {
        None => Ok(default),
        Some(v) => v.parse::<u64>().map_err(|_| {
            PdsError::Validation(format!(
                "{name} must be a non-negative integer (got: {:?})",
                v
            ))
        }),
    }
}

fn parse_u64_env_opt(name: &str, raw: Option<String>) -> PdsResult<Option<u64>> {
    match raw {
        None => Ok(None),
        Some(v) if v.is_empty() => Ok(None),
        Some(v) => v.parse::<u64>().map(Some).map_err(|_| {
            PdsError::Validation(format!(
                "{name} must be a non-negative integer (got: {:?})",
                v
            ))
        }),
    }
}

/// Service-level configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceConfig {
    pub hostname: String,
    pub port: u16,
    pub service_did: String,
    pub version: String,
    pub blob_upload_limit: usize,
}

/// Storage configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    pub data_directory: PathBuf,
    pub account_db: PathBuf,
    pub sequencer_db: PathBuf,
    pub did_cache_db: PathBuf,
    pub actor_store_directory: PathBuf,
    pub blobstore: BlobstoreConfig,
}

/// Blob storage configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BlobstoreConfig {
    Disk {
        location: PathBuf,
        tmp_location: PathBuf,
    },
    S3 {
        bucket: String,
        region: String,
        access_key_id: String,
        secret_access_key: String,
        endpoint: Option<String>,
        /// Object key prefix (Aurora extension; default `"blobs/"`).
        #[serde(default = "default_s3_prefix")]
        prefix: String,
        /// Path-style addressing toggle (default `false`).
        #[serde(default)]
        force_path_style: bool,
        /// Upload operation timeout in milliseconds (default `20000`).
        #[serde(default = "default_s3_upload_timeout_ms")]
        upload_timeout_ms: u64,
    },
}

fn default_s3_prefix() -> String {
    "blobs/".to_string()
}

fn default_s3_upload_timeout_ms() -> u64 {
    20_000
}

impl BlobstoreConfig {
    /// Construct a `BlobstoreConfig` from explicit option-typed values.
    ///
    /// Factored out of `ServerConfig::from_env` for testability — env vars
    /// are process-global and racy in parallel test runs, but pure
    /// `Option<String>` inputs are not. The wrapper in `from_env` reads
    /// env::var and threads the results into this function.
    ///
    /// Behavior:
    /// - Both S3 bucket and disk location set → `Validation` error (mutual
    ///   exclusion: operators must pick one backend).
    /// - S3 bucket set, no disk location → S3 variant, with credentials
    ///   required (missing access key or secret key → error naming the
    ///   missing env var).
    /// - No S3 bucket → Disk variant, defaulting `location` to
    ///   `data_directory/blobs` and `tmp_location` to `data_directory/temp`
    ///   when the disk env vars are unset.
    // Arg count mirrors the env-var fan-in for this constructor; collapsing
    // into a struct would obscure the call-site mapping.
    #[allow(clippy::too_many_arguments)]
    pub fn from_env_values(
        data_directory: &Path,
        s3_bucket: Option<String>,
        s3_region: Option<String>,
        s3_endpoint: Option<String>,
        s3_access_key_id: Option<String>,
        s3_secret_access_key: Option<String>,
        s3_prefix: Option<String>,
        s3_force_path_style: Option<String>,
        s3_upload_timeout_ms: Option<String>,
        disk_location: Option<String>,
        disk_tmp_location: Option<String>,
    ) -> PdsResult<Self> {
        if s3_bucket.is_some() && disk_location.is_some() {
            return Err(PdsError::Validation(
                "Configure either S3 or disk blob storage, not both. \
                 Set PDS_BLOBSTORE_S3_BUCKET for S3 or \
                 PDS_BLOBSTORE_DISK_LOCATION for disk."
                    .to_string(),
            ));
        }

        if let Some(bucket) = s3_bucket {
            // Parse force_path_style: accept "true"/"false"/"1"/"0"
            // case-insensitively. Default false on missing or unrecognised
            // (rejecting unrecognised would be more strict but operators
            // typo-prone; bsky-PDS treats anything-not-true as false).
            let force_path_style = match s3_force_path_style.as_deref() {
                None => false,
                Some(v) => matches!(v.to_ascii_lowercase().as_str(), "true" | "1"),
            };

            // Parse upload_timeout_ms: required to be a valid u64 if set;
            // unparseable input is an error rather than a silent default
            // since timeouts are operator-meaningful.
            let upload_timeout_ms = match s3_upload_timeout_ms {
                None => 20_000,
                Some(v) => v.parse::<u64>().map_err(|_| {
                    PdsError::Validation(format!(
                        "PDS_BLOBSTORE_S3_UPLOAD_TIMEOUT_MS must be a non-negative \
                         integer (got: {:?})",
                        v
                    ))
                })?,
            };

            return Ok(BlobstoreConfig::S3 {
                bucket,
                region: s3_region.unwrap_or_else(|| "us-east-1".to_string()),
                access_key_id: s3_access_key_id.ok_or_else(|| {
                    PdsError::Validation(
                        "PDS_BLOBSTORE_S3_ACCESS_KEY_ID is required when \
                         PDS_BLOBSTORE_S3_BUCKET is set"
                            .to_string(),
                    )
                })?,
                secret_access_key: s3_secret_access_key.ok_or_else(|| {
                    PdsError::Validation(
                        "PDS_BLOBSTORE_S3_SECRET_ACCESS_KEY is required when \
                         PDS_BLOBSTORE_S3_BUCKET is set"
                            .to_string(),
                    )
                })?,
                endpoint: s3_endpoint,
                prefix: s3_prefix.unwrap_or_else(default_s3_prefix),
                force_path_style,
                upload_timeout_ms,
            });
        }

        Ok(BlobstoreConfig::Disk {
            location: disk_location
                .map(PathBuf::from)
                .unwrap_or_else(|| data_directory.join("blobs")),
            tmp_location: disk_tmp_location
                .map(PathBuf::from)
                .unwrap_or_else(|| data_directory.join("temp")),
        })
    }
}

/// Authentication configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    pub jwt_secret: String,
    pub repo_signing_key: String,
    pub plc_rotation_key: String,
    /// DID(s) allowed to access admin panel (comma-separated)
    pub admin_dids: Vec<String>,
    /// OAuth configuration for admin login
    pub oauth: OAuthConfig,
    /// JWT deprecation sunset date (RFC 7231 format: "Sat, 31 Dec 2024 23:59:59 GMT")
    /// When JWT auth will be removed in favor of OAuth 2.1
    #[serde(default = "default_jwt_sunset_date")]
    pub jwt_sunset_date: String,
    /// URL to OAuth migration guide for developers
    #[serde(default = "default_migration_guide_url")]
    pub oauth_migration_guide_url: String,
    /// OAuth feature flags for production deployment
    #[serde(default)]
    pub oauth_features: OAuthFeatureFlags,
}

fn default_jwt_sunset_date() -> String {
    // Default to 90 days from now
    use chrono::{Duration, Utc};
    let sunset = Utc::now() + Duration::days(90);
    sunset.format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

fn default_migration_guide_url() -> String {
    "https://docs.atproto.com/guides/oauth-migration".to_string()
}

/// OAuth configuration for admin authentication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthConfig {
    /// OAuth client ID (URL to client metadata)
    pub client_id: String,
    /// OAuth redirect URI
    pub redirect_uri: String,
    /// PDS URL for OAuth (e.g., https://bsky.social)
    pub pds_url: String,
}

/// OAuth feature flags for controlled production deployment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthFeatureFlags {
    /// Enable OAuth 2.1 authorization endpoints
    #[serde(default = "default_oauth_enabled")]
    pub enabled: bool,

    /// Percentage of users to roll out OAuth to (0-100)
    /// Allows gradual rollout based on DID hash
    #[serde(default = "default_rollout_percentage")]
    pub rollout_percentage: u8,

    /// Require DPoP token binding (enforces security in production)
    /// When false, DPoP is optional (development mode)
    #[serde(default = "default_require_dpop")]
    pub require_dpop: bool,

    /// Enable authorization endpoint (/oauth/authorize)
    #[serde(default = "default_oauth_enabled")]
    pub enable_authorize: bool,

    /// Enable token endpoint (/oauth/token)
    #[serde(default = "default_oauth_enabled")]
    pub enable_token: bool,

    /// Enable device management endpoints
    #[serde(default)]
    pub enable_device_management: bool,

    /// Allow JWT fallback during transition period
    /// When false, reject all JWT tokens
    #[serde(default = "default_allow_jwt_fallback")]
    pub allow_jwt_fallback: bool,
}

impl Default for OAuthFeatureFlags {
    fn default() -> Self {
        Self {
            enabled: default_oauth_enabled(),
            rollout_percentage: default_rollout_percentage(),
            require_dpop: default_require_dpop(),
            enable_authorize: default_oauth_enabled(),
            enable_token: default_oauth_enabled(),
            enable_device_management: false,
            allow_jwt_fallback: default_allow_jwt_fallback(),
        }
    }
}

fn default_oauth_enabled() -> bool {
    false // Disabled by default for safety
}

fn default_rollout_percentage() -> u8 {
    0 // Start with 0% rollout
}

fn default_require_dpop() -> bool {
    false // Optional in development
}

fn default_allow_jwt_fallback() -> bool {
    true // Allow JWT during transition
}

/// Load OAuth feature flags from environment variables
fn load_oauth_features_from_env() -> OAuthFeatureFlags {
    OAuthFeatureFlags {
        enabled: env::var("OAUTH_ENABLED")
            .unwrap_or_else(|_| "false".to_string())
            .parse()
            .unwrap_or(false),
        rollout_percentage: env::var("OAUTH_ROLLOUT_PERCENTAGE")
            .unwrap_or_else(|_| "0".to_string())
            .parse()
            .unwrap_or(0),
        require_dpop: env::var("OAUTH_REQUIRE_DPOP")
            .unwrap_or_else(|_| "false".to_string())
            .parse()
            .unwrap_or(false),
        enable_authorize: env::var("OAUTH_ENABLE_AUTHORIZE")
            .unwrap_or_else(|_| "false".to_string())
            .parse()
            .unwrap_or(false),
        enable_token: env::var("OAUTH_ENABLE_TOKEN")
            .unwrap_or_else(|_| "false".to_string())
            .parse()
            .unwrap_or(false),
        enable_device_management: env::var("OAUTH_ENABLE_DEVICE_MANAGEMENT")
            .unwrap_or_else(|_| "false".to_string())
            .parse()
            .unwrap_or(false),
        allow_jwt_fallback: env::var("OAUTH_ALLOW_JWT_FALLBACK")
            .unwrap_or_else(|_| "true".to_string())
            .parse()
            .unwrap_or(true),
    }
}

/// Identity configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityConfig {
    pub did_plc_url: String,
    pub service_handle_domains: Vec<String>,
    pub did_cache_stale_ttl: u64,
    pub did_cache_max_ttl: u64,
}

/// Email configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailConfig {
    pub smtp_url: String,
    pub from_address: String,
}

/// Invite system configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InviteConfig {
    pub required: bool,
    pub interval: u64,
    pub epoch: String,
}

/// Rate limiting configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    pub enabled: bool,
    pub global_requests_per_minute: u32,
    /// Enable distributed Redis-backed rate limiting for multi-instance deployments
    pub use_redis: bool,
    /// Redis connection URL for distributed rate limiting (e.g., redis://localhost:6379)
    pub redis_url: Option<String>,
}

/// Logging configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    pub level: String,
}

/// Federation configuration for Bluesky network integration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationConfig {
    /// Enable federation with Bluesky relay
    pub enabled: bool,
    /// Bluesky relay URL (e.g., https://bsky.network)
    pub relay_urls: Vec<String>,
    /// AppView URL for feed/profile proxying (e.g., https://api.bsky.app)
    pub appview_url: Option<String>,
    /// Enable firehose WebSocket endpoint for event streaming
    pub firehose_enabled: bool,
    /// Allow relay to crawl repositories
    pub crawl_enabled: bool,
    /// Public URL for this PDS (must be accessible from internet)
    pub public_url: Option<String>,
    /// Enable automatic event streaming to relay
    pub auto_stream_events: bool,
}

impl ServerConfig {
    /// Load configuration from environment variables
    pub fn from_env() -> PdsResult<Self> {
        dotenv::dotenv().ok();

        let hostname = env::var("PDS_HOSTNAME").unwrap_or_else(|_| "localhost".to_string());
        let port = env::var("PDS_PORT")
            .unwrap_or_else(|_| "2583".to_string())
            .parse()
            .map_err(|_| PdsError::Validation("Invalid port number".to_string()))?;

        let service_did =
            env::var("PDS_SERVICE_DID").unwrap_or_else(|_| format!("did:web:{}", hostname));
        let version = env::var("PDS_VERSION").unwrap_or_else(|_| "0.1.0".to_string());
        let blob_upload_limit = env::var("PDS_BLOB_UPLOAD_LIMIT")
            .unwrap_or_else(|_| "5242880".to_string())
            .parse()
            .unwrap_or(5242880);

        let data_directory: PathBuf = env::var("PDS_DATA_DIRECTORY")
            .unwrap_or_else(|_| "./data".to_string())
            .into();
        let account_db = env::var("PDS_ACCOUNT_DB_LOCATION")
            .map(PathBuf::from)
            .unwrap_or_else(|_| data_directory.join("account.sqlite"));
        let sequencer_db = env::var("PDS_SEQUENCER_DB_LOCATION")
            .map(PathBuf::from)
            .unwrap_or_else(|_| data_directory.join("sequencer.sqlite"));
        let did_cache_db = env::var("PDS_DID_CACHE_DB_LOCATION")
            .map(PathBuf::from)
            .unwrap_or_else(|_| data_directory.join("did_cache.sqlite"));
        let actor_store_directory = env::var("PDS_ACTOR_STORE_DIRECTORY")
            .map(PathBuf::from)
            .unwrap_or_else(|_| data_directory.join("actors"));

        let blobstore = BlobstoreConfig::from_env_values(
            &data_directory,
            env::var("PDS_BLOBSTORE_S3_BUCKET").ok(),
            env::var("PDS_BLOBSTORE_S3_REGION").ok(),
            env::var("PDS_BLOBSTORE_S3_ENDPOINT").ok(),
            env::var("PDS_BLOBSTORE_S3_ACCESS_KEY_ID").ok(),
            env::var("PDS_BLOBSTORE_S3_SECRET_ACCESS_KEY").ok(),
            env::var("PDS_BLOBSTORE_S3_PREFIX").ok(),
            env::var("PDS_BLOBSTORE_S3_FORCE_PATH_STYLE").ok(),
            env::var("PDS_BLOBSTORE_S3_UPLOAD_TIMEOUT_MS").ok(),
            env::var("PDS_BLOBSTORE_DISK_LOCATION").ok(),
            env::var("PDS_BLOBSTORE_DISK_TMP_LOCATION").ok(),
        )?;

        let jwt_secret = env::var("PDS_JWT_SECRET")
            .map_err(|_| PdsError::Validation("JWT secret required".to_string()))?;
        let repo_signing_key = env::var("PDS_REPO_SIGNING_KEY_K256_PRIVATE_KEY_HEX")
            .map_err(|_| PdsError::Validation("Repo signing key required".to_string()))?;
        let plc_rotation_key = env::var("PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX")
            .map_err(|_| PdsError::Validation("PLC rotation key required".to_string()))?;

        // Parse admin DIDs from comma-separated list
        let admin_dids = env::var("PDS_ADMIN_DIDS")
            .unwrap_or_else(|_| String::new())
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect::<Vec<String>>();

        // OAuth configuration for admin login
        let oauth_client_id = env::var("PDS_OAUTH_CLIENT_ID")
            .unwrap_or_else(|_| format!("https://{}/oauth/client-metadata.json", hostname));
        let oauth_redirect_uri = env::var("PDS_OAUTH_REDIRECT_URI")
            .unwrap_or_else(|_| format!("https://{}/admin-oauth/callback", hostname));
        let oauth_pds_url =
            env::var("PDS_OAUTH_PDS_URL").unwrap_or_else(|_| "https://bsky.social".to_string());

        let did_plc_url =
            env::var("PDS_DID_PLC_URL").unwrap_or_else(|_| "https://plc.directory".to_string());
        let service_handle_domains = env::var("PDS_SERVICE_HANDLE_DOMAINS")
            .unwrap_or_else(|_| format!(".{}", hostname))
            .split(',')
            .map(|s| s.trim().to_string())
            .collect();
        let did_cache_stale_ttl = env::var("PDS_DID_CACHE_STALE_TTL")
            .unwrap_or_else(|_| "3600".to_string())
            .parse()
            .unwrap_or(3600);
        let did_cache_max_ttl = env::var("PDS_DID_CACHE_MAX_TTL")
            .unwrap_or_else(|_| "86400".to_string())
            .parse()
            .unwrap_or(86400);

        let email = if let Ok(smtp_url) = env::var("PDS_EMAIL_SMTP_URL") {
            Some(EmailConfig {
                smtp_url,
                from_address: env::var("PDS_EMAIL_FROM_ADDRESS")
                    .unwrap_or_else(|_| format!("noreply@{}", hostname)),
            })
        } else {
            None
        };

        let invite_required = env::var("PDS_INVITE_REQUIRED")
            .unwrap_or_else(|_| "false".to_string())
            .parse()
            .unwrap_or(false);
        let invite_interval = env::var("PDS_INVITE_INTERVAL")
            .unwrap_or_else(|_| "604800".to_string())
            .parse()
            .unwrap_or(604800);
        let invite_epoch =
            env::var("PDS_INVITE_EPOCH").unwrap_or_else(|_| "2024-01-01T00:00:00Z".to_string());

        let rate_limit_enabled = env::var("PDS_RATE_LIMITS_ENABLED")
            .unwrap_or_else(|_| "true".to_string())
            .parse()
            .unwrap_or(true);
        let rate_limit_requests = env::var("PDS_RATE_LIMIT_GLOBAL_REQUESTS_PER_MINUTE")
            .unwrap_or_else(|_| "3000".to_string())
            .parse()
            .unwrap_or(3000);
        let rate_limit_use_redis = env::var("PDS_RATE_LIMIT_USE_REDIS")
            .unwrap_or_else(|_| "false".to_string())
            .parse()
            .unwrap_or(false);
        let rate_limit_redis_url = env::var("PDS_RATE_LIMIT_REDIS_URL").ok();

        let log_level = env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());

        // Validation mode
        let validation_mode = env::var("VALIDATION_MODE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();

        // Federation configuration
        let federation_enabled = env::var("PDS_FEDERATION_ENABLED")
            .unwrap_or_else(|_| "false".to_string())
            .parse()
            .unwrap_or(false);
        let relay_urls = env::var("PDS_FEDERATION_RELAY_URLS")
            .unwrap_or_else(|_| "https://bsky.network".to_string())
            .split(',')
            .map(|s| s.trim().to_string())
            .collect();
        let appview_url = env::var("PDS_APPVIEW_URL").ok();
        let firehose_enabled = env::var("PDS_FEDERATION_FIREHOSE_ENABLED")
            .unwrap_or_else(|_| "false".to_string())
            .parse()
            .unwrap_or(false);
        let crawl_enabled = env::var("PDS_FEDERATION_CRAWL_ENABLED")
            .unwrap_or_else(|_| "false".to_string())
            .parse()
            .unwrap_or(false);
        let public_url = env::var("PDS_PUBLIC_URL").ok();
        let auto_stream_events = env::var("PDS_FEDERATION_AUTO_STREAM")
            .unwrap_or_else(|_| "false".to_string())
            .parse()
            .unwrap_or(false);

        let database = DatabaseConfig::from_env_values(
            env::var("PDS_DB_BACKEND").ok(),
            env::var("PDS_DB_URL").ok(),
            env::var("PDS_DB_MAX_CONNECTIONS").ok(),
            env::var("PDS_DB_MIN_CONNECTIONS").ok(),
            env::var("PDS_DB_ACQUIRE_TIMEOUT_SECS").ok(),
            env::var("PDS_DB_IDLE_TIMEOUT_SECS").ok(),
            env::var("PDS_DB_MAX_LIFETIME_SECS").ok(),
            env::var("PDS_SEQUENCER_LEADER_RETRY_MS").ok(),
        )?;

        Ok(ServerConfig {
            service: ServiceConfig {
                hostname,
                port,
                service_did,
                version,
                blob_upload_limit,
            },
            storage: StorageConfig {
                data_directory,
                account_db,
                sequencer_db,
                did_cache_db,
                actor_store_directory,
                blobstore,
            },
            database,
            authentication: AuthConfig {
                jwt_secret,
                repo_signing_key,
                plc_rotation_key,
                admin_dids,
                oauth: OAuthConfig {
                    client_id: oauth_client_id,
                    redirect_uri: oauth_redirect_uri,
                    pds_url: oauth_pds_url,
                },
                jwt_sunset_date: default_jwt_sunset_date(),
                oauth_migration_guide_url: default_migration_guide_url(),
                oauth_features: load_oauth_features_from_env(),
            },
            identity: IdentityConfig {
                did_plc_url,
                service_handle_domains,
                did_cache_stale_ttl,
                did_cache_max_ttl,
            },
            email,
            invites: InviteConfig {
                required: invite_required,
                interval: invite_interval,
                epoch: invite_epoch,
            },
            rate_limit: RateLimitConfig {
                enabled: rate_limit_enabled,
                global_requests_per_minute: rate_limit_requests,
                use_redis: rate_limit_use_redis,
                redis_url: rate_limit_redis_url,
            },
            logging: LoggingConfig { level: log_level },
            federation: FederationConfig {
                enabled: federation_enabled,
                relay_urls,
                appview_url,
                firehose_enabled,
                crawl_enabled,
                public_url,
                auto_stream_events,
            },
            validation_mode,
        })
    }

    /// Validate configuration
    pub fn validate(&self) -> PdsResult<()> {
        if self.service.hostname.is_empty() {
            return Err(PdsError::Validation("Hostname cannot be empty".to_string()));
        }

        if self.authentication.jwt_secret.len() < 32 {
            return Err(PdsError::Validation(
                "JWT secret must be at least 32 characters".to_string(),
            ));
        }

        // Admin password removed - OAuth uses DID-based authentication

        Ok(())
    }
}

#[cfg(test)]
mod database_tests {
    use super::*;

    #[test]
    fn from_env_values_defaults_to_sqlite() {
        let cfg = DatabaseConfig::from_env_values(None, None, None, None, None, None, None,
            None,
        )
            .unwrap();
        assert_eq!(cfg.backend, DatabaseBackend::Sqlite);
        assert!(cfg.url.is_none());
        assert_eq!(cfg.max_connections, 25);
        assert_eq!(cfg.min_connections, 5);
        assert_eq!(cfg.acquire_timeout_secs, 30);
        assert!(cfg.idle_timeout_secs.is_none());
        assert!(cfg.max_lifetime_secs.is_none());
    }

    #[test]
    fn from_env_values_postgres_with_url() {
        let cfg = DatabaseConfig::from_env_values(
            Some("postgres".to_string()),
            Some("postgres://user:pw@localhost/aurora".to_string()),
            None,
            None,
            None,
            None,
            None,
        
            None,)
        .unwrap();
        assert_eq!(cfg.backend, DatabaseBackend::Postgres);
        assert_eq!(
            cfg.url.as_deref(),
            Some("postgres://user:pw@localhost/aurora")
        );
    }

    #[test]
    fn from_env_values_postgres_accepts_postgresql_scheme() {
        let cfg = DatabaseConfig::from_env_values(
            Some("postgresql".to_string()),
            Some("postgresql://localhost/aurora".to_string()),
            None,
            None,
            None,
            None,
            None,
        
            None,)
        .unwrap();
        assert_eq!(cfg.backend, DatabaseBackend::Postgres);
    }

    #[test]
    fn from_env_values_postgres_without_url_rejected() {
        let err = DatabaseConfig::from_env_values(
            Some("postgres".to_string()),
            None,
            None,
            None,
            None,
            None,
            None,
        
            None,)
        .expect_err("postgres without URL should be rejected");
        assert!(err.to_string().contains("PDS_DB_URL is required"));
    }

    #[test]
    fn from_env_values_postgres_with_wrong_scheme_rejected() {
        let err = DatabaseConfig::from_env_values(
            Some("postgres".to_string()),
            Some("mysql://localhost/aurora".to_string()),
            None,
            None,
            None,
            None,
            None,
        
            None,)
        .expect_err("non-postgres URL should be rejected");
        assert!(err.to_string().contains("postgres://"));
    }

    #[test]
    fn from_env_values_invalid_backend_rejected() {
        let err = DatabaseConfig::from_env_values(
            Some("mysql".to_string()),
            None,
            None,
            None,
            None,
            None,
            None,
        
            None,)
        .expect_err("unknown backend should be rejected");
        let msg = err.to_string();
        assert!(msg.contains("sqlite"));
        assert!(msg.contains("postgres"));
    }

    #[test]
    fn from_env_values_min_exceeds_max_rejected() {
        let err = DatabaseConfig::from_env_values(
            None,
            None,
            Some("10".to_string()),
            Some("20".to_string()),
            None,
            None,
            None,
        
            None,)
        .expect_err("min > max should be rejected");
        let msg = err.to_string();
        assert!(msg.contains("MIN_CONNECTIONS"));
        assert!(msg.contains("MAX_CONNECTIONS"));
    }

    #[test]
    fn from_env_values_zero_max_connections_rejected() {
        let err = DatabaseConfig::from_env_values(
            None,
            None,
            Some("0".to_string()),
            None,
            None,
            None,
            None,
        
            None,)
        .expect_err("max=0 should be rejected");
        assert!(err.to_string().contains("greater than 0"));
    }

    #[test]
    fn from_env_values_zero_acquire_timeout_rejected() {
        let err = DatabaseConfig::from_env_values(
            None,
            None,
            None,
            None,
            Some("0".to_string()),
            None,
            None,
        
            None,)
        .expect_err("acquire timeout=0 should be rejected");
        assert!(err.to_string().contains("ACQUIRE_TIMEOUT_SECS"));
    }

    #[test]
    fn from_env_values_non_numeric_timeout_rejected() {
        let err = DatabaseConfig::from_env_values(
            None,
            None,
            None,
            None,
            Some("not-a-number".to_string()),
            None,
            None,
        
            None,)
        .expect_err("non-integer timeout should be rejected");
        assert!(err.to_string().contains("non-negative integer"));
    }

    #[test]
    fn from_env_values_optional_timeouts_parse() {
        let cfg = DatabaseConfig::from_env_values(
            None,
            None,
            None,
            None,
            None,
            Some("60".to_string()),
            Some("3600".to_string()),
        
            None,)
        .unwrap();
        assert_eq!(cfg.idle_timeout_secs, Some(60));
        assert_eq!(cfg.max_lifetime_secs, Some(3600));
    }
}

#[cfg(test)]
mod blobstore_tests {
    use super::*;

    fn data_dir() -> PathBuf {
        PathBuf::from("/tmp/aurora-test-data")
    }

    #[test]
    fn from_env_values_defaults_to_disk_when_nothing_set() {
        let cfg = BlobstoreConfig::from_env_values(
            &data_dir(),
            None, None, None, None, None, None, None, None, None, None,
        )
        .unwrap();
        match cfg {
            BlobstoreConfig::Disk {
                location,
                tmp_location,
            } => {
                assert_eq!(location, data_dir().join("blobs"));
                assert_eq!(tmp_location, data_dir().join("temp"));
            }
            _ => panic!("expected Disk variant"),
        }
    }

    #[test]
    fn from_env_values_disk_only_uses_provided_location() {
        let cfg = BlobstoreConfig::from_env_values(
            &data_dir(),
            None, None, None, None, None, None, None, None,
            Some("/var/blobs".to_string()),
            None,
        )
        .unwrap();
        match cfg {
            BlobstoreConfig::Disk { location, .. } => {
                assert_eq!(location, PathBuf::from("/var/blobs"));
            }
            _ => panic!("expected Disk variant"),
        }
    }

    #[test]
    fn from_env_values_s3_only_constructs_s3_variant() {
        let cfg = BlobstoreConfig::from_env_values(
            &data_dir(),
            Some("my-bucket".to_string()),
            Some("eu-west-1".to_string()),
            Some("https://s3.eu-west-1.amazonaws.com".to_string()),
            Some("AKIAEXAMPLE".to_string()),
            Some("secret-example".to_string()),
            None, None, None,
            None, None,
        )
        .unwrap();
        match cfg {
            BlobstoreConfig::S3 {
                bucket,
                region,
                endpoint,
                access_key_id,
                secret_access_key,
                prefix,
                force_path_style,
                upload_timeout_ms,
            } => {
                assert_eq!(bucket, "my-bucket");
                assert_eq!(region, "eu-west-1");
                assert_eq!(
                    endpoint.as_deref(),
                    Some("https://s3.eu-west-1.amazonaws.com")
                );
                assert_eq!(access_key_id, "AKIAEXAMPLE");
                assert_eq!(secret_access_key, "secret-example");
                // Phase 3 defaults populate when env vars are unset.
                assert_eq!(prefix, "blobs/");
                assert!(!force_path_style);
                assert_eq!(upload_timeout_ms, 20_000);
            }
            _ => panic!("expected S3 variant"),
        }
    }

    #[test]
    fn from_env_values_s3_defaults_region_to_us_east_1() {
        let cfg = BlobstoreConfig::from_env_values(
            &data_dir(),
            Some("b".to_string()),
            None, None,
            Some("k".to_string()),
            Some("s".to_string()),
            None, None, None,
            None, None,
        )
        .unwrap();
        match cfg {
            BlobstoreConfig::S3 { region, .. } => assert_eq!(region, "us-east-1"),
            _ => panic!("expected S3 variant"),
        }
    }

    #[test]
    fn from_env_values_rejects_both_s3_and_disk() {
        let err = BlobstoreConfig::from_env_values(
            &data_dir(),
            Some("bucket".to_string()),
            None, None,
            Some("k".to_string()),
            Some("s".to_string()),
            None, None, None,
            Some("/var/blobs".to_string()),
            None,
        )
        .expect_err("both backends configured should be rejected");
        let msg = err.to_string();
        assert!(msg.contains("not both"));
        assert!(msg.contains("PDS_BLOBSTORE_S3_BUCKET"));
        assert!(msg.contains("PDS_BLOBSTORE_DISK_LOCATION"));
    }

    #[test]
    fn from_env_values_rejects_s3_bucket_without_access_key() {
        let err = BlobstoreConfig::from_env_values(
            &data_dir(),
            Some("bucket".to_string()),
            None, None, None,
            Some("s".to_string()),
            None, None, None,
            None, None,
        )
        .expect_err("S3 bucket without access key should be rejected");
        assert!(err.to_string().contains("PDS_BLOBSTORE_S3_ACCESS_KEY_ID"));
    }

    #[test]
    fn from_env_values_rejects_s3_bucket_without_secret_key() {
        let err = BlobstoreConfig::from_env_values(
            &data_dir(),
            Some("bucket".to_string()),
            None, None,
            Some("k".to_string()),
            None,
            None, None, None,
            None, None,
        )
        .expect_err("S3 bucket without secret key should be rejected");
        assert!(err
            .to_string()
            .contains("PDS_BLOBSTORE_S3_SECRET_ACCESS_KEY"));
    }

    // ---- Phase 3 (#73): force_path_style, upload_timeout_ms, prefix ------

    /// Helper for Phase 3 tests — base S3 config minus the three Phase 3
    /// fields, returns the S3 variant. Spreads each test out into the
    /// specific knob it targets without duplicating boilerplate.
    fn s3_with_phase3(
        prefix: Option<String>,
        force_path_style: Option<String>,
        upload_timeout_ms: Option<String>,
    ) -> PdsResult<BlobstoreConfig> {
        BlobstoreConfig::from_env_values(
            &data_dir(),
            Some("bucket".to_string()),
            None, None,
            Some("k".to_string()),
            Some("s".to_string()),
            prefix,
            force_path_style,
            upload_timeout_ms,
            None, None,
        )
    }

    #[test]
    fn from_env_values_force_path_style_true_strings() {
        for v in &["true", "TRUE", "True", "1"] {
            let cfg = s3_with_phase3(None, Some(v.to_string()), None).unwrap();
            match cfg {
                BlobstoreConfig::S3 { force_path_style, .. } => {
                    assert!(force_path_style, "value {:?} should parse as true", v);
                }
                _ => panic!("expected S3"),
            }
        }
    }

    #[test]
    fn from_env_values_force_path_style_false_strings() {
        for v in &["false", "FALSE", "0", "no", ""] {
            let cfg = s3_with_phase3(None, Some(v.to_string()), None).unwrap();
            match cfg {
                BlobstoreConfig::S3 { force_path_style, .. } => {
                    assert!(!force_path_style, "value {:?} should parse as false", v);
                }
                _ => panic!("expected S3"),
            }
        }
    }

    #[test]
    fn from_env_values_upload_timeout_ms_explicit() {
        let cfg = s3_with_phase3(None, None, Some("5000".to_string())).unwrap();
        match cfg {
            BlobstoreConfig::S3 { upload_timeout_ms, .. } => {
                assert_eq!(upload_timeout_ms, 5000);
            }
            _ => panic!("expected S3"),
        }
    }

    #[test]
    fn from_env_values_rejects_unparseable_upload_timeout_ms() {
        let err = s3_with_phase3(None, None, Some("not-a-number".to_string()))
            .expect_err("non-integer timeout should be rejected");
        let msg = err.to_string();
        assert!(msg.contains("PDS_BLOBSTORE_S3_UPLOAD_TIMEOUT_MS"));
        assert!(msg.contains("non-negative integer"));
    }

    #[test]
    fn from_env_values_prefix_explicit() {
        let cfg = s3_with_phase3(Some("custom/".to_string()), None, None).unwrap();
        match cfg {
            BlobstoreConfig::S3 { prefix, .. } => assert_eq!(prefix, "custom/"),
            _ => panic!("expected S3"),
        }
    }
}
