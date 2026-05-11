/// Application context and dependency injection
use crate::{
    account::AccountManager,
    actor_store::{ActorStore, ActorStoreConfig},
    admin::{AdminRoleManager, InviteCodeManager, LabelManager, ModerationManager, ReportManager},
    blob_store::{BlobBackendType, BlobStorageConfig, BlobStore, BlobStoreConfig},
    config::{BlobstoreConfig, DatabaseConfig, DistributedStateMode, ServerConfig},
    db,
    distributed::{DistributedStore, PostgresCasStore},
    error::{PdsError, PdsResult},
    federation::{
        authentication::FederationAuthenticator,
        discovery::PdsDiscovery,
        dpop::{DPopNonceStore, DPopVerifier},
        search::FederatedSearch,
        NonceStore, RelayClient, RelayConfig,
    },
    identity::{DidCache, IdentityResolver, IdentityResolverApi, IdentityResolverConfig},
    mailer::Mailer,
    oauth::{ClientManager, DeviceManager},
    rate_limit::RateLimiter,
    read_after_write::LocalRecordsCache,
    sequencer::{Sequencer, SequencerConfig},
};
use std::sync::Arc;

/// Application context holding all shared services
#[derive(Clone)]
pub struct AppContext {
    pub config: Arc<ServerConfig>,
    /// Shared-database pool for account, sequencer, OAuth tables, etc.
    /// Backend is selected by `config.database.backend` (SQLite or
    /// Postgres). `AnyPool` makes the dispatch transparent to consumers.
    pub account_db: sqlx::AnyPool,
    pub account_manager: Arc<AccountManager>,
    pub actor_store: Arc<ActorStore>,
    pub blob_store: Arc<BlobStore>,
    pub identity_resolver: Arc<dyn IdentityResolverApi>,
    // Admin & Moderation
    pub admin_role_manager: Arc<AdminRoleManager>,
    pub moderation_manager: Arc<ModerationManager>,
    pub label_manager: Arc<LabelManager>,
    pub invite_manager: Arc<InviteCodeManager>,
    pub report_manager: Arc<ReportManager>,
    // OAuth server components (for third-party app authorization)
    #[allow(dead_code)] // Future OAuth client management
    pub oauth_client_manager: Arc<ClientManager>,
    #[allow(dead_code)] // Future OAuth device flow
    pub oauth_device_manager: Arc<DeviceManager>,
    // Sequencer for event streaming
    pub sequencer: Arc<Sequencer>,
    // Relay client for federation
    pub relay_client: Option<Arc<tokio::sync::Mutex<RelayClient>>>,
    // Federation components
    pub federation_auth: Option<Arc<FederationAuthenticator>>,
    pub pds_discovery: Option<Arc<PdsDiscovery>>,
    pub federated_search: Option<Arc<FederatedSearch>>,
    pub nonce_store: Option<Arc<NonceStore>>,
    /// DPoP §8 nonce challenge store. Federation-gated because the
    /// `/xrpc/com.atproto.federation.getDpopNonce` endpoint is the
    /// only thing that issues server-side nonces. The DPoP verifier
    /// (next field) holds its own Arc to the same store when
    /// federation is enabled, or to a dedicated store otherwise — the
    /// keyspaces don't conflict.
    pub dpop_nonce_store: Option<Arc<DPopNonceStore>>,
    /// DPoP verifier — always present. Used by the OAuth token
    /// endpoint at issuance and by `OAuthAuthContext` on every
    /// resource request that has a DPoP-bound token. RFC 9449 §4.3
    /// `ath` binding is checked at the resource-request site; the
    /// JTI replay set is shared with the federation §8 challenge
    /// store when federation is enabled (see field above).
    pub dpop_verifier: Arc<DPopVerifier>,
    // Rate limiter
    pub rate_limiter: Arc<RateLimiter>,
    // Distributed rate limiter (Redis-backed, for multi-instance deployments)
    #[allow(dead_code)] // Future distributed rate limiting
    pub distributed_rate_limiter: Option<Arc<crate::rate_limit_new::DistributedRateLimiter>>,
    // Email mailer
    pub mailer: Arc<Mailer>,
    // Read-after-write cache
    pub local_records_cache: Arc<LocalRecordsCache>,
    /// Front door for cache invalidations — does local invalidation
    /// plus (Postgres only) cross-instance NOTIFY emit. Write handlers
    /// call `cache_invalidator.invalidate_did(did)` instead of touching
    /// `local_records_cache.invalidate_did` directly. See
    /// chainlink #90 / docs/AURORA_DESIGN.md §5.4.2.
    pub cache_invalidator: Arc<crate::cache::invalidation::CacheInvalidator>,
    /// File-tier runtime settings loaded once at startup from
    /// `<data_directory>/runtime.yaml` (override via `PDS_RUNTIME_FILE`).
    /// Per Arc 5 §9.4.2 / chainlink #124: sits between the runtime
    /// row and the compiled-in default in `get_runtime_setting`'s
    /// lookup. `Arc<HashMap>` keeps `AppContext::clone()` cheap;
    /// the cache is read-only post-startup. Reload-on-SIGHUP is a
    /// v0.4 follow-up — runtime_settings rows are the hot path for
    /// in-process changes.
    pub file_tier_settings: Arc<std::collections::HashMap<String, serde_json::Value>>,
    /// Dedicated maintenance pool for the distributed-state
    /// substrate (Arc 7, V04_DESIGN.md §6.4.0 Q8b). Isolated from
    /// the main `account_db` pool so DPoP / OAuth-state /
    /// rate-limit roundtrips can't starve regular request
    /// handling. `None` in `DistributedStateMode::SingleInstanceInmemory`
    /// — the substrate isn't constructed in that mode.
    pub maintenance_pool: Option<Arc<sqlx::AnyPool>>,
    /// Distributed-state substrate (Arc 7, V04_DESIGN.md §6.3.2).
    /// Operates against `maintenance_pool` when present. `None`
    /// in `SingleInstanceInmemory` mode; consumers (DPoP, OAuth
    /// state, rate-limit — wired in Steps 2-3) fall back to
    /// in-process state when the substrate is absent.
    pub distributed_store: Option<Arc<dyn DistributedStore>>,
}

impl AppContext {
    /// Create a new application context from configuration
    pub async fn new(config: ServerConfig) -> PdsResult<Self> {
        // Validate configuration
        config.validate()?;

        // Create data directories if they don't exist
        Self::ensure_directories(&config).await?;

        // Load file-tier runtime settings (Arc 5 §9.4.2 / chainlink
        // #124). Path defaults to `<data_directory>/runtime.yaml`;
        // env-var override via `PDS_RUNTIME_FILE`. Missing file =>
        // empty map (file tier is optional). Malformed YAML =>
        // startup error with file path. Unknown keys (vs.
        // KNOWN_RUNTIME_KEYS) and invalid per-key values
        // warn-and-skip — operator typos surface in logs without
        // bringing the deployment down.
        let runtime_file_path = std::env::var(
            crate::api::aurora_admin::RUNTIME_FILE_ENV,
        )
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| config.storage.data_directory.join("runtime.yaml"));
        let file_tier_settings = Arc::new(
            crate::api::aurora_admin::load_file_tier_settings(&runtime_file_path)?,
        );

        // Open the shared-database pool. `db::create_any_pool` dispatches
        // on `config.database.backend` to either SQLite (using the
        // configured file path as the fallback) or Postgres (using the
        // configured URL). Phase 3 (chainlink #76) collapsed the
        // dual-pool transient that existed during the SqlitePool→AnyPool
        // refactor into this single AnyPool.
        let account_db =
            db::create_any_pool(&config.database, &config.storage.account_db).await?;
        db::run_any_migrations(&account_db, &config.database).await?;

        // Distributed-state substrate's dedicated maintenance pool
        // (Arc 7, V04_DESIGN.md §6.4.0 Q8b). Same database as
        // `account_db` (so migrations run once against the shared
        // pool above), but a separate pool so DPoP / OAuth-state /
        // rate-limit roundtrips have their own connection budget
        // and can't starve regular request handling under load.
        // Constructed only in `Distributed` mode;
        // `SingleInstanceInmemory` mode skips the substrate
        // entirely. `Redis` is rejected at `config.validate()`
        // time so it never reaches this branch.
        let (maintenance_pool, distributed_store) = match config.distributed_state_mode {
            DistributedStateMode::Distributed => {
                let maintenance_db_config = DatabaseConfig {
                    backend: config.database.backend,
                    url: config.database.url.clone(),
                    max_connections: config.maintenance_pool.max_connections,
                    min_connections: config.maintenance_pool.min_connections,
                    acquire_timeout_secs: config.maintenance_pool.acquire_timeout_secs,
                    idle_timeout_secs: config.database.idle_timeout_secs,
                    max_lifetime_secs: config.database.max_lifetime_secs,
                    leader_retry_interval_ms: config.database.leader_retry_interval_ms,
                };
                let pool = Arc::new(
                    db::create_any_pool(&maintenance_db_config, &config.storage.account_db)
                        .await?,
                );
                let store: Arc<dyn DistributedStore> =
                    Arc::new(PostgresCasStore::new(Arc::clone(&pool)));
                tracing::info!(
                    max_connections = config.maintenance_pool.max_connections,
                    min_connections = config.maintenance_pool.min_connections,
                    "Distributed-state substrate initialized (Postgres-CAS)"
                );
                (Some(pool), Some(store))
            }
            DistributedStateMode::SingleInstanceInmemory => {
                tracing::info!(
                    "Distributed-state substrate disabled \
                     (PDS_DISTRIBUTED_STATE_MODE=single_instance_inmemory) — \
                     auth state lost on restart"
                );
                (None, None)
            }
            DistributedStateMode::Redis => {
                // Unreachable: config.validate() rejects Redis at
                // startup. Defensive return for completeness.
                return Err(PdsError::Validation(
                    "Redis distributed-state mode not implemented in v0.4".to_string(),
                ));
            }
        };

        // Initialize account manager
        let account_manager = Arc::new(AccountManager::new(
            account_db.clone(),
            Arc::new(config.clone()),
        ));

        // Initialize actor store
        let actor_store_config = ActorStoreConfig {
            base_directory: config.storage.actor_store_directory.clone(),
            cache_size: 100,
        };
        let actor_store = Arc::new(ActorStore::new(actor_store_config));

        // Initialize blob store. Convert the config-layer `BlobstoreConfig`
        // to the storage-layer `BlobBackendType`, then hand it to
        // `BlobStore::new` which dispatches to the disk or S3 backend.
        let blob_store_config = build_blob_store_config(&config)?;
        let blob_store =
            Arc::new(BlobStore::new(blob_store_config, account_db.clone()).await?);

        // Initialize identity cache database. The DidCache holds an
        // AnyPool that dispatches to the configured backend. SQLite-only
        // tuning (WAL mode, autocheckpoint, synchronous=NORMAL) is
        // applied via PRAGMA when the backend is SQLite; PRAGMAs are
        // a no-op via sqlx::query on Postgres but the early-return
        // guards against accidentally running them.
        //
        // The cache uses its own DatabaseConfig synthesized from the
        // configured did_cache_db path so that operators don't have to
        // configure a separate Postgres database for the cache — for now
        // it always uses SQLite at the configured file path. (Future
        // work: a separate cache backend selector if desired.)
        let did_cache_db = {
            let cache_config = crate::config::DatabaseConfig {
                backend: crate::config::DatabaseBackend::Sqlite,
                url: None,
                ..config.database.clone()
            };
            db::create_any_pool(&cache_config, &config.storage.did_cache_db).await?
        };
        db::run_any_migrations(
            &did_cache_db,
            &crate::config::DatabaseConfig {
                backend: crate::config::DatabaseBackend::Sqlite,
                url: None,
                ..config.database.clone()
            },
        )
        .await?;
        // SQLite tuning PRAGMAs (silent no-ops if Postgres ever takes over).
        let _ = sqlx::query("PRAGMA wal_autocheckpoint = 1000")
            .execute(&did_cache_db)
            .await;
        let _ = sqlx::query("PRAGMA synchronous = NORMAL")
            .execute(&did_cache_db)
            .await;

        // Initialize identity resolver with separate WAL-enabled cache database
        let did_cache = DidCache::new(did_cache_db).with_did_doc_ttls(
            chrono::Duration::seconds(config.identity.did_cache_stale_ttl as i64),
            chrono::Duration::seconds(config.identity.did_cache_max_ttl as i64),
        );
        let identity_config = IdentityResolverConfig {
            user_agent: format!("Aurora-Locus/{}", config.service.version),
            use_doh: false,
            plc_directory_url: config.identity.did_plc_url.clone(),
            max_retries: 3,
            retry_base_delay_ms: 100,
            retry_max_delay_ms: 5000,
        };
        let identity_resolver: Arc<dyn IdentityResolverApi> =
            Arc::new(IdentityResolver::new(did_cache, identity_config)?);

        // Initialize admin & moderation managers
        let admin_role_manager = Arc::new(AdminRoleManager::new(account_db.clone()));
        let moderation_manager = Arc::new(ModerationManager::new(
            account_db.clone(),
            account_manager.clone(),
        ));
        let label_manager = Arc::new(LabelManager::new(
            account_db.clone(),
            config.service.service_did.clone(),
        ));
        let invite_manager = Arc::new(InviteCodeManager::new(account_db.clone()));
        let report_manager = Arc::new(ReportManager::new(account_db.clone()));

        // Initialize OAuth server managers
        // For now, initialize with empty client list. In production, load from config.
        // TODO: Add OAuth client configuration to ServerConfig
        tracing::info!("Initializing OAuth server managers (ClientManager, DeviceManager)");
        let oauth_client_manager = Arc::new(ClientManager::new(account_db.clone(), vec![]));
        let oauth_device_manager = Arc::new(DeviceManager::new(account_db.clone()));

        // Initialize relay client first (optional - only if relay servers configured and federation enabled)
        let relay_client = if config.federation.enabled && !config.federation.relay_urls.is_empty()
        {
            tracing::info!(
                "Federation enabled with {} relay server(s)",
                config.federation.relay_urls.len()
            );
            let relay_config = RelayConfig {
                servers: config.federation.relay_urls.clone(),
                reconnect_interval: 5,
                buffer_size: 1000,
                enable_compression: true,
            };
            let client = RelayClient::new(relay_config);
            Some(Arc::new(tokio::sync::Mutex::new(client)))
        } else {
            tracing::info!("Federation disabled - no relay integration");
            None
        };

        // Initialize federation components (Phase 1)
        let (federation_auth, pds_discovery) = if config.federation.enabled {
            tracing::info!("Initializing federation authenticator and PDS discovery");

            // Federation authenticator for cross-PDS authentication
            let auth = Arc::new(FederationAuthenticator::new(Arc::clone(&identity_resolver)));

            // PDS discovery for finding other instances
            let discovery = Arc::new(PdsDiscovery::new(config.federation.relay_urls.clone()));

            (Some(auth), Some(discovery))
        } else {
            (None, None)
        };

        // Initialize federated search (Phase 2)
        let federated_search = if config.federation.enabled {
            if let Some(ref discovery) = pds_discovery {
                tracing::info!("Initializing federated search (max_concurrent: 10, timeout: 30s)");
                Some(Arc::new(FederatedSearch::new(
                    Arc::clone(discovery),
                    10, // max_concurrent requests
                    30, // timeout_secs
                )))
            } else {
                None
            }
        } else {
            None
        };

        // Initialize nonce store for service auth (Phase 4)
        let nonce_store = if config.federation.enabled {
            tracing::info!("Initializing nonce store for replay prevention (retention: 120s)");
            Some(Arc::new(NonceStore::new()))
        } else {
            None
        };

        // Initialize DPoP support. Verifier is always constructed —
        // DPoP is an OAuth concern, not federation-gated. The
        // §8 server-issued nonce store is federation-gated because
        // only the federation-namespace endpoint issues those nonces;
        // when federation is off the verifier still has its own JTI
        // replay tracker (separate Arc), which is what RFC 9449 §11.1
        // requires regardless of the §8 challenge flow.
        let dpop_nonce_store: Option<Arc<DPopNonceStore>> = if config.federation.enabled {
            tracing::info!(
                "Initializing DPoP §8 nonce challenge store (federation enabled)"
            );
            Some(Arc::new(DPopNonceStore::new()))
        } else {
            None
        };
        let dpop_verifier = {
            let store_for_verifier = match &dpop_nonce_store {
                Some(s) => Arc::clone(s),
                None => Arc::new(DPopNonceStore::new()),
            };
            Arc::new(DPopVerifier::new(store_for_verifier))
        };

        // Initialize sequencer with relay client (using account_db for now, could be separate database)
        let mut seq = Sequencer::with_relay(
            account_db.clone(),
            SequencerConfig::default(),
            relay_client.clone(),
        );

        // Multi-instance leader election (Postgres only). SQLite
        // deployments are inherently single-instance and skip election;
        // the sequencer's default-true `is_leader` flag remains in place.
        // See chainlink #89 / docs/AURORA_DESIGN.md §5.4.1.
        //
        // The election task runs for the lifetime of the process and is
        // not joined explicitly here — graceful shutdown is handled by
        // the runtime tearing down. A future refactor to expose a
        // top-level shutdown handle could call LeaderElection::shutdown
        // for explicit `pg_advisory_unlock` on cooperative termination
        // (see chainlink #89 / design doc §3.5 and the `ShutdownHandle`
        // open question).
        if matches!(
            config.database.backend,
            crate::config::DatabaseBackend::Postgres
        ) {
            use crate::sequencer::{
                LeaderElection, LeaderElectionConfig, PostgresLockProvider,
                SEQUENCER_LEADER_LOCK_KEY,
            };
            // Standby until first acquire tick.
            seq.attach_leader_flag(Arc::new(std::sync::atomic::AtomicBool::new(false)));
            // Threading the URL (rather than the pool) into the
            // provider gives it a dedicated lock connection separate
            // from the application pool, per
            // POSTGRES_PHASE_4 §5.1's pool_size+2 sizing rule. The
            // +2 are the lock connection (this one) and the LISTEN
            // connection (cache::invalidation).
            let leader_db_url = config.database.url.clone().ok_or_else(|| {
                PdsError::Validation(
                    "PDS_DB_URL is required for Postgres backend leader election".to_string(),
                )
            })?;
            let provider = Arc::new(PostgresLockProvider::new(
                leader_db_url,
                SEQUENCER_LEADER_LOCK_KEY,
            ));
            let mut election = LeaderElection::new(provider, seq.leader_flag());
            election.spawn(LeaderElectionConfig {
                retry_interval: std::time::Duration::from_millis(
                    config.database.leader_retry_interval_ms,
                ),
            });
            // Election handle leaks intentionally — it owns the JoinHandle
            // and lives for the process lifetime. See comment above.
            std::mem::forget(election);
            tracing::info!(
                "Sequencer leader election spawned (retry interval: {}ms)",
                config.database.leader_retry_interval_ms
            );
        }

        let sequencer = Arc::new(seq);

        // Initialize distributed rate limiter if Redis is enabled
        let distributed_rate_limiter = if config.rate_limit.use_redis {
            if let Some(ref redis_url) = config.rate_limit.redis_url {
                tracing::info!(
                    "Initializing distributed Redis-backed rate limiter: {}",
                    redis_url
                );
                // Create cache client for Redis
                let cache_config = crate::cache::CacheConfig {
                    enabled: true,
                    redis_url: redis_url.clone(),
                    ..Default::default()
                };
                let cache_client = crate::cache::CacheClient::new(cache_config).await?;
                let dist_limiter = crate::rate_limit_new::DistributedRateLimiter::new(
                    cache_client,
                    config.rate_limit.global_requests_per_minute,
                );
                Some(Arc::new(dist_limiter))
            } else {
                tracing::warn!("Redis rate limiting enabled but no redis_url configured");
                None
            }
        } else {
            None
        };

        // Initialize rate limiter with Bluesky-compatible endpoint limits.
        // The `exempt_admin_assets` flag is the only env-driven runtime
        // tuning currently plumbed through; the rest of the runtime quotas
        // remain at their compiled-in defaults.
        let rate_limiter = Arc::new(RateLimiter::with_bluesky_defaults(
            crate::rate_limit::RateLimitConfig {
                exempt_admin_assets: config.rate_limit.exempt_admin_assets,
                ..crate::rate_limit::RateLimitConfig::default()
            },
        ));

        // Initialize mailer
        let mailer = Arc::new(Mailer::new(config.email.clone())?);

        // Initialize read-after-write cache (5s TTL, 10k entries) and the
        // cache invalidator front door. Multi-instance Postgres
        // deployments wire a NOTIFY emitter so writes here propagate
        // to other instances; SQLite skips the emitter (single-instance
        // by definition). See chainlink #90 / docs/AURORA_DESIGN.md §5.4.2.
        let local_records_cache = Arc::new(LocalRecordsCache::new());
        let notify_emitter: Option<Arc<dyn crate::cache::invalidation::NotifyEmitter>> =
            if matches!(
                config.database.backend,
                crate::config::DatabaseBackend::Postgres
            ) {
                Some(Arc::new(crate::cache::invalidation::PostgresNotifyEmitter::new(
                    account_db.clone(),
                )))
            } else {
                None
            };
        let cache_invalidator = Arc::new(crate::cache::invalidation::CacheInvalidator::new(
            Arc::clone(&local_records_cache),
            notify_emitter,
        ));

        // Spawn the LISTEN loop on Postgres so this instance receives
        // NOTIFYs from peer instances and applies them to its local
        // cache. SQLite skips entirely. The listener task lives for the
        // process lifetime; like the leader-election task in Phase 4.2,
        // we leak the handle here pending a top-level shutdown handle
        // (chainlink #89 §3.5 follow-up).
        if matches!(
            config.database.backend,
            crate::config::DatabaseBackend::Postgres
        ) {
            if let Some(url) = config.database.url.clone() {
                let listener = crate::cache::invalidation::CacheInvalidationListener::spawn(
                    url,
                    Arc::clone(&cache_invalidator),
                );
                std::mem::forget(listener);
                tracing::info!(
                    channel = crate::cache::invalidation::CHANNEL_NAME,
                    "Cache invalidation listener spawned"
                );
            } else {
                // Validation in DatabaseConfig::from_env_values rejects
                // postgres-without-URL, so this branch should be
                // unreachable in practice. Logging instead of unwrap
                // keeps the code defensive against config-loading paths
                // that bypass validation (e.g. test fixtures).
                tracing::warn!(
                    "Postgres backend without URL — cache invalidation listener not spawned"
                );
            }
        }

        Ok(Self {
            config: Arc::new(config),
            account_db,
            account_manager,
            actor_store,
            blob_store,
            identity_resolver,
            admin_role_manager,
            moderation_manager,
            label_manager,
            invite_manager,
            report_manager,
            oauth_client_manager,
            oauth_device_manager,
            sequencer,
            relay_client,
            federation_auth,
            pds_discovery,
            federated_search,
            nonce_store,
            dpop_nonce_store,
            dpop_verifier,
            rate_limiter,
            distributed_rate_limiter,
            mailer,
            local_records_cache,
            cache_invalidator,
            file_tier_settings,
            maintenance_pool,
            distributed_store,
        })
    }

    /// Ensure required directories exist
    async fn ensure_directories(config: &ServerConfig) -> PdsResult<()> {
        let dirs = vec![
            &config.storage.data_directory,
            &config.storage.actor_store_directory,
        ];

        for dir in dirs {
            if !dir.exists() {
                tokio::fs::create_dir_all(dir).await.map_err(|e| {
                    PdsError::Internal(format!("Failed to create directory {:?}: {}", dir, e))
                })?;
            }
        }

        // Create blob storage directories if using disk storage
        if let crate::config::BlobstoreConfig::Disk {
            location,
            tmp_location,
        } = &config.storage.blobstore
        {
            tokio::fs::create_dir_all(location).await?;
            tokio::fs::create_dir_all(tmp_location).await?;
        }

        Ok(())
    }

    /// Get service URL
    pub fn service_url(&self) -> String {
        format!(
            "http://{}:{}",
            self.config.service.hostname, self.config.service.port
        )
    }

    /// Get service DID
    pub fn service_did(&self) -> &str {
        &self.config.service.service_did
    }
}

/// Convert the configuration-layer `BlobstoreConfig` (S3 vs Disk variants
/// loaded from env vars) into the storage-layer `BlobStoreConfig` that
/// `BlobStore::new` consumes. Centralised here so the dispatch lives
/// next to `AppContext::new`'s blob store construction.
fn build_blob_store_config(config: &ServerConfig) -> PdsResult<BlobStoreConfig> {
    // `tmp_location` and `temp_dir` are conceptually the same — the disk
    // backend writes pending uploads to a temp directory before atomically
    // renaming into place. We keep the config-layer name `tmp_location`
    // and pass it through to the storage layer's `temp_dir`.
    let (backend, temp_dir) = match &config.storage.blobstore {
        BlobstoreConfig::Disk {
            location,
            tmp_location,
        } => (
            BlobBackendType::Disk {
                location: location.clone(),
            },
            tmp_location.clone(),
        ),
        BlobstoreConfig::S3 {
            bucket,
            region,
            access_key_id,
            secret_access_key,
            endpoint,
            prefix,
            force_path_style,
            upload_timeout_ms,
        } => (
            BlobBackendType::S3 {
                bucket: bucket.clone(),
                region: region.clone(),
                endpoint: endpoint.clone(),
                access_key_id: access_key_id.clone(),
                secret_access_key: secret_access_key.clone(),
                prefix: prefix.clone(),
                force_path_style: *force_path_style,
                upload_timeout_ms: *upload_timeout_ms,
            },
            // S3 backend doesn't need a local temp dir for blob bodies,
            // but the wrapper's other code paths still expect one.
            // Reuse the configured data directory.
            config.storage.data_directory.join("temp"),
        ),
    };

    Ok(BlobStoreConfig {
        storage: BlobStorageConfig {
            backend,
            max_blob_size: config.service.blob_upload_limit,
            temp_dir,
        },
    })
}
