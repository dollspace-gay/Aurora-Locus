// Allow dead_code - repository management for future use
#![allow(dead_code)]

//! Repository manager — bridges `proto_blue::repo::Repo` with the per-actor
//! SQLite blockstore.
//!
//! `Repo` from proto-blue manages the MST + signed-commit logic; this
//! module wraps it with the per-record metadata table that Aurora-Locus
//! needs for record listing, swap-CID checks, and firehose ops tracking.
//!
//! ## Threading model
//!
//! `proto_blue::repo::Repo` is sync. `SqliteRepoStorage` (the bridge to
//! our async `ActorStore`) calls `Handle::block_on` from each `RepoStorage`
//! method, so the entire `Repo` interaction MUST run inside
//! `tokio::task::spawn_blocking`. This module enforces that pattern in
//! every commit path.
//!
//! ## Migration note
//!
//! The previous in-house SDK kept the MST in memory and rebuilt it from
//! record blocks on every load. proto-blue persists MST nodes alongside
//! record blocks, so pre-existing repos created under the old layout
//! will be missing those MST node blocks and `Repo::load` will fail with
//! `RepoError::MissingBlock`. A one-shot migration tool that walks
//! existing records and re-emits a fresh signed commit is a separate
//! piece of work; greenfield repos initialised through this module work
//! immediately.

use crate::{
    actor_store::{repo_storage::SqliteRepoStorage, ActorStore},
    error::{PdsError, PdsResult},
    sequencer::{
        events::{CommitEvent, CommitOp, OpAction},
        Sequencer,
    },
    validation::{validation_errors_to_pds_error, RecordValidator, ValidationMode},
};
use proto_blue::common::next_tid;
use proto_blue::crypto::Signer;
use proto_blue::lex_cbor::cid_for_lex;
use proto_blue::lex_data::Cid;
use proto_blue::lex_json::json_to_lex;
use proto_blue::repo::{
    block_map::BlockMap, car::blocks_to_car, error::RepoError as ProtoRepoError,
    storage::RepoStorage, Repo, RepoWrite,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Write operation action
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WriteOpAction {
    Create,
    Update,
    Delete,
}

/// Write operation for applyWrites
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteOp {
    pub action: WriteOpAction,
    pub collection: String,
    pub rkey: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validate: Option<bool>,
    /// Expected CID of the current record (for optimistic concurrency)
    /// If provided, the operation will fail if the current record's CID doesn't match
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swap_cid: Option<String>,
}

/// Per-record bookkeeping captured during commit prep so we can update
/// the SQLite `record` table after the proto-blue commit lands.
struct PreparedRecord {
    op: OpAction,
    collection: String,
    rkey: String,
    uri: String,
    /// Block CID for create/update; `None` for delete.
    cid: Option<String>,
}

/// Repository manager for a single actor.
pub struct RepositoryManager {
    did: String,
    store: ActorStore,
    validator: RecordValidator,
    sequencer: Option<Arc<Sequencer>>,
}

impl RepositoryManager {
    /// Create a new repository manager for a DID with default validation mode
    pub fn new(did: String, store: ActorStore) -> Self {
        Self::with_validation_mode(did, store, ValidationMode::default())
    }

    /// Create a new repository manager with specific validation mode
    pub fn with_validation_mode(did: String, store: ActorStore, mode: ValidationMode) -> Self {
        Self {
            did,
            store,
            validator: RecordValidator::with_mode(mode),
            sequencer: None,
        }
    }

    /// Create a new repository manager with sequencer support (default validation mode)
    pub fn with_sequencer(did: String, store: ActorStore, sequencer: Arc<Sequencer>) -> Self {
        Self::with_sequencer_and_validation(did, store, sequencer, ValidationMode::default())
    }

    /// Create a new repository manager with sequencer support and specific validation mode
    pub fn with_sequencer_and_validation(
        did: String,
        store: ActorStore,
        sequencer: Arc<Sequencer>,
        mode: ValidationMode,
    ) -> Self {
        Self {
            did,
            store,
            validator: RecordValidator::with_mode(mode),
            sequencer: Some(sequencer),
        }
    }

    /// Initialize a new repository for an actor.
    ///
    /// Creates the underlying SQLite layout. The first `apply_writes`
    /// call will seed the empty signed commit via `Repo::create`.
    pub async fn initialize(&self) -> PdsResult<()> {
        self.store.create(&self.did).await?;
        Ok(())
    }

    /// Build the storage adapter for this actor.
    fn make_storage(&self) -> Arc<SqliteRepoStorage> {
        Arc::new(SqliteRepoStorage::new(
            Arc::new(self.store.clone()),
            self.did.clone(),
        ))
    }

    /// Validate a single write — runs the codec validator under the
    /// configured mode and tracks failures on the actor store.
    async fn validate_write(&self, write: &WriteOp) -> PdsResult<()> {
        let value = match &write.value {
            Some(v) => v,
            None => return Ok(()), // delete or no-op
        };
        if !write.validate.unwrap_or(true) {
            return Ok(());
        }
        if let Err(errors) = self.validator.validate(&write.collection, value) {
            let uri = format!("at://{}/{}/{}", self.did, write.collection, write.rkey);

            if self.validator.mode() == ValidationMode::Optimistic {
                if let Err(e) = self
                    .store
                    .track_validation_failure(&self.did, &write.collection, &uri, &errors)
                    .await
                {
                    tracing::warn!("Failed to track validation failure for {}: {}", uri, e);
                }
                tracing::warn!(
                    "Validation failed for {} but accepting in Optimistic mode: {} error(s)",
                    uri,
                    errors.len()
                );
                return Ok(());
            }

            // Required mode — track and reject.
            let _ = self
                .store
                .track_validation_failure(&self.did, &write.collection, &uri, &errors)
                .await;
            return Err(validation_errors_to_pds_error(errors));
        }
        Ok(())
    }

    /// Apply write operations and create a new commit.
    ///
    /// The `signer` produces ECDSA signatures over the unsigned commit
    /// payload — pass a `RepoSigner` wrapping the actor's repo-signing
    /// key.
    pub async fn apply_writes(
        &self,
        writes: Vec<WriteOp>,
        signer: Arc<dyn Signer>,
    ) -> PdsResult<(String, String)> {
        // ATProto spec: max 200 writes per commit.
        if writes.len() > 200 {
            return Err(PdsError::Validation(
                "Too many writes in batch. Maximum: 200 operations per commit".to_string(),
            ));
        }

        // Validate every write up-front so we don't half-apply.
        for write in &writes {
            self.validate_write(write).await?;
        }

        // Arc 14 §7.3.2: capture prior record CIDs for update/delete
        // ops before applying writes, so each CommitOp can carry its
        // `prev` (prior record version CID) on the firehose event.
        // Create ops have no prior version → not queried.
        let mut prior_record_cids: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for write in &writes {
            if matches!(
                write.action,
                WriteOpAction::Update | WriteOpAction::Delete
            ) {
                let uri = format!(
                    "at://{}/{}/{}",
                    self.did, write.collection, write.rkey
                );
                if let Some(prior) =
                    self.store.get_record(&self.did, &uri).await?
                {
                    prior_record_cids.insert(uri, prior.cid);
                }
            }
        }

        // Convert each WriteOp into:
        //   * a `proto_blue::repo::RepoWrite` for the MST/commit machinery
        //   * a `PreparedRecord` for the post-commit metadata writeback
        let mut repo_writes: Vec<RepoWrite> = Vec::with_capacity(writes.len());
        let mut prepared: Vec<PreparedRecord> = Vec::with_capacity(writes.len());

        for write in writes {
            let collection = write.collection;
            let rkey = write.rkey;
            let uri = format!("at://{}/{}/{}", self.did, collection, rkey);

            match write.action {
                WriteOpAction::Create | WriteOpAction::Update => {
                    let value = write.value.ok_or_else(|| {
                        PdsError::Validation("Create/Update requires value".to_string())
                    })?;

                    // Convert serde_json -> LexValue (lenient — mirrors TS lex-json
                    // behaviour, recognising $link/$bytes special objects).
                    let lex_value = json_to_lex(&value);

                    // Pre-compute the CID we'll store in the per-record metadata
                    // table. This MUST equal the CID proto-blue assigns to the
                    // record block during apply_writes — both go through
                    // `cid_for_lex` over the same LexValue, so they agree by
                    // construction.
                    let record_cid = cid_for_lex(&lex_value).map_err(|e| {
                        PdsError::Internal(format!("CID computation failed: {}", e))
                    })?;

                    let op = match write.action {
                        WriteOpAction::Create => OpAction::Create,
                        WriteOpAction::Update => OpAction::Update,
                        WriteOpAction::Delete => unreachable!(),
                    };

                    prepared.push(PreparedRecord {
                        op,
                        collection: collection.clone(),
                        rkey: rkey.clone(),
                        uri,
                        cid: Some(record_cid.to_string()),
                    });

                    let pb_write = match write.action {
                        WriteOpAction::Create => RepoWrite::Create {
                            collection,
                            rkey,
                            value: lex_value,
                        },
                        WriteOpAction::Update => RepoWrite::Update {
                            collection,
                            rkey,
                            value: lex_value,
                        },
                        WriteOpAction::Delete => unreachable!(),
                    };
                    repo_writes.push(pb_write);
                }
                WriteOpAction::Delete => {
                    prepared.push(PreparedRecord {
                        op: OpAction::Delete,
                        collection: collection.clone(),
                        rkey: rkey.clone(),
                        uri,
                        cid: None,
                    });
                    repo_writes.push(RepoWrite::Delete { collection, rkey });
                }
            }
        }

        // Drive proto-blue's commit machinery on a blocking thread —
        // the storage adapter calls `block_on` internally, which would
        // dead-lock a worker thread if invoked directly.
        let storage = self.make_storage();
        let did = self.did.clone();
        let signer_arc = signer;

        let (commit_cid, new_rev, prev_commit, prev_data, blocks): (
            Cid,
            String,
            Option<Cid>,
            Option<Cid>,
            BlockMap,
        ) = tokio::task::spawn_blocking(
            move || -> Result<_, ProtoRepoError> {
                // Load the existing repo, or seed an empty one on the first commit.
                let storage_dyn: Arc<dyn RepoStorage> = storage.clone();
                let mut repo = match Repo::load(storage_dyn.clone()) {
                    Ok(r) => r,
                    Err(ProtoRepoError::Storage(_)) => {
                        // Empty store — initialise with an empty signed commit.
                        Repo::create(storage_dyn.clone(), did, signer_arc.as_ref())?
                    }
                    Err(e) => return Err(e),
                };

                let prev = repo.commit_cid().cloned();
                // Arc 14 §7.3.2: prior commit's MST root CID (`data`
                // field of the prior signed commit). `None` for
                // genesis (no prior commit exists). Captured BEFORE
                // apply_writes so it reflects the prior state.
                let prev_data = repo.commit().map(|c| c.data.clone());
                let data = repo.apply_writes(&repo_writes, signer_arc.as_ref())?;

                Ok((
                    data.commit_cid,
                    data.commit.rev.clone(),
                    prev,
                    prev_data,
                    data.blocks,
                ))
            },
        )
        .await
        .map_err(|e| PdsError::Internal(format!("commit join failed: {}", e)))?
        .map_err(|e| PdsError::Internal(format!("Commit creation failed: {}", e)))?;

        // Update the per-record metadata table now that the commit has landed.
        let mut commit_ops: Vec<CommitOp> = Vec::with_capacity(prepared.len());
        for rec in prepared {
            // Arc 14 §7.3.2: `prev` = prior record version CID for
            // update/delete ops; None for create ops.
            let prev_cid = match rec.op {
                OpAction::Update | OpAction::Delete => {
                    prior_record_cids.get(&rec.uri).cloned()
                }
                OpAction::Create => None,
            };
            match rec.op {
                OpAction::Create | OpAction::Update => {
                    let cid = rec
                        .cid
                        .clone()
                        .expect("create/update has cid by construction");
                    self.store
                        .put_record(
                            &self.did,
                            &rec.uri,
                            &cid,
                            &rec.collection,
                            &rec.rkey,
                            &new_rev,
                        )
                        .await?;
                    commit_ops.push(CommitOp {
                        action: rec.op,
                        path: format!("{}/{}", rec.collection, rec.rkey),
                        cid: Some(cid),
                        prev: prev_cid,
                    });
                }
                OpAction::Delete => {
                    self.store.delete_record(&self.did, &rec.uri).await?;
                    commit_ops.push(CommitOp {
                        action: rec.op,
                        path: format!("{}/{}", rec.collection, rec.rkey),
                        cid: None,
                        prev: prev_cid,
                    });
                }
            }
        }

        // Build the firehose CAR (commit block + new MST nodes + new record blocks).
        let car_bytes = blocks_to_car(Some(&commit_cid), &blocks)
            .map_err(|e| PdsError::Internal(format!("CAR export failed: {}", e)))?;

        // ATProto spec: max 2MB per commit event.
        if car_bytes.len() > 2_000_000 {
            return Err(PdsError::Validation(
                "Commit too large. Maximum: 2MB per commit event".to_string(),
            ));
        }

        // Emit firehose event.
        if let Some(ref sequencer) = self.sequencer {
            let commit_event = CommitEvent::new(
                self.did.clone(),
                commit_cid.to_string(),
                new_rev.clone(),
                prev_commit.map(|c| c.to_string()),
                // Arc 14 §7.3.2: prior commit's MST root CID.
                prev_data.map(|c| c.to_string()),
                car_bytes,
                commit_ops,
            );

            sequencer
                .sequence_commit(commit_event)
                .await
                .map_err(|e| {
                    tracing::warn!("Failed to sequence commit event: {}", e);
                    e
                })
                .ok();
        }

        Ok((commit_cid.to_string(), new_rev))
    }

    /// Create a single record
    pub async fn create_record(
        &self,
        collection: &str,
        rkey: Option<&str>,
        value: serde_json::Value,
        validate: Option<bool>,
        signer: Arc<dyn Signer>,
    ) -> PdsResult<(String, String, String)> {
        // Generate rkey if not provided — proto-blue's next_tid is monotonic
        // per process, so successive calls won't collide even sub-millisecond.
        let rkey = match rkey {
            Some(k) => k.to_string(),
            None => next_tid(None).to_string(),
        };

        let writes = vec![WriteOp {
            action: WriteOpAction::Create,
            collection: collection.to_string(),
            rkey: rkey.clone(),
            value: Some(value),
            validate,
            swap_cid: None,
        }];

        let (commit_cid, rev) = self.apply_writes(writes, signer).await?;
        let uri = format!("at://{}/{}/{}", self.did, collection, rkey);
        Ok((uri, commit_cid, rev))
    }

    /// Update a record
    pub async fn update_record(
        &self,
        collection: &str,
        rkey: &str,
        value: serde_json::Value,
        validate: Option<bool>,
        signer: Arc<dyn Signer>,
    ) -> PdsResult<(String, String)> {
        let writes = vec![WriteOp {
            action: WriteOpAction::Update,
            collection: collection.to_string(),
            rkey: rkey.to_string(),
            value: Some(value),
            validate,
            swap_cid: None,
        }];

        self.apply_writes(writes, signer).await
    }

    /// Delete a record
    pub async fn delete_record(
        &self,
        collection: &str,
        rkey: &str,
        signer: Arc<dyn Signer>,
    ) -> PdsResult<(String, String)> {
        let writes = vec![WriteOp {
            action: WriteOpAction::Delete,
            collection: collection.to_string(),
            rkey: rkey.to_string(),
            value: None,
            validate: None,
            swap_cid: None,
        }];

        self.apply_writes(writes, signer).await
    }

    /// Get a record by AT-URI
    pub async fn get_record(&self, uri: &str) -> PdsResult<Option<serde_json::Value>> {
        let record = self.store.get_record(&self.did, uri).await?;

        if let Some(rec) = record {
            if let Some(content) = self.store.get_block(&self.did, &rec.cid).await? {
                // Records are stored as DAG-CBOR. Decode then convert to JSON
                // for the public API surface.
                let lex_value = proto_blue::lex_cbor::decode(&content).map_err(|e| {
                    PdsError::Internal(format!("Failed to decode record block: {}", e))
                })?;
                let value = proto_blue::lex_json::lex_to_json(&lex_value);

                Ok(Some(serde_json::json!({
                    "uri": rec.uri,
                    "cid": rec.cid,
                    "value": value
                })))
            } else {
                Err(PdsError::Internal(format!(
                    "Block not found for record {}",
                    uri
                )))
            }
        } else {
            Ok(None)
        }
    }

    /// List records in a collection
    pub async fn list_records(
        &self,
        collection: &str,
        limit: i64,
        cursor: Option<&str>,
    ) -> PdsResult<Vec<serde_json::Value>> {
        let records = self
            .store
            .list_records(&self.did, collection, limit, cursor)
            .await?;

        let mut results = Vec::new();
        for rec in records {
            if let Some(content) = self.store.get_block(&self.did, &rec.cid).await? {
                let lex_value = proto_blue::lex_cbor::decode(&content).map_err(|e| {
                    PdsError::Internal(format!("Failed to decode record block: {}", e))
                })?;
                let value = proto_blue::lex_json::lex_to_json(&lex_value);

                results.push(serde_json::json!({
                    "uri": rec.uri,
                    "cid": rec.cid,
                    "value": value
                }));
            } else {
                tracing::warn!("Block not found for record {}", rec.uri);
            }
        }

        Ok(results)
    }

    /// Get repository description
    pub async fn describe_repo(
        &self,
        account_manager: Option<&crate::account::AccountManager>,
        identity_resolver: Option<&dyn crate::identity::IdentityResolverApi>,
    ) -> PdsResult<serde_json::Value> {
        let _repo_root = self.store.get_repo_root(&self.did).await?;

        let handle = if let Some(acc_mgr) = account_manager {
            acc_mgr
                .get_account(&self.did)
                .await
                .ok()
                .and_then(|acc| acc.handle)
        } else {
            None
        }
        .or_else(|| {
            if self.did.starts_with("did:web:") {
                self.did.strip_prefix("did:web:").map(|s| s.to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".to_string());

        let did_doc = if let Some(resolver) = identity_resolver {
            resolver
                .resolve_did(&self.did)
                .await
                .ok()
                .and_then(|doc| serde_json::to_value(&doc).ok())
        } else {
            None
        };

        let collections = self.store.get_collections(&self.did).await?;

        let handle_is_correct = if let Some(resolver) = identity_resolver {
            resolver
                .get_handle_for_did(&self.did)
                .await
                .ok()
                .flatten()
                .map(|resolved| resolved == handle)
                .unwrap_or(true)
        } else {
            true
        };

        Ok(serde_json::json!({
            "did": self.did,
            "handle": handle,
            "didDoc": did_doc,
            "collections": collections,
            "handleIsCorrect": handle_is_correct,
        }))
    }

    /// Export repository to a CAR file.
    ///
    /// Emits the full set of blocks (commit + every MST node + every record)
    /// rooted at the current commit CID.
    pub async fn export_car(&self, _since: Option<&str>) -> PdsResult<Vec<u8>> {
        crate::actor_store::car::export_repo_to_car(&self.store, &self.did, _since).await
    }

    // ==================== Batch Operations ====================

    /// Prepare write operations for batch execution
    pub fn prepare_writes(
        &self,
        writes: Vec<WriteOp>,
    ) -> PdsResult<Vec<crate::actor_store::models::PreparedWrite>> {
        use crate::actor_store::models::WriteOpAction as ModelAction;
        let mut prepared = Vec::new();

        for write in writes {
            let action = match write.action {
                WriteOpAction::Create => ModelAction::Create,
                WriteOpAction::Update => ModelAction::Update,
                WriteOpAction::Delete => ModelAction::Delete,
            };

            prepared.push(crate::actor_store::models::PreparedWrite {
                action,
                collection: write.collection,
                rkey: write.rkey,
                record: write.value,
                swap_cid: write.swap_cid,
                validate: write.validate,
            });
        }

        Ok(prepared)
    }

    /// Validate batch operations before execution
    pub async fn validate_batch(
        &self,
        writes: &[crate::actor_store::models::PreparedWrite],
    ) -> PdsResult<()> {
        use crate::actor_store::models::WriteOpAction;
        use std::collections::HashSet;
        let mut seen_keys: HashSet<String> = HashSet::new();

        for write in writes {
            if !write.collection.contains('.') {
                return Err(PdsError::Validation(format!(
                    "Invalid collection format: {}",
                    write.collection
                )));
            }

            if write.rkey.is_empty() || write.rkey.len() > 512 {
                return Err(PdsError::Validation(format!(
                    "Invalid rkey length: {}",
                    write.rkey.len()
                )));
            }

            let key = format!("{}/{}", write.collection, write.rkey);
            if !seen_keys.insert(key.clone()) {
                return Err(PdsError::Validation(format!(
                    "Duplicate operation for {}/{}",
                    write.collection, write.rkey
                )));
            }

            match write.action {
                WriteOpAction::Create | WriteOpAction::Update => {
                    if write.record.is_none() {
                        return Err(PdsError::Validation(format!(
                            "Create/Update requires record value for {}/{}",
                            write.collection, write.rkey
                        )));
                    }

                    if let Some(ref record) = write.record {
                        let record_bytes = serde_json::to_vec(record).map_err(|e| {
                            PdsError::Internal(format!("Failed to serialize record: {}", e))
                        })?;

                        const MAX_RECORD_SIZE: usize = 1024 * 1024;
                        if record_bytes.len() > MAX_RECORD_SIZE {
                            return Err(PdsError::Validation(format!(
                                "Record exceeds maximum size of {}MB: {} bytes",
                                MAX_RECORD_SIZE / (1024 * 1024),
                                record_bytes.len()
                            )));
                        }
                    }
                }
                WriteOpAction::Delete => {
                    if write.record.is_some() {
                        return Err(PdsError::Validation(format!(
                            "Delete operation should not have record value for {}/{}",
                            write.collection, write.rkey
                        )));
                    }
                }
            }

            // Optimistic-concurrency swap-CID check.
            if let Some(ref swap_cid) = write.swap_cid {
                match write.action {
                    WriteOpAction::Update | WriteOpAction::Delete => {
                        let uri = format!("at://{}/{}/{}", self.did, write.collection, write.rkey);

                        match self.store.get_record(&self.did, &uri).await? {
                            Some(current_record) => {
                                if current_record.cid != *swap_cid {
                                    return Err(PdsError::Validation(format!(
                                        "Swap CID mismatch for {}/{}: expected '{}', found '{}'",
                                        write.collection, write.rkey, swap_cid, current_record.cid
                                    )));
                                }
                            }
                            None => {
                                return Err(PdsError::NotFound(format!(
                                    "Cannot swap CID - record not found: {}/{}",
                                    write.collection, write.rkey
                                )));
                            }
                        }
                    }
                    WriteOpAction::Create => {
                        return Err(PdsError::Validation(format!(
                            "swap_cid cannot be used with Create action for {}/{}",
                            write.collection, write.rkey
                        )));
                    }
                }
            }
        }

        Ok(())
    }

    /// Apply batch writes atomically
    pub async fn apply_batch_writes(
        &self,
        writes: Vec<crate::actor_store::models::PreparedWrite>,
        signer: Arc<dyn Signer>,
    ) -> PdsResult<(String, String)> {
        use crate::actor_store::models::WriteOpAction as ModelAction;

        self.validate_batch(&writes).await?;

        let ops: Vec<WriteOp> = writes
            .into_iter()
            .map(|w| {
                let action = match w.action {
                    ModelAction::Create => WriteOpAction::Create,
                    ModelAction::Update => WriteOpAction::Update,
                    ModelAction::Delete => WriteOpAction::Delete,
                };

                WriteOp {
                    action,
                    collection: w.collection,
                    rkey: w.rkey,
                    value: w.record,
                    validate: w.validate,
                    swap_cid: w.swap_cid,
                }
            })
            .collect();

        self.apply_writes(ops, signer).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor_store::ActorStoreConfig;
    use crate::crypto::proto_blue_signer::RepoSigner;
    use proto_blue::crypto::{ExportableKeypair, K256Keypair};
    use tempfile::TempDir;
    use uuid::Uuid;

    /// Build a fresh, isolated `ActorStore` plus the `TempDir` whose
    /// lifetime backs it. The caller MUST keep the `TempDir` alive — the
    /// underlying directory is removed on drop, which would invalidate
    /// every open SQLite connection. Returning the tuple makes the
    /// drop-order requirement obvious at the call site.
    fn test_store() -> (ActorStore, TempDir) {
        let temp = TempDir::new().expect("tempdir");
        let config = ActorStoreConfig {
            base_directory: temp.path().to_path_buf(),
            cache_size: 10,
        };
        (ActorStore::new(config), temp)
    }

    fn unique_did() -> String {
        // PLC DIDs are technically base32 lowercase, but for the in-memory
        // tests the signer doesn't enforce the DID format — uniqueness is
        // all that matters here. Hex from a fresh UUID is plenty.
        format!(
            "did:plc:test{}",
            Uuid::new_v4()
                .simple()
                .to_string()
                .chars()
                .take(24)
                .collect::<String>()
        )
    }

    fn test_signer() -> Arc<dyn Signer> {
        let kp = K256Keypair::generate();
        Arc::new(RepoSigner::from_bytes(&kp.export_private_key()).unwrap())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_repository_initialization() {
        let (store, _tmp) = test_store();
        let repo_mgr = RepositoryManager::new(unique_did(), store);

        let result = repo_mgr.initialize().await;
        assert!(result.is_ok());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_create_record() {
        let (store, _tmp) = test_store();
        let did = unique_did();
        let repo_mgr = RepositoryManager::new(did.clone(), store);

        repo_mgr.initialize().await.unwrap();

        let value = serde_json::json!({
            "text": "Hello, ATProto!",
            "createdAt": "2025-01-01T00:00:00Z"
        });

        let result = repo_mgr
            .create_record("app.bsky.feed.post", None, value, None, test_signer())
            .await;

        assert!(result.is_ok(), "create_record failed: {:?}", result.err());
        let (uri, cid, rev) = result.unwrap();
        assert!(uri.starts_with(&format!("at://{}/app.bsky.feed.post/", did)));
        assert!(!cid.is_empty());
        assert!(!rev.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_apply_writes() {
        let (store, _tmp) = test_store();
        let repo_mgr = RepositoryManager::new(unique_did(), store);

        repo_mgr.initialize().await.unwrap();

        let writes = vec![
            WriteOp {
                action: WriteOpAction::Create,
                collection: "app.bsky.feed.post".to_string(),
                rkey: "post1".to_string(),
                value: Some(serde_json::json!({"text": "Post 1"})),
                validate: None,
                swap_cid: None,
            },
            WriteOp {
                action: WriteOpAction::Create,
                collection: "app.bsky.feed.post".to_string(),
                rkey: "post2".to_string(),
                value: Some(serde_json::json!({"text": "Post 2"})),
                validate: None,
                swap_cid: None,
            },
        ];

        let result = repo_mgr.apply_writes(writes, test_signer()).await;
        assert!(result.is_ok(), "apply_writes failed: {:?}", result.err());
    }
}
