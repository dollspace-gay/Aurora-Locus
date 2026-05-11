// Allow dead_code - background jobs for future use
#![allow(dead_code)]

use std::sync::Arc;
use tokio::time::{interval, sleep, Duration};
use tracing::{error, info, warn};

pub mod tasks;

/// Job execution result with retry support
#[derive(Debug)]
pub enum JobResult {
    Success,
    Retry { after: Duration, attempt: u32 },
    Failed,
}

/// Execute a job with retry logic and monitoring
async fn execute_with_retry<F, Fut>(
    job_name: &str,
    max_retries: u32,
    operation: F,
) -> Result<(), String>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<(), crate::error::PdsError>>,
{
    let start = std::time::Instant::now();
    let mut attempt = 0;

    loop {
        attempt += 1;
        let attempt_start = std::time::Instant::now();

        match operation().await {
            Ok(_) => {
                let duration = start.elapsed().as_secs_f64();
                crate::metrics::record_background_job(job_name, "success", duration);
                return Ok(());
            }
            Err(e) => {
                if attempt >= max_retries {
                    let duration = start.elapsed().as_secs_f64();
                    crate::metrics::record_background_job(job_name, "failed", duration);
                    return Err(format!("Job failed after {} attempts: {}", attempt, e));
                }

                // Exponential backoff: 2^attempt seconds
                let backoff = Duration::from_secs(2u64.pow(attempt - 1).min(300)); // Max 5 minutes
                warn!(
                    "Job {} failed (attempt {}/{}): {}. Retrying in {:?}",
                    job_name, attempt, max_retries, e, backoff
                );

                crate::metrics::record_background_job(
                    job_name,
                    "retry",
                    attempt_start.elapsed().as_secs_f64(),
                );
                sleep(backoff).await;
            }
        }
    }
}

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
        tokio::spawn(Self::mod_event_seq_cleanup_job(Arc::clone(&self)));

        // Spawn monitoring tasks
        tokio::spawn(Self::health_check_job(Arc::clone(&self)));
        tokio::spawn(Self::metrics_collection_job(Arc::clone(&self)));

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

        // Spawn dpop_jti_replay reaper (Arc 7, chainlink #53).
        // Substrate-level cleanup for the cross-instance JTI
        // replay table; spawn unconditionally — the loop itself
        // skips the sweep in SingleInstanceInmemory mode where
        // distributed_store is None.
        tokio::spawn(Self::dpop_jti_replay_reaper_job(Arc::clone(&self)));
        info!("dpop_jti_replay reaper job started");

        // Spawn OAuth authorization_request cleanup job. The
        // sweeper has existed since Arc 7 Step 0 recon Q1 but
        // was previously unwired (no JobScheduler entry). Step 1
        // folds this in alongside the new reaper since Step 2
        // would need it wired anyway.
        tokio::spawn(Self::oauth_authorization_request_cleanup_job(Arc::clone(&self)));
        info!("OAuth authorization_request cleanup job started");

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
                        info!(
                            "Cleaned up {} expired tokens (sessions + refresh tokens)",
                            count
                        );
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

    /// Retention-bound the `mod_event_seq` table (runs every 24
    /// hours). Per chainlink #115 / docs/AURORA_ADMIN_UI_DESIGN.md
    /// §3.5, the live subscription channel is retention-bounded
    /// while `moderation_event` retains forever. Window controlled
    /// by `PDS_MOD_EVENT_RETENTION_DAYS` env var (default 7). Best-
    /// effort: a failed run logs at warn-level; the next run picks
    /// up the work. The table grows by one window's worth on a
    /// missed cleanup, no operational urgency.
    async fn mod_event_seq_cleanup_job(scheduler: Arc<Self>) {
        let mut interval = interval(Duration::from_secs(86400)); // Every 24 hours

        loop {
            interval.tick().await;
            let retention = tasks::mod_event_retention_days();
            info!(
                "Running mod_event_seq cleanup (retention: {} days)",
                retention
            );

            match tasks::cleanup_mod_event_seq(&scheduler.context).await {
                Ok(count) => {
                    if count > 0 {
                        info!(
                            "Cleaned up {} mod_event_seq rows older than {} days",
                            count, retention
                        );
                    }
                }
                Err(e) => warn!(
                    "Failed to cleanup mod_event_seq (retention {} days): {}",
                    retention, e
                ),
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
            if event_count.is_multiple_of(100) {
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

    /// `dpop_jti_replay` reaper sweep (Arc 7, V04_DESIGN.md
    /// §6.3.7). Substrate-level cleanup for the cross-instance
    /// JTI replay table. Runs every 5 minutes; per V04_DESIGN.md
    /// §6.3.7 the sweep is idempotent so concurrent invocations
    /// from sibling instances are fine.
    ///
    /// In `SingleInstanceInmemory` mode `distributed_store` is
    /// `None` and this loop tick is a continue/no-op. The task
    /// stays alive for process lifetime regardless of mode so a
    /// future runtime-toggle of the mode doesn't require
    /// respawning.
    async fn dpop_jti_replay_reaper_job(scheduler: Arc<Self>) {
        let mut interval = interval(Duration::from_secs(300)); // Every 5 minutes

        loop {
            interval.tick().await;
            let Some(store) = scheduler.context.distributed_store.as_ref() else {
                continue;
            };
            let now_epoch_ms = chrono::Utc::now().timestamp_millis();
            match store.reap_expired("dpop_jti_replay", now_epoch_ms).await {
                Ok(count) if count > 0 => info!(
                    table = "dpop_jti_replay",
                    count,
                    "Reaper swept expired entries"
                ),
                Ok(_) => {}
                Err(e) => warn!(
                    table = "dpop_jti_replay",
                    error = %e,
                    "Reaper sweep failed"
                ),
            }
        }
    }

    /// Sweep expired OAuth authorization requests every 5
    /// minutes. Pre-existing sweeper at
    /// `src/oauth/authorize.rs:cleanup_expired_requests` was
    /// previously unwired (Step 0 Q1 finding). Arc 7 Step 1
    /// folds the wiring in alongside the new substrate reaper.
    ///
    /// Not gated on the distributed-state mode — the
    /// `authorization_request` table lives in `account_db`
    /// regardless of substrate mode, so the sweeper is useful
    /// even in `SingleInstanceInmemory` deployments.
    async fn oauth_authorization_request_cleanup_job(scheduler: Arc<Self>) {
        let mut interval = interval(Duration::from_secs(300)); // Every 5 minutes

        loop {
            interval.tick().await;
            match crate::oauth::authorize::cleanup_expired_requests(&scheduler.context).await {
                Ok(count) if count > 0 => info!(
                    table = "authorization_request",
                    count,
                    "OAuth state cleanup swept expired requests"
                ),
                Ok(_) => {}
                Err(e) => warn!(
                    table = "authorization_request",
                    error = %e,
                    "OAuth state cleanup failed"
                ),
            }
        }
    }

    /// Metrics collection job (runs every 15 minutes)
    ///
    /// Periodically collects aggregate metrics about the PDS state:
    /// - Total accounts, active sessions
    /// - Repository record counts by collection
    /// - Storage sizes
    /// - Sequencer positions
    async fn metrics_collection_job(scheduler: Arc<Self>) {
        let mut interval = interval(Duration::from_secs(900)); // Every 15 minutes

        loop {
            interval.tick().await;

            match execute_with_retry("metrics_collection", 3, || {
                tasks::collect_aggregate_metrics(&scheduler.context)
            })
            .await
            {
                Ok(_) => {
                    // Silent success
                }
                Err(e) => error!("Metrics collection job error: {}", e),
            }
        }
    }
}
