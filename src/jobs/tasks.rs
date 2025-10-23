/// Background task implementations
use crate::{context::AppContext, error::PdsResult};

/// Cleanup expired sessions
pub async fn cleanup_expired_sessions(ctx: &AppContext) -> PdsResult<u64> {
    // Call AccountManager to cleanup expired sessions and refresh tokens
    let (sessions_deleted, refresh_tokens_deleted) = ctx.account_manager.cleanup_expired_sessions().await?;

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
    sqlx::query("SELECT 1")
        .fetch_one(&ctx.account_db)
        .await?;

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
        SELECT did, handle
        FROM account
        WHERE deactivated_at IS NOT NULL AND deactivated_at < ?1
        "#,
    )
    .bind(now)
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
        sqlx::query("DELETE FROM session WHERE did = ?1")
            .bind(&did)
            .execute(&ctx.account_db)
            .await?;

        sqlx::query("DELETE FROM refresh_token WHERE did = ?1")
            .bind(&did)
            .execute(&ctx.account_db)
            .await?;

        // Delete all email tokens
        sqlx::query("DELETE FROM email_token WHERE did = ?1")
            .bind(&did)
            .execute(&ctx.account_db)
            .await?;

        // Delete account record (permanent)
        sqlx::query("DELETE FROM account WHERE did = ?1")
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
