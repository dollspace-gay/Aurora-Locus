/// Application context and dependency injection
use crate::{
    account::AccountManager,
    actor_store::{ActorStore, ActorStoreConfig},
    admin::{
        AdminRoleManager, InviteCodeManager, LabelManager, ModerationManager, ReportManager,
    },
    blob_store::{BlobStore, BlobStoreConfig},
    config::ServerConfig,
    db,
    error::{PdsError, PdsResult},
    federation::{
        authentication::FederationAuthenticator,
        discovery::PdsDiscovery,
        dpop::{DPopNonceStore, DPopVerifier},
        search::FederatedSearch,
        NonceStore,
        RelayClient,
        RelayConfig,
    },
    identity::{DidCache, IdentityResolver, IdentityResolverConfig},
    mailer::Mailer,
    rate_limit::{RateLimiter, RateLimitConfig},
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
    pub dpop_verifier: Option<Arc<DPopVerifier>>,
    // Rate limiter
    pub rate_limiter: Arc<RateLimiter>,
    // Email mailer
    pub mailer: Arc<Mailer>,
}

impl AppContext {
    /// Create a new application context from configuration
    pub async fn new(config: ServerConfig) -> PdsResult<Self> {
        // Validate configuration
        config.validate()?;

        // Create data directories if they don't exist
        Self::ensure_directories(&config).await?;

        // Initialize account database
        let account_db = db::create_pool(&config.storage.account_db, db::DatabaseOptions::default()).await?;

        // Run database migrations (includes OAuth tables)
        db::run_migrations(&account_db).await?;

        // Test connection
        db::test_connection(&account_db).await?;

        // Initialize account manager
        let account_manager = Arc::new(AccountManager::new(account_db.clone(), Arc::new(config.clone())));

        // Initialize actor store
        let actor_store_config = ActorStoreConfig {
            base_directory: config.storage.actor_store_directory.clone(),
            cache_size: 100,
        };
        let actor_store = Arc::new(ActorStore::new(actor_store_config));

        // Initialize blob store
        let blob_store_config = BlobStoreConfig::default();
        let blob_store = Arc::new(BlobStore::new(blob_store_config, account_db.clone())?);

        // Initialize identity resolver
        // Note: Using account_db for now; could be separate database in future
        let did_cache = DidCache::new(account_db.clone());
        let identity_config = IdentityResolverConfig::default();
        let identity_resolver = Arc::new(
            IdentityResolver::new(did_cache, identity_config)?
        );

        // Initialize admin & moderation managers
        let admin_role_manager = Arc::new(AdminRoleManager::new(account_db.clone()));
        let moderation_manager = Arc::new(ModerationManager::new(account_db.clone(), account_manager.clone()));
        let label_manager = Arc::new(LabelManager::new(
            account_db.clone(),
            config.service.service_did.clone(),
        ));
        let invite_manager = Arc::new(InviteCodeManager::new(account_db.clone()));
        let report_manager = Arc::new(ReportManager::new(account_db.clone()));

        // Initialize relay client first (optional - only if relay servers configured and federation enabled)
        let relay_client = if config.federation.enabled && !config.federation.relay_urls.is_empty() {
            tracing::info!("Federation enabled with {} relay server(s)", config.federation.relay_urls.len());
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
            let auth = Arc::new(FederationAuthenticator::new(
                Arc::clone(&identity_resolver)
            ));

            // PDS discovery for finding other instances
            let discovery = Arc::new(PdsDiscovery::new(
                config.federation.relay_urls.clone()
            ));

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
                    10,  // max_concurrent requests
                    30,  // timeout_secs
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
            relay_client.clone()
        ));

        // Initialize rate limiter with Bluesky-compatible endpoint limits
        let rate_limiter = Arc::new(RateLimiter::with_bluesky_defaults(RateLimitConfig::default()));

        // Initialize mailer
        let mailer = Arc::new(Mailer::new(config.email.clone())?);

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
            sequencer,
            relay_client,
            federation_auth,
            pds_discovery,
            federated_search,
            nonce_store,
            dpop_nonce_store,
            dpop_verifier,
            rate_limiter,
            mailer,
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
