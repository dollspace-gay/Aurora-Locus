//! Arc 15 §8.3.8 — `create_account_emit_sequence` orchestrator.
//!
//! Called by both the production createAccount handler
//! (`src/api/server.rs:create_account`) and the dev-route createAccount
//! handler (`src/api/dev_routes.rs:create_account`) once the account
//! row exists and the actor store is initialized.
//!
//! Emits four frames in order per §8.3.8 (matching bsky-PDS):
//!
//!   1. `#identity` — DID + handle.
//!   2. `#account` — Active (Pattern A: status hardcoded, no row
//!      re-read needed; per §8.3.4 selection rule).
//!   3. `#commit` — genesis commit (created explicitly per Sub-step
//!      0.1 recon Case B-with-wrinkle; `RepositoryManager::initialize`
//!      doesn't seed the commit).
//!   4. `#sync` — minimal CAR slice projection per §8.3.9.
//!
//! Errors abort the sequence; partial-emission state is acceptable
//! (sequencer is append-only; consumer-side replay catches up).

use std::sync::Arc;

use crate::{
    actor_store::RepositoryManager,
    context::AppContext,
    crypto::proto_blue_signer::RepoSigner,
    error::{PdsError, PdsResult},
    sequencer::events::{
        sync_evt_data_from_commit, AccountEvent, CommitEvent, CommitOp, IdentityEvent, SyncEvent,
    },
};
use proto_blue::crypto::Signer;
use proto_blue::repo::{blocks_to_car, CommitData};

/// Arc 15 §8.3.8: four-emit sequence at createAccount.
///
/// Loads the actor's signing key (per Arc 13 §6.3.2 key separation),
/// produces the genesis commit explicitly, projects sync data,
/// emits all four frames.
pub async fn create_account_emit_sequence(
    ctx: &AppContext,
    did: &str,
    handle: &str,
) -> PdsResult<()> {
    // 1. Identity.
    ctx.sequencer
        .sequence_identity(IdentityEvent {
            did: did.to_string(),
            handle: Some(handle.to_string()),
        })
        .await?;

    // 2. Account Active (Pattern A: status hardcoded).
    ctx.sequencer
        .sequence_account(AccountEvent::active(did.to_string()))
        .await?;

    // 3. Genesis commit. Load signer, create commit, sequence #commit.
    let signing_key_bytes = ctx.account_manager.get_atproto_signing_key_bytes(did).await?;
    let signer: Arc<dyn Signer> = Arc::new(
        RepoSigner::from_bytes(&signing_key_bytes)
            .map_err(|e| PdsError::Internal(format!("genesis signer construction failed: {}", e)))?,
    );

    let repo_mgr = RepositoryManager::with_sequencer(
        did.to_string(),
        (*ctx.actor_store).clone(),
        ctx.sequencer.clone(),
    );
    let commit_data: CommitData = repo_mgr.create_genesis_commit(signer).await?;

    // CAR-encode the genesis commit's full block map for the
    // #commit.blocks payload. Genesis has no prior data CID (None)
    // and no ops (the empty-MST root is the only structural content).
    let car_bytes = blocks_to_car(Some(&commit_data.commit_cid), &commit_data.blocks)
        .map_err(|e| PdsError::Internal(format!("genesis CAR export failed: {}", e)))?;

    let commit_event = CommitEvent::new(
        did.to_string(),
        commit_data.commit_cid.to_string(),
        commit_data.commit.rev.clone(),
        None, // since: genesis has no prior commit
        None, // prev_data: genesis has no prior MST root
        car_bytes,
        Vec::<CommitOp>::new(),
    );
    ctx.sequencer.sequence_commit(commit_event).await?;

    // 4. Sync — minimal slice projection from the same CommitData.
    let sync_data = sync_evt_data_from_commit(&commit_data)?;
    let sync_event = SyncEvent::from_sync_data(did.to_string(), sync_data)?;
    ctx.sequencer.sequence_sync(sync_event).await?;

    tracing::info!(
        did = did,
        commit_cid = %commit_data.commit_cid,
        "create_account: emitted identity → account → commit → sync"
    );

    Ok(())
}
