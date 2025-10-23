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
}
