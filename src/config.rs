/// Configuration management for Aurora Locus PDS
use crate::error::{PdsError, PdsResult};
use crate::validation::ValidationMode;
use serde::{Deserialize, Serialize};
use std::env;
use std::path::PathBuf;

/// Main server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub service: ServiceConfig,
    pub storage: StorageConfig,
    pub authentication: AuthConfig,
    pub identity: IdentityConfig,
    pub email: Option<EmailConfig>,
    pub invites: InviteConfig,
    pub rate_limit: RateLimitConfig,
    pub logging: LoggingConfig,
    pub federation: FederationConfig,
    pub validation_mode: ValidationMode,
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
    },
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

        let service_did = env::var("PDS_SERVICE_DID")
            .unwrap_or_else(|_| format!("did:web:{}", hostname));
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

        let blobstore = if let Ok(bucket) = env::var("PDS_BLOBSTORE_S3_BUCKET") {
            BlobstoreConfig::S3 {
                bucket,
                region: env::var("PDS_BLOBSTORE_S3_REGION")
                    .unwrap_or_else(|_| "us-east-1".to_string()),
                access_key_id: env::var("PDS_BLOBSTORE_S3_ACCESS_KEY_ID")
                    .map_err(|_| PdsError::Validation("S3 access key required".to_string()))?,
                secret_access_key: env::var("PDS_BLOBSTORE_S3_SECRET_ACCESS_KEY")
                    .map_err(|_| PdsError::Validation("S3 secret key required".to_string()))?,
                endpoint: env::var("PDS_BLOBSTORE_S3_ENDPOINT").ok(),
            }
        } else {
            BlobstoreConfig::Disk {
                location: env::var("PDS_BLOBSTORE_DISK_LOCATION")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| data_directory.join("blobs")),
                tmp_location: env::var("PDS_BLOBSTORE_DISK_TMP_LOCATION")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| data_directory.join("temp")),
            }
        };

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
        let oauth_pds_url = env::var("PDS_OAUTH_PDS_URL")
            .unwrap_or_else(|_| "https://bsky.social".to_string());

        let did_plc_url = env::var("PDS_DID_PLC_URL")
            .unwrap_or_else(|_| "https://plc.directory".to_string());
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
        let invite_epoch = env::var("PDS_INVITE_EPOCH")
            .unwrap_or_else(|_| "2024-01-01T00:00:00Z".to_string());

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
            .and_then(|s| ValidationMode::from_str(&s))
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
