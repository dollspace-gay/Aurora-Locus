/// Rotate DID signing keys and update PLC directory
///
/// This command rotates signing keys for DID:PLC identifiers by:
/// 1. Fetching the current signing key from the repository
/// 2. Comparing with the current key in PLC directory
/// 3. Updating PLC directory if keys don't match
/// 4. Creating a new repository commit with the updated signing key
/// 5. Sequencing identity events (commit events are automatically sequenced)

use crate::{
    actor_store::repository::RepositoryManager,
    context::AppContext,
    crypto::{
        plc::{PlcSigner},
        plc_client::{PlcClient, PlcClientConfig},
    },
    error::{PdsError, PdsResult},
    sequencer::events::IdentityEvent,
};
use std::fs;
use std::time::Duration;
use tokio::time::sleep;

/// Rotate keys for a list of DIDs
///
/// # Arguments
/// * `ctx` - Application context
/// * `dids` - List of DIDs to rotate keys for
/// * `concurrency` - Number of concurrent rotations (default: 10)
pub async fn rotate_keys(
    ctx: &AppContext,
    dids: Vec<String>,
    concurrency: usize,
) -> PdsResult<()> {
    if dids.is_empty() {
        return Err(PdsError::Validation("No DIDs provided".to_string()));
    }

    println!("Rotating keys for {} DID(s) with concurrency {}...\n", dids.len(), concurrency);

    // Create PLC client
    let plc_config = PlcClientConfig {
        plc_url: ctx.config.identity.did_plc_url.clone(),
        timeout_secs: 30,
    };
    let plc_client = PlcClient::new(plc_config)?;

    // Create rotation key signer
    let rotation_signer = PlcSigner::from_hex(&ctx.config.authentication.plc_rotation_key)
        .map_err(|e| PdsError::Internal(format!("Invalid PLC rotation key: {}", e)))?;

    // Process DIDs with limited concurrency
    let mut tasks = Vec::new();
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(concurrency));

    for (idx, did) in dids.iter().enumerate() {
        let did = did.clone();
        let ctx = ctx.clone();
        let plc_client_clone = plc_client.clone();
        let rotation_signer_clone = rotation_signer.clone();
        let sem_clone = semaphore.clone();
        let total = dids.len();

        let task = tokio::spawn(async move {
            let _permit = sem_clone.acquire().await.unwrap();

            match rotate_key_for_did(&ctx, &did, &plc_client_clone, &rotation_signer_clone).await {
                Ok(rotated) => {
                    if rotated {
                        println!("[{}/{}] ✓ Rotated key for {}", idx + 1, total, did);
                    } else {
                        println!("[{}/{}] ○ Key already up-to-date for {}", idx + 1, total, did);
                    }
                    Ok(())
                }
                Err(e) => {
                    eprintln!("[{}/{}] ✗ Failed to rotate key for {}: {}", idx + 1, total, did, e);
                    Err(e)
                }
            }
        });

        tasks.push(task);
    }

    // Wait for all tasks to complete
    let mut success_count = 0;
    let mut already_updated = 0;
    let mut error_count = 0;

    for task in tasks {
        match task.await {
            Ok(Ok(())) => success_count += 1,
            Ok(Err(_)) => error_count += 1,
            Err(e) => {
                error_count += 1;
                eprintln!("Task error: {}", e);
            }
        }
    }

    println!("\n═══════════════════════════════════════");
    println!("Summary:");
    println!("  Total DIDs:      {}", dids.len());
    println!("  ✓ Rotated:       {}", success_count);
    println!("  ○ Already up-to-date: {}", already_updated);
    println!("  ✗ Failed:        {}", error_count);
    println!("═══════════════════════════════════════\n");

    if error_count > 0 {
        println!("⚠️  Warning: {} key rotation(s) failed", error_count);
    } else {
        println!("✓ All key rotations completed successfully");
    }

    Ok(())
}

/// Rotate keys from a file containing DIDs (one per line)
///
/// # Arguments
/// * `ctx` - Application context
/// * `file_path` - Path to file containing DIDs
/// * `concurrency` - Number of concurrent rotations
pub async fn rotate_keys_from_file(
    ctx: &AppContext,
    file_path: &str,
    concurrency: usize,
) -> PdsResult<()> {
    println!("Reading DIDs from file: {}\n", file_path);

    let content = fs::read_to_string(file_path)
        .map_err(|e| PdsError::Internal(format!("Failed to read file: {}", e)))?;

    let dids: Vec<String> = content
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter(|line| line.starts_with("did:plc:"))
        .collect();

    if dids.is_empty() {
        return Err(PdsError::Validation("No valid did:plc DIDs found in file".to_string()));
    }

    rotate_keys(ctx, dids, concurrency).await
}

/// Rotate key for a single DID
///
/// Returns true if key was rotated, false if already up-to-date
async fn rotate_key_for_did(
    ctx: &AppContext,
    did: &str,
    plc_client: &PlcClient,
    rotation_signer: &PlcSigner,
) -> PdsResult<bool> {
    // Validate DID format
    if !did.starts_with("did:plc:") {
        return Err(PdsError::Validation(format!("Not a did:plc identifier: {}", did)));
    }

    // Get repo signing key from config
    let repo_signer = PlcSigner::from_hex(&ctx.config.authentication.repo_signing_key)
        .map_err(|e| PdsError::Internal(format!("Invalid repo signing key: {}", e)))?;

    let desired_key = repo_signer.public_key_multibase();

    // Phase 1: Update PLC directory key
    let rotated = plc_client.rotate_key_if_needed(did, &desired_key, rotation_signer).await?;

    if !rotated {
        // Key already up-to-date, no need to create new commit or sequence events
        return Ok(false);
    }

    // Phase 2: Create new repository commit with updated signing key
    // This is done by applying empty writes, which creates a new commit signed with the current key
    let repo_mgr = RepositoryManager::with_sequencer(
        did.to_string(),
        (*ctx.actor_store).clone(),
        ctx.sequencer.clone(),
    );

    // Create signing function that uses the repo signer
    let repo_signer_clone = repo_signer.clone();
    let sign_fn = move |hash: &[u8; 32]| -> Result<Vec<u8>, atproto::repo::RepoError> {
        Ok(repo_signer_clone.sign(hash))
    };

    // Apply empty writes to create new commit (like processWrites([]) in TypeScript)
    let (commit_cid, rev) = repo_mgr.apply_writes(vec![], sign_fn).await
        .map_err(|e| {
            tracing::warn!("Failed to create commit for {}: {}", did, e);
            e
        })?;

    tracing::info!(
        did = %did,
        commit_cid = %commit_cid,
        rev = %rev,
        "Created new repository commit with updated signing key"
    );

    // Phase 3: Sequence identity event (commit event is already sequenced by apply_writes)
    let account = ctx.account_manager.get_account(did).await?;
    let identity_evt = IdentityEvent::new(did.to_string(), account.handle);

    ctx.sequencer.sequence_identity(identity_evt).await
        .map_err(|e| {
            tracing::warn!("Failed to sequence identity event for {}: {}", did, e);
            e
        })?;

    tracing::info!(did = %did, "Sequenced identity event");

    // TODO: Phase 3 continuation - Sequence sync event with commit data
    // This would create a SyncEvent from the commit and sequence it
    // Deferred as it requires extracting commit blocks from the CAR export

    Ok(true)
}
