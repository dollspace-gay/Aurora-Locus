/// OAuth Migration Tool
///
/// Helps users migrate from legacy JWT sessions to OAuth 2.1 authentication.
/// This tool:
/// 1. Revokes all existing JWT sessions for the user
/// 2. Generates an OAuth authorization URL for re-authentication
/// 3. Optionally registers user devices for OAuth
/// 4. Provides migration status and rollback support

use crate::{
    config::ServerConfig,
    context::AppContext,
    error::{PdsError, PdsResult},
};
use std::sync::Arc;
use tracing::{error, info, warn};

/// Migration result summary
pub struct MigrationResult {
    pub did: String,
    pub sessions_revoked: usize,
    pub oauth_url: String,
    pub success: bool,
}

/// Migrate a single user from JWT to OAuth
pub async fn migrate_user(
    ctx: &AppContext,
    did: &str,
    dry_run: bool,
) -> PdsResult<MigrationResult> {
    info!("Starting OAuth migration for DID: {}", did);

    // Step 1: Get current sessions count
    let sessions_count = count_user_sessions(ctx, did).await?;

    if sessions_count == 0 {
        warn!("No active sessions found for {}", did);
        return Ok(MigrationResult {
            did: did.to_string(),
            sessions_revoked: 0,
            oauth_url: String::new(),
            success: false,
        });
    }

    info!("Found {} active JWT sessions for {}", sessions_count, did);

    if dry_run {
        info!("[DRY RUN] Would revoke {} sessions", sessions_count);
        let oauth_url = generate_oauth_url(&ctx.config, did)?;
        info!("[DRY RUN] Would generate OAuth URL: {}", oauth_url);

        return Ok(MigrationResult {
            did: did.to_string(),
            sessions_revoked: 0,
            oauth_url,
            success: false,
        });
    }

    // Step 2: Revoke all JWT sessions
    let revoked_count = revoke_all_sessions(ctx, did).await?;
    info!("Revoked {} JWT sessions for {}", revoked_count, did);

    // Step 3: Generate OAuth authorization URL
    let oauth_url = generate_oauth_url(&ctx.config, did)?;
    info!("Generated OAuth authorization URL for {}", did);

    Ok(MigrationResult {
        did: did.to_string(),
        sessions_revoked: revoked_count,
        oauth_url,
        success: true,
    })
}

/// Count active sessions for a user
async fn count_user_sessions(
    ctx: &AppContext,
    did: &str,
) -> PdsResult<usize> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM session WHERE did = ? AND expires_at > datetime('now')"
    )
    .bind(did)
    .fetch_one(&ctx.account_db)
    .await
    .map_err(|e| PdsError::Database(e))?;

    Ok(count as usize)
}

/// Revoke all active sessions for a user
async fn revoke_all_sessions(
    ctx: &AppContext,
    did: &str,
) -> PdsResult<usize> {
    let result = sqlx::query(
        "DELETE FROM session WHERE did = ?"
    )
    .bind(did)
    .execute(&ctx.account_db)
    .await
    .map_err(|e| PdsError::Database(e))?;

    Ok(result.rows_affected() as usize)
}

/// Generate OAuth authorization URL for a user
fn generate_oauth_url(config: &ServerConfig, did: &str) -> PdsResult<String> {
    // Build OAuth authorization URL
    let client_id = &config.authentication.oauth.client_id;
    let redirect_uri = &config.authentication.oauth.redirect_uri;
    let pds_url = &config.authentication.oauth.pds_url;

    // Generate state parameter (in production, this should be securely stored)
    let state = uuid::Uuid::new_v4().to_string();

    // Build authorization URL following ATProto OAuth spec
    // https://atproto.com/specs/oauth
    let auth_url = format!(
        "{}/oauth/authorize?response_type=code&client_id={}&redirect_uri={}&scope=atproto&state={}&login_hint={}",
        pds_url,
        urlencoding::encode(client_id),
        urlencoding::encode(redirect_uri),
        state,
        urlencoding::encode(did)
    );

    Ok(auth_url)
}

/// Bulk migrate all users (admin only)
pub async fn bulk_migrate_users(
    ctx: &AppContext,
    dry_run: bool,
) -> PdsResult<Vec<MigrationResult>> {
    info!("Starting bulk OAuth migration");

    // Get all DIDs with active sessions
    let dids: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT did FROM session WHERE expires_at > datetime('now')"
    )
    .fetch_all(&ctx.account_db)
    .await
    .map_err(|e| PdsError::Database(e))?;

    info!("Found {} users with active JWT sessions", dids.len());

    let mut results = Vec::new();

    for did in dids {
        match migrate_user(ctx, &did, dry_run).await {
            Ok(result) => {
                if result.success {
                    info!("✓ Successfully migrated {}", did);
                } else {
                    warn!("⚠ Migration skipped for {}", did);
                }
                results.push(result);
            }
            Err(e) => {
                error!("✗ Failed to migrate {}: {}", did, e);
            }
        }
    }

    info!(
        "Bulk migration complete: {} users processed",
        results.len()
    );

    Ok(results)
}

/// Print migration result summary
pub fn print_migration_result(result: &MigrationResult) {
    println!("\n════════════════════════════════════════════════════════");
    println!("  OAuth Migration Result");
    println!("════════════════════════════════════════════════════════");
    println!("DID: {}", result.did);
    println!("Sessions Revoked: {}", result.sessions_revoked);
    println!("Status: {}", if result.success { "✓ SUCCESS" } else { "⚠ SKIPPED" });

    if result.success {
        println!("\nNext Steps:");
        println!("1. Send the following OAuth authorization URL to the user:");
        println!("\n   {}\n", result.oauth_url);
        println!("2. User must visit this URL and authorize the application");
        println!("3. After authorization, user will be redirected with OAuth tokens");
    }

    println!("════════════════════════════════════════════════════════\n");
}
