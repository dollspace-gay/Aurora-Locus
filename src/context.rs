/// Application context and dependency injection
use crate::{
    account::AccountManager,
    actor_store::{ActorStore, ActorStoreConfig},
    admin::{AdminRoleManager, InviteCodeManager, LabelManager, ModerationManager, ReportManager},
    blob_store::{BlobBackendType, BlobStorageConfig, BlobStore, BlobStoreConfig},
    config::{BlobstoreConfig, DatabaseConfig, DistributedStateMode, ServerConfig},
    db,
    distributed::{DistributedStore, DistributedStoreRegistry, PostgresCasStore},
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
    // Rate limiter (governor-backed, per-instance).
    pub rate_limiter: Arc<RateLimiter>,
    // Cross-instance rate-limit primitive (Arc 7 Step 3).
    // `Some` in Distributed mode, `None` in SingleInstanceInmemory.
    // The middleware consults this BEFORE the governor's
    // per-endpoint check so cross-instance correctness is
    // enforced first; the governor still runs as
    // per-instance defense-in-depth.
    pub distributed_rate_limiter: Option<Arc<crate::rate_limit::DistributedRateLimiter>>,
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
    /// Route registry for capability advertisement (Arc 8,
    /// V04_DESIGN.md §7.3.2 + §7.3.3). Populated at startup by
    /// `aurora_route_builder()` in `main.rs` and threaded
    /// through `AppContext::new`; consumed by
    /// `describe_capabilities` at request time once Step 3
    /// switches that handler over to the registry. Test
    /// fixtures pass an empty default — the field exists for
    /// every consumer but only `describe_capabilities` reads
    /// it once Step 3 lands. See [`crate::api::registry`].
    pub route_registry: Arc<crate::api::registry::RouteRegistry>,
    /// Arc 12 §5.3.3.1 trusted-iss allowlist for the
    /// service-auth fallback path. Constructed at
    /// `AppContext::new` from `[ctx.service_did(),
    /// ctx.entryway_did()?, config.federation.peer_pds[*].did,
    /// local_service_dids]` and **immutable for the process
    /// lifetime** per §5.5.7 restart-requirement. Constant-
    /// time membership lookup via `AppContext::is_trusted_iss`.
    /// Iss values failing the membership check reject at
    /// routing without PLC fetch per §5.3.3.1 boundary-case
    /// rejection (also rejects empty / non-DID / missing iss).
    pub trusted_iss: Arc<std::collections::HashSet<String>>,
    /// Arc 12 §5.3.9 + §5.4 Step 1.4 — forwarded-handler entryway
    /// HTTP client. `Some` when `config.entryway` is set; `None`
    /// in standalone mode. Used by §5.3.8 forwarded handlers
    /// (`signPlcOperation`, `updateHandle`, `getSession`,
    /// `requestPasswordReset`) to forward XRPC calls to the
    /// entryway.
    pub entryway_client: Option<Arc<crate::federation::EntrywayClient>>,
    /// Arc 12 §5.3.9 — admin-tier entryway client with the
    /// `Basic` auth header pre-bound from
    /// `config.entryway.admin_token`. `Some`/`None` symmetrically
    /// with `entryway_client`.
    pub entryway_admin_client: Option<Arc<crate::federation::EntrywayAdminClient>>,
}

/// Manual `Debug` impl per Arc 9 Step 2 (chainlink #55, V04_DESIGN.md
/// §8.4.1 Item 8). Two constraints drove the shape:
///
/// - `identity_resolver: Arc<dyn IdentityResolverApi>` and
///   `distributed_store: Option<Arc<dyn DistributedStore>>` hold
///   trait objects whose traits have no `Debug` supertrait;
///   `#[derive(Debug)]` would not compile.
/// - Many fields hold secret or auth-flow-relevant material that
///   must never appear in test logs, panic messages, or snapshot
///   fixtures: `config` (jwt_secret, repo signing key, PLC
///   rotation key, S3 secret_access_key, SMTP creds), `mailer`,
///   `nonce_store`, `dpop_nonce_store`, `dpop_verifier`, and the
///   user-record cache `local_records_cache`.
///
/// The impl prints opaque `<TypeName>` placeholders for those
/// fields. Pool / registry / file-tier-config fields print
/// normally — `sqlx::AnyPool::Debug` already redacts URLs, and
/// `RouteRegistry` plus `file_tier_settings` carry public
/// registration / configuration data. Future fields default to
/// opaque unless the author confirms they hold no secrets.
impl std::fmt::Debug for AppContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppContext")
            .field("config", &"<redacted: ServerConfig>")
            .field("account_db", &self.account_db)
            .field("account_manager", &"<AccountManager>")
            .field("actor_store", &"<ActorStore>")
            .field("blob_store", &"<BlobStore>")
            .field("identity_resolver", &"<dyn IdentityResolverApi>")
            .field("admin_role_manager", &"<AdminRoleManager>")
            .field("moderation_manager", &"<ModerationManager>")
            .field("label_manager", &"<LabelManager>")
            .field("invite_manager", &"<InviteCodeManager>")
            .field("report_manager", &"<ReportManager>")
            .field("oauth_client_manager", &"<ClientManager>")
            .field("oauth_device_manager", &"<DeviceManager>")
            .field("sequencer", &"<Sequencer>")
            .field(
                "relay_client",
                &self.relay_client.as_ref().map(|_| "<RelayClient>"),
            )
            .field(
                "federation_auth",
                &self.federation_auth.as_ref().map(|_| "<FederationAuthenticator>"),
            )
            .field(
                "pds_discovery",
                &self.pds_discovery.as_ref().map(|_| "<PdsDiscovery>"),
            )
            .field(
                "federated_search",
                &self.federated_search.as_ref().map(|_| "<FederatedSearch>"),
            )
            .field(
                "nonce_store",
                &self.nonce_store.as_ref().map(|_| "<NonceStore>"),
            )
            .field(
                "dpop_nonce_store",
                &self.dpop_nonce_store.as_ref().map(|_| "<DPopNonceStore>"),
            )
            .field("dpop_verifier", &"<DPopVerifier>")
            .field("rate_limiter", &"<RateLimiter>")
            .field(
                "distributed_rate_limiter",
                &self.distributed_rate_limiter.as_ref().map(|_| "<DistributedRateLimiter>"),
            )
            .field("mailer", &"<Mailer>")
            .field("local_records_cache", &"<LocalRecordsCache>")
            .field("cache_invalidator", &"<CacheInvalidator>")
            .field("file_tier_settings", &self.file_tier_settings)
            .field("maintenance_pool", &self.maintenance_pool)
            .field(
                "distributed_store",
                &self.distributed_store.as_ref().map(|_| "<dyn DistributedStore>"),
            )
            .field("route_registry", &self.route_registry)
            .finish()
    }
}

impl AppContext {
    /// Create a new application context from configuration.
    ///
    /// `route_registry` is the populated registry returned by
    /// `crate::api::routes()`'s builder pair, threaded in by the
    /// startup flow so this constructor doesn't need to know
    /// about route declarations. Tests use
    /// `Arc::new(crate::api::registry::RouteRegistry::default())`
    /// — the empty registry is fine for non-`describe_capabilities`
    /// code paths.
    pub async fn new(
        config: ServerConfig,
        route_registry: Arc<crate::api::registry::RouteRegistry>,
    ) -> PdsResult<Self> {
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
        // Substrate (DPoP + rate-limit tables, in the
        // maintenance pool). Optional — `SingleInstanceInmemory`
        // mode skips it. `Redis` mode is rejected at
        // config.validate() so it never reaches this match.
        let (maintenance_pool, substrate) = match config.distributed_state_mode {
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
                let substrate: Arc<dyn DistributedStore> =
                    Arc::new(PostgresCasStore::new(Arc::clone(&pool)));
                tracing::info!(
                    max_connections = config.maintenance_pool.max_connections,
                    min_connections = config.maintenance_pool.min_connections,
                    "Distributed-state substrate initialized (Postgres-CAS)"
                );
                (Some(pool), Some(substrate))
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

        // OAuth-state adapter (wraps account_db, not the
        // maintenance pool) — always present. The underlying
        // authorization_request table lives in account_db
        // regardless of substrate mode, and OAuth flows need
        // cross-instance coherence even in
        // SingleInstanceInmemory mode (where the substrate
        // skipping is fine because there are no siblings).
        let oauth_adapter: Arc<dyn DistributedStore> = Arc::new(
            crate::oauth::OAuthFlowStateAdapter::new(Arc::new(account_db.clone())),
        );

        // Registry: consumer-facing facade routing per-table
        // operations to the right impl. AppContext consumers
        // depend on Arc<dyn DistributedStore>; the registry
        // hides the dispatch.
        let distributed_store: Option<Arc<dyn DistributedStore>> = Some(Arc::new(
            DistributedStoreRegistry::new(substrate, Arc::clone(&oauth_adapter)),
        ));

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

            // Arc 12 §5.3.2 Gap 3: at-startup bootstrap of
            // peer-PDS map from `config.federation.peer_pds`.
            // The map is populated once here; runtime mutation
            // surfaces (`refresh_instances`, ops endpoints)
            // continue to layer on top. Per §5.5.1, two-instance
            // Phase B doesn't require runtime cross-instance
            // routing — config-bootstrap suffices for v0.5.
            for peer in &config.federation.peer_pds {
                let instance = crate::federation::discovery::PdsInstance {
                    did: peer.did.clone(),
                    url: peer.url.clone(),
                    name: None,
                    open_registrations: false,
                    user_count: None,
                    last_seen: None,
                    features: Vec::new(),
                };
                discovery.add_instance(instance).await;
                tracing::info!(
                    did = %peer.did,
                    url = %peer.url,
                    "Arc 12 §5.3.2 Gap 3: registered peer-PDS at startup"
                );
            }

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
        //
        // Arc 7 Step 3: in Distributed mode the JTI-replay path
        // additionally routes through the substrate (cross-instance
        // single-use enforcement). The substrate handle is wired
        // through the `with_distributed_store` builder; the
        // server-nonce half stays in-memory regardless of mode
        // (federation-scoped, no cross-instance correctness story
        // in v0.4 per Step 0 OQ3).
        let make_dpop_store = || {
            let store = DPopNonceStore::new();
            if let Some(substrate) = distributed_store.as_ref() {
                store.with_distributed_store(Arc::clone(substrate))
            } else {
                store
            }
        };
        let dpop_nonce_store: Option<Arc<DPopNonceStore>> = if config.federation.enabled {
            tracing::info!(
                "Initializing DPoP §8 nonce challenge store (federation enabled)"
            );
            Some(Arc::new(make_dpop_store()))
        } else {
            None
        };
        let dpop_verifier = {
            let store_for_verifier = match &dpop_nonce_store {
                Some(s) => Arc::clone(s),
                None => Arc::new(make_dpop_store()),
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

        // Distributed rate-limit primitive (Arc 7 Step 3). One
        // construction site, mode-gated on the maintenance pool's
        // presence — `Distributed` mode has both; the other modes
        // have neither. The middleware consults this for
        // cross-instance bucket coherence; the governor above
        // stays running as per-instance defense.
        let distributed_rate_limiter = maintenance_pool.as_ref().map(|pool| {
            Arc::new(crate::rate_limit::DistributedRateLimiter::new(Arc::clone(pool)))
        });

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

        // Route registry — threaded in by the caller (Step 2:
        // `main.rs` builds `aurora_route_builder()` first, then
        // passes the populated registry here). Step 3 will
        // switch `describe_capabilities` to read from this
        // field; until then, the handler still reads the
        // hand-curated lists at `admin.rs`, so the registry's
        // contents are write-only at runtime — but the
        // construction-time wiring is load-bearing so the test
        // fixtures' empty-registry paths also flow through this
        // arg.

        // Arc 12 §5.3.3.1 trusted-iss allowlist — built once at
        // construction time and frozen for the process lifetime
        // (§5.5.7 restart-requirement). Pre-Step-1, the entryway
        // DID slot is absent; Step 1.1 lands EntrywayConfig and
        // a follow-up amendment extends this construction.
        // local_service_dids is a forward-compatibility slot for
        // admin-tier service identities from src/service_auth.rs
        // flows — currently empty; future cycles may populate.
        let mut trusted_iss_set = std::collections::HashSet::new();
        trusted_iss_set.insert(config.service.service_did.clone());
        for peer in &config.federation.peer_pds {
            trusted_iss_set.insert(peer.did.clone());
        }
        // Arc 12 §5.4 Step 1.1: seed the entryway DID once
        // EntrywayConfig is set. The set remains immutable for the
        // process lifetime per §5.5.7 — toggling entryway mode
        // requires a restart.
        if let Some(entryway) = &config.entryway {
            trusted_iss_set.insert(entryway.did.clone());
        }
        let trusted_iss = Arc::new(trusted_iss_set);

        // Arc 12 §5.4 Step 1.4: entryway HTTP clients. Constructed
        // once at startup when entryway mode is configured;
        // `None`/`None` in standalone mode. The clients are
        // dispatch-only wrappers in Step 1; method surfaces for
        // mint-pattern forwarding (`entryway_auth_headers`) and
        // passthru forwarding (`entryway_passthru_headers`) land in
        // Step 2, and per-handler dispatch lands in Step 3.
        let (entryway_client, entryway_admin_client) = match &config.entryway {
            Some(entryway_cfg) => {
                let client = crate::federation::EntrywayClient::new(entryway_cfg.url.clone())
                    .map_err(|e| {
                        PdsError::Internal(format!(
                            "Failed to build entryway forwarded-handler HTTP client: {}",
                            e
                        ))
                    })?;
                let admin_client = crate::federation::EntrywayAdminClient::new(
                    entryway_cfg.url.clone(),
                    &entryway_cfg.admin_token,
                )
                .map_err(|e| {
                    PdsError::Internal(format!(
                        "Failed to build entryway admin HTTP client: {}",
                        e
                    ))
                })?;
                tracing::info!(
                    entryway_url = %entryway_cfg.url,
                    entryway_did = %entryway_cfg.did,
                    "Arc 12 §5.3.9: constructed entryway clients (forwarded + admin)"
                );
                (Some(Arc::new(client)), Some(Arc::new(admin_client)))
            }
            None => (None, None),
        };

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
            route_registry,
            trusted_iss,
            entryway_client,
            entryway_admin_client,
        })
    }

    /// Arc 12 §5.3.3.1: constant-time membership check for the
    /// trusted-iss allowlist. Empty / non-DID / missing iss
    /// uniformly rejects (the HashSet stores fully-qualified
    /// DIDs; shape-invalid values can't be members). Caller
    /// MUST reject without PLC fetch when this returns `false`.
    pub fn is_trusted_iss(&self, iss: &str) -> bool {
        if iss.is_empty() || !iss.starts_with("did:") {
            return false;
        }
        self.trusted_iss.contains(iss)
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

    /// Get service URL.
    ///
    /// Arc 12 §5.3.2 Gap 1 closure: delegates to
    /// `ServiceConfig::effective_public_url()` which reads
    /// `service.public_url` when set (via
    /// `PDS_SERVICE_PUBLIC_URL`), otherwise derives
    /// `{scheme}://{hostname}[:{port}]` with localhost-aware
    /// scheme selection. Preserves v0.4 behavior when
    /// `public_url` is unset on a localhost deployment.
    pub fn service_url(&self) -> String {
        self.config.service.effective_public_url()
    }

    /// Get service DID
    pub fn service_did(&self) -> &str {
        &self.config.service.service_did
    }

    /// Arc 12 §5.3.4 / §5.3.9: configured entryway DID, `None` in
    /// standalone mode. Used by `require_auth_forwarded` to build
    /// the multi-audience allowlist and by `AppContext::new` to
    /// seed the trusted-iss set with the entryway DID.
    pub fn entryway_did(&self) -> Option<&str> {
        self.config.entryway.as_ref().map(|c| c.did.as_str())
    }

    /// Arc 12 §5.3.4.1 shared verification helper. Routes a bearer
    /// token through the §5.3.3 tuple table, honoring the caller-
    /// supplied audience allowlist for the destination routes that
    /// check audience. The two middleware variants
    /// (`require_auth_unified` / `require_auth_forwarded`) are thin
    /// wrappers around this method that differ only in their
    /// allowlist.
    ///
    /// Returns a `UnifiedAuthContext` whose variant identifies the
    /// validated path: `Local` for the DB-lookup local-verify path,
    /// `OAuth` for the opaque-token DB-lookup OAuth path, and
    /// `CrossPDS` for both the entryway external-verify and the
    /// trusted-iss service-auth fallback (both produce a verified
    /// did-bearing claim from a remote-trust path).
    pub async fn verify_jwt_with_allowlist(
        &self,
        token: &str,
        audience_allowlist: &[&str],
    ) -> PdsResult<crate::api::middleware::UnifiedAuthContext> {
        crate::auth::verify_jwt_with_allowlist_impl(self, token, audience_allowlist).await
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
