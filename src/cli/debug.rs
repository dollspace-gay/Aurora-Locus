/// Debug Utilities CLI Commands
///
/// Provides debugging and inspection tools for troubleshooting server issues.

use crate::context::AppContext;
use crate::error::PdsResult;
use serde_json::json;
use std::fs;

/// Inspect account details by DID or handle
pub async fn inspect_account(ctx: &AppContext, identifier: &str, format: &str) -> PdsResult<()> {
    println!("════════════════════════════════════════════════════════");
    println!("  Account Inspection");
    println!("════════════════════════════════════════════════════════\n");

    // Get account by identifier (DID or handle)
    let account = ctx.account_manager.get_account_by_identifier(identifier).await?;

    // Get app passwords
    let app_passwords = ctx
        .account_manager
        .list_app_passwords(&account.did)
        .await
        .unwrap_or_else(|_| Vec::new());

    // Get invite codes (if available)
    let invite_codes = ctx
        .account_manager
        .list_invite_codes(&account.did)
        .await
        .unwrap_or_else(|_| Vec::new());

    // Get blob count
    let blob_count = ctx
        .blob_store
        .list_for_user(&account.did, 1000)
        .await
        .map(|blobs| blobs.len())
        .unwrap_or(0);

    // Check if account has repository
    let has_repo = ctx.actor_store.exists(&account.did).await;

    // Output results
    match format.to_lowercase().as_str() {
        "json" => {
            let output = json!({
                "account": {
                    "did": account.did,
                    "handle": account.handle,
                    "email": account.email,
                    "email_confirmed_at": account.email_confirmed_at,
                    "created_at": account.created_at,
                    "deactivated_at": account.deactivated_at,
                    "takedown_ref": account.takedown_ref,
                },
                "app_passwords": app_passwords.iter().map(|ap| json!({
                    "name": ap.name,
                    "created_at": ap.created_at,
                })).collect::<Vec<_>>(),
                "invite_codes": invite_codes.len(),
                "blob_count": blob_count,
                "has_repository": has_repo,
            });
            println!("{}", serde_json::to_string_pretty(&output).unwrap());
        }
        _ => {
            println!("DID:              {}", account.did);
            if let Some(handle) = &account.handle {
                println!("Handle:           {}", handle);
            }
            if let Some(email) = &account.email {
                println!("Email:            {}", email);
                println!(
                    "Email Confirmed:  {}",
                    if account.email_confirmed_at.is_some() { "Yes" } else { "No" }
                );
            }
            println!("Created:          {}", account.created_at);

            if account.deactivated_at.is_some() {
                println!("\n⚠️  Status:          DEACTIVATED");
            }

            if let Some(takedown) = &account.takedown_ref {
                println!("\n⚠️  Takedown:        {}", takedown);
            }

            println!("\nApp Passwords:    {}", app_passwords.len());
            if !app_passwords.is_empty() {
                for ap in &app_passwords {
                    println!("  - {} (created: {})", ap.name, ap.created_at);
                }
            }

            println!("Invite Codes:     {}", invite_codes.len());
            println!("Blobs:            {}", blob_count);
            println!(
                "Repository:       {}",
                if has_repo { "Exists" } else { "Not initialized" }
            );
        }
    }

    println!("\n════════════════════════════════════════════════════════\n");
    Ok(())
}

/// Inspect repository state and content
pub async fn inspect_repo(ctx: &AppContext, did: &str, format: &str) -> PdsResult<()> {
    println!("════════════════════════════════════════════════════════");
    println!("  Repository Inspection");
    println!("════════════════════════════════════════════════════════\n");

    // Check if repo exists
    if !ctx.actor_store.exists(did).await {
        println!("❌ Repository does not exist for DID: {}", did);
        return Ok(());
    }

    // Get repo root
    let repo_root = ctx.actor_store.get_repo_root(did).await?;

    // Get collections
    let collections = ctx.actor_store.get_collections(did).await?;

    // Count records per collection
    let mut collection_counts = Vec::new();
    for collection in &collections {
        let count = ctx
            .actor_store
            .count_records(did, collection)
            .await
            .unwrap_or(0);
        collection_counts.push((collection.clone(), count));
    }

    // Get total block count
    let blocks = ctx.actor_store.get_all_blocks(did).await?;
    let total_blocks = blocks.len();
    let total_block_size: usize = blocks.iter().map(|(_, data)| data.len()).sum();

    // Output results
    match format.to_lowercase().as_str() {
        "json" => {
            let output = json!({
                "did": did,
                "repo_root": {
                    "cid": repo_root.cid,
                    "rev": repo_root.rev,
                },
                "collections": collection_counts.iter().map(|(name, count)| json!({
                    "name": name,
                    "record_count": count,
                })).collect::<Vec<_>>(),
                "blocks": {
                    "count": total_blocks,
                    "total_size_bytes": total_block_size,
                },
            });
            println!("{}", serde_json::to_string_pretty(&output).unwrap());
        }
        _ => {
            println!("DID:          {}", did);
            println!("Root CID:     {}", repo_root.cid);
            println!("Revision:     {}", repo_root.rev);
            println!("\nCollections:  {}", collections.len());
            println!("────────────────────────────────────────────────────────");

            for (collection, count) in &collection_counts {
                println!("  {} ({} records)", collection, count);
            }

            let total_records: i64 = collection_counts.iter().map(|(_, count)| count).sum();
            println!("\nTotal Records: {}", total_records);
            println!("Total Blocks:  {}", total_blocks);
            println!(
                "Total Size:    {} bytes ({} KB)",
                total_block_size,
                total_block_size / 1024
            );
        }
    }

    println!("\n════════════════════════════════════════════════════════\n");
    Ok(())
}

/// List active sessions
pub async fn list_sessions(ctx: &AppContext, did: Option<&str>, format: &str) -> PdsResult<()> {
    println!("════════════════════════════════════════════════════════");
    println!("  Active Sessions");
    println!("════════════════════════════════════════════════════════\n");

    // Query sessions
    let query = if let Some(filter_did) = did {
        sqlx::query_as::<_, (String, String, i64, Option<String>)>(
            "SELECT id, did, expires_at, app_password_name FROM session WHERE did = ?1 ORDER BY expires_at DESC",
        )
        .bind(filter_did)
    } else {
        sqlx::query_as::<_, (String, String, i64, Option<String>)>(
            "SELECT id, did, expires_at, app_password_name FROM session ORDER BY expires_at DESC",
        )
    };

    let sessions = query.fetch_all(&ctx.account_db).await?;

    let now = chrono::Utc::now().timestamp();

    // Separate active and expired sessions
    let mut active_sessions = Vec::new();
    let mut expired_sessions = Vec::new();

    for (id, session_did, expires_at, app_password) in sessions {
        let is_expired = expires_at < now;
        let session_info = (id, session_did, expires_at, app_password);

        if is_expired {
            expired_sessions.push(session_info);
        } else {
            active_sessions.push(session_info);
        }
    }

    // Output results
    match format.to_lowercase().as_str() {
        "json" => {
            let output = json!({
                "active_sessions": active_sessions.len(),
                "expired_sessions": expired_sessions.len(),
                "sessions": active_sessions.iter().map(|(id, did, expires_at, app_password)| {
                    json!({
                        "id": id,
                        "did": did,
                        "expires_at": expires_at,
                        "app_password": app_password,
                        "status": "active",
                    })
                }).chain(expired_sessions.iter().map(|(id, did, expires_at, app_password)| {
                    json!({
                        "id": id,
                        "did": did,
                        "expires_at": expires_at,
                        "app_password": app_password,
                        "status": "expired",
                    })
                })).collect::<Vec<_>>(),
            });
            println!("{}", serde_json::to_string_pretty(&output).unwrap());
        }
        _ => {
            println!("Active Sessions:  {}", active_sessions.len());
            println!("Expired Sessions: {}\n", expired_sessions.len());

            if !active_sessions.is_empty() {
                println!("Active:");
                println!("────────────────────────────────────────────────────────");
                for (id, session_did, expires_at, app_password) in &active_sessions {
                    let expires = chrono::DateTime::from_timestamp(*expires_at, 0)
                        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                        .unwrap_or_else(|| "Invalid timestamp".to_string());

                    if let Some(app_pw) = app_password {
                        println!("  {} ({})", session_did, app_pw);
                    } else {
                        println!("  {}", session_did);
                    }
                    println!("    Session ID: {}", id);
                    println!("    Expires:    {}", expires);
                    println!();
                }
            }

            if !expired_sessions.is_empty() {
                println!("\nExpired:");
                println!("────────────────────────────────────────────────────────");
                for (id, session_did, expires_at, app_password) in &expired_sessions {
                    let expires = chrono::DateTime::from_timestamp(*expires_at, 0)
                        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                        .unwrap_or_else(|| "Invalid timestamp".to_string());

                    if let Some(app_pw) = app_password {
                        println!("  {} ({})", session_did, app_pw);
                    } else {
                        println!("  {}", session_did);
                    }
                    println!("    Session ID: {}", id);
                    println!("    Expired:    {}", expires);
                    println!();
                }
            }
        }
    }

    println!("════════════════════════════════════════════════════════\n");
    Ok(())
}

/// Check blob store integrity
pub async fn check_blobs(ctx: &AppContext, did: Option<&str>, orphaned: bool) -> PdsResult<()> {
    println!("════════════════════════════════════════════════════════");
    println!("  Blob Store Integrity Check");
    println!("════════════════════════════════════════════════════════\n");

    if orphaned {
        // Check for orphaned temporary blobs
        println!("Checking for orphaned temporary blobs...\n");

        let orphaned_blobs = ctx.blob_store.list_orphaned_temp_blobs(24).await?;

        println!("Found {} orphaned temporary blobs", orphaned_blobs.len());

        if !orphaned_blobs.is_empty() {
            println!("────────────────────────────────────────────────────────");
            for (i, cid) in orphaned_blobs.iter().enumerate() {
                if let Ok(Some(metadata)) = ctx.blob_store.get_metadata(cid).await {
                    println!(
                        "{}. {} ({} bytes, created: {})",
                        i + 1,
                        cid,
                        metadata.size,
                        metadata.created_at
                    );
                } else {
                    println!("{}. {}", i + 1, cid);
                }
            }
        }
    } else if let Some(user_did) = did {
        // Check blobs for specific user
        println!("Checking blobs for DID: {}\n", user_did);

        let blobs = ctx.blob_store.list_for_user(user_did, 1000).await?;

        println!("Found {} blobs", blobs.len());

        if !blobs.is_empty() {
            println!("────────────────────────────────────────────────────────");

            let mut total_size = 0u64;

            for (i, blob) in blobs.iter().enumerate() {
                total_size += blob.size as u64;
                println!(
                    "{}. {} ({} bytes, {})",
                    i + 1,
                    blob.cid,
                    blob.size,
                    blob.mime_type
                );
                println!("    Created: {}", blob.created_at);

                if i < blobs.len() - 1 {
                    println!();
                }
            }

            println!("\nTotal Size: {} bytes ({} KB, {} MB)",
                total_size,
                total_size / 1024,
                total_size / 1024 / 1024
            );
        }
    } else {
        // Check overall blob statistics
        println!("Collecting blob statistics...\n");

        // Get all accounts
        let accounts = ctx.account_manager.list_accounts(None, 1000).await?;

        let mut total_blobs = 0;
        let mut total_size = 0u64;
        let mut accounts_with_blobs = 0;

        for account in &accounts {
            let blobs = ctx
                .blob_store
                .list_for_user(&account.did, 1000)
                .await
                .unwrap_or_else(|_| Vec::new());

            if !blobs.is_empty() {
                accounts_with_blobs += 1;
                total_blobs += blobs.len();

                for blob in &blobs {
                    total_size += blob.size as u64;
                }
            }
        }

        println!("Total Accounts:         {}", accounts.len());
        println!("Accounts with Blobs:    {}", accounts_with_blobs);
        println!("Total Blobs:            {}", total_blobs);
        println!(
            "Total Size:             {} bytes ({} KB, {} MB)",
            total_size,
            total_size / 1024,
            total_size / 1024 / 1024
        );

        if accounts_with_blobs > 0 {
            println!(
                "Average Blobs/Account:  {:.2}",
                total_blobs as f64 / accounts_with_blobs as f64
            );
            println!(
                "Average Size/Blob:      {} bytes",
                if total_blobs > 0 {
                    total_size / total_blobs as u64
                } else {
                    0
                }
            );
        }
    }

    println!("\n════════════════════════════════════════════════════════\n");
    Ok(())
}

/// Export account data for debugging
pub async fn export_account(ctx: &AppContext, did: &str, output: &str) -> PdsResult<()> {
    println!("════════════════════════════════════════════════════════");
    println!("  Account Data Export");
    println!("════════════════════════════════════════════════════════\n");

    println!("Exporting account: {}", did);

    // Get account details
    let account = ctx.account_manager.get_account(did).await?;

    // Get app passwords
    let app_passwords = ctx
        .account_manager
        .list_app_passwords(did)
        .await
        .unwrap_or_else(|_| Vec::new());

    // Get invite codes
    let invite_codes = ctx
        .account_manager
        .list_invite_codes(did)
        .await
        .unwrap_or_else(|_| Vec::new());

    // Get blobs
    let blobs = ctx
        .blob_store
        .list_for_user(did, 1000)
        .await
        .unwrap_or_else(|_| Vec::new());

    // Get repository data if exists
    let mut repo_data = json!(null);
    if ctx.actor_store.exists(did).await {
        let repo_root = ctx.actor_store.get_repo_root(did).await?;
        let collections = ctx.actor_store.get_collections(did).await?;

        let mut collection_data = Vec::new();
        for collection in &collections {
            let count = ctx
                .actor_store
                .count_records(did, collection)
                .await
                .unwrap_or(0);
            collection_data.push(json!({
                "name": collection,
                "record_count": count,
            }));
        }

        let blocks = ctx.actor_store.get_all_blocks(did).await?;

        repo_data = json!({
            "root_cid": repo_root.cid,
            "revision": repo_root.rev,
            "collections": collection_data,
            "block_count": blocks.len(),
            "total_block_size": blocks.iter().map(|(_, data)| data.len()).sum::<usize>(),
        });
    }

    // Get active sessions
    let sessions = sqlx::query_as::<_, (String, i64, Option<String>)>(
        "SELECT id, expires_at, app_password_name FROM session WHERE did = ?1",
    )
    .bind(did)
    .fetch_all(&ctx.account_db)
    .await?;

    // Build export data
    let export_data = json!({
        "exported_at": chrono::Utc::now().to_rfc3339(),
        "account": {
            "did": account.did,
            "handle": account.handle,
            "email": account.email,
            "email_confirmed_at": account.email_confirmed_at,
            "created_at": account.created_at,
            "deactivated_at": account.deactivated_at,
            "takedown_ref": account.takedown_ref,
        },
        "app_passwords": app_passwords.iter().map(|ap| json!({
            "name": ap.name,
            "created_at": ap.created_at,
        })).collect::<Vec<_>>(),
        "invite_codes": invite_codes.iter().map(|ic| json!({
            "code": ic.code,
            "available_uses": ic.available_uses,
            "disabled": ic.disabled,
            "created_at": ic.created_at,
        })).collect::<Vec<_>>(),
        "blobs": blobs.iter().map(|b| json!({
            "cid": b.cid,
            "size": b.size,
            "mime_type": b.mime_type,
            "created_at": b.created_at,
        })).collect::<Vec<_>>(),
        "repository": repo_data,
        "sessions": sessions.iter().map(|(id, expires_at, app_password)| json!({
            "id": id,
            "expires_at": expires_at,
            "app_password": app_password,
        })).collect::<Vec<_>>(),
    });

    // Write to file
    let json_str = serde_json::to_string_pretty(&export_data).map_err(|e| {
        crate::error::PdsError::Internal(format!("Failed to serialize export data: {}", e))
    })?;

    fs::write(output, &json_str).map_err(|e| {
        crate::error::PdsError::Internal(format!("Failed to write export file: {}", e))
    })?;

    println!("\n✓ Account data exported successfully");
    println!("  Output: {}", output);
    println!("  Size:   {} bytes\n", json_str.len());

    println!("Export Summary:");
    println!("  App Passwords:  {}", app_passwords.len());
    println!("  Invite Codes:   {}", invite_codes.len());
    println!("  Blobs:          {}", blobs.len());
    println!("  Sessions:       {}", sessions.len());

    println!("\n════════════════════════════════════════════════════════\n");
    Ok(())
}
