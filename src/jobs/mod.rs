use std::sync::Arc;
use tokio::time::{interval, Duration};
use tracing::{error, info};

pub mod tasks;

/// Job scheduler for background tasks
pub struct JobScheduler {
    context: Arc<crate::context::AppContext>,
}

impl JobScheduler {
    pub fn new(context: Arc<crate::context::AppContext>) -> Self {
        Self { context }
    }

    /// Start all background jobs
    pub fn start(self: Arc<Self>) {
        info!("Starting background job scheduler");

        // Spawn cleanup tasks
        tokio::spawn(Self::expired_session_cleanup_job(Arc::clone(&self)));
        tokio::spawn(Self::expired_suspension_cleanup_job(Arc::clone(&self)));
        tokio::spawn(Self::identity_cache_cleanup_job(Arc::clone(&self)));
        tokio::spawn(Self::account_deletion_job(Arc::clone(&self)));
        tokio::spawn(Self::temp_blob_cleanup_job(Arc::clone(&self)));

        // Spawn monitoring tasks
        tokio::spawn(Self::health_check_job(Arc::clone(&self)));

        // Spawn federation jobs (Phase 1)
        if self.context.config.federation.enabled && self.context.pds_discovery.is_some() {
            tokio::spawn(Self::pds_discovery_refresh_job(Arc::clone(&self)));
            info!("Federation discovery job started");
        }

        // Spawn relay firehose subscription job (Phase 3)
        if self.context.config.federation.enabled && self.context.relay_client.is_some() {
            tokio::spawn(Self::relay_firehose_subscription_job(Arc::clone(&self)));
            info!("Relay firehose subscription job started");
        }

        // Spawn nonce cleanup job (Phase 4)
        if self.context.config.federation.enabled && self.context.nonce_store.is_some() {
            tokio::spawn(Self::nonce_cleanup_job(Arc::clone(&self)));
            info!("Nonce cleanup job started");
        }

        // Spawn DPoP nonce cleanup job (Phase 4)
        if self.context.config.federation.enabled && self.context.dpop_nonce_store.is_some() {
            tokio::spawn(Self::dpop_nonce_cleanup_job(Arc::clone(&self)));
            info!("DPoP nonce cleanup job started");
        }

        info!("Background jobs started");
    }

    /// Cleanup expired sessions (runs every hour)
    async fn expired_session_cleanup_job(scheduler: Arc<Self>) {
        let mut interval = interval(Duration::from_secs(3600)); // Every hour

        loop {
            interval.tick().await;
            info!("Running expired session cleanup");

            match tasks::cleanup_expired_sessions(&scheduler.context).await {
                Ok(count) => {
                    if count > 0 {
                        info!("Cleaned up {} expired tokens (sessions + refresh tokens)", count);
                    } else {
                        info!("Session cleanup: no expired tokens found");
                    }
                }
                Err(e) => error!("Failed to cleanup expired sessions: {}", e),
            }
        }
    }

    /// Cleanup expired suspensions (runs every 15 minutes)
    async fn expired_suspension_cleanup_job(scheduler: Arc<Self>) {
        let mut interval = interval(Duration::from_secs(900)); // Every 15 minutes

        loop {
            interval.tick().await;
            info!("Running expired suspension cleanup");

            match tasks::cleanup_expired_suspensions(&scheduler.context).await {
                Ok(count) => {
                    if count > 0 {
                        info!("Cleaned up {} expired suspensions", count);
                    }
                }
                Err(e) => error!("Failed to cleanup expired suspensions: {}", e),
            }
        }
    }

    /// Cleanup expired identity cache entries (runs every 30 minutes)
    async fn identity_cache_cleanup_job(scheduler: Arc<Self>) {
        let mut interval = interval(Duration::from_secs(1800)); // Every 30 minutes

        loop {
            interval.tick().await;
            info!("Running identity cache cleanup");

            match tasks::cleanup_identity_cache(&scheduler.context).await {
                Ok(_) => {
                    // Silent success
                }
                Err(e) => error!("Failed to cleanup identity cache: {}", e),
            }
        }
    }

    /// Purge deleted accounts after grace period (runs daily)
    async fn account_deletion_job(scheduler: Arc<Self>) {
        let mut interval = interval(Duration::from_secs(86400)); // Every 24 hours

        loop {
            interval.tick().await;
            info!("Running account deletion job");

            match tasks::purge_deleted_accounts(&scheduler.context).await {
                Ok(count) => {
                    if count > 0 {
                        info!("Purged {} accounts after grace period", count);
                    } else {
                        info!("Account deletion: no accounts ready for purge");
                    }
                }
                Err(e) => error!("Failed to purge deleted accounts: {}", e),
            }
        }
    }

    /// Cleanup orphaned temp blobs (runs every 6 hours)
    async fn temp_blob_cleanup_job(scheduler: Arc<Self>) {
        let mut interval = interval(Duration::from_secs(21600)); // Every 6 hours

        loop {
            interval.tick().await;
            info!("Running temp blob cleanup job");

            match tasks::cleanup_orphaned_temp_blobs(&scheduler.context).await {
                Ok(count) => {
                    if count > 0 {
                        info!("Cleaned up {} orphaned temp blobs", count);
                    } else {
                        info!("Temp blob cleanup: no orphaned blobs found");
                    }
                }
                Err(e) => error!("Failed to cleanup orphaned temp blobs: {}", e),
            }
        }
    }

    /// Health check job (runs every 5 minutes)
    async fn health_check_job(scheduler: Arc<Self>) {
        let mut interval = interval(Duration::from_secs(300)); // Every 5 minutes

        loop {
            interval.tick().await;

            match tasks::health_check(&scheduler.context).await {
                Ok(_) => {
                    // Silent success - health is good
                }
                Err(e) => error!("Health check failed: {}", e),
            }
        }
    }

    /// PDS discovery refresh job (runs every 6 hours) - Phase 1
    async fn pds_discovery_refresh_job(scheduler: Arc<Self>) {
        let mut interval = interval(Duration::from_secs(21600)); // Every 6 hours

        loop {
            interval.tick().await;

            if let Some(discovery) = &scheduler.context.pds_discovery {
                info!("Running PDS discovery refresh");

                match discovery.refresh_instances().await {
                    Ok(_) => {
                        let instances = discovery.get_known_instances().await;
                        info!("PDS discovery: {} instance(s) found", instances.len());
                    }
                    Err(e) => error!("Failed to refresh PDS instances: {}", e),
                }
            }
        }
    }

    /// Relay firehose subscription job - Phase 3
    async fn relay_firehose_subscription_job(scheduler: Arc<Self>) {
        info!("Starting relay firehose subscription");

        let relay_client = match &scheduler.context.relay_client {
            Some(client) => client,
            None => {
                error!("Relay client not initialized");
                return;
            }
        };

        // Subscribe to firehose
        let mut relay_client_locked = relay_client.lock().await;
        let mut event_receiver = match relay_client_locked.subscribe_firehose().await {
            Ok(rx) => {
                info!("✓ Successfully subscribed to relay firehose");
                rx
            }
            Err(e) => {
                error!("Failed to subscribe to relay firehose: {}", e);
                return;
            }
        };
        drop(relay_client_locked);

        // Process events as they arrive
        let mut event_count = 0u64;
        while let Some(event) = event_receiver.recv().await {
            event_count += 1;

            // Log progress every 100 events
            if event_count % 100 == 0 {
                info!("Processed {} relay events", event_count);
            }

            // Process the event
            if let Err(e) = tasks::process_relay_event(&scheduler.context, event).await {
                error!("Failed to process relay event: {}", e);
            }
        }

        error!("Relay firehose subscription ended unexpectedly");
    }

    /// Nonce cleanup job (runs every 5 minutes) - Phase 4
    async fn nonce_cleanup_job(scheduler: Arc<Self>) {
        let mut interval = interval(Duration::from_secs(300)); // Every 5 minutes

        loop {
            interval.tick().await;

            if let Some(nonce_store) = &scheduler.context.nonce_store {
                match nonce_store.cleanup_expired().await {
                    Ok(count) => {
                        if count > 0 {
                            info!("Cleaned up {} expired nonces", count);
                        }
                    }
                    Err(e) => error!("Failed to cleanup expired nonces: {}", e),
                }
            }
        }
    }

    /// DPoP nonce cleanup job (runs every 5 minutes) - Phase 4
    async fn dpop_nonce_cleanup_job(scheduler: Arc<Self>) {
        let mut interval = interval(Duration::from_secs(300)); // Every 5 minutes

        loop {
            interval.tick().await;

            if let Some(dpop_nonce_store) = &scheduler.context.dpop_nonce_store {
                match dpop_nonce_store.cleanup_expired().await {
                    Ok(count) => {
                        if count > 0 {
                            info!("Cleaned up {} expired DPoP nonces", count);
                        }
                    }
                    Err(e) => error!("Failed to cleanup expired DPoP nonces: {}", e),
                }
            }
        }
    }
}
