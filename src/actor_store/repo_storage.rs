//! Bridge from `proto_blue::repo::storage::RepoStorage` (sync) onto our
//! sqlx-async `ActorStore`.
//!
//! `RepoStorage` is intentionally synchronous — `proto_blue::repo::Repo`'s
//! whole write path (`format_commit`, `apply_writes`, `apply_commit`) is
//! sync, and the storage trait is the seam where blocks are persisted.
//! Aurora-Locus's blockstore is sqlx-async, so this adapter calls
//! `Handle::block_on` on the current Tokio runtime to drive the async
//! work synchronously.
//!
//! # Threading
//!
//! Callers MUST run the work that touches a `Repo` inside
//! `tokio::task::spawn_blocking` (or similar). Calling `block_on` from a
//! Tokio worker thread directly would dead-lock the runtime; from a
//! blocking-pool thread it's safe.
//!
//! # Atomicity
//!
//! `apply_commit` is overridden to extract the new revision TID from the
//! incoming commit block before persisting, so the on-disk
//! `(cid, rev)` pair stays consistent. Block writes themselves are
//! looped one-by-one through `ActorStore::put_block` to preserve the
//! foreign-key invariants the `record` table relies on; an opportunistic
//! transactional batch is a fine follow-up but doesn't change the
//! observable contract.

use std::str::FromStr;
use std::sync::Arc;

use proto_blue::lex_data::Cid;
use proto_blue::repo::{
    block_map::BlockMap, commit::SignedCommit, error::RepoError, storage::RepoStorage,
};
use tokio::runtime::Handle;

use crate::actor_store::ActorStore;

/// `RepoStorage` adapter backed by a single DID's actor store.
pub struct SqliteRepoStorage {
    store: Arc<ActorStore>,
    did: String,
}

impl SqliteRepoStorage {
    /// Wrap an `ActorStore` for use by a `Repo` operating on `did`.
    pub fn new(store: Arc<ActorStore>, did: String) -> Self {
        Self { store, did }
    }

    /// Run a future to completion on the current Tokio runtime, blocking
    /// the calling thread. Caller is responsible for being on a
    /// blocking-pool thread (see module docs).
    fn block_on<F, T>(future: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        Handle::current().block_on(future)
    }
}

impl RepoStorage for SqliteRepoStorage {
    fn get_block(&self, cid: &Cid) -> Result<Option<Vec<u8>>, RepoError> {
        let cid_str = cid.to_string();
        Self::block_on(async {
            self.store
                .get_block(&self.did, &cid_str)
                .await
                .map_err(|e| RepoError::Storage(format!("get_block({}): {}", cid_str, e)))
        })
    }

    fn put_block(&self, cid: Cid, bytes: Vec<u8>) -> Result<(), RepoError> {
        let cid_str = cid.to_string();
        Self::block_on(async {
            self.store
                .put_block(&self.did, &cid_str, &bytes)
                .await
                .map_err(|e| RepoError::Storage(format!("put_block({}): {}", cid_str, e)))
        })
    }

    fn get_root(&self) -> Result<Option<Cid>, RepoError> {
        Self::block_on(async {
            // `get_repo_root` returns NotFound for an uninitialised repo; map
            // that to `Ok(None)` so `Repo::load` sees an empty store rather
            // than an error.
            match self.store.get_repo_root(&self.did).await {
                Ok(root) => {
                    let cid = Cid::from_str(&root.cid).map_err(|e| {
                        RepoError::Storage(format!("invalid stored root CID {}: {}", root.cid, e))
                    })?;
                    Ok(Some(cid))
                }
                Err(crate::error::PdsError::NotFound(_)) => Ok(None),
                Err(e) => Err(RepoError::Storage(format!("get_root: {}", e))),
            }
        })
    }

    /// Standalone root pointer update. Reuses the existing on-disk `rev`
    /// — the only way to call this without a fresh commit block in hand
    /// is when proto-blue wants to nudge the root pointer without
    /// changing the underlying MST, which doesn't happen in our flow.
    /// We override `apply_commit` below; this method exists only to
    /// satisfy the trait.
    fn update_root(&self, new_root: Cid) -> Result<(), RepoError> {
        let new_root_str = new_root.to_string();
        Self::block_on(async {
            let existing_rev = self
                .store
                .get_repo_root(&self.did)
                .await
                .map(|r| r.rev)
                .unwrap_or_default();
            self.store
                .update_repo_root(&self.did, &new_root_str, &existing_rev)
                .await
                .map_err(|e| RepoError::Storage(format!("update_root: {}", e)))
        })
    }

    /// Atomic-ish commit application: persist every block in `blocks`,
    /// then atomically advance the on-disk `(cid, rev)` pair. The new
    /// `rev` is read straight out of the signed commit block so we never
    /// disagree with what's actually in storage.
    fn apply_commit(&self, new_root: Cid, blocks: &BlockMap) -> Result<(), RepoError> {
        // Decode the commit block to extract its revision TID.
        let commit_bytes = blocks.get(&new_root).ok_or_else(|| {
            RepoError::Storage(format!(
                "apply_commit: commit block {} missing from BlockMap",
                new_root
            ))
        })?;
        let commit_value = proto_blue::lex_cbor::decode(commit_bytes)
            .map_err(|e| RepoError::Storage(format!("apply_commit: decode commit block: {}", e)))?;
        let signed = SignedCommit::from_lex_value(&commit_value)?;
        let new_rev = signed.rev.clone();
        let new_root_str = new_root.to_string();

        Self::block_on(async {
            for (cid, bytes) in blocks.iter() {
                self.store
                    .put_block(&self.did, &cid.to_string(), bytes)
                    .await
                    .map_err(|e| RepoError::Storage(format!("put_block({}): {}", cid, e)))?;
            }
            self.store
                .update_repo_root(&self.did, &new_root_str, &new_rev)
                .await
                .map_err(|e| RepoError::Storage(format!("update_root: {}", e)))?;
            Ok::<_, RepoError>(())
        })
    }
}
