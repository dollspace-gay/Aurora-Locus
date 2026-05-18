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
    /// docs/AURORA_DESIGN.md §5.2 / chainlink #75).
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
    /// Distributed-state substrate mode (Arc 7, V04_DESIGN.md
    /// §6.3.6). Default `Distributed` — Postgres-CAS substrate
    /// for multi-instance correctness. See
    /// [`DistributedStateMode`] for variants.
    #[serde(default)]
    pub distributed_state_mode: DistributedStateMode,
    /// Connection-pool sizing for the substrate's dedicated
    /// maintenance pool (Arc 7, V04_DESIGN.md §6.4.0 Q8b).
    /// Constructed when `distributed_state_mode == Distributed`;
    /// ignored in `SingleInstanceInmemory` mode.
    #[serde(default)]
    pub maintenance_pool: MaintenancePoolConfig,
    /// GC sweep configuration (Arc 10, V04_DESIGN.md §9.4.3).
    /// Off-by-default: `enabled = false` so existing
    /// deployments don't gain a new background task silently.
    /// When enabled, the scheduled `gc_sweep_job` reconciles
    /// blob storage against `blob` / `temp_blob_metadata` and
    /// deletes confirmed orphans subject to the safety cap.
    /// The Arc 10 sweep primitive
    /// ([`crate::blob_store::gc::run_sweep`]) is the consumer.
    #[serde(default)]
    pub gc_sweep: GcSweepConfig,
    /// Entryway-mode configuration per Arc 12 §5.3.9 + §5.4 Step 1.1.
    /// `None` = standalone mode. `Some` = forwarded handlers proxy
    /// to the entryway and OAuth metadata advertises the entryway
    /// as the authorization server. Populated from
    /// `PDS_ENTRYWAY_*` env vars (all-or-nothing per §5.4 Step 1.2).
    /// Marked `#[serde(skip)]` because the parsed `VerifyingKey`
    /// is not round-trippable through standard serde derives —
    /// env-var loading is the sole construction path.
    #[serde(skip)]
    pub entryway: Option<EntrywayConfig>,
}

/// Distributed-state substrate selector (Arc 7, V04_DESIGN.md
/// §6.3.6 amended). Controls which backing store the
/// `DistributedStore` trait is wired against at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DistributedStateMode {
    /// Postgres-CAS substrate (the v0.4 default). Required for
    /// multi-instance correctness; works on single-instance
    /// deployments too at the cost of a few extra Postgres
    /// roundtrips per request.
    #[default]
    Distributed,
    /// In-process state only. Auth state is lost on restart.
    /// Operator-confirmed opt-in for single-instance
    /// deployments that want the perf of in-memory state and
    /// accept the durability trade-off.
    SingleInstanceInmemory,
    /// Forward-compat slot for a Redis backend. Not implemented
    /// in v0.4 — selecting this fails at startup with a clear
    /// error. Kept as an enum variant so the config surface is
    /// stable across cycles even before the backend ships.
    Redis,
}

impl DistributedStateMode {
    /// Parse from an env-var value with the same case-insensitive
    /// + aliased-form pattern `DatabaseBackend::from_env_values`
    ///   uses. Returns an error naming the valid options on
    ///   unrecognised input so operator typos surface
    ///   actionably.
    pub fn from_env_value(s: &str) -> PdsResult<Self> {
        match s.to_ascii_lowercase().as_str() {
            "distributed" => Ok(Self::Distributed),
            "single_instance_inmemory" => Ok(Self::SingleInstanceInmemory),
            "redis" => Ok(Self::Redis),
            other => Err(PdsError::Validation(format!(
                "PDS_DISTRIBUTED_STATE_MODE must be one of \
                 'distributed', 'single_instance_inmemory', \
                 'redis' (got: {:?})",
                other
            ))),
        }
    }
}

/// Connection-pool sizing for the substrate's dedicated
/// maintenance pool. Defaults sized for typical multi-instance
/// deployments (Step 0 Q8 recon recommendation): smaller than
/// the main pool to keep total Postgres connection count
/// predictable, with a faster acquire timeout so DPoP / rate-
/// limit hot paths fail fast under contention rather than
/// blocking the request thread.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaintenancePoolConfig {
    pub max_connections: u32,
    pub min_connections: u32,
    pub acquire_timeout_secs: u64,
}

impl Default for MaintenancePoolConfig {
    fn default() -> Self {
        Self {
            max_connections: 15,
            min_connections: 2,
            acquire_timeout_secs: 10,
        }
    }
}

impl MaintenancePoolConfig {
    /// Construct from explicit option-typed env-var values.
    /// Mirrors the pattern in `DatabaseConfig::from_env_values`.
    pub fn from_env_values(
        max_connections: Option<String>,
        min_connections: Option<String>,
        acquire_timeout_secs: Option<String>,
    ) -> PdsResult<Self> {
        let defaults = Self::default();
        let max_connections =
            parse_u32_env("PDS_MAINTENANCE_DB_MAX_CONNECTIONS", max_connections, defaults.max_connections)?;
        let min_connections =
            parse_u32_env("PDS_MAINTENANCE_DB_MIN_CONNECTIONS", min_connections, defaults.min_connections)?;
        let acquire_timeout_secs = parse_u64_env(
            "PDS_MAINTENANCE_DB_ACQUIRE_TIMEOUT_SECS",
            acquire_timeout_secs,
            defaults.acquire_timeout_secs,
        )?;
        if max_connections == 0 {
            return Err(PdsError::Validation(
                "PDS_MAINTENANCE_DB_MAX_CONNECTIONS must be greater than 0".to_string(),
            ));
        }
        if min_connections > max_connections {
            return Err(PdsError::Validation(format!(
                "PDS_MAINTENANCE_DB_MIN_CONNECTIONS ({}) must not exceed \
                 PDS_MAINTENANCE_DB_MAX_CONNECTIONS ({})",
                min_connections, max_connections
            )));
        }
        if acquire_timeout_secs == 0 {
            return Err(PdsError::Validation(
                "PDS_MAINTENANCE_DB_ACQUIRE_TIMEOUT_SECS must be greater than 0".to_string(),
            ));
        }
        Ok(Self {
            max_connections,
            min_connections,
            acquire_timeout_secs,
        })
    }
}

/// GC sweep configuration (Arc 10, V04_DESIGN.md §9.4.3 /
/// chainlink #57). Controls the scheduled background sweep
/// that reconciles blob storage against the `blob` /
/// `temp_blob_metadata` tables and deletes confirmed orphans
/// subject to the safety cap. The Arc 10 sweep primitive
/// ([`crate::blob_store::gc::run_sweep`]) is the consumer.
///
/// Off-by-default (`enabled = false`) so v0.4 ships without
/// adding a new background task to existing deployments.
/// Operators opt in by setting `PDS_GC_SWEEP_ENABLED=true`.
/// `dry_run` defaults to `true` — the first runs are
/// classify-and-log so operators can observe orphan rates
/// before promoting to destructive mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GcSweepConfig {
    /// If `true`, the scheduled background sweep runs at
    /// `interval_secs` cadence. Default `false` — operators
    /// opt in explicitly per the v0.4 design.
    #[serde(default)]
    pub enabled: bool,

    /// Cadence between scheduled sweep runs, in seconds.
    /// Default 86400 (24 hours).
    #[serde(default = "default_gc_sweep_interval_secs")]
    pub interval_secs: u64,

    /// If `true`, the sweep classifies and logs but does not
    /// delete. Default `true` — operators promote to
    /// destructive mode only after observing the report
    /// cadence in production.
    #[serde(default = "default_gc_sweep_dry_run")]
    pub dry_run: bool,

    /// Safety cap: max blobs to delete per sweep run.
    /// Default 10000. Confirmed orphans beyond the cap are
    /// logged and deferred to the next run.
    #[serde(default = "default_gc_sweep_max_deletes")]
    pub max_deletes_per_run: usize,

    /// Belt-and-braces freshness threshold in seconds. Blobs
    /// younger than this are never classified as orphans, even
    /// when absent from `temp_blob_metadata`. Default 3600
    /// (1 hour) per Step 0 Q9's analysis: the tracking surface
    /// is authoritative; this threshold catches the rare race
    /// where a `temp_blob_metadata` row hasn't committed yet.
    #[serde(default = "default_gc_sweep_threshold_secs")]
    pub freshness_threshold_secs: u64,

    /// Storage page size for the sweep's pagination. Default
    /// 500 — Step 1 benchmark validated this stays index-driven
    /// on SQLite at 100k seeded rows (6.98ms / well under the
    /// 50ms threshold).
    #[serde(default = "default_gc_sweep_page_size")]
    pub page_size: usize,
}

impl Default for GcSweepConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_secs: default_gc_sweep_interval_secs(),
            dry_run: default_gc_sweep_dry_run(),
            max_deletes_per_run: default_gc_sweep_max_deletes(),
            freshness_threshold_secs: default_gc_sweep_threshold_secs(),
            page_size: default_gc_sweep_page_size(),
        }
    }
}

fn default_gc_sweep_interval_secs() -> u64 {
    86_400
}
fn default_gc_sweep_dry_run() -> bool {
    true
}
fn default_gc_sweep_max_deletes() -> usize {
    10_000
}
fn default_gc_sweep_threshold_secs() -> u64 {
    3_600
}
fn default_gc_sweep_page_size() -> usize {
    500
}

impl GcSweepConfig {
    /// Construct from explicit option-typed env-var values.
    /// Mirrors the pattern in `MaintenancePoolConfig::from_env_values`.
    pub fn from_env_values(
        enabled: Option<String>,
        interval_secs: Option<String>,
        dry_run: Option<String>,
        max_deletes_per_run: Option<String>,
        freshness_threshold_secs: Option<String>,
        page_size: Option<String>,
    ) -> PdsResult<Self> {
        let defaults = Self::default();
        let enabled = parse_bool_env("PDS_GC_SWEEP_ENABLED", enabled, defaults.enabled)?;
        let interval_secs = parse_u64_env(
            "PDS_GC_SWEEP_INTERVAL_SECS",
            interval_secs,
            defaults.interval_secs,
        )?;
        let dry_run = parse_bool_env("PDS_GC_SWEEP_DRY_RUN", dry_run, defaults.dry_run)?;
        let max_deletes_per_run = parse_usize_env(
            "PDS_GC_SWEEP_MAX_DELETES_PER_RUN",
            max_deletes_per_run,
            defaults.max_deletes_per_run,
        )?;
        let freshness_threshold_secs = parse_u64_env(
            "PDS_GC_SWEEP_FRESHNESS_THRESHOLD_SECS",
            freshness_threshold_secs,
            defaults.freshness_threshold_secs,
        )?;
        let page_size =
            parse_usize_env("PDS_GC_SWEEP_PAGE_SIZE", page_size, defaults.page_size)?;

        if interval_secs == 0 {
            return Err(PdsError::Validation(
                "PDS_GC_SWEEP_INTERVAL_SECS must be greater than 0".to_string(),
            ));
        }
        if page_size == 0 {
            return Err(PdsError::Validation(
                "PDS_GC_SWEEP_PAGE_SIZE must be greater than 0".to_string(),
            ));
        }

        Ok(Self {
            enabled,
            interval_secs,
            dry_run,
            max_deletes_per_run,
            freshness_threshold_secs,
            page_size,
        })
    }

    /// Convert to [`crate::blob_store::gc::SweepParams`] for
    /// the GC sweep primitive. `report_only` is supplied by
    /// the caller — `false` from the scheduled-job path,
    /// driven by the `--report-only` flag from the CLI path.
    pub fn to_sweep_params(
        &self,
        report_only: bool,
    ) -> crate::blob_store::gc::SweepParams {
        crate::blob_store::gc::SweepParams {
            dry_run: self.dry_run,
            report_only,
            max_deletes_per_run: self.max_deletes_per_run,
            freshness_threshold: std::time::Duration::from_secs(self.freshness_threshold_secs),
            page_size: self.page_size,
        }
    }
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
    /// docs/AURORA_DESIGN.md §5.4.1.
    pub leader_retry_interval_ms: u64,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            backend: DatabaseBackend::Sqlite,
            url: None,
            // Defaults per docs/AURORA_DESIGN.md §5.3 (connection model).
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

fn parse_usize_env(name: &str, raw: Option<String>, default: usize) -> PdsResult<usize> {
    match raw {
        None => Ok(default),
        Some(v) => v.parse::<usize>().map_err(|_| {
            PdsError::Validation(format!(
                "{name} must be a non-negative integer (got: {:?})",
                v
            ))
        }),
    }
}

fn parse_bool_env(name: &str, raw: Option<String>, default: bool) -> PdsResult<bool> {
    match raw {
        None => Ok(default),
        Some(v) => v.parse::<bool>().map_err(|_| {
            PdsError::Validation(format!(
                "{name} must be 'true' or 'false' (got: {:?})",
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
    /// Externally-reachable public URL. Arc 12 §5.3.2 Gap 1
    /// closure: when set (typically via `PDS_SERVICE_PUBLIC_URL`),
    /// every site that needs the self-PDS public URL reads
    /// this. When unset, the URL is derived from
    /// `derive_url_scheme(hostname)` + hostname + port —
    /// preserving v0.4 backward compatibility for deployments
    /// that don't set the env var. `https` is the default for
    /// non-localhost hostnames; `http` for localhost / 127.x.x.x
    /// / 0.0.0.0.
    #[serde(default)]
    pub public_url: Option<String>,
}

impl ServiceConfig {
    /// Effective public URL per Arc 12 §5.3.2 Gap 1.
    ///
    /// Returns `self.public_url` when set; otherwise derives
    /// `{scheme}://{hostname}[:{port}]` with scheme picked by
    /// `derive_url_scheme(hostname)`. Standard ports (80 for
    /// http, 443 for https) are omitted from the derived form.
    #[must_use]
    pub fn effective_public_url(&self) -> String {
        if let Some(url) = &self.public_url {
            return url.clone();
        }
        let scheme = derive_url_scheme(&self.hostname);
        let port_is_standard = (scheme == "http" && self.port == 80)
            || (scheme == "https" && self.port == 443);
        if port_is_standard {
            format!("{}://{}", scheme, self.hostname)
        } else {
            format!("{}://{}:{}", scheme, self.hostname, self.port)
        }
    }
}

/// Pick `http` for localhost-shaped hosts; `https` otherwise.
///
/// Arc 12 §5.3.2 Gap 1 helper. Used by `ServiceConfig::effective_public_url`
/// for self-URL derivation, and by remote-PDS-URL formatters
/// (`src/federation/discovery.rs`, `src/identity/resolver.rs`)
/// for peer-URL scheme selection.
#[must_use]
pub fn derive_url_scheme(host: &str) -> &'static str {
    // `host` may be hostname-only or hostname:port; strip the port
    // before classifying.
    let host_only = host.split(':').next().unwrap_or(host);
    if host_only == "localhost"
        || host_only == "0.0.0.0"
        || host_only.starts_with("127.")
    {
        "http"
    } else {
        "https"
    }
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
    /// Bypass the limiter for GET requests to admin UI static assets.
    /// Defaults to `true`; see `crate::rate_limit::is_admin_asset_exempt`
    /// for the exact path/method matrix. Set to `false` to opt admin
    /// assets back into the limiter.
    pub exempt_admin_assets: bool,
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
    /// Trusted peer PDS list (Arc 12 §5.3.2 Gap 2 + §5.3.7
    /// env-var parser). Populated from
    /// `PDS_FEDERATION_PEER_PDS=did1@url1,did2@url2,...` at
    /// startup; consumed by Arc 12 §5.3.3.1 trusted-iss
    /// allowlist and Arc 12 §5.3.2 Gap 3 `PdsDiscovery`
    /// bootstrap. Empty Vec when env var unset.
    #[serde(default)]
    pub peer_pds: Vec<PeerPdsConfig>,
}

/// Trusted peer-PDS entry parsed from
/// `PDS_FEDERATION_PEER_PDS`. Format per Arc 12 §5.3.7:
/// `did@url`. Malformed entries reject at startup per
/// §5.4 Step 1.2 all-or-nothing discipline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerPdsConfig {
    pub did: String,
    pub url: String,
}

impl PeerPdsConfig {
    /// Parse a single `did@url` entry. Returns `Err` with a
    /// descriptive message naming the offending input on
    /// any of: missing `@`, empty did, empty url, non-DID
    /// `did` (no `did:` prefix), non-URL `url` (no `http://`
    /// or `https://` prefix).
    pub fn parse_entry(entry: &str) -> PdsResult<Self> {
        let (did, url) = entry.split_once('@').ok_or_else(|| {
            PdsError::Validation(format!(
                "PDS_FEDERATION_PEER_PDS entry missing '@' separator: {:?}",
                entry
            ))
        })?;
        let did = did.trim();
        let url = url.trim();
        if did.is_empty() {
            return Err(PdsError::Validation(format!(
                "PDS_FEDERATION_PEER_PDS entry has empty did: {:?}",
                entry
            )));
        }
        if url.is_empty() {
            return Err(PdsError::Validation(format!(
                "PDS_FEDERATION_PEER_PDS entry has empty url: {:?}",
                entry
            )));
        }
        if !did.starts_with("did:") {
            return Err(PdsError::Validation(format!(
                "PDS_FEDERATION_PEER_PDS entry did missing 'did:' prefix: {:?}",
                entry
            )));
        }
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(PdsError::Validation(format!(
                "PDS_FEDERATION_PEER_PDS entry url must start with http:// or https://: {:?}",
                entry
            )));
        }
        Ok(PeerPdsConfig {
            did: did.to_string(),
            url: url.to_string(),
        })
    }

    /// Parse a comma-separated list per Arc 12 §5.3.7
    /// (`did1@url1,did2@url2,...`). Empty input → empty Vec.
    /// Any malformed entry → Err with the offending entry
    /// named.
    pub fn parse_list(raw: &str) -> PdsResult<Vec<Self>> {
        if raw.trim().is_empty() {
            return Ok(Vec::new());
        }
        raw.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(Self::parse_entry)
            .collect()
    }
}

/// Entryway-mode configuration (Arc 12 §5.3.9 + §5.4 Step 1.1).
///
/// When `Some`, Aurora-Locus runs in entryway-attached mode:
/// forwarded handlers proxy to the entryway, the OAuth
/// protected-resource metadata advertises the entryway as the
/// authorization server, and the §5.3.3 routing's "ES256K + known
/// entryway kid" row is enabled. When `None`, Aurora-Locus runs
/// in standalone mode (default).
///
/// All four `PDS_ENTRYWAY_*` env vars are required together
/// (all-or-nothing per §5.4 Step 1.2). Partial config (any subset)
/// is rejected at startup with a clear error naming the missing
/// variable(s).
///
/// The `jwt_public_key` field holds the entryway's ES256K JWT-signing
/// public key as a parsed `k256::ecdsa::VerifyingKey`, decoded once
/// at env-load time from `PDS_ENTRYWAY_JWT_PUBLIC_KEY_HEX`. The
/// hex form (33 bytes SEC1-compressed, 66 hex chars) is the wire
/// format Aurora-Locus shares with the entryway operator.
#[derive(Debug, Clone)]
pub struct EntrywayConfig {
    /// Base URL of the entryway (e.g., `https://entryway.example.com`).
    /// Populated from `PDS_ENTRYWAY_URL`.
    pub url: String,
    /// Admin Basic-auth token, pre-bound on `entryway_admin_client`
    /// per §5.3.9. Populated from `PDS_ENTRYWAY_ADMIN_TOKEN`.
    pub admin_token: String,
    /// Entryway's ES256K JWT-signing public key, parsed at startup
    /// from `PDS_ENTRYWAY_JWT_PUBLIC_KEY_HEX`. Consumed by
    /// `validate_external_access_token` (§5.3.3) when the routing
    /// table's "ES256K + known entryway kid" row fires.
    pub jwt_public_key: k256::ecdsa::VerifyingKey,
    /// Entryway DID. Populated from `PDS_ENTRYWAY_DID`. Joins the
    /// trusted-iss allowlist (§5.3.3.1) and becomes an accepted
    /// audience for `require_auth_forwarded` (§5.3.4) routes.
    pub did: String,
}

impl EntrywayConfig {
    /// Parse from env-var values. Implements the all-or-nothing
    /// rule per §5.4 Step 1.2:
    ///
    /// - All four `Some` → returns `Ok(Some(EntrywayConfig{..}))`
    ///   if every value validates; rejection otherwise names the
    ///   offending input.
    /// - All four `None` → returns `Ok(None)` (standalone mode).
    /// - Any other subset → returns `Err(PdsError::Validation(..))`
    ///   identifying which variable(s) are missing.
    ///
    /// Each input is the env-var value (`env::var(..).ok()`).
    pub fn from_env_values(
        url: Option<String>,
        admin_token: Option<String>,
        jwt_public_key_hex: Option<String>,
        did: Option<String>,
    ) -> PdsResult<Option<Self>> {
        let any_set = url.is_some()
            || admin_token.is_some()
            || jwt_public_key_hex.is_some()
            || did.is_some();
        if !any_set {
            return Ok(None);
        }

        let mut missing: Vec<&'static str> = Vec::new();
        if url.is_none() {
            missing.push("PDS_ENTRYWAY_URL");
        }
        if admin_token.is_none() {
            missing.push("PDS_ENTRYWAY_ADMIN_TOKEN");
        }
        if jwt_public_key_hex.is_none() {
            missing.push("PDS_ENTRYWAY_JWT_PUBLIC_KEY_HEX");
        }
        if did.is_none() {
            missing.push("PDS_ENTRYWAY_DID");
        }
        if !missing.is_empty() {
            return Err(PdsError::Validation(format!(
                "EntrywayConfig is partially specified \
                 (PDS_ENTRYWAY_* are all-or-nothing per §5.4 Step 1.2); \
                 missing: {}",
                missing.join(", ")
            )));
        }

        let url = url.expect("checked Some above");
        let admin_token = admin_token.expect("checked Some above");
        let jwt_public_key_hex = jwt_public_key_hex.expect("checked Some above");
        let did = did.expect("checked Some above");

        if url.is_empty() {
            return Err(PdsError::Validation(
                "PDS_ENTRYWAY_URL must not be empty".to_string(),
            ));
        }
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(PdsError::Validation(format!(
                "PDS_ENTRYWAY_URL must start with http:// or https:// — got {:?}",
                url
            )));
        }
        if admin_token.is_empty() {
            return Err(PdsError::Validation(
                "PDS_ENTRYWAY_ADMIN_TOKEN must not be empty".to_string(),
            ));
        }
        if !did.starts_with("did:") {
            return Err(PdsError::Validation(format!(
                "PDS_ENTRYWAY_DID must be a DID (start with 'did:') — got {:?}",
                did
            )));
        }

        let key_bytes = hex::decode(&jwt_public_key_hex).map_err(|e| {
            PdsError::Validation(format!(
                "PDS_ENTRYWAY_JWT_PUBLIC_KEY_HEX is not valid hex: {}",
                e
            ))
        })?;
        let jwt_public_key =
            k256::ecdsa::VerifyingKey::from_sec1_bytes(&key_bytes).map_err(|e| {
                PdsError::Validation(format!(
                    "PDS_ENTRYWAY_JWT_PUBLIC_KEY_HEX is not a valid \
                     SEC1-encoded k256 public key: {}",
                    e
                ))
            })?;

        Ok(Some(EntrywayConfig {
            url,
            admin_token,
            jwt_public_key,
            did,
        }))
    }
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

        // OAuth configuration for admin login.
        // Arc 12 §5.3.2 Gap 1: classified (C) in recon but promoted
        // to (M) — these defaults baked hostname directly without
        // routing through service_url(). Use derive_url_scheme()
        // for localhost-aware scheme so localhost deployments
        // don't default to https.
        let oauth_default_scheme = derive_url_scheme(&hostname);
        let oauth_client_id = env::var("PDS_OAUTH_CLIENT_ID").unwrap_or_else(|_| {
            format!(
                "{}://{}/oauth/client-metadata.json",
                oauth_default_scheme, hostname
            )
        });
        let oauth_redirect_uri = env::var("PDS_OAUTH_REDIRECT_URI").unwrap_or_else(|_| {
            format!(
                "{}://{}/admin-oauth/callback",
                oauth_default_scheme, hostname
            )
        });
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
        // PDS_RATE_LIMIT_USE_REDIS / PDS_RATE_LIMIT_REDIS_URL
        // were inputs to the now-retired rate_limit_new module
        // (Arc 7 Step 1 disposition). Reading them here would
        // mislead operators into thinking they still affect
        // anything; the substrate's Redis hook is reserved for
        // a future cycle under DistributedStateMode::Redis.
        let rate_limit_exempt_admin_assets = env::var("PDS_RATE_LIMIT_EXEMPT_ADMIN_ASSETS")
            .unwrap_or_else(|_| "true".to_string())
            .parse()
            .unwrap_or(true);

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
        // Empty-entry filter is load-bearing: `AppContext::new` gates
        // RelayClient construction (and JobScheduler's
        // relay_firehose_subscription_job spawn) on
        // `!relay_urls.is_empty()`. Setting `PDS_FEDERATION_RELAY_URLS=""`
        // is the explicit "federation on for peer_pds + entryway forwarding,
        // but no relay loop" override; without this filter, `""` would
        // collect to `vec![""]` (length 1) and spawn a connect loop
        // against an empty URL.
        let relay_urls: Vec<String> = env::var("PDS_FEDERATION_RELAY_URLS")
            .unwrap_or_else(|_| "https://bsky.network".to_string())
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
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

        // Arc 12 §5.3.2 Gap 2 + §5.3.7: peer-PDS env-var parser
        // with all-or-nothing validation per §5.4 Step 1.2.
        let peer_pds = match env::var("PDS_FEDERATION_PEER_PDS") {
            Ok(raw) => PeerPdsConfig::parse_list(&raw)?,
            Err(_) => Vec::new(),
        };

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

        let distributed_state_mode = match env::var("PDS_DISTRIBUTED_STATE_MODE") {
            Ok(s) => DistributedStateMode::from_env_value(&s)?,
            Err(_) => DistributedStateMode::default(),
        };

        let maintenance_pool = MaintenancePoolConfig::from_env_values(
            env::var("PDS_MAINTENANCE_DB_MAX_CONNECTIONS").ok(),
            env::var("PDS_MAINTENANCE_DB_MIN_CONNECTIONS").ok(),
            env::var("PDS_MAINTENANCE_DB_ACQUIRE_TIMEOUT_SECS").ok(),
        )?;

        let gc_sweep = GcSweepConfig::from_env_values(
            env::var("PDS_GC_SWEEP_ENABLED").ok(),
            env::var("PDS_GC_SWEEP_INTERVAL_SECS").ok(),
            env::var("PDS_GC_SWEEP_DRY_RUN").ok(),
            env::var("PDS_GC_SWEEP_MAX_DELETES_PER_RUN").ok(),
            env::var("PDS_GC_SWEEP_FRESHNESS_THRESHOLD_SECS").ok(),
            env::var("PDS_GC_SWEEP_PAGE_SIZE").ok(),
        )?;

        // Arc 12 §5.3.2 Gap 1: public_url override via env var.
        // Renamed locally to avoid shadowing federation's
        // existing `public_url` binding (different env var,
        // different field, same name).
        let service_public_url = env::var("PDS_SERVICE_PUBLIC_URL").ok();

        // Arc 12 §5.4 Step 1.1 + 1.2: EntrywayConfig from
        // `PDS_ENTRYWAY_*` env vars, all-or-nothing.
        let entryway = EntrywayConfig::from_env_values(
            env::var("PDS_ENTRYWAY_URL").ok(),
            env::var("PDS_ENTRYWAY_ADMIN_TOKEN").ok(),
            env::var("PDS_ENTRYWAY_JWT_PUBLIC_KEY_HEX").ok(),
            env::var("PDS_ENTRYWAY_DID").ok(),
        )?;

        Ok(ServerConfig {
            service: ServiceConfig {
                hostname,
                port,
                service_did,
                version,
                blob_upload_limit,
                public_url: service_public_url,
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
                exempt_admin_assets: rate_limit_exempt_admin_assets,
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
                peer_pds,
            },
            validation_mode,
            distributed_state_mode,
            maintenance_pool,
            gc_sweep,
            entryway,
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

        // Redis substrate is a forward-compat slot in the enum
        // but not implemented in v0.4. Fail-fast at startup so
        // an operator selecting it gets a clear error instead of
        // silently falling through to default behaviour.
        if matches!(self.distributed_state_mode, DistributedStateMode::Redis) {
            return Err(PdsError::Validation(
                "PDS_DISTRIBUTED_STATE_MODE=redis is not implemented in v0.4; \
                 use 'distributed' (default) or 'single_instance_inmemory'"
                    .to_string(),
            ));
        }

        // Distributed mode needs Postgres — SQLite is single-
        // instance by definition and the maintenance pool would
        // operate against the same DB as the application pool
        // with no isolation benefit. Surface the mismatch
        // explicitly rather than producing a confusing runtime
        // behavior; operators on SQLite who want the substrate
        // can either switch to Postgres or run in
        // SingleInstanceInmemory mode.
        if matches!(
            self.distributed_state_mode,
            DistributedStateMode::Distributed
        ) && matches!(self.database.backend, DatabaseBackend::Sqlite)
        {
            // Note: this is a warning rather than a hard error
            // because single-instance SQLite deployments may
            // legitimately want the substrate tables present
            // (for tooling, future migration). Log loudly but
            // don't refuse startup. Production operators
            // generally combine PDS_DB_BACKEND=postgres with
            // PDS_DISTRIBUTED_STATE_MODE=distributed; mismatches
            // mostly come from incomplete env-var copies during
            // upgrade.
            tracing::warn!(
                "PDS_DISTRIBUTED_STATE_MODE=distributed combined with \
                 PDS_DB_BACKEND=sqlite — distributed substrate operates \
                 against the same SQLite database as the application pool; \
                 no multi-instance benefit. Consider Postgres or \
                 PDS_DISTRIBUTED_STATE_MODE=single_instance_inmemory."
            );
        }

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

#[cfg(test)]
mod gc_sweep_tests {
    use super::*;

    #[test]
    fn default_matches_v04_design_safety_stance() {
        let c = GcSweepConfig::default();
        assert!(!c.enabled, "default must be disabled");
        assert!(c.dry_run, "default must be dry_run=true");
        assert_eq!(c.interval_secs, 86_400);
        assert_eq!(c.max_deletes_per_run, 10_000);
        assert_eq!(c.freshness_threshold_secs, 3_600);
        assert_eq!(c.page_size, 500);
    }

    #[test]
    fn from_env_values_all_none_yields_default() {
        let c = GcSweepConfig::from_env_values(None, None, None, None, None, None).unwrap();
        assert_eq!(c.enabled, GcSweepConfig::default().enabled);
        assert_eq!(c.dry_run, GcSweepConfig::default().dry_run);
        assert_eq!(c.interval_secs, GcSweepConfig::default().interval_secs);
        assert_eq!(c.max_deletes_per_run, GcSweepConfig::default().max_deletes_per_run);
        assert_eq!(
            c.freshness_threshold_secs,
            GcSweepConfig::default().freshness_threshold_secs
        );
        assert_eq!(c.page_size, GcSweepConfig::default().page_size);
    }

    #[test]
    fn from_env_values_parses_all_fields() {
        let c = GcSweepConfig::from_env_values(
            Some("true".to_string()),
            Some("3600".to_string()),
            Some("false".to_string()),
            Some("500".to_string()),
            Some("1800".to_string()),
            Some("250".to_string()),
        )
        .unwrap();
        assert!(c.enabled);
        assert_eq!(c.interval_secs, 3_600);
        assert!(!c.dry_run);
        assert_eq!(c.max_deletes_per_run, 500);
        assert_eq!(c.freshness_threshold_secs, 1_800);
        assert_eq!(c.page_size, 250);
    }

    #[test]
    fn from_env_values_zero_interval_rejected() {
        let err = GcSweepConfig::from_env_values(
            Some("true".to_string()),
            Some("0".to_string()),
            None,
            None,
            None,
            None,
        )
        .expect_err("zero interval must be rejected");
        assert!(err.to_string().contains("PDS_GC_SWEEP_INTERVAL_SECS"));
    }

    #[test]
    fn from_env_values_zero_page_size_rejected() {
        let err = GcSweepConfig::from_env_values(
            None,
            None,
            None,
            None,
            None,
            Some("0".to_string()),
        )
        .expect_err("zero page size must be rejected");
        assert!(err.to_string().contains("PDS_GC_SWEEP_PAGE_SIZE"));
    }

    #[test]
    fn from_env_values_invalid_bool_rejected() {
        let err = GcSweepConfig::from_env_values(
            Some("yes".to_string()),
            None,
            None,
            None,
            None,
            None,
        )
        .expect_err("'yes' is not a valid bool");
        assert!(err.to_string().contains("PDS_GC_SWEEP_ENABLED"));
    }

    #[test]
    fn to_sweep_params_propagates_fields() {
        let cfg = GcSweepConfig {
            enabled: true,
            interval_secs: 7200,
            dry_run: false,
            max_deletes_per_run: 250,
            freshness_threshold_secs: 600,
            page_size: 100,
        };
        let p = cfg.to_sweep_params(true);
        assert!(!p.dry_run);
        assert!(p.report_only);
        assert_eq!(p.max_deletes_per_run, 250);
        assert_eq!(p.freshness_threshold, std::time::Duration::from_secs(600));
        assert_eq!(p.page_size, 100);
    }

    #[test]
    fn to_sweep_params_report_only_false_passes_through() {
        let cfg = GcSweepConfig::default();
        let p = cfg.to_sweep_params(false);
        assert!(!p.report_only);
        assert!(p.dry_run, "config dry_run propagates to params");
    }
}

#[cfg(test)]
mod relay_urls_parse_tests {
    //! Inline parser-shape tests for `PDS_FEDERATION_RELAY_URLS`.
    //! The filter is load-bearing: `AppContext::new`
    //! gates `RelayClient` construction (and its background
    //! `JobScheduler::relay_firehose_subscription_job` spawn) on
    //! `!relay_urls.is_empty()`. Setting the env var to `""` is the
    //! explicit "federation on, no relay loop" override.

    /// Mirror the exact parse pipeline used in `ServerConfig::from_env`
    /// so this test verifies the contract by-shape rather than by
    /// poking process-global env vars (which races with parallel
    /// tests).
    fn parse(raw: &str) -> Vec<String> {
        raw.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    #[test]
    fn empty_string_yields_empty_vec_so_relay_client_is_none() {
        assert_eq!(parse(""), Vec::<String>::new());
    }

    #[test]
    fn whitespace_only_yields_empty_vec() {
        assert_eq!(parse("   "), Vec::<String>::new());
    }

    #[test]
    fn commas_with_no_entries_yield_empty_vec() {
        assert_eq!(parse(",,, ,"), Vec::<String>::new());
    }

    #[test]
    fn single_url_default_shape() {
        assert_eq!(parse("https://bsky.network"), vec!["https://bsky.network"]);
    }

    #[test]
    fn multi_url_comma_split() {
        assert_eq!(
            parse("https://a.example,https://b.example"),
            vec!["https://a.example", "https://b.example"]
        );
    }

    #[test]
    fn empty_entries_between_real_urls_are_dropped() {
        assert_eq!(
            parse("https://a,,https://b,"),
            vec!["https://a", "https://b"]
        );
    }

    #[test]
    fn whitespace_around_entries_is_trimmed() {
        assert_eq!(
            parse("  https://a  ,  https://b  "),
            vec!["https://a", "https://b"]
        );
    }
}
