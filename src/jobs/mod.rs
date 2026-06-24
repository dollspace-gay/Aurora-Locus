// Allow dead_code - background jobs for future use
#![allow(dead_code)]

use std::sync::Arc;
use tokio::time::{interval, sleep, Duration, MissedTickBehavior};
use tracing::{debug, error, info, warn};

pub mod tasks;

/// v0.8 arc 1 (#180) — keyset page size for the bind-audit orphan-marker
/// reconciliation sweep. Matches `GcSweepConfig`'s default page size; the
/// marker table is bounded by the (rare-by-construction) relay-race
/// failure rate, so a fixed constant suffices rather than a config knob.
const BIND_AUDIT_ORPHAN_RECONCILE_PAGE_SIZE: usize = 500;

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

        // Spawn rate_limit_buckets reaper (Arc 7 Step 3).
        // Inactivity-based GC: rows whose window_start hasn't
        // moved in 7 days are presumed cold and swept. Hourly
        // cadence — the 7-day threshold is coarse so frequent
        // sweeps add no value.
        tokio::spawn(Self::rate_limit_buckets_reaper_job(Arc::clone(&self)));
        info!("rate_limit_buckets reaper job started");

        // Spawn OAuth authorization_request cleanup job. The
        // sweeper has existed since Arc 7 Step 0 recon Q1 but
        // was previously unwired (no JobScheduler entry). Step 1
        // folds this in alongside the new reaper since Step 2
        // would need it wired anyway.
        tokio::spawn(Self::oauth_authorization_request_cleanup_job(Arc::clone(&self)));
        info!("OAuth authorization_request cleanup job started");

        // Spawn Arc 10 GC sweep job (chainlink #57). Off-by-
        // default — operators opt in via PDS_GC_SWEEP_ENABLED.
        // When enabled, the job reconciles blob storage
        // against `blob` + `temp_blob_metadata` and deletes
        // confirmed orphans subject to dry_run +
        // max_deletes_per_run safety mechanisms.
        if self.context.config.gc_sweep.enabled {
            tokio::spawn(Self::gc_sweep_job(Arc::clone(&self)));
            info!(
                interval_secs = self.context.config.gc_sweep.interval_secs,
                dry_run = self.context.config.gc_sweep.dry_run,
                max_deletes_per_run = self.context.config.gc_sweep.max_deletes_per_run,
                "GC sweep job scheduled"
            );
        } else {
            tracing::warn!("GC sweep job disabled (gc_sweep.enabled = false)");
        }

        // Arc 16d §9.4.4 Step 3.4: conditional spawn for row-walker.
        // Independent enable flag (`row_sweep_enabled`) so operators
        // can run byte-walker without row-walker (or vice versa).
        // Both walkers safely concurrent per §9.4.3.9 — Arc 10's
        // byte-walker doesn't touch `blob_metadata`, and Arc 16d's
        // row-walker doesn't touch `blob`/`temp_blob_metadata`.
        if self.context.config.gc_sweep.row_sweep_enabled {
            tokio::spawn(Self::row_sweep_job(Arc::clone(&self)));
            info!(
                interval_secs = self.context.config.gc_sweep.interval_secs,
                dry_run = self.context.config.gc_sweep.dry_run,
                untethered_ttl_seconds =
                    self.context.config.gc_sweep.untethered_ttl_seconds,
                max_deletes_per_run = self.context.config.gc_sweep.max_deletes_per_run,
                "row-sweep job scheduled (Arc 16d)"
            );
        } else {
            tracing::warn!(
                "row-sweep job disabled (gc_sweep.row_sweep_enabled = false)"
            );
        }

        // v0.8 arc 1 (#180): bind-audit orphan-marker reconciliation.
        // On-by-default (BindAuditOrphanMarkerConfig::enabled, opposite
        // polarity from gc_sweep) — orphan reconciliation is a
        // forensic/safety mechanism. Mirrors the conditional-spawn
        // shape of the GC sweeps above.
        if self.context.config.bind_audit_orphan_marker.enabled {
            tokio::spawn(Self::bind_audit_orphan_reconcile_job(Arc::clone(&self)));
            info!(
                interval_secs =
                    self.context.config.bind_audit_orphan_marker.reconcile_interval_secs,
                "bind-audit orphan reconcile job scheduled (v0.8 arc 1)"
            );
        } else {
            tracing::warn!("bind-audit orphan reconcile job disabled");
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

    /// Arc 10 GC sweep (V04_DESIGN.md §9.4.3, chainlink #57).
    /// Reconciles blob storage against `blob` +
    /// `temp_blob_metadata`; deletes confirmed orphans subject
    /// to dry_run + max_deletes_per_run safety mechanisms.
    /// Cadence + safety parameters come from
    /// `config.gc_sweep`; only spawned when
    /// `config.gc_sweep.enabled = true` (see `start()`).
    async fn gc_sweep_job(scheduler: Arc<Self>) {
        let interval_secs = scheduler.context.config.gc_sweep.interval_secs;
        let mut interval = interval(Duration::from_secs(interval_secs));
        // Arc 16d §9.4.4 Step 3.3: MissedTickBehavior::Skip on both
        // walkers. Without this, after a slow cycle (e.g., a large
        // backlog page taking > interval_secs), tokio fires the
        // backlog of missed ticks back-to-back which compounds load
        // exactly when the system is already strained.
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            interval.tick().await;

            let params = scheduler
                .context
                .config
                .gc_sweep
                .to_sweep_params(false);
            let now = chrono::Utc::now();

            info!(
                dry_run = params.dry_run,
                max_deletes_per_run = params.max_deletes_per_run,
                page_size = params.page_size,
                "Running GC sweep job"
            );

            match scheduler
                .context
                .blob_store
                .run_gc_sweep(params, now)
                .await
            {
                Ok(report) => {
                    info!(
                        pages_scanned = report.pages_scanned,
                        blobs_examined = report.blobs_examined,
                        authorized = report.authorized,
                        in_flight = report.in_flight,
                        too_young = report.too_young,
                        confirmed_orphans_found = report.confirmed_orphans_found,
                        orphans_deleted = report.orphans_deleted,
                        orphans_skipped_safety_cap = report.orphans_skipped_safety_cap,
                        duration_seconds = report.duration_seconds,
                        "GC sweep complete"
                    );
                }
                Err(e) => error!("GC sweep failed: {}", e),
            }
        }
    }

    /// Arc 16d §9.4.4 Step 3.2 — row-walker orchestrator parallel
    /// to [`Self::gc_sweep_job`]. Single-tasked per §9.4.3.7 (no
    /// self-overlap mutex needed — `tokio::spawn` of one task plus
    /// `MissedTickBehavior::Skip` together give the topology the
    /// design assumes).
    ///
    /// Cadence shared with the byte-walker via
    /// `gc_sweep.interval_secs`. Cycle-completion summary log
    /// includes `RowSweepReport` counters per Step 3.5; per-row
    /// INFO logging for every successful `backend.delete` is
    /// emitted inside `sweep_untethered_rows` itself (round-4 F2
    /// closure / §9.4.3.2 / §9.4.5.9 operator-action references
    /// require the log emission for residual-race investigation).
    async fn row_sweep_job(scheduler: Arc<Self>) {
        let cfg = &scheduler.context.config.gc_sweep;
        let interval_secs = cfg.interval_secs;
        let mut interval = interval(Duration::from_secs(interval_secs));
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            interval.tick().await;

            // Snapshot config per-cycle so a hot-reload (if any
            // ever lands) takes effect at cycle boundary.
            let cfg = &scheduler.context.config.gc_sweep;
            let params = crate::blob_store::gc::RowSweepParams {
                dry_run: cfg.dry_run,
                max_deletes_per_run: cfg.max_deletes_per_run,
                page_size: cfg.page_size,
                untethered_ttl: Duration::from_secs(cfg.untethered_ttl_seconds),
                #[cfg(test)]
                after_phase2_delete_hook: None,
            };
            let now = chrono::Utc::now();

            info!(
                dry_run = params.dry_run,
                max_deletes_per_run = params.max_deletes_per_run,
                page_size = params.page_size,
                untethered_ttl_seconds = cfg.untethered_ttl_seconds,
                "Running row-sweep job (Arc 16d)"
            );

            match scheduler
                .context
                .blob_store
                .run_row_sweep(params, now)
                .await
            {
                Ok(report) => {
                    info!(
                        rows_deleted = report.rows_deleted,
                        race_skip_count = report.race_skip_count,
                        bytes_delete_failure_count = report.bytes_delete_failure_count,
                        db_error_skip_count = report.db_error_skip_count,
                        bytes_delete_skipped_fresh_row_count =
                            report.bytes_delete_skipped_fresh_row_count,
                        total_eligible_count = ?report.total_eligible_count,
                        would_delete_count = ?report.would_delete_count,
                        pages_scanned = report.pages_scanned,
                        duration_seconds = report.duration_seconds,
                        "row-sweep cycle complete"
                    );
                }
                Err(e) => error!("row-sweep failed: {}", e),
            }
        }
    }

    /// v0.8 arc 1 (#180) — bind-audit orphan-marker reconciliation
    /// sweep. Mirrors `row_sweep_job`'s shape: `MissedTickBehavior::Skip`
    /// plus an immediate first tick (no offset = startup pass that
    /// catches markers left over from a process death between insert and
    /// sweep), snapshot config per cycle, INFO-log the report, and warn
    /// on failure then continue (the next cycle re-acquires leftovers).
    async fn bind_audit_orphan_reconcile_job(scheduler: Arc<Self>) {
        let interval_secs = scheduler
            .context
            .config
            .bind_audit_orphan_marker
            .reconcile_interval_secs;
        let mut interval = interval(Duration::from_secs(interval_secs));
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            interval.tick().await;

            let now = chrono::Utc::now();
            match crate::actor_store::orphan_reconcile::run_reconcile_pass(
                &scheduler.context.account_db,
                &scheduler.context.actor_store,
                BIND_AUDIT_ORPHAN_RECONCILE_PAGE_SIZE,
                now,
            )
            .await
            {
                Ok(report) => {
                    info!(
                        examined = report.examined,
                        marked_confirmed_orphan = report.marked_confirmed_orphan,
                        marked_record_present = report.marked_record_present,
                        left_unresolved_for_retry = report.left_unresolved_for_retry,
                        pages_scanned = report.pages_scanned,
                        duration_seconds = report.duration.as_secs_f64(),
                        "bind_audit_orphan_reconcile cycle complete"
                    );
                }
                // Best-effort posture (mirrors mod_event_seq_cleanup_job):
                // a failed run logs at warn-level; the next run picks up
                // the work.
                Err(e) => {
                    tracing::warn!(
                        target: "aurora_locus::orphan_reconcile",
                        event = "bind_audit_orphan_reconcile_cycle_failed",
                        error = %e,
                        "bind-audit orphan reconcile cycle failed; retrying next cycle"
                    );
                }
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

            // v0.9 Federation Pattern-1 Phase D (#354 / addendum §A6 M-5): skip
            // the scan while the boot-seed-failure flag is set — state-mutating
            // discovery (pending upsert / auto-accept) would be incoherent with
            // the refused operator mutation XRPCs.
            if scheduler
                .context
                .boot_seed_failed
                .load(std::sync::atomic::Ordering::Acquire)
            {
                debug!("Skipping discovery scan: boot_seed_failed flag is set");
                continue;
            }

            // v0.9 Federation Pattern-1 Phase C (#353 / design §3.2): read the
            // discovery mode once at scan-start. `discovery-disabled` skips the
            // scan entirely (no fetch, no scheduled_discovery_ran audit).
            let mode =
                crate::api::federation_discovery::current_mode(&scheduler.context).await;
            if mode == crate::api::federation_discovery::DiscoveryMode::DiscoveryDisabled {
                debug!("Discovery mode is discovery-disabled; skipping scheduled scan");
                continue;
            }

            if let Some(discovery) = &scheduler.context.pds_discovery {
                info!("Running PDS discovery refresh");

                match discovery.refresh_instances().await {
                    Ok(_) => {
                        let instances = discovery.get_known_instances().await;
                        info!("PDS discovery: {} instance(s) found", instances.len());
                        // Mode-aware per-peer processing + scheduled_discovery_ran
                        // audit (the scan_id is generated inside process_scan).
                        crate::api::federation_discovery::process_scan(
                            &scheduler.context,
                            &instances,
                            mode,
                            true,
                        )
                        .await;
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

    /// `rate_limit_buckets` reaper sweep (Arc 7 Step 3).
    /// Inactivity-based GC at a 7-day threshold (constant
    /// inside the substrate impl) — buckets with no recent
    /// `window_start_at_epoch_ms` updates are presumed cold
    /// and swept. The next first-touch self-reconstructs at
    /// full max_tokens, so the cost of an over-eager sweep is
    /// one extra INSERT.
    ///
    /// Hourly cadence: the 7-day threshold is coarse; minute-
    /// scale sweeps add no value. Per V04_DESIGN.md §6.3.7 the
    /// sweep is idempotent (DELETE WHERE) so concurrent
    /// invocations from sibling instances are fine.
    async fn rate_limit_buckets_reaper_job(scheduler: Arc<Self>) {
        let mut interval = interval(Duration::from_secs(3600)); // 1 hour

        loop {
            interval.tick().await;
            let Some(store) = scheduler.context.distributed_store.as_ref() else {
                continue;
            };
            let now_epoch_ms = chrono::Utc::now().timestamp_millis();
            match store.reap_expired("rate_limit_buckets", now_epoch_ms).await {
                Ok(count) if count > 0 => info!(
                    table = "rate_limit_buckets",
                    count,
                    "Rate-limit bucket reaper swept inactive buckets"
                ),
                Ok(_) => {}
                Err(e) => warn!(
                    table = "rate_limit_buckets",
                    error = %e,
                    "Rate-limit bucket reaper failed"
                ),
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
    /// minutes. Step 1 wired the pre-existing sweeper here
    /// (Step 0 Q1 finding); Step 2 routes the call through
    /// the `DistributedStore` trait now that the OAuth adapter
    /// exists, so the surface stays uniform with the
    /// `dpop_jti_replay_reaper_job` shape above.
    ///
    /// Not gated on the distributed-state mode — the
    /// `authorization_request` table lives in `account_db`
    /// regardless of substrate mode, and the registry's OAuth
    /// adapter is constructed in every mode. The
    /// `distributed_store` check is a defensive precondition
    /// matching the substrate reaper.
    async fn oauth_authorization_request_cleanup_job(scheduler: Arc<Self>) {
        let mut interval = interval(Duration::from_secs(300)); // Every 5 minutes

        loop {
            interval.tick().await;
            let Some(store) = scheduler.context.distributed_store.as_ref() else {
                continue;
            };
            let now_epoch_ms = chrono::Utc::now().timestamp_millis();
            match store.reap_expired("oauth_flow_state", now_epoch_ms).await {
                Ok(count) if count > 0 => info!(
                    table = "oauth_flow_state",
                    count,
                    "OAuth state reaper swept expired"
                ),
                Ok(_) => {}
                Err(e) => warn!(
                    table = "oauth_flow_state",
                    error = %e,
                    "OAuth state reaper failed"
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
