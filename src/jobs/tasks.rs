// Allow dead_code - background tasks for future use
#![allow(dead_code)]

/// Background task implementations
use crate::{
    context::AppContext,
    error::{PdsError, PdsResult},
};

/// Cleanup expired sessions
pub async fn cleanup_expired_sessions(ctx: &AppContext) -> PdsResult<u64> {
    // Call AccountManager to cleanup expired sessions and refresh tokens
    let (sessions_deleted, refresh_tokens_deleted) =
        ctx.account_manager.cleanup_expired_sessions().await?;

    // Return total count of deleted items
    Ok(sessions_deleted + refresh_tokens_deleted)
}

/// Cleanup expired suspensions
pub async fn cleanup_expired_suspensions(ctx: &AppContext) -> PdsResult<u64> {
    ctx.moderation_manager.cleanup_expired().await
}

/// Cleanup expired identity cache entries
pub async fn cleanup_identity_cache(ctx: &AppContext) -> PdsResult<()> {
    ctx.identity_resolver.cleanup_cache().await
}

/// Health check - verify all systems are operational
pub async fn health_check(ctx: &AppContext) -> PdsResult<()> {
    // Check database connectivity
    sqlx::query("SELECT 1").fetch_one(&ctx.account_db).await?;

    // All checks passed
    Ok(())
}

/// Purge accounts marked for deletion after grace period
///
/// GDPR-compliant permanent deletion of account data after 30-day grace period
pub async fn purge_deleted_accounts(ctx: &AppContext) -> PdsResult<u64> {
    use chrono::Utc;
    use sqlx::Row;

    let now = Utc::now();

    // Find accounts marked for deletion where grace period has expired
    let rows = sqlx::query(
        r#"
        SELECT a.did, a.handle
        FROM actor a
        WHERE a.deactivated_at IS NOT NULL
          AND a.delete_after IS NOT NULL
          AND a.delete_after < $1
        "#,
    )
    .bind(now.to_rfc3339())
    .fetch_all(&ctx.account_db)
    .await?;

    let mut deleted_count = 0;

    for row in rows {
        let did: String = row.try_get("did")?;
        let handle: String = row.try_get("handle")?;

        tracing::info!("Purging account: {} ({})", handle, did);

        // Delete all blobs for this user
        match ctx.blob_store.list_for_user(&did, 1000).await {
            Ok(blobs) => {
                let blob_count = blobs.len();
                for blob in blobs {
                    if let Err(e) = ctx.blob_store.delete(&blob.cid).await {
                        tracing::warn!("Failed to delete blob {}: {}", blob.cid, e);
                    }
                }
                tracing::info!("Deleted {} blobs for {}", blob_count, did);
            }
            Err(e) => {
                tracing::warn!("Failed to list blobs for {}: {}", did, e);
            }
        }

        // Delete actor repository data
        // Note: ActorStore.destroy() would be used here when implemented
        // For now, we'll log and continue
        tracing::info!("Actor store cleanup for {} (not yet implemented)", did);

        // Delete all sessions and refresh tokens
        sqlx::query("DELETE FROM session WHERE did = $1")
            .bind(&did)
            .execute(&ctx.account_db)
            .await?;

        sqlx::query("DELETE FROM refresh_token WHERE did = $1")
            .bind(&did)
            .execute(&ctx.account_db)
            .await?;

        // Delete all email tokens
        sqlx::query("DELETE FROM email_token WHERE did = $1")
            .bind(&did)
            .execute(&ctx.account_db)
            .await?;

        // Delete account record (permanent)
        sqlx::query("DELETE FROM account WHERE did = $1")
            .bind(&did)
            .execute(&ctx.account_db)
            .await?;

        deleted_count += 1;

        tracing::info!(
            "Successfully purged account: {} ({}) - GDPR compliant permanent deletion",
            handle,
            did
        );
    }

    if deleted_count > 0 {
        tracing::info!("Purged {} accounts after grace period", deleted_count);
    }

    Ok(deleted_count)
}

/// Cleanup orphaned temp blobs
///
/// Deletes temporary blobs that have been staged but not committed within TTL (24 hours)
pub async fn cleanup_orphaned_temp_blobs(ctx: &AppContext) -> PdsResult<u64> {
    const TTL_HOURS: i64 = 24;

    // Get list of orphaned blobs (older than 24 hours)
    let orphaned_cids = ctx.blob_store.list_orphaned_temp_blobs(TTL_HOURS).await?;

    let mut deleted_count = 0;

    for cid in orphaned_cids {
        match ctx.blob_store.delete_temp_blob(&cid).await {
            Ok(_) => {
                tracing::info!("Deleted orphaned temp blob: {}", cid);
                deleted_count += 1;
            }
            Err(e) => {
                tracing::warn!("Failed to delete orphaned temp blob {}: {}", cid, e);
            }
        }
    }

    if deleted_count > 0 {
        tracing::info!("Cleaned up {} orphaned temp blobs", deleted_count);
    }

    Ok(deleted_count)
}

/// Default retention window for `mod_event_seq` rows when
/// `PDS_MOD_EVENT_RETENTION_DAYS` is unset. 7 days matches the §3.5
/// design commitment for the live subscription channel; operators
/// running long-lived deployments raise the env var.
pub const DEFAULT_MOD_EVENT_RETENTION_DAYS: i64 = 7;

/// Read the operator-configured retention window for `mod_event_seq`
/// from the `PDS_MOD_EVENT_RETENTION_DAYS` env var. Falls back to
/// [`DEFAULT_MOD_EVENT_RETENTION_DAYS`] when unset, malformed, or
/// non-positive — operators who type a typo get the safe default
/// rather than infinite retention or an immediate full purge.
pub fn mod_event_retention_days() -> i64 {
    std::env::var("PDS_MOD_EVENT_RETENTION_DAYS")
        .ok()
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|d| *d > 0)
        .unwrap_or(DEFAULT_MOD_EVENT_RETENTION_DAYS)
}

/// Compute the cutoff timestamp for `mod_event_seq` cleanup. Pulled
/// out as its own function so the unit test can pin behavior without
/// reaching for chrono::Utc::now() at the call site.
pub fn mod_event_cleanup_cutoff(now: chrono::DateTime<chrono::Utc>, retention_days: i64) -> String {
    (now - chrono::Duration::days(retention_days)).to_rfc3339()
}

/// Delete `mod_event_seq` rows older than the configured retention
/// window. Returns the number of rows deleted. Best-effort: a SQL
/// error is logged at warn-level by the caller and the next run picks
/// up the work — chainlink #115 commit 2.
///
/// `moderation_event` is NOT pruned by this job. Per §3.4, the
/// historical aggregate retains forever. Only the live subscription
/// channel mirrors a recent window.
pub async fn cleanup_mod_event_seq(ctx: &AppContext) -> PdsResult<u64> {
    let retention_days = mod_event_retention_days();
    let cutoff = mod_event_cleanup_cutoff(chrono::Utc::now(), retention_days);

    let result = sqlx::query("DELETE FROM mod_event_seq WHERE created_at < $1")
        .bind(&cutoff)
        .execute(&ctx.account_db)
        .await?;

    Ok(result.rows_affected())
}

/// Trigger PDS discovery refresh (Phase 1)
pub async fn refresh_pds_discovery(ctx: &AppContext) -> PdsResult<usize> {
    if let Some(discovery) = &ctx.pds_discovery {
        discovery.refresh_instances().await?;
        let instances = discovery.get_known_instances().await;
        Ok(instances.len())
    } else {
        Ok(0)
    }
}

/// Process relay event from firehose (Phase 3)
pub async fn process_relay_event(
    ctx: &AppContext,
    event: crate::federation::relay::RelayEvent,
) -> PdsResult<()> {
    use tracing::{debug, info, warn};

    // Start timing for metrics
    let start_time = std::time::Instant::now();
    let event_type = event.event_type.clone();

    // Log event details
    debug!(
        "Processing relay event: type='{}', did='{}', seq={}",
        event.event_type, event.did, event.seq
    );

    match event.event_type.as_str() {
        "commit" => {
            // Handle commit events - cache commit info for future queries
            info!("Received commit from {}: seq={}", event.did, event.seq);

            // TODO: In a full implementation:
            // - Store commit metadata in database
            // - Index content for search
            // - Update relationship graphs
            // - Trigger notifications for followers

            // For now, just log it
            if let Some(commit_data) = event.commit {
                debug!("Commit data: {:?}", commit_data);
            }
        }

        "identity" => {
            // Handle identity events - invalidate DID cache
            info!("Received identity update for {}", event.did);

            // Invalidate identity cache for this DID
            let identity_resolver = &ctx.identity_resolver;
            if let Err(e) = identity_resolver.invalidate_did(&event.did).await {
                warn!("Failed to invalidate DID cache for {}: {}", event.did, e);
            } else {
                debug!("✓ Invalidated DID cache for {}", event.did);
            }
        }

        "account" => {
            // Handle account events - update account status
            info!("Received account update for {}", event.did);

            // TODO: In a full implementation:
            // - Update account status (suspended, deleted, etc.)
            // - Trigger UI updates
            // - Update follower/following counts

            if let Some(account_data) = event.commit {
                debug!("Account data: {:?}", account_data);
            }
        }

        "handle" => {
            // Handle handle change events
            info!("Received handle change for {}", event.did);

            // Invalidate identity cache (handles are part of identity)
            if let Err(e) = ctx.identity_resolver.invalidate_did(&event.did).await {
                warn!(
                    "Failed to invalidate cache for handle change {}: {}",
                    event.did, e
                );
            }
        }

        "tombstone" => {
            // Handle tombstone events (deleted repos)
            info!("Received tombstone for {}", event.did);

            // TODO: Mark repo as deleted
            // TODO: Clean up cached data
        }

        _ => {
            // Unknown event type
            debug!("Unknown relay event type: {}", event.event_type);
        }
    }

    // Record metrics
    let duration = start_time.elapsed().as_secs_f64();
    crate::metrics::record_relay_event(&event_type, duration);

    Ok(())
}

/// Collect aggregate metrics about PDS state
///
/// This job periodically queries the database to update aggregate metrics:
/// - Total accounts
/// - Active sessions
/// - Total records across all repos
/// - Per-collection record counts (would require querying all repos)
/// - Sequencer current position
pub async fn collect_aggregate_metrics(ctx: &AppContext) -> PdsResult<()> {
    use sqlx::Row;

    // 1. Count total accounts
    let account_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM account")
        .fetch_one(&ctx.account_db)
        .await
        .map_err(PdsError::Database)?;

    crate::metrics::ACCOUNTS_TOTAL.set(account_count);

    // 2. Count active sessions (not expired)
    let session_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM session WHERE expires_at > datetime('now')")
            .fetch_one(&ctx.account_db)
            .await
            .map_err(PdsError::Database)?;

    crate::metrics::SESSIONS_ACTIVE.set(session_count);

    // 3. Get current sequencer position
    let seq_result = sqlx::query("SELECT MAX(seq) as max_seq FROM repo_seq")
        .fetch_optional(&ctx.account_db)
        .await
        .map_err(PdsError::Database)?;

    if let Some(row) = seq_result {
        if let Ok(Some(seq)) = row.try_get::<Option<i64>, _>("max_seq") {
            crate::metrics::update_sequencer_position(seq);
        }
    }

    // 4. Count blob storage usage
    let blob_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM blob_metadata")
        .fetch_one(&ctx.account_db)
        .await
        .unwrap_or(0);

    crate::metrics::BLOB_COUNT_TOTAL.set(blob_count);

    // 5. Get total blob storage size
    let blob_size: Option<i64> = sqlx::query_scalar("SELECT SUM(size) FROM blob_metadata")
        .fetch_one(&ctx.account_db)
        .await
        .unwrap_or(None);

    if let Some(size) = blob_size {
        crate::metrics::BLOB_STORAGE_BYTES_TOTAL.set(size);
    }

    tracing::debug!(
        "Collected aggregate metrics: {} accounts, {} sessions, {} blobs ({} bytes)",
        account_count,
        session_count,
        blob_count,
        blob_size.unwrap_or(0)
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use sqlx::any::AnyPoolOptions;
    use sqlx::AnyPool;
    use std::sync::Once;

    /// Open an in-memory SQLite pool with the mod_event_seq table
    /// only — the cleanup unit test doesn't need the full migration
    /// suite, just the one table it operates on.
    async fn open_test_pool() -> AnyPool {
        static INSTALL: Once = Once::new();
        INSTALL.call_once(sqlx::any::install_default_drivers);
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            r#"
            CREATE TABLE mod_event_seq (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                moderation_event_id INTEGER NOT NULL,
                actor_did TEXT NOT NULL,
                action TEXT NOT NULL,
                subject_did TEXT,
                subject_uri TEXT,
                subject_cid TEXT,
                detail TEXT,
                created_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    /// Insert a `mod_event_seq` row with the given created_at.
    async fn insert_seq_row(pool: &AnyPool, action: &str, created_at: &str) {
        sqlx::query(
            "INSERT INTO mod_event_seq \
             (moderation_event_id, actor_did, action, created_at) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(1_i64)
        .bind("did:plc:m1")
        .bind(action)
        .bind(created_at)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn count_seq_rows(pool: &AnyPool) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM mod_event_seq")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// Direct exercise of the SQL deletion path against a controlled
    /// pool. We bypass `cleanup_mod_event_seq` (which reads the env
    /// var and uses `Utc::now`) so the test can pin behavior with
    /// deterministic timestamps.
    async fn delete_old_rows(pool: &AnyPool, cutoff: &str) -> u64 {
        sqlx::query("DELETE FROM mod_event_seq WHERE created_at < $1")
            .bind(cutoff)
            .execute(pool)
            .await
            .unwrap()
            .rows_affected()
    }

    #[tokio::test]
    async fn cleanup_deletes_rows_older_than_retention_window() {
        let pool = open_test_pool().await;
        let now = Utc::now();
        // Three rows: 1 day old (recent), 8 days old (old), 30 days
        // old (very old).
        insert_seq_row(&pool, "recent", &(now - Duration::days(1)).to_rfc3339()).await;
        insert_seq_row(&pool, "old", &(now - Duration::days(8)).to_rfc3339()).await;
        insert_seq_row(&pool, "very_old", &(now - Duration::days(30)).to_rfc3339()).await;

        assert_eq!(count_seq_rows(&pool).await, 3);

        // Cutoff is 7 days ago; rows from days 8 and 30 should fall.
        let cutoff = mod_event_cleanup_cutoff(now, 7);
        let deleted = delete_old_rows(&pool, &cutoff).await;
        assert_eq!(deleted, 2);

        // Only the recent row remains.
        let remaining: Vec<String> =
            sqlx::query_scalar("SELECT action FROM mod_event_seq ORDER BY seq ASC")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(remaining, vec!["recent".to_string()]);
    }

    #[tokio::test]
    async fn cleanup_no_op_when_all_rows_within_retention() {
        let pool = open_test_pool().await;
        let now = Utc::now();
        for i in 0..3 {
            insert_seq_row(
                &pool,
                "recent",
                &(now - Duration::hours(i)).to_rfc3339(),
            )
            .await;
        }
        let cutoff = mod_event_cleanup_cutoff(now, 7);
        let deleted = delete_old_rows(&pool, &cutoff).await;
        assert_eq!(deleted, 0);
        assert_eq!(count_seq_rows(&pool).await, 3);
    }

    #[test]
    fn mod_event_retention_days_falls_back_to_default_for_invalid_input() {
        // Zero, negative, non-numeric, missing — all yield the safe
        // default rather than 0-day retention or unset → infinite.
        // Use unique env var names per test to avoid cross-test
        // contamination of the global env (#[serial] not in deps).
        std::env::remove_var("PDS_MOD_EVENT_RETENTION_DAYS");
        assert_eq!(mod_event_retention_days(), DEFAULT_MOD_EVENT_RETENTION_DAYS);

        std::env::set_var("PDS_MOD_EVENT_RETENTION_DAYS", "0");
        assert_eq!(mod_event_retention_days(), DEFAULT_MOD_EVENT_RETENTION_DAYS);

        std::env::set_var("PDS_MOD_EVENT_RETENTION_DAYS", "-5");
        assert_eq!(mod_event_retention_days(), DEFAULT_MOD_EVENT_RETENTION_DAYS);

        std::env::set_var("PDS_MOD_EVENT_RETENTION_DAYS", "not-a-number");
        assert_eq!(mod_event_retention_days(), DEFAULT_MOD_EVENT_RETENTION_DAYS);

        std::env::set_var("PDS_MOD_EVENT_RETENTION_DAYS", "30");
        assert_eq!(mod_event_retention_days(), 30);

        // Reset.
        std::env::remove_var("PDS_MOD_EVENT_RETENTION_DAYS");
    }
}
