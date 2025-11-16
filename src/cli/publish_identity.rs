/// Publish identity events to sequencer
///
/// Used to manually trigger identity event publishing, typically needed when:
/// - Handles have changed
/// - DID documents have been updated
/// - Identity information needs to be re-broadcast to the network
///
/// Supports both single DIDs and bulk publishing from files.

use crate::{
    context::AppContext,
    error::{PdsError, PdsResult},
    sequencer::events::IdentityEvent,
};
use std::fs;
use std::time::Duration;
use tokio::time::sleep;

/// Publish identity events for a list of DIDs
///
/// # Arguments
/// * `ctx` - Application context containing sequencer
/// * `dids` - List of DIDs to publish identity events for
/// * `delay_ms` - Delay in milliseconds between each publish
pub async fn publish_identity(
    ctx: &AppContext,
    dids: Vec<String>,
    delay_ms: u64,
) -> PdsResult<()> {
    if dids.is_empty() {
        return Err(PdsError::Validation("No DIDs provided".to_string()));
    }

    println!("Publishing identity events for {} DID(s)...\n", dids.len());

    let mut success_count = 0;
    let mut error_count = 0;

    for (idx, did) in dids.iter().enumerate() {
        let did = did.trim();
        if did.is_empty() {
            continue;
        }

        match publish_identity_event(ctx, did).await {
            Ok(seq) => {
                success_count += 1;
                println!("[{}/{}] ✓ Published identity event for {}",
                    idx + 1, dids.len(), did);
                println!("        Sequence: {}", seq);
            }
            Err(e) => {
                error_count += 1;
                eprintln!("[{}/{}] ✗ Failed to publish identity event for {}: {}",
                    idx + 1, dids.len(), did, e);
            }
        }

        // Delay between publishes to avoid overloading the system
        if delay_ms > 0 && idx < dids.len() - 1 {
            sleep(Duration::from_millis(delay_ms)).await;
        }
    }

    println!("\n═══════════════════════════════════════");
    println!("Summary:");
    println!("  Total DIDs:  {}", dids.len());
    println!("  ✓ Success:   {}", success_count);
    println!("  ✗ Failed:    {}", error_count);
    println!("═══════════════════════════════════════\n");

    if error_count > 0 {
        println!("⚠️  Warning: {} identity event(s) failed to publish", error_count);
    } else {
        println!("✓ All identity events published successfully");
    }

    Ok(())
}

/// Publish identity events from a file containing DIDs (one per line)
///
/// # Arguments
/// * `ctx` - Application context containing sequencer
/// * `file_path` - Path to file containing DIDs (one per line)
/// * `delay_ms` - Delay in milliseconds between each publish
pub async fn publish_identity_from_file(
    ctx: &AppContext,
    file_path: &str,
    delay_ms: u64,
) -> PdsResult<()> {
    println!("Reading DIDs from file: {}\n", file_path);

    let content = fs::read_to_string(file_path)
        .map_err(|e| PdsError::Internal(format!("Failed to read file: {}", e)))?;

    let dids: Vec<String> = content
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();

    if dids.is_empty() {
        return Err(PdsError::Validation("No valid DIDs found in file".to_string()));
    }

    publish_identity(ctx, dids, delay_ms).await
}

/// Internal function to publish a single identity event
async fn publish_identity_event(ctx: &AppContext, did: &str) -> PdsResult<i64> {
    // Validate DID format
    if !did.starts_with("did:") {
        return Err(PdsError::Validation(format!("Invalid DID format: {}", did)));
    }

    // Get account to verify it exists and get current handle
    let account = ctx.account_manager.get_account(did).await?;

    // Create identity event
    let evt = IdentityEvent::new(
        did.to_string(),
        account.handle.clone(),
    );

    // Sequence the event
    let seq = ctx.sequencer.sequence_identity(evt).await?;

    Ok(seq)
}
