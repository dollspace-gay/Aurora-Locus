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
    /// v0.8 arc 1 (#180) — bind-audit orphan-marker reconciliation.
    /// On by default; the scheduled sweep flips persisted orphan
    /// markers to their terminal state. Interval env override:
    /// `PDS_BIND_AUDIT_ORPHAN_RECONCILE_INTERVAL_SECS`.
    #[serde(default)]
    pub bind_audit_orphan_marker: BindAuditOrphanMarkerConfig,
    /// Arc 16c §9.3.4 Step 1 — blob lifecycle config.
    /// `stage_ttl_seconds` controls the temp_blob_metadata reaper's
    /// TTL window (product knob: how long a client has to commit
    /// an upload to a record). Separate from
    /// `gc_sweep.freshness_threshold_secs` (substrate-safety bound,
    /// in-flight-upload detection) per recon Step 0.5 precedent.
    /// Default 86400 = 24h, matching bsky-PDS tempKey TTL parity.
    /// Env override: `PDS_BLOB_STAGE_TTL_SECONDS`.
    #[serde(default)]
    pub blob_metadata: BlobMetadataConfig,
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
    /// Arc 17 §17.3 dynamic-lexicon-loading config. Off-by-default
    /// (`enabled: false`) per §17.5 friction-risk posture — operators
    /// opt in via `PDS_LEXICON_ENABLED=true`. See [`LexiconConfig`].
    #[serde(default)]
    pub lexicon: LexiconConfig,
    /// Kryphocron substrate integration. On by default as of v0.9
    /// (Aurora-Locus ships as "the kryphocron PDS"); set
    /// `PDS_KRYPHOCRON_ENABLED=false` to opt out. When enabled, kryphocron
    /// lexicons are validated against `kryphocron::lexicons()` at startup,
    /// the `tools.kryphocron.*` namespace becomes closed (no dynamic-resolver
    /// fall-through), Aurora-Locus's dedicated kryphocron endpoints are
    /// reachable, and the operator admin surfaces (Overview, Audiences,
    /// Laquna, Tier Activity) render live. See [`KryphocronConfig`].
    #[serde(default)]
    pub kryphocron: KryphocronConfig,
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
    /// `interval_secs` cadence. **Default `true` post-Arc-16d**
    /// (V05_DESIGN.md §9.4.1.3 / §9.4.4 Step 1.3): Arc 16d ships
    /// production-grade row-driven GC; both walkers are on by
    /// default and operators opt OUT explicitly. Pre-Arc-16d
    /// (Arc 10 only), this defaulted to `false`.
    #[serde(default = "default_gc_sweep_enabled")]
    pub enabled: bool,

    /// Arc 16d row-walker on/off (V05_DESIGN.md §9.4.2.1). When
    /// `true` the `row_sweep_job` orchestrates
    /// `sweep_untethered_rows`. Default `true`; cadence shared
    /// with byte-walker via `interval_secs`.
    #[serde(default = "default_gc_sweep_row_sweep_enabled")]
    pub row_sweep_enabled: bool,

    /// Cadence between scheduled sweep runs, in seconds.
    /// Default 86400 (24 hours). **Shared between byte-walker
    /// and row-walker** per V05_DESIGN.md §9.4.2.1.
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
    /// logged and deferred to the next run. **Shared between
    /// byte-walker and row-walker** per V05_DESIGN.md §9.4.2.1.
    #[serde(default = "default_gc_sweep_max_deletes")]
    pub max_deletes_per_run: usize,

    /// Belt-and-braces freshness threshold in seconds. Blobs
    /// younger than this are never classified as orphans, even
    /// when absent from `temp_blob_metadata`. Default 3600
    /// (1 hour) per Step 0 Q9's analysis: the tracking surface
    /// is authoritative; this threshold catches the rare race
    /// where a `temp_blob_metadata` row hasn't committed yet.
    /// **Arc 10 classifier knob, UNCHANGED by Arc 16d** (the
    /// row-walker uses `untethered_ttl_seconds` instead).
    #[serde(default = "default_gc_sweep_threshold_secs")]
    pub freshness_threshold_secs: u64,

    /// Storage page size for the sweep's pagination. Default
    /// 500 — Step 1 benchmark validated this stays index-driven
    /// on SQLite at 100k seeded rows (6.98ms / well under the
    /// 50ms threshold). **Shared between byte-walker and
    /// row-walker** per V05_DESIGN.md §9.4.2.1.
    #[serde(default = "default_gc_sweep_page_size")]
    pub page_size: usize,

    /// Arc 16d row-sweep TTL anchor in seconds. Untethered
    /// `blob_metadata` rows (`temp_key IS NOT NULL`) older than
    /// `untethered_ttl_seconds` are eligible for sweep DELETE +
    /// bytes-delete. Default 86400 (24h, matching the bsky-PDS
    /// tempKey TTL parity that Arc 16c's `stage_ttl_seconds`
    /// established for the adjacent staging surface). Per
    /// V05_DESIGN.md §9.4.2.1 / §9.4.4 Step 1.1.
    #[serde(default = "default_gc_sweep_untethered_ttl_secs")]
    pub untethered_ttl_seconds: u64,
}

impl Default for GcSweepConfig {
    fn default() -> Self {
        Self {
            enabled: default_gc_sweep_enabled(),
            row_sweep_enabled: default_gc_sweep_row_sweep_enabled(),
            interval_secs: default_gc_sweep_interval_secs(),
            dry_run: default_gc_sweep_dry_run(),
            max_deletes_per_run: default_gc_sweep_max_deletes(),
            freshness_threshold_secs: default_gc_sweep_threshold_secs(),
            page_size: default_gc_sweep_page_size(),
            untethered_ttl_seconds: default_gc_sweep_untethered_ttl_secs(),
        }
    }
}

/// Arc 16d §9.4.4 Step 1.3: default flipped `false → true`.
/// Both walkers (byte + row) are on by default starting at v0.5.
fn default_gc_sweep_enabled() -> bool {
    true
}
/// Arc 16d §9.4.2.1: row-walker on/off; default `true`.
fn default_gc_sweep_row_sweep_enabled() -> bool {
    true
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
/// Arc 16d §9.4.2.1 / §9.4.4 Step 1.1: row-sweep TTL default 86400 (24h).
fn default_gc_sweep_untethered_ttl_secs() -> u64 {
    86_400
}

impl GcSweepConfig {
    /// Construct from explicit option-typed env-var values.
    /// Mirrors the pattern in `MaintenancePoolConfig::from_env_values`.
    ///
    /// Arc 16d §9.4.4 Step 1.5 adds two new env vars:
    /// `PDS_GC_SWEEP_ROW_SWEEP_ENABLED` (boolean toggle) and
    /// `PDS_GC_SWEEP_UNTETHERED_TTL_SECS` (TTL anchor seconds).
    #[allow(clippy::too_many_arguments)]
    pub fn from_env_values(
        enabled: Option<String>,
        row_sweep_enabled: Option<String>,
        interval_secs: Option<String>,
        dry_run: Option<String>,
        max_deletes_per_run: Option<String>,
        freshness_threshold_secs: Option<String>,
        page_size: Option<String>,
        untethered_ttl_seconds: Option<String>,
    ) -> PdsResult<Self> {
        let defaults = Self::default();
        let enabled = parse_bool_env("PDS_GC_SWEEP_ENABLED", enabled, defaults.enabled)?;
        let row_sweep_enabled = parse_bool_env(
            "PDS_GC_SWEEP_ROW_SWEEP_ENABLED",
            row_sweep_enabled,
            defaults.row_sweep_enabled,
        )?;
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
        let untethered_ttl_seconds = parse_u64_env(
            "PDS_GC_SWEEP_UNTETHERED_TTL_SECS",
            untethered_ttl_seconds,
            defaults.untethered_ttl_seconds,
        )?;

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
        if untethered_ttl_seconds == 0 {
            return Err(PdsError::Validation(
                "PDS_GC_SWEEP_UNTETHERED_TTL_SECS must be greater than 0".to_string(),
            ));
        }

        Ok(Self {
            enabled,
            row_sweep_enabled,
            interval_secs,
            dry_run,
            max_deletes_per_run,
            freshness_threshold_secs,
            page_size,
            untethered_ttl_seconds,
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

/// Arc 16c §9.3.4 Step 1 — blob lifecycle config.
///
/// `stage_ttl_seconds` controls how long a `temp_blob_metadata` row
/// persists before the (Arc 16d-shipped) reaper reclaims it. Default
/// 86400 (24h), matching bsky-PDS tempKey TTL parity. Per Step 0.5
/// recon, separate from `GcSweepConfig.freshness_threshold_secs`
/// (substrate-safety bound used by Arc 10's in-flight classifier).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobMetadataConfig {
    pub stage_ttl_seconds: u64,
}

impl Default for BlobMetadataConfig {
    fn default() -> Self {
        Self { stage_ttl_seconds: 86400 }
    }
}

impl BlobMetadataConfig {
    /// Build from optional env-var override; falls back to default.
    pub fn from_env_values(stage_ttl_seconds: Option<String>) -> PdsResult<Self> {
        let defaults = Self::default();
        let stage_ttl_seconds = parse_u64_env(
            "PDS_BLOB_STAGE_TTL_SECONDS",
            stage_ttl_seconds,
            defaults.stage_ttl_seconds,
        )?;
        Ok(Self { stage_ttl_seconds })
    }
}

/// v0.8 arc 1 (#180) — bind-audit orphan-marker reconciliation config.
///
/// Drives [`crate::actor_store::orphan_reconcile::run_reconcile_pass`]
/// via the scheduled `bind_audit_orphan_reconcile_job`. Unlike the
/// off-by-default `GcSweepConfig`, this is **on by default**: orphan
/// reconciliation is a forensic/safety mechanism that should run unless
/// an operator explicitly disables it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BindAuditOrphanMarkerConfig {
    /// If `true` (default), `JobScheduler::start()` spawns the
    /// reconciliation sweep. The off case logs at `debug` and skips
    /// the spawn.
    #[serde(default = "default_bind_audit_orphan_enabled")]
    pub enabled: bool,

    /// Cadence between sweep cycles, in seconds. Default 300 (5 min),
    /// matching the `nonce_cleanup_job` / `dpop_jti_replay_reaper_job`
    /// cadence — operationally responsive for a relay-race-window
    /// signal, cheap when (usually) empty. Env override:
    /// `PDS_BIND_AUDIT_ORPHAN_RECONCILE_INTERVAL_SECS`.
    #[serde(default = "default_bind_audit_orphan_reconcile_interval_secs")]
    pub reconcile_interval_secs: u64,
}

impl Default for BindAuditOrphanMarkerConfig {
    fn default() -> Self {
        Self {
            enabled: default_bind_audit_orphan_enabled(),
            reconcile_interval_secs: default_bind_audit_orphan_reconcile_interval_secs(),
        }
    }
}

/// Default `true` — opposite polarity from `GcSweepConfig`'s historical
/// off-by-default; orphan reconciliation runs unless opted out.
fn default_bind_audit_orphan_enabled() -> bool {
    true
}

fn default_bind_audit_orphan_reconcile_interval_secs() -> u64 {
    300
}

impl BindAuditOrphanMarkerConfig {
    /// Build from the optional interval env-var override; `enabled`
    /// comes from config/serde default (no env override per design
    /// §4.1). Mirrors `BlobMetadataConfig::from_env_values`.
    pub fn from_env_values(
        reconcile_interval_secs: Option<String>,
    ) -> PdsResult<Self> {
        let defaults = Self::default();
        let reconcile_interval_secs = parse_u64_env(
            "PDS_BIND_AUDIT_ORPHAN_RECONCILE_INTERVAL_SECS",
            reconcile_interval_secs,
            defaults.reconcile_interval_secs,
        )?;
        if reconcile_interval_secs == 0 {
            return Err(PdsError::Validation(
                "PDS_BIND_AUDIT_ORPHAN_RECONCILE_INTERVAL_SECS must be greater than 0"
                    .to_string(),
            ));
        }
        Ok(Self {
            enabled: defaults.enabled,
            reconcile_interval_secs,
        })
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
    /// Arc 16d §9.4.3.6 / §9.4.4 Step 1.4: Postgres connection-level
    /// transaction isolation pin. Default `"read committed"` —
    /// Aurora-Locus's sweep DELETE predicate-disjointness argument
    /// (§9.4.3.4) relies on statement-scoped snapshot semantics.
    /// Higher isolation levels (REPEATABLE READ / SERIALIZABLE)
    /// produce serialization-failure (40001) errors on the sweep's
    /// per-row autocommit DELETE that the sweep doesn't
    /// retry-classify (deferred to a future cycle per V05_DESIGN.md §9.4.1.2). Pool builder
    /// reads this; `validate_gc_sweep_config` warns case-insensitively
    /// when Postgres is the active backend and the value differs
    /// from "read committed". SQLite-only deployments don't trip
    /// the warning. Overrides cluster-level `postgresql.conf`
    /// per §9.4.3.6 operator-visible-precedent note.
    #[serde(default = "default_pg_transaction_isolation")]
    pub pg_transaction_isolation: String,
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
            pg_transaction_isolation: default_pg_transaction_isolation(),
        }
    }
}

/// Arc 16d §9.4.4 Step 1.4: Postgres isolation pin default.
fn default_pg_transaction_isolation() -> String {
    "read committed".to_string()
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
        pg_transaction_isolation: Option<String>,
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

        // Arc 16d §9.4.4 Step 1.5: pg_transaction_isolation env override.
        // Default `"read committed"`. Stored verbatim as the operator typed
        // it (case-preserving); validator + pool builder normalize via
        // `to_ascii_lowercase` at comparison time.
        let pg_transaction_isolation = pg_transaction_isolation
            .unwrap_or_else(default_pg_transaction_isolation);

        Ok(Self {
            backend,
            url,
            max_connections,
            min_connections,
            acquire_timeout_secs,
            idle_timeout_secs,
            max_lifetime_secs,
            leader_retry_interval_ms,
            pg_transaction_isolation,
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
    /// Per-blob memory cap for the Arc 16f origin-fetch primitive
    /// (`src/federation/blob_fetch.rs`). Closes round-1 F10: defends
    /// against `max_import_size`-respecting CARs that reference
    /// oversized individual blobs. Enforced via HEAD `Content-Length`
    /// pre-check; falls back to streaming-bound enforcement when the
    /// origin omits Content-Length on HEAD. Default `50_000_000`
    /// matches bsky-PDS `PDS_BLOB_UPLOAD_LIMIT` order. Env var:
    /// `PDS_SERVICE_MAX_BLOB_FETCH_SIZE`.
    #[serde(default = "default_max_blob_fetch_size")]
    pub max_blob_fetch_size: u64,
    /// Per-attempt timeout for origin-blob fetches in seconds. Applies
    /// to one HTTP GET attempt; the primitive's inner retry budget
    /// (`blob_fetch_max_retries`) may issue multiple attempts. Default
    /// `30` seconds. Env var: `PDS_SERVICE_BLOB_FETCH_TIMEOUT_SECONDS`.
    #[serde(default = "default_blob_fetch_timeout_seconds")]
    pub blob_fetch_timeout_seconds: u64,
    /// Per-CID retry budget for the origin-blob fetch primitive.
    /// Counts retries *after* the first attempt — so total attempts ≤
    /// `1 + blob_fetch_max_retries`. Only 5xx / network / timeout
    /// errors retry; 4xx are durable. Default `3`. Env var:
    /// `PDS_SERVICE_BLOB_FETCH_MAX_RETRIES`.
    #[serde(default = "default_blob_fetch_max_retries")]
    pub blob_fetch_max_retries: u32,
    /// Arc 16f §9.6.1.1 — kill-switch for the importRepo handler.
    /// When `false`, the handler short-circuits with HTTP 503 inside
    /// the single-flight lock so operators can drain in-flight
    /// imports before halting new ones. Default `true` (importRepo
    /// available). Env var: `PDS_SERVICE_ACCEPTING_IMPORTS`.
    #[serde(default = "default_accepting_imports")]
    pub accepting_imports: bool,
    /// Arc 16f §9.6.1.1 + round-1 F21 — streaming size cap for
    /// importRepo CAR bodies. Enforced during decode (decode loop
    /// aborts at the first chunk that would push the accumulated
    /// byte count past the cap, returning HTTP 413). `None` disables
    /// the cap — discouraged for production, useful for self-import
    /// dev workflows. No default; set explicitly. Env var:
    /// `PDS_SERVICE_MAX_IMPORT_SIZE` (numeric).
    #[serde(default)]
    pub max_import_size: Option<u64>,
}

fn default_max_blob_fetch_size() -> u64 {
    50_000_000
}

fn default_blob_fetch_timeout_seconds() -> u64 {
    30
}

fn default_blob_fetch_max_retries() -> u32 {
    3
}

fn default_accepting_imports() -> bool {
    true
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
    /// Password-login fallback toggle (#442). When `false` (the default), the
    /// `/admin-oauth/password-login` endpoint and the `/admin/password-login.html`
    /// page both 302-redirect to `/admin/` — OAuth is the default admin auth and
    /// password login is off unless an operator opts in per-deployment. A
    /// boot-time env decision (`PDS_ADMIN_PASSWORD_LOGIN_ENABLED=true`), cached
    /// here at startup; not a runtime toggle.
    #[serde(default)]
    pub password_login_enabled: bool,
}

fn default_jwt_sunset_date() -> String {
    // Fallback only: a rolling 90-days-from-boot window used when
    // `PDS_JWT_SUNSET_DATE` is unset. Because it recomputes on each boot it
    // never actually arrives — operators pinning a real deprecation deadline
    // MUST set `PDS_JWT_SUNSET_DATE` (housekeeping #421 §5: the env override was
    // added so the field is tunable like every other; the rolling default is
    // documented, not silent).
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
    /// Arc 13 §6.3.3 PDS-wide recovery key (did:key format), env
    /// `PDS_IDENTITY_RECOVERY_DID_KEY`. Optional — when set, every
    /// new account's genesis op gets this did:key prepended to its
    /// `rotation_keys` after any per-account `recovery_key` input
    /// but before the PDS-wide rotation key.
    ///
    /// Per §6.5.7: the PDS-wide rotation key is in every account's
    /// rotation_keys; single compromise rotates every account
    /// (until the 72-hour timelock + recovery key intervene).
    /// Operators are expected to configure this key + hold its
    /// private material separately so a PDS compromise doesn't
    /// also compromise account recovery.
    #[serde(default)]
    pub recovery_did_key: Option<String>,
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
    // NOTE (housekeeping #421 §5): `interval`/`epoch` have zero runtime readers
    // (dead fields). Removal is deferred — it fans out to ~8 test-construction
    // sites; batch it with the `blob_metadata`/`stage_ttl_seconds` removal (same
    // fan-out) in a focused config-field-removal commit.
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
    /// Inactivity threshold for `rate_limit_buckets` reaper sweeps,
    /// in whole days. Buckets whose `window_start_at_epoch_ms`
    /// hasn't been touched in this duration are deleted at the
    /// hourly sweep. Defaults to 7 (the v0.4 in-code constant);
    /// operator-tunable via `PDS_RATE_LIMIT_BUCKETS_RETENTION_DAYS`
    /// per V06 batch tail G7.2.
    pub buckets_retention_days: u32,
}

/// Logging configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    pub level: String,
}

/// Arc 17 §17.3 — dynamic-lexicon-loading configuration. Off-by-default
/// for v0.5 (`enabled: false`); when enabled, unknown-NSID records are
/// validated against lexicon documents fetched lazily from each NSID's
/// authority DID (resolved via `_lexicon.<host>` DNS TXT then PLC then
/// HTTP GET against the hosting PDS).
///
/// Env-var loading lives in [`LexiconConfig::from_env_values`]. All
/// knobs are optional; defaults mirror the v2 design at §17.3.2 /
/// §17.5.4.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LexiconConfig {
    /// Master switch. When `false`, the validator skips the lexicon
    /// fall-through entirely and unknown NSIDs route to Aurora's
    /// existing Optimistic mode. Default `false`.
    pub enabled: bool,

    /// Optional override for authority resolution. When `Some`, the
    /// LexResolver bypasses DNS TXT + PLC and uses this DID as the
    /// authority for every NSID. Useful for testing and for
    /// homogeneous-federation deployments where a single PDS hosts
    /// every Aurora-specific lexicon. Default `None`.
    pub did_authority: Option<String>,

    /// Behavior when a lexicon fetch fails (DNS / PLC / HTTP).
    /// `HardFail` propagates `PdsError::LexiconFetchFailed` to the
    /// caller (record validation fails); `Warn` emits a WARN log and
    /// falls back to Optimistic acceptance. Default `Warn` for v0.5
    /// (operators opt into the strict posture explicitly). The v1
    /// `Quarantine` variant is DROPPED for v0.5 per §17.5.7 /
    /// round-1 F1.
    pub fetch_failure_behavior: FetchFailureBehavior,

    /// Number of HTTP retries on the lexicon-record fetch step
    /// (§17.3.1 step 6 only — DNS retries inherit `hickory-resolver`
    /// defaults; PLC retries inherit Arc 13 defaults; round-1 F18
    /// closure). Default 3.
    pub fetch_max_retries: u32,

    /// Per-attempt timeout for the HTTP lexicon-record fetch (§17.3.1
    /// step 6). Default 30s; `Warn` mode's worst-case latency floor
    /// per §17.5.4.
    pub fetch_timeout_secs: u64,

    /// In-memory cache TTL; expired entries trigger background
    /// re-fetch while serving cached value (§17.3.1 step 1). Default
    /// 86400s (24h).
    pub cache_ttl_secs: u64,

    /// Throttle floor for on-disk `last_used_at` writes (round-1 F11
    /// closure / §17.3.2). In-memory `last_used_at` updates
    /// immediately; on-disk update fires only when the in-memory
    /// value advances by ≥ this many seconds. Default 60s. Keeps
    /// hot-NSID cache reads from hammering the `lexicon_cache` table.
    pub last_used_persist_threshold_secs: u64,

    /// NSID prefix denylist (round-1 F2 closure / §17.3.3). When a
    /// record's collection NSID starts with any prefix in this list,
    /// the validator rejects with `PdsError::NamespaceDenied`. Intent
    /// B per §17.3.3. Default empty.
    pub namespace_denylist: Option<Vec<String>>,

    /// NSID prefix allowlist (round-1 F2 closure / §17.3.3). When
    /// `Some` and non-empty, only collections matching one of these
    /// prefixes route to the lexicon-fetch path; non-matching
    /// collections fall through to Optimistic (Intent A per §17.3.3
    /// — exclusion is NOT rejection). Default `None` (no allowlist
    /// gate; every unknown NSID is fetchable).
    pub namespace_allowlist: Option<Vec<String>>,

    /// Whether CAR-import write records are subject to lexicon
    /// validation. Default `true` per §17.3.4 — heterogeneous-
    /// federation default. Operators running homogeneous federation
    /// (multiple Aurora-Locus instances with identical lexicon
    /// configs) can set `false` to skip redundant work on import.
    /// When `true`, the validator overrides Arc 16e's per-write
    /// bypass for known NSIDs too (round-1 F4 closure).
    pub validate_imports: bool,
}

/// kryphocron substrate integration config (v0.7 arc 1).
///
/// Master switch + (later arcs) per-feature knobs. Arc 1 ships with
/// just the `enabled` flag; arc 2+ adds audit-retry tuning, audience
/// member limits, oracle cache TTLs, etc.
///
/// Env-var loading lives in [`KryphocronConfig::from_env_values`].
/// Default `enabled: true` (v0.9) — Aurora-Locus's identity is "the
/// kryphocron PDS", so the substrate ships on out of the box; operators who
/// don't want it set `PDS_KRYPHOCRON_ENABLED=false`. (v0.7–0.8 defaulted off
/// per the v07_DESIGN.md §9 friction-risk posture, before the admin surfaces
/// landed.) The closed-namespace policy applies whether the switch is on or
/// off; the registered-NSID branch of the dispatcher and the
/// `kryphocron::lexicons()` startup load only fire when enabled.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KryphocronConfig {
    /// Master switch. Default `true` (v0.9). When `false`, the dispatcher's
    /// closed-namespace check still rejects `tools.kryphocron.*` writes
    /// from the generic path with `UnsupportedNamespace`, but the
    /// registry tier lookup, lexicon validation, and bind pipeline are
    /// inert. Per v07_DESIGN.md §6 lines 3247-3257 the master-switch-off
    /// state is behaviorally indistinguishable from "not compiled in"
    /// for clients.
    pub enabled: bool,
}

impl Default for KryphocronConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}


impl KryphocronConfig {
    /// Build a [`KryphocronConfig`] from already-extracted env values.
    /// Single-knob in arc 1; later arcs add fields here.
    pub fn from_env_values(enabled: Option<String>) -> PdsResult<Self> {
        let defaults = Self::default();
        let enabled = parse_bool_env("PDS_KRYPHOCRON_ENABLED", enabled, defaults.enabled)?;
        Ok(Self { enabled })
    }
}

/// Arc 17 §17.3.6 / round-1 F1 — what to do when a lexicon fetch fails
/// at validate-phase. `Quarantine` (v1) was DROPPED for v0.5 per §17.5.7
/// because Arc 16b's `blob_quarantine` is blob-CID-keyed while lexicon
/// fetch failures are NSID-scoped — structural mismatch. v0.6+ candidate
/// gates on a separate `record_quarantine` surface design.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FetchFailureBehavior {
    /// Propagate `PdsError::LexiconFetchFailed` to the caller. Record
    /// validation fails; client sees HTTP 502.
    HardFail,
    /// Emit a WARN log and fall back to Aurora's existing Optimistic
    /// acceptance (record is written; failure is recorded via
    /// `track_validation_failure`). Default.
    #[default]
    Warn,
}

impl FetchFailureBehavior {
    /// Parse from env-var value with the same case-insensitive
    /// pattern other config enums use.
    pub fn from_env_value(s: &str) -> PdsResult<Self> {
        match s.to_ascii_lowercase().as_str() {
            "hard_fail" | "hardfail" | "strict" => Ok(Self::HardFail),
            "warn" | "optimistic" => Ok(Self::Warn),
            other => Err(PdsError::Validation(format!(
                "PDS_LEXICON_FETCH_FAILURE_BEHAVIOR must be one of \
                 'hard_fail', 'warn' (got: {:?})",
                other
            ))),
        }
    }
}

impl Default for LexiconConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            did_authority: None,
            fetch_failure_behavior: FetchFailureBehavior::default(),
            fetch_max_retries: 3,
            fetch_timeout_secs: 30,
            cache_ttl_secs: 86_400,
            last_used_persist_threshold_secs: 60,
            namespace_denylist: None,
            namespace_allowlist: None,
            validate_imports: true,
        }
    }
}

impl LexiconConfig {
    /// Construct from explicit option-typed env-var values. Pure
    /// function so unit tests can exercise without touching
    /// process-global env. Empty / unset → default value.
    ///
    /// Conditional per-field derivation from option-typed env-var
    /// inputs is what justifies the mutable-default-then-assign
    /// shape and the 10-arg surface.
    #[allow(clippy::field_reassign_with_default, clippy::too_many_arguments)]
    pub fn from_env_values(
        enabled: Option<String>,
        did_authority: Option<String>,
        fetch_failure_behavior: Option<String>,
        fetch_max_retries: Option<String>,
        fetch_timeout_secs: Option<String>,
        cache_ttl_secs: Option<String>,
        last_used_persist_threshold_secs: Option<String>,
        namespace_denylist: Option<String>,
        namespace_allowlist: Option<String>,
        validate_imports: Option<String>,
    ) -> PdsResult<Self> {
        let mut cfg = Self::default();

        if let Some(v) = enabled {
            cfg.enabled = matches!(v.to_ascii_lowercase().as_str(), "true" | "1" | "yes" | "on");
        }
        if let Some(v) = did_authority {
            let trimmed = v.trim();
            if !trimmed.is_empty() {
                cfg.did_authority = Some(trimmed.to_string());
            }
        }
        if let Some(v) = fetch_failure_behavior {
            cfg.fetch_failure_behavior = FetchFailureBehavior::from_env_value(&v)?;
        }
        if let Some(v) = fetch_max_retries {
            cfg.fetch_max_retries = v.parse().map_err(|_| {
                PdsError::Validation(format!(
                    "PDS_LEXICON_FETCH_MAX_RETRIES must be a non-negative integer (got {:?})",
                    v
                ))
            })?;
        }
        if let Some(v) = fetch_timeout_secs {
            cfg.fetch_timeout_secs = v.parse().map_err(|_| {
                PdsError::Validation(format!(
                    "PDS_LEXICON_FETCH_TIMEOUT_SECS must be a positive integer (got {:?})",
                    v
                ))
            })?;
            if cfg.fetch_timeout_secs == 0 {
                return Err(PdsError::Validation(
                    "PDS_LEXICON_FETCH_TIMEOUT_SECS must be greater than 0".to_string(),
                ));
            }
        }
        if let Some(v) = cache_ttl_secs {
            cfg.cache_ttl_secs = v.parse().map_err(|_| {
                PdsError::Validation(format!(
                    "PDS_LEXICON_CACHE_TTL_SECS must be a positive integer (got {:?})",
                    v
                ))
            })?;
            if cfg.cache_ttl_secs == 0 {
                return Err(PdsError::Validation(
                    "PDS_LEXICON_CACHE_TTL_SECS must be greater than 0".to_string(),
                ));
            }
        }
        if let Some(v) = last_used_persist_threshold_secs {
            cfg.last_used_persist_threshold_secs = v.parse().map_err(|_| {
                PdsError::Validation(format!(
                    "PDS_LEXICON_LAST_USED_PERSIST_THRESHOLD_SECS must be a non-negative integer (got {:?})",
                    v
                ))
            })?;
        }
        cfg.namespace_denylist = parse_csv_prefix_list(namespace_denylist);
        cfg.namespace_allowlist = parse_csv_prefix_list(namespace_allowlist);
        if let Some(v) = validate_imports {
            cfg.validate_imports =
                matches!(v.to_ascii_lowercase().as_str(), "true" | "1" | "yes" | "on");
        }
        Ok(cfg)
    }
}

fn parse_csv_prefix_list(raw: Option<String>) -> Option<Vec<String>> {
    raw.and_then(|v| {
        let items: Vec<String> = v
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if items.is_empty() {
            None
        } else {
            Some(items)
        }
    })
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
        dotenvy::dotenv().ok();

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

        // Password-login fallback toggle (#442): OAuth is the default admin auth;
        // password login is off unless the operator opts in per-deployment. Read
        // once here and cached in the config; the value is logged at boot for
        // observability.
        let password_login_enabled = env::var("PDS_ADMIN_PASSWORD_LOGIN_ENABLED")
            .ok()
            .map(|v| matches!(v.trim(), "true" | "1"))
            .unwrap_or(false);
        if password_login_enabled {
            tracing::info!("Password login: ENABLED via PDS_ADMIN_PASSWORD_LOGIN_ENABLED=true");
        } else {
            tracing::info!("Password login: DISABLED (default)");
        }

        let did_plc_url =
            env::var("PDS_DID_PLC_URL").unwrap_or_else(|_| "https://plc.directory".to_string());
        let service_handle_domains = env::var("PDS_SERVICE_HANDLE_DOMAINS")
            .unwrap_or_else(|_| format!(".{}", hostname))
            .split(',')
            .map(|s| s.trim().to_string())
            .collect();
        // Arc 13 §6.3.3 / Step 2.1: PDS-wide recovery key env var.
        // Optional — no default. Validated as non-empty when set
        // (empty string would silently turn into a recovery slot
        // for the empty did:key, which is nonsense).
        let recovery_did_key = env::var("PDS_IDENTITY_RECOVERY_DID_KEY")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
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
        // V06 batch tail G7.2 — rate_limit_buckets reaper inactivity
        // threshold, in whole days. Default 7 preserves the v0.4
        // in-code constant exactly; operator-tunable to handle
        // deployments with different bucket churn / retention asks.
        let rate_limit_buckets_retention_days =
            env::var("PDS_RATE_LIMIT_BUCKETS_RETENTION_DAYS")
                .unwrap_or_else(|_| "7".to_string())
                .parse()
                .unwrap_or(7);

        let log_level = env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());

        // Validation mode
        let validation_mode = env::var("VALIDATION_MODE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();

        // Federation configuration. ATProto PDSes are federation peers by
        // design; the default is `true`. Operators with closed-network,
        // single-tenant, or development deployments set
        // `PDS_FEDERATION_ENABLED=false` to opt out.
        let federation_enabled = env::var("PDS_FEDERATION_ENABLED")
            .unwrap_or_else(|_| "true".to_string())
            .parse()
            .unwrap_or(true);
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
        // v0.9 Federation runtime-mutability arc §2.9 (#404 / G2) — PDS_PUBLIC_URL
        // is vestigial (federation.public_url has no functional consumer; the
        // deployment's real public URL is PDS_SERVICE_PUBLIC_URL → service.public_url).
        // It still parses for backward compatibility in v0.9 but is removed in
        // v0.10; warn operators to migrate. Tracing is initialised before
        // `from_env` runs (see `main.rs`), so this reaches the log.
        if public_url.is_some() {
            tracing::warn!(
                "PDS_PUBLIC_URL is deprecated and will be removed in v0.10. The \
                 deployment's public URL is configured via PDS_SERVICE_PUBLIC_URL \
                 (service.public_url, now editable from the Federation policy page). \
                 Remove PDS_PUBLIC_URL from your environment."
            );
        }

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
            // Arc 16d §9.4.4 Step 1.5: Postgres isolation pin env override.
            env::var("PDS_DATABASE_PG_TRANSACTION_ISOLATION").ok(),
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
            // Arc 16d §9.4.4 Step 1.5: new row-walker env vars.
            env::var("PDS_GC_SWEEP_ROW_SWEEP_ENABLED").ok(),
            env::var("PDS_GC_SWEEP_INTERVAL_SECS").ok(),
            env::var("PDS_GC_SWEEP_DRY_RUN").ok(),
            env::var("PDS_GC_SWEEP_MAX_DELETES_PER_RUN").ok(),
            env::var("PDS_GC_SWEEP_FRESHNESS_THRESHOLD_SECS").ok(),
            env::var("PDS_GC_SWEEP_PAGE_SIZE").ok(),
            env::var("PDS_GC_SWEEP_UNTETHERED_TTL_SECS").ok(),
        )?;

        // Arc 16c §9.3.4 Step 1 / chainlink #92: blob lifecycle
        // config (stage_ttl_seconds product knob, separate from
        // gc_sweep.freshness_threshold_secs).
        let blob_metadata = BlobMetadataConfig::from_env_values(
            env::var("PDS_BLOB_STAGE_TTL_SECONDS").ok(),
        )?;

        // v0.8 arc 1 (#180): bind-audit orphan-marker reconciliation.
        let bind_audit_orphan_marker = BindAuditOrphanMarkerConfig::from_env_values(
            env::var("PDS_BIND_AUDIT_ORPHAN_RECONCILE_INTERVAL_SECS").ok(),
        )?;

        // Arc 17 §17.3 — dynamic lexicon loading. Off-by-default;
        // operators opt in via PDS_LEXICON_ENABLED=true. See
        // [`LexiconConfig`] for every knob.
        let lexicon = LexiconConfig::from_env_values(
            env::var("PDS_LEXICON_ENABLED").ok(),
            env::var("PDS_LEXICON_DID_AUTHORITY").ok(),
            env::var("PDS_LEXICON_FETCH_FAILURE_BEHAVIOR").ok(),
            env::var("PDS_LEXICON_FETCH_MAX_RETRIES").ok(),
            env::var("PDS_LEXICON_FETCH_TIMEOUT_SECS").ok(),
            env::var("PDS_LEXICON_CACHE_TTL_SECS").ok(),
            env::var("PDS_LEXICON_LAST_USED_PERSIST_THRESHOLD_SECS").ok(),
            env::var("PDS_LEXICON_NAMESPACE_DENYLIST").ok(),
            env::var("PDS_LEXICON_NAMESPACE_ALLOWLIST").ok(),
            env::var("PDS_LEXICON_VALIDATE_IMPORTS").ok(),
        )?;

        // v0.7 arc 1 — kryphocron substrate integration. On by default as of
        // v0.9 (Aurora-Locus ships as "the kryphocron PDS"); set
        // PDS_KRYPHOCRON_ENABLED=false to opt out.
        let kryphocron = KryphocronConfig::from_env_values(
            env::var("PDS_KRYPHOCRON_ENABLED").ok(),
        )?;

        // Arc 12 §5.3.2 Gap 1: public_url override via env var.
        // Renamed locally to avoid shadowing federation's
        // existing `public_url` binding (different env var,
        // different field, same name).
        let service_public_url = env::var("PDS_SERVICE_PUBLIC_URL").ok();

        // Arc 16f §9.6.1.1 — origin-blob-fetch primitive knobs. Each
        // falls back to the same default the serde-default helpers
        // use, so YAML deserialisation and env-var construction agree.
        let max_blob_fetch_size = env::var("PDS_SERVICE_MAX_BLOB_FETCH_SIZE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(default_max_blob_fetch_size);
        let blob_fetch_timeout_seconds = env::var("PDS_SERVICE_BLOB_FETCH_TIMEOUT_SECONDS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(default_blob_fetch_timeout_seconds);
        let blob_fetch_max_retries = env::var("PDS_SERVICE_BLOB_FETCH_MAX_RETRIES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(default_blob_fetch_max_retries);

        // Arc 16f §9.6.1.1 — importRepo gate knobs. `accepting_imports`
        // is the operator drain switch (default true); `max_import_size`
        // is the streaming-cap (None = unbounded, only sensible in dev).
        let accepting_imports = env::var("PDS_SERVICE_ACCEPTING_IMPORTS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(default_accepting_imports);
        let max_import_size = env::var("PDS_SERVICE_MAX_IMPORT_SIZE")
            .ok()
            .and_then(|v| v.parse().ok());

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
                max_blob_fetch_size,
                blob_fetch_timeout_seconds,
                blob_fetch_max_retries,
                accepting_imports,
                max_import_size,
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
                jwt_sunset_date: env::var("PDS_JWT_SUNSET_DATE")
                    .unwrap_or_else(|_| default_jwt_sunset_date()),
                oauth_migration_guide_url: env::var("PDS_OAUTH_MIGRATION_GUIDE_URL")
                    .unwrap_or_else(|_| default_migration_guide_url()),
                password_login_enabled,
            },
            identity: IdentityConfig {
                did_plc_url,
                service_handle_domains,
                did_cache_stale_ttl,
                did_cache_max_ttl,
                recovery_did_key,
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
                buckets_retention_days: rate_limit_buckets_retention_days,
            },
            logging: LoggingConfig { level: log_level },
            federation: FederationConfig {
                enabled: federation_enabled,
                relay_urls,
                appview_url,
                firehose_enabled,
                crawl_enabled,
                public_url,
                peer_pds,
            },
            validation_mode,
            distributed_state_mode,
            maintenance_pool,
            gc_sweep,
            bind_audit_orphan_marker,
            blob_metadata,
            entryway,
            lexicon,
            kryphocron,
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
        let cfg = DatabaseConfig::from_env_values(
            None, None, None, None, None, None, None, None, None,
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
        
            None,
            None,
        )
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
        
            None,
            None,
        )
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
        
            None,
            None,
        )
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
        
            None,
            None,
        )
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
        
            None,
            None,
        )
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
        
            None,
            None,
        )
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
        
            None,
            None,
        )
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
        
            None,
            None,
        )
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
        
            None,
            None,
        )
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
        
            None,
            None,
        )
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

    /// Updated for Arc 16d §9.4.4 Step 1.3: `gc_sweep.enabled`
    /// default flipped from `false → true`. v0.4's "off-by-default
    /// safety stance" is superseded by v0.5's "row-walker + byte-
    /// walker both default-on" — Arc 16d ships production-grade
    /// row-driven GC and the design accepts both walkers running by
    /// default. Operators who want pre-Arc-16d behavior set
    /// `PDS_GC_SWEEP_ENABLED=false` explicitly.
    #[test]
    fn default_matches_v05_arc16d_lock_stance() {
        let c = GcSweepConfig::default();
        assert!(c.enabled, "Arc 16d §9.4.4 Step 1.3: default flipped to enabled");
        assert!(
            c.row_sweep_enabled,
            "Arc 16d §9.4.2.1: row-walker default enabled"
        );
        assert!(c.dry_run, "default must be dry_run=true (unchanged from v0.4)");
        assert_eq!(c.interval_secs, 86_400);
        assert_eq!(c.max_deletes_per_run, 10_000);
        assert_eq!(c.freshness_threshold_secs, 3_600);
        assert_eq!(c.page_size, 500);
        assert_eq!(
            c.untethered_ttl_seconds, 86_400,
            "Arc 16d §9.4.4 Step 1.1: row-sweep TTL default 86400 (24h)"
        );
    }

    #[test]
    fn from_env_values_all_none_yields_default() {
        let c = GcSweepConfig::from_env_values(
            None, None, None, None, None, None, None, None,
        )
        .unwrap();
        // Arc 16d §9.4.4 Step 1.3: enabled default flipped false→true.
        assert_eq!(c.enabled, GcSweepConfig::default().enabled);
        assert!(c.enabled);
        // Arc 16d §9.4.2.1: row_sweep_enabled default true.
        assert!(c.row_sweep_enabled);
        assert_eq!(c.dry_run, GcSweepConfig::default().dry_run);
        assert_eq!(c.interval_secs, GcSweepConfig::default().interval_secs);
        assert_eq!(c.max_deletes_per_run, GcSweepConfig::default().max_deletes_per_run);
        assert_eq!(
            c.freshness_threshold_secs,
            GcSweepConfig::default().freshness_threshold_secs
        );
        assert_eq!(c.page_size, GcSweepConfig::default().page_size);
        // Arc 16d §9.4.4 Step 1.1: untethered_ttl_seconds default 86400.
        assert_eq!(c.untethered_ttl_seconds, 86_400);
    }

    #[test]
    fn from_env_values_parses_all_fields() {
        let c = GcSweepConfig::from_env_values(
            Some("true".to_string()),
            Some("false".to_string()), // row_sweep_enabled
            Some("3600".to_string()),
            Some("false".to_string()),
            Some("500".to_string()),
            Some("1800".to_string()),
            Some("250".to_string()),
            Some("172800".to_string()), // untethered_ttl_seconds (48h)
        )
        .unwrap();
        assert!(c.enabled);
        assert!(!c.row_sweep_enabled);
        assert_eq!(c.interval_secs, 3_600);
        assert!(!c.dry_run);
        assert_eq!(c.max_deletes_per_run, 500);
        assert_eq!(c.freshness_threshold_secs, 1_800);
        assert_eq!(c.page_size, 250);
        assert_eq!(c.untethered_ttl_seconds, 172_800);
    }

    #[test]
    fn from_env_values_zero_interval_rejected() {
        let err = GcSweepConfig::from_env_values(
            Some("true".to_string()),
            None,
            Some("0".to_string()),
            None,
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
            None,
            Some("0".to_string()),
            None,
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
            None,
            None,
        )
        .expect_err("'yes' is not a valid bool");
        assert!(err.to_string().contains("PDS_GC_SWEEP_ENABLED"));
    }

    /// Arc 16d §9.4.4 Step 1.5: zero `untethered_ttl_seconds` rejected.
    #[test]
    fn from_env_values_zero_untethered_ttl_rejected() {
        let err = GcSweepConfig::from_env_values(
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some("0".to_string()),
        )
        .expect_err("zero untethered_ttl_seconds must be rejected");
        assert!(err.to_string().contains("PDS_GC_SWEEP_UNTETHERED_TTL_SECS"));
    }

    #[test]
    fn to_sweep_params_propagates_fields() {
        let cfg = GcSweepConfig {
            enabled: true,
            row_sweep_enabled: true,
            interval_secs: 7200,
            dry_run: false,
            max_deletes_per_run: 250,
            freshness_threshold_secs: 600,
            page_size: 100,
            untethered_ttl_seconds: 86_400,
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

#[cfg(test)]
mod env_example_lint_tests {
    //! chainlink #94: keep `.env.example` from re-introducing the
    //! data_directory shadowing trap. Path env vars that auto-derive
    //! from `PDS_DATA_DIRECTORY` (in `from_env_values` for blob paths
    //! and in `ServerConfig::from_env` for the per-component DB paths)
    //! must NOT be committed as active (uncommented) `KEY=value` lines
    //! in `.env.example`. Operators routinely export only
    //! `PDS_DATA_DIRECTORY` for per-instance overlays (e.g., Phase B
    //! pds-a / pds-b); committed `.env.example` entries are copied into
    //! the operator's local `.env`, dotenvy populates them at process
    //! start (since the operator's shell didn't export them), and the
    //! derivation-from-data-directory path is silently bypassed —
    //! stranding components at the .env-set path instead of following
    //! the operator's directory move. The Arc 16c Phase B Scenario 2
    //! report (#94) caught blob bytes landing at `./data/blobs/` while
    //! `PDS_DATA_DIRECTORY=./phase-b/pds-a` was exported.

    const SHADOWING_KEYS: &[&str] = &[
        "PDS_ACCOUNT_DB_LOCATION",
        "PDS_SEQUENCER_DB_LOCATION",
        "PDS_DID_CACHE_DB_LOCATION",
        "PDS_ACTOR_STORE_DIRECTORY",
        "PDS_BLOBSTORE_DISK_LOCATION",
        "PDS_BLOBSTORE_DISK_TMP_LOCATION",
    ];

    #[test]
    fn env_example_does_not_shadow_data_directory_derived_defaults() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".env.example");
        let example = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {}", path.display(), e));

        for key in SHADOWING_KEYS {
            let prefix = format!("{}=", key);
            let active_line = example.lines().find(|line| {
                let trimmed = line.trim_start();
                !trimmed.starts_with('#') && trimmed.starts_with(&prefix)
            });
            assert!(
                active_line.is_none(),
                ".env.example must not have an active `{}=...` line \
                 (found: {:?}). These paths auto-derive from \
                 PDS_DATA_DIRECTORY; committing them as active values \
                 creates a precedence trap where operator-shell-exported \
                 PDS_DATA_DIRECTORY no longer moves the component (the \
                 dotenvy-loaded .env value wins because the operator's \
                 shell didn't export this specific key). Comment the \
                 entry out — operators who genuinely want a non-default \
                 component path can uncomment. See chainlink #94.",
                key,
                active_line,
            );
        }
    }
}
