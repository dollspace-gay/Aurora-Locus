/// Application context and dependency injection
use crate::{
    account::AccountManager,
    actor_store::{ActorStore, ActorStoreConfig},
    admin::{AdminRoleManager, InviteCodeManager, LabelManager, ModerationManager, ReportManager},
    blob_store::{BlobBackendType, BlobStorageConfig, BlobStore, BlobStoreConfig},
    config::{BlobstoreConfig, ServerConfig},
    db,
    error::{PdsError, PdsResult},
    federation::{
        authentication::FederationAuthenticator,
        discovery::PdsDiscovery,
        dpop::{DPopNonceStore, DPopVerifier},
        search::FederatedSearch,
        NonceStore, RelayClient, RelayConfig,
    },
    identity::{DidCache, IdentityResolver, IdentityResolverConfig},
    mailer::Mailer,
    oauth::{ClientManager, DeviceManager},
    rate_limit::RateLimiter,
    read_after_write::LocalRecordsCache,
    sequencer::{Sequencer, SequencerConfig},
};
use sqlx::SqlitePool;
use std::sync::Arc;

/// Application context holding all shared services
#[derive(Clone)]
pub struct AppContext {
    pub config: Arc<ServerConfig>,
    pub account_db: SqlitePool,
    pub account_manager: Arc<AccountManager>,
    pub actor_store: Arc<ActorStore>,
    pub blob_store: Arc<BlobStore>,
    pub identity_resolver: Arc<IdentityResolver>,
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
    // DPoP support (Phase 4)
    pub dpop_nonce_store: Option<Arc<DPopNonceStore>>,
    #[allow(dead_code)] // Future DPoP verification
    pub dpop_verifier: Option<Arc<DPopVerifier>>,
    // Rate limiter
    pub rate_limiter: Arc<RateLimiter>,
    // Distributed rate limiter (Redis-backed, for multi-instance deployments)
    #[allow(dead_code)] // Future distributed rate limiting
    pub distributed_rate_limiter: Option<Arc<crate::rate_limit_new::DistributedRateLimiter>>,
    // Email mailer
    pub mailer: Arc<Mailer>,
    // Read-after-write cache
    pub local_records_cache: Arc<LocalRecordsCache>,
}

impl AppContext {
    /// Create a new application context from configuration
    pub async fn new(config: ServerConfig) -> PdsResult<Self> {
        // Validate configuration
        config.validate()?;

        // Phase 2 (chainlink #75) added DatabaseConfig with backend
        // selection, but Phase 3 (#76) has not yet refactored the 16
        // shared-DB consumer modules from SqlitePool to AnyPool. Until
        // that lands, only SQLite is constructible at runtime — reject
        // Postgres explicitly so operators get a clear error rather
        // than a build-time mismatch deeper in the stack.
        if matches!(
            config.database.backend,
            crate::config::DatabaseBackend::Postgres
        ) {
            return Err(PdsError::Validation(
                "PDS_DB_BACKEND=postgres is not yet wired into the runtime; \
                 the dispatch layer is in place but per-file refactoring \
                 (Postgres workstream Phase 3, chainlink #76) must land \
                 first. Use PDS_DB_BACKEND=sqlite (or unset it) for now."
                    .to_string(),
            ));
        }

        // Create data directories if they don't exist
        Self::ensure_directories(&config).await?;

        // Initialize account database
        let account_db =
            db::create_pool(&config.storage.account_db, db::DatabaseOptions::default()).await?;

        // Run database migrations (includes OAuth tables)
        db::run_migrations(&account_db).await?;

        // Test connection
        db::test_connection(&account_db).await?;

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

        // Initialize identity cache database with WAL mode enabled
        // WAL mode provides better read concurrency - reads don't block during cache writes
        let did_cache_db = db::create_pool(
            &config.storage.did_cache_db,
            db::DatabaseOptions {
                max_connections: 10,
                enable_wal: true, // Enable WAL mode for concurrent reads during writes
            },
        )
        .await?;

        // Run migrations for identity cache
        db::run_migrations(&did_cache_db).await?;

        // Configure WAL checkpoint settings for optimal performance
        // autocheckpoint=1000 pages (~4MB with default 4KB page size)
        sqlx::query("PRAGMA wal_autocheckpoint = 1000")
            .execute(&did_cache_db)
            .await
            .map_err(PdsError::Database)?;

        // Set synchronous=NORMAL for better write performance while maintaining durability
        // NORMAL is safe with WAL mode and provides good balance of performance/safety
        sqlx::query("PRAGMA synchronous = NORMAL")
            .execute(&did_cache_db)
            .await
            .map_err(PdsError::Database)?;

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
        let identity_resolver = Arc::new(IdentityResolver::new(did_cache, identity_config)?);

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

        // Initialize DPoP support (Phase 4)
        let (dpop_nonce_store, dpop_verifier) = if config.federation.enabled {
            tracing::info!("Initializing DPoP support for client-to-PDS authentication");
            let dpop_nonce = Arc::new(DPopNonceStore::new());
            let dpop_verify = Arc::new(DPopVerifier::new(Arc::clone(&dpop_nonce)));
            (Some(dpop_nonce), Some(dpop_verify))
        } else {
            (None, None)
        };

        // Initialize sequencer with relay client (using account_db for now, could be separate database)
        let sequencer = Arc::new(Sequencer::with_relay(
            account_db.clone(),
            SequencerConfig::default(),
            relay_client.clone(),
        ));

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

        // Initialize rate limiter with Bluesky-compatible endpoint limits
        let rate_limiter = Arc::new(RateLimiter::with_bluesky_defaults(
            crate::rate_limit::RateLimitConfig::default(),
        ));

        // Initialize mailer
        let mailer = Arc::new(Mailer::new(config.email.clone())?);

        // Initialize read-after-write cache (5s TTL, 10k entries)
        let local_records_cache = Arc::new(LocalRecordsCache::new());

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
