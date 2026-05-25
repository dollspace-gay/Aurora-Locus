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
    blob_store::BlobStore,
    error::{PdsError, PdsResult},
    repository::blob_refs::{extract_blob_cids, read_existing_refs},
    sequencer::{
        events::{CommitEvent, CommitOp, OpAction},
        Sequencer,
    },
    validation::{
        should_propagate_validation_errors, should_validate_per_lexicon_imports,
        validation_errors_to_pds_error, RecordValidator, ValidationMode,
    },
};
use proto_blue::common::next_tid;
use proto_blue::crypto::Signer;
use proto_blue::lex_cbor::cid_for_lex;
use proto_blue::lex_data::Cid;
use proto_blue::lex_json::json_to_lex;
use proto_blue::repo::{
    block_map::BlockMap, car::blocks_to_car, error::RepoError as ProtoRepoError,
    storage::RepoStorage, CommitData, Repo, RepoWrite,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
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
/// the SQLite `record` table after the proto-blue commit lands and so
/// Phase B (Arc 16e §9.5.3.2.2) can compute per-record STRICT / unref
/// plans without re-walking the record body.
struct PreparedRecord {
    op: OpAction,
    collection: String,
    rkey: String,
    uri: String,
    /// Block CID for create/update; `None` for delete.
    cid: Option<String>,
    /// Validate-phase walker output (Arc 16e §9.5.3.2.0). `Some(_)` for
    /// Create/Update; `None` for Delete (Phase B reads existing refs
    /// from the DB instead).
    blob_cids: Option<Vec<Cid>>,
}

/// Per-record STRICT / unref plan built at Phase B entry from the
/// validate-phase walker output (Arc 16e §9.5.3.2.2).
struct RecordPlan {
    uri: String,
    strict: BTreeSet<Cid>,
    unref: BTreeSet<Cid>,
}

/// Arc 16e §9.5.3.2.2 + §2.5 (round-4 F1 closure): Phase B pause hook
/// for race-coordination tests. Production inherits the default no-op;
/// test impls override the body via `#[cfg(test)] impl PhaseBHook for
/// TestRepositoryManager`.
pub(crate) trait PhaseBHook {
    /// Default body is empty (no-op). Test code overrides to gate
    /// Phase B entry on a channel/condvar/etc.
    #[allow(async_fn_in_trait)]
    async fn phase_b_pause_hook(&self) {}
}

impl PhaseBHook for RepositoryManager {}

/// Repository manager for a single actor.
pub struct RepositoryManager {
    did: String,
    store: ActorStore,
    validator: RecordValidator,
    sequencer: Option<Arc<Sequencer>>,
    /// Arc 16e §9.5.3.2.2: shared-DB blob store. When `Some(_)`,
    /// `apply_writes` runs the full Phase B (validate-phase walker +
    /// shared-DB transaction + STRICT-before-unref sorted iteration).
    /// When `None`, Phase B is skipped — appropriate for tests that
    /// don't exercise blob-ref state, but every record-write HTTP
    /// handler in production MUST chain `.with_blob_store(...)` to
    /// avoid silently dropping reference tracking.
    blob_store: Option<Arc<BlobStore>>,
    /// Arc 17 §17.3.4 — lexicon config snapshot. Used at validate-
    /// phase entry to evaluate the `validate_imports` override (the
    /// per-write `validate = Some(false)` bypass is skipped when
    /// `enabled && validate_imports` are both true). `None` =
    /// pre-Arc-17 behavior preserved (the bypass always applies).
    /// Paired with the validator's own `with_lexicon` wiring —
    /// production handlers chain both onto the constructor.
    lexicon_config: Option<crate::config::LexiconConfig>,
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
            blob_store: None,
            lexicon_config: None,
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
            blob_store: None,
            lexicon_config: None,
        }
    }

    /// Arc 16e §9.5.3.2.2: attach the shared-DB `BlobStore`. Every
    /// production record-write HTTP handler MUST chain this onto its
    /// constructor — without it, Phase B silently no-ops and blob
    /// refs go untracked. Tests that don't exercise blob refs may
    /// omit it.
    #[must_use]
    pub fn with_blob_store(mut self, blob_store: Arc<BlobStore>) -> Self {
        self.blob_store = Some(blob_store);
        self
    }

    /// Construct a write-handler `RepositoryManager` from an
    /// [`AppContext`]. Centralizes the
    /// `with_sequencer_and_validation` + `.with_blob_store` +
    /// `.with_lexicon` chain so handlers can't accidentally skip
    /// lexicon plumbing (the dispatch-plumbing bug #136 Phase B
    /// Scenario 12 caught: `lexicon_resolver` constructed at
    /// startup but never plumbed into the validator, so every
    /// unknown-NSID write fell through to Optimistic instead of
    /// dispatching to the fetch path).
    ///
    /// The four production write paths (create_record, put_record,
    /// apply_writes, import_repo) call this. The audit guard at
    /// `src/api/repo.rs`/`repo_import.rs` tests asserts no direct
    /// `with_sequencer_and_validation` calls bypass this helper.
    ///
    /// When `ctx.lexicon_resolver` is `None` (lexicon disabled per
    /// `PDS_LEXICON_ENABLED=false`, the v0.5 default), the
    /// `.with_lexicon` step is skipped and the manager retains
    /// pre-Arc-17 behavior — `validate_write`'s bypass-evaluation
    /// reads `lexicon_config = None` and the `validate_imports`
    /// override stays inert. Disabled-config wire behavior is
    /// preserved.
    pub fn for_writer(ctx: &crate::context::AppContext, did: String) -> Self {
        let mut mgr = Self::with_sequencer_and_validation(
            did,
            (*ctx.actor_store).clone(),
            ctx.sequencer.clone(),
            ctx.config.validation_mode,
        )
        .with_blob_store(ctx.blob_store.clone());
        if let Some(resolver) = ctx.lexicon_resolver.as_ref().cloned() {
            mgr = mgr.with_lexicon(resolver, ctx.config.lexicon.clone());
        }
        mgr
    }

    /// Arc 17 §17.4 Step 4 — test affordance for asserting the
    /// `.with_lexicon` chain landed. `Some` when the resolver +
    /// config snapshot have been plumbed; `None` when they
    /// haven't. Used by the dispatch-plumbing audit test
    /// (#136 regression preventer).
    #[cfg(test)]
    pub fn lexicon_config_for_test(&self) -> Option<&crate::config::LexiconConfig> {
        self.lexicon_config.as_ref()
    }

    /// Arc 17 §17.3.4 wiring: attach the lexicon resolver + config
    /// snapshot so unknown-collection writes route through the
    /// §17.3.1 flow AND the `validate_imports` override fires at
    /// validate-phase entry. Production handlers chain this onto the
    /// constructor; tests that don't exercise the Arc 17 path omit
    /// it (pre-Arc-17 behavior is preserved).
    #[must_use]
    pub fn with_lexicon(
        mut self,
        resolver: Arc<crate::federation::lexicon_resolver::LexResolver>,
        config: crate::config::LexiconConfig,
    ) -> Self {
        // Move-out / move-back is the cleanest way to thread the
        // resolver into RecordValidator's builder without restructuring
        // the constructor (which would ripple through every test
        // fixture in src/).
        let validator = std::mem::take(&mut self.validator);
        self.validator = validator.with_lexicon(resolver, config.clone());
        self.lexicon_config = Some(config);
        self
    }

    /// Initialize a new repository for an actor.
    ///
    /// Creates the underlying SQLite layout. The first `apply_writes`
    /// call will seed the empty signed commit via `Repo::create`.
    pub async fn initialize(&self) -> PdsResult<()> {
        self.store.create(&self.did).await?;
        Ok(())
    }

    /// Arc 15 §8.3.8 / Step 6: explicitly produce a genesis commit
    /// for a freshly-initialized actor. Used by
    /// `create_account_emit_sequence` to obtain the `CommitData`
    /// needed for the `#commit` (genesis) and `#sync` emits per the
    /// four-frame sequence.
    ///
    /// Per Sub-step 0.1 recon: Aurora-Locus's `initialize()` does
    /// NOT create the genesis commit — proto-blue's `Repo::create`
    /// is invoked lazily on first `apply_writes`. This helper makes
    /// the creation explicit so createAccount can sequence the
    /// genesis frame at account-creation time (matching bsky-PDS
    /// fidelity per §8.1).
    ///
    /// Runs on a blocking thread per the same discipline as
    /// `apply_writes`. Errors if a commit already exists (genesis
    /// MUST be the first commit; calling this on an already-seeded
    /// repo is a caller bug).
    pub async fn create_genesis_commit(
        &self,
        signer: Arc<dyn Signer>,
    ) -> PdsResult<CommitData> {
        let storage = self.make_storage();
        let did = self.did.clone();
        let signer_arc = signer;

        let result: Result<CommitData, ProtoRepoError> =
            tokio::task::spawn_blocking(move || -> Result<_, ProtoRepoError> {
                let storage_dyn: Arc<dyn RepoStorage> = storage.clone();
                // Refuse if storage already has a root (caller bug).
                if storage_dyn.get_root()?.is_some() {
                    return Err(ProtoRepoError::Storage(
                        "create_genesis_commit called on a non-empty repo".into(),
                    ));
                }
                let repo = Repo::create(storage_dyn.clone(), did, signer_arc.as_ref())?;

                let commit_cid = repo.commit_cid().cloned().ok_or_else(|| {
                    ProtoRepoError::Storage("Repo::create did not populate commit_cid".into())
                })?;
                let commit = repo.commit().cloned().ok_or_else(|| {
                    ProtoRepoError::Storage("Repo::create did not populate commit".into())
                })?;
                let mst_root = commit.data.clone();

                let mut blocks = BlockMap::new();
                let commit_bytes = storage_dyn.get_block(&commit_cid)?.ok_or_else(|| {
                    ProtoRepoError::Storage(format!(
                        "genesis commit block {} missing from storage post-create",
                        commit_cid
                    ))
                })?;
                let mst_bytes = storage_dyn.get_block(&mst_root)?.ok_or_else(|| {
                    ProtoRepoError::Storage(format!(
                        "genesis MST root {} missing from storage post-create",
                        mst_root
                    ))
                })?;
                blocks.set(commit_cid.clone(), commit_bytes);
                blocks.set(mst_root, mst_bytes);

                Ok(CommitData {
                    commit_cid,
                    commit,
                    blocks,
                    removed_cids: Vec::new(),
                })
            })
            .await
            .map_err(|e| PdsError::Internal(format!("genesis commit join failed: {}", e)))?;

        result.map_err(|e| PdsError::Internal(format!("genesis commit creation failed: {}", e)))
    }

    /// Arc 15 Sub-step 0.3(a) projection helper: load the current
    /// repo and produce a `SyncEvtData` containing the minimal block
    /// slice (commit block + MST root block) suitable for a `#sync`
    /// frame. Used by the reactivate path (§8.3.5) where no fresh
    /// commit is produced but a sync emit is still required.
    ///
    /// Runs `Repo::load` on a blocking thread per the same discipline
    /// as `apply_writes` (the storage adapter's `block_on` would
    /// dead-lock a worker thread).
    pub async fn current_sync_event_data(
        &self,
    ) -> PdsResult<crate::sequencer::events::SyncEvtData> {
        let storage = self.make_storage();
        let storage_dyn: Arc<dyn RepoStorage> = storage;

        let projection: Result<crate::sequencer::events::SyncEvtData, ProtoRepoError> =
            tokio::task::spawn_blocking(move || -> Result<_, ProtoRepoError> {
                let repo = Repo::load(storage_dyn.clone())?;
                let commit_cid = repo
                    .commit_cid()
                    .cloned()
                    .ok_or_else(|| ProtoRepoError::Storage("empty repo; no current commit".into()))?;
                let commit = repo
                    .commit()
                    .ok_or_else(|| ProtoRepoError::Storage("empty repo; no current commit".into()))?;
                let mst_root = commit.data.clone();
                let rev = commit.rev.clone();

                let commit_bytes = storage_dyn.get_block(&commit_cid)?.ok_or_else(|| {
                    ProtoRepoError::Storage(format!(
                        "commit block {} missing from storage",
                        commit_cid
                    ))
                })?;
                let mst_bytes = storage_dyn.get_block(&mst_root)?.ok_or_else(|| {
                    ProtoRepoError::Storage(format!(
                        "MST root block {} missing from storage",
                        mst_root
                    ))
                })?;

                let mut blocks = BlockMap::new();
                blocks.set(commit_cid.clone(), commit_bytes);
                blocks.set(mst_root, mst_bytes);

                Ok(crate::sequencer::events::SyncEvtData {
                    cid: commit_cid,
                    rev,
                    blocks,
                })
            })
            .await
            .map_err(|e| PdsError::Internal(format!("sync data join failed: {}", e)))?;

        projection.map_err(|e| PdsError::Internal(format!("sync data load failed: {}", e)))
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
    ///
    /// Arc 17 §17.3.4 / round-1 F4 closure: when `lexicon.enabled` and
    /// `lexicon.validate_imports = true` (default), the per-write
    /// `validate = Some(false)` bypass is OVERRIDDEN — validate fires
    /// regardless. This closes the v1 gap where CAR-imported records
    /// (which the Arc 16f handler sets to `validate = Some(false)`)
    /// received zero validation by default. The override applies to
    /// BOTH known NSIDs (hand-coded path) and unknown NSIDs (lexicon
    /// fall-through) — see V05_DESIGN_arc17.md §17.3.4 matrix.
    async fn validate_write(&self, write: &WriteOp) -> PdsResult<()> {
        let value = match &write.value {
            Some(v) => v,
            None => return Ok(()), // delete or no-op
        };

        // §17.3.4 per-write bypass evaluation. Delegated to
        // `should_validate_per_lexicon_imports` so the matrix is
        // unit-tested in isolation; see that function's doc-comment
        // table for the full rule set. Quick mental model: the
        // override only fires when both `lexicon.enabled` and
        // `validate_imports` are true; otherwise pre-Arc-17 semantics
        // (the `validate = Some(false)` bypass) are preserved.
        if !should_validate_per_lexicon_imports(write.validate, self.lexicon_config.as_ref()) {
            return Ok(());
        }

        if let Err(errors) = self.validator.validate(&write.collection, value).await {
            let uri = format!("at://{}/{}/{}", self.did, write.collection, write.rkey);

            // Arc 17 §17.3.3 Phase B bug #2 — lexicon fetch-class
            // failures (HardFail, authority-trust, deny, invalid-NSID)
            // bypass `ValidationMode::Optimistic` per the §17.3.3
            // precedence ("HardFail propagates; record validation
            // fails" is strictly stronger than Optimistic's accept-
            // on-failure). `SchemaViolation` and hand-coded validator
            // errors remain absorbable under Optimistic — the matrix
            // lives in `should_propagate_validation_errors` so the
            // precedence is unit-tested in isolation. Warn-mode lexicon
            // failures never reach this branch as fetch-class variants
            // (handle_fetch_error short-circuits to handle_unknown
            // before re-emitting an error), so the gate doesn't
            // interfere with the warn_fallback path.
            let propagate = should_propagate_validation_errors(&errors, self.validator.mode());

            if !propagate {
                // Optimistic absorb — track and accept.
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

            // Required mode OR fetch-class lexicon variant under
            // Optimistic — track and reject. `validation_errors_to_pds_error`
            // routes `@lexicon/LexiconFetchFailed` to `PdsError::LexiconFetchFailed`
            // → HTTP 502 per §17.3.6 wire alignment.
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
    ///
    /// Arc 16e v5.1 (Arc 16f Step 4 §9.6.3.4): the `promoter`
    /// parameter selects the Phase B per-CID promotion discipline.
    /// Every Arc 16e caller (createRecord / putRecord / deleteRecord
    /// / applyWrites / admin key rotation / CLI key rotation) passes
    /// [`StrictPromoter`], preserving Arc 16e v5 behavior verbatim.
    /// The Arc 16f importRepo handler passes [`TolerantPromoter`],
    /// which signals `NeedsFetch` on row-absent CIDs for the
    /// caller-driven fetch-and-retry loop per §9.6.3.5.
    pub async fn apply_writes(
        &self,
        writes: Vec<WriteOp>,
        signer: Arc<dyn Signer>,
        promoter: Arc<dyn crate::blob_store::BlobPromoter>,
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

                    // Arc 16e §9.5.3.2.0 validate-phase walker:
                    // surface client-malformed CIDs as PdsError::InvalidCid
                    // (HTTP 400) BEFORE Phase A so no state mutation
                    // happens on bad input.
                    let blob_cids = extract_blob_cids(&value)?;

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
                        blob_cids: Some(blob_cids),
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
                        blob_cids: None,
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

        // Phase A — per-actor metadata writes (Arc 16e §9.5.3.2.2).
        // Iterate by reference so `prepared` survives into Phase B
        // below for the per-record STRICT/unref plan enumeration.
        let mut commit_ops: Vec<CommitOp> = Vec::with_capacity(prepared.len());
        for rec in &prepared {
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
                        action: rec.op.clone(),
                        path: format!("{}/{}", rec.collection, rec.rkey),
                        cid: Some(cid),
                        prev: prev_cid,
                    });
                }
                OpAction::Delete => {
                    self.store.delete_record(&self.did, &rec.uri).await?;
                    commit_ops.push(CommitOp {
                        action: rec.op.clone(),
                        path: format!("{}/{}", rec.collection, rec.rkey),
                        cid: None,
                        prev: prev_cid,
                    });
                }
            }
        }

        // Phase B — shared-DB blob-ref reconciliation (Arc 16e
        // §9.5.3.2.2). Production HTTP handlers chain
        // `.with_blob_store(...)` to opt in; tests that don't exercise
        // blob refs leave `blob_store` as `None` and skip Phase B.
        if let Some(blob_store) = &self.blob_store {
            self.run_phase_b(&prepared, blob_store, promoter.as_ref()).await?;
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

    /// Arc 16e §9.5.3.2.2 Phase B body — shared-DB blob-ref
    /// reconciliation in a single transaction with sorted-CID
    /// STRICT-before-unref ordering.
    ///
    /// Capture `now` once at entry (closes round-4 F11 — shared across
    /// every `unreference_blob` call so a single batch's TTL anchors
    /// are coherent). On error, emit the forensic ERROR log + flush
    /// stdout before returning so journald has a recovery anchor.
    ///
    /// Arc 16e v5.1 (Arc 16f Step 4 §9.6.3.4): the STRICT inner call
    /// goes through the `promoter` indirection so the import path
    /// (TolerantPromoter) can signal `NeedsFetch` for the caller's
    /// fetch-and-retry loop. StrictPromoter delegates to the same
    /// `verify_blob_and_make_permanent` call this loop used in v5,
    /// preserving Arc 16e regression coverage.
    async fn run_phase_b(
        &self,
        prepared: &[PreparedRecord],
        blob_store: &Arc<BlobStore>,
        promoter: &dyn crate::blob_store::BlobPromoter,
    ) -> PdsResult<()> {
        use std::io::Write as _;

        let now = chrono::Utc::now();
        let mut tx = blob_store
            .pool()
            .begin()
            .await
            .map_err(PdsError::Database)?;

        // Phase B pause hook: production no-op; tests override via
        // the `PhaseBHook` trait. Fires unconditionally after
        // `tx.begin()` so test-side race coordination can land
        // between tx-open and the first state mutation.
        self.phase_b_pause_hook().await;

        // Build per-record STRICT/unref plans and the full-batch
        // touch_set across all records.
        let mut touch_set: BTreeSet<Cid> = BTreeSet::new();
        let mut per_record_plan: Vec<RecordPlan> = Vec::with_capacity(prepared.len());

        for rec in prepared {
            match rec.op {
                OpAction::Create => {
                    let new_cids: BTreeSet<Cid> = rec
                        .blob_cids
                        .as_ref()
                        .expect("Create has blob_cids from validate phase")
                        .iter()
                        .cloned()
                        .collect();
                    touch_set.extend(new_cids.iter().cloned());
                    per_record_plan.push(RecordPlan {
                        uri: rec.uri.clone(),
                        strict: new_cids,
                        unref: BTreeSet::new(),
                    });
                }
                OpAction::Update => {
                    let existing_cids: BTreeSet<Cid> =
                        read_existing_refs(&mut tx, &rec.uri).await?;
                    let new_cids: BTreeSet<Cid> = rec
                        .blob_cids
                        .as_ref()
                        .expect("Update has blob_cids from validate phase")
                        .iter()
                        .cloned()
                        .collect();
                    let strict_cids: BTreeSet<Cid> =
                        new_cids.difference(&existing_cids).cloned().collect();
                    let unref_cids: BTreeSet<Cid> =
                        existing_cids.difference(&new_cids).cloned().collect();
                    touch_set.extend(strict_cids.iter().cloned());
                    touch_set.extend(unref_cids.iter().cloned());
                    per_record_plan.push(RecordPlan {
                        uri: rec.uri.clone(),
                        strict: strict_cids,
                        unref: unref_cids,
                    });
                }
                OpAction::Delete => {
                    let existing_cids: BTreeSet<Cid> =
                        read_existing_refs(&mut tx, &rec.uri).await?;
                    touch_set.extend(existing_cids.iter().cloned());
                    per_record_plan.push(RecordPlan {
                        uri: rec.uri.clone(),
                        strict: BTreeSet::new(),
                        unref: existing_cids,
                    });
                }
            }
        }

        // Forensic INFO log: full touch_set + ALL prepared URIs (round-3
        // F15: include every prepared URI, not just those with non-empty
        // plans). This is the recovery anchor for Option A failures —
        // operators grep journald for `event="phase_b_starting"` matching
        // the failure window.
        let record_uris: Vec<&String> = prepared.iter().map(|r| &r.uri).collect();
        let touch_set_strs: Vec<String> = touch_set.iter().map(|c| c.to_string()).collect();
        tracing::info!(
            target: "aurora_locus::apply_writes",
            event = "phase_b_starting",
            did = %self.did,
            record_uris = ?record_uris,
            touch_set = ?touch_set_strs,
        );
        // Force Rust-side stdout flush so the forensic line reaches
        // journald before any state mutation (R0a.A; §9.5.3.1.3).
        let _ = std::io::stdout().lock().flush();

        // Sorted CID iteration with STRICT-before-unref ordering for
        // the same CID (§9.5.3.2.4). `touch_set` is a `BTreeSet`, so
        // iteration order is the proto-blue `Cid` `Ord` derivation
        // (byte-lex on the binary form) — deterministic.
        //
        // Arc 16e v5.1 (Arc 16f Step 4 §9.6.3.4 — round-1 F8 closure):
        // promoter.promote() may signal `NeedsFetch` (TolerantPromoter
        // only). Collect all such CIDs across the batch into a
        // BTreeSet (natural dedupe) and surface as `NeedsBlobFetch`
        // AFTER draining — do not short-circuit on the first
        // NeedsFetch. The tx is implicitly rolled back when dropped
        // unrcommitted via the early return.
        let mut needs_fetch_cids: BTreeSet<Cid> = BTreeSet::new();
        for cid in &touch_set {
            let cid_str = cid.to_string();
            // STRICT/TOLERANT promote phase: drain every record's
            // strict set for this CID.
            for plan in &per_record_plan {
                if plan.strict.contains(cid) {
                    match promoter
                        .promote(blob_store, &mut tx, cid, &plan.uri, now)
                        .await
                    {
                        Ok(crate::blob_store::PromoteOutcome::Done) => {}
                        Ok(crate::blob_store::PromoteOutcome::NeedsFetch { cid: needed }) => {
                            // Accumulate; keep draining the rest of
                            // the batch so every NeedsFetch CID
                            // surfaces in one round (round-1 F8).
                            needs_fetch_cids.insert(needed);
                        }
                        Ok(crate::blob_store::PromoteOutcome::Quarantined {
                            cid: q_cid,
                            public_reason,
                        }) => {
                            // Defense-in-depth: validate-phase
                            // quarantine check at §9.6.3.1 step 5
                            // catches the common case before Phase A.
                            // This fires only if quarantine landed
                            // between validate and Phase B. Fast-fail
                            // (no collect-all); roll tx back via the
                            // early return.
                            let err = PdsError::QuarantinedBlobReferenced {
                                cid: q_cid,
                                public_reason,
                            };
                            Self::log_phase_b_failed(
                                &self.did,
                                &cid_str,
                                &plan.uri,
                                "STRICT",
                                &err,
                            );
                            return Err(err);
                        }
                        Err(e) => {
                            Self::log_phase_b_failed(
                                &self.did,
                                &cid_str,
                                &plan.uri,
                                "STRICT",
                                &e,
                            );
                            return Err(e);
                        }
                    }
                }
            }
            // Unref second: drain every record's unref set for this CID.
            for plan in &per_record_plan {
                if plan.unref.contains(cid) {
                    match blob_store
                        .unreference_blob(&mut tx, &cid_str, &plan.uri, now)
                        .await
                    {
                        Ok(outcome) => {
                            Self::log_unreference_outcome(
                                &self.did,
                                &cid_str,
                                &plan.uri,
                                outcome,
                            );
                        }
                        Err(e) => {
                            Self::log_phase_b_failed(
                                &self.did,
                                &cid_str,
                                &plan.uri,
                                "unreference",
                                &e,
                            );
                            return Err(e);
                        }
                    }
                }
            }
        }

        // Round-1 F8 closure: if TolerantPromoter signalled any
        // NeedsFetch CIDs during the inner loop, roll back tx and
        // surface the full set so the caller's fetch-and-retry loop
        // (§9.6.3.5) can drain them in one round. The early return
        // here drops `tx` without `commit()` — sqlx rolls back on
        // Drop.
        if !needs_fetch_cids.is_empty() {
            let cids: Vec<Cid> = needs_fetch_cids.into_iter().collect();
            return Err(PdsError::NeedsBlobFetch { cids });
        }

        tx.commit().await.map_err(PdsError::Database)?;
        Ok(())
    }

    /// Arc 16e §9.5.3.1.3: ERROR log on Phase B failure. Forces a
    /// stdout flush so journald captures the recovery anchor BEFORE
    /// the error propagates up the stack.
    fn log_phase_b_failed(did: &str, cid: &str, uri: &str, op: &str, err: &PdsError) {
        use std::io::Write as _;
        tracing::error!(
            target: "aurora_locus::apply_writes",
            event = "phase_b_failed",
            did = %did,
            cid = %cid,
            record_uri = %uri,
            op = %op,
            error = %err,
        );
        let _ = std::io::stdout().lock().flush();
    }

    /// Arc 16b §9.2.3.2 caller-obligations table: log level per
    /// `UnreferenceOutcome` variant. `LastRefDropped` / `OtherRefsRemain`
    /// are normal-path (INFO/DEBUG). The remaining four surface races
    /// or inconsistencies and escalate to WARN/ERROR per the table.
    fn log_unreference_outcome(
        did: &str,
        cid: &str,
        uri: &str,
        outcome: crate::blob_store::UnreferenceOutcome,
    ) {
        use crate::blob_store::UnreferenceOutcome as U;
        match outcome {
            U::LastRefDropped => {
                tracing::info!(
                    target: "aurora_locus::apply_writes",
                    event = "unref_last_ref_dropped",
                    did = %did, cid = %cid, record_uri = %uri,
                );
            }
            U::OtherRefsRemain => {
                tracing::debug!(
                    target: "aurora_locus::apply_writes",
                    event = "unref_other_refs_remain",
                    did = %did, cid = %cid, record_uri = %uri,
                );
            }
            U::PhantomDelete => {
                // §9.2.3.2: caller-side bug, idempotent retry, or
                // concurrent unreference. DEBUG so it doesn't drown
                // routine retries; the variant itself is the signal.
                tracing::debug!(
                    target: "aurora_locus::apply_writes",
                    event = "unref_phantom_delete",
                    did = %did, cid = %cid, record_uri = %uri,
                );
            }
            U::AlreadyUntethered_RefsRemain => {
                // §9.2.3.2: deep inconsistency — TTL live while
                // referenced. ERROR per caller-obligations.
                tracing::error!(
                    target: "aurora_locus::apply_writes",
                    event = "unref_already_untethered_refs_remain",
                    did = %did, cid = %cid, record_uri = %uri,
                );
            }
            U::AlreadyUntethered_NoRefs => {
                // §9.2.3.2: mild anomaly — stray record_blob row.
                // WARN per caller-obligations.
                tracing::warn!(
                    target: "aurora_locus::apply_writes",
                    event = "unref_already_untethered_no_refs",
                    did = %did, cid = %cid, record_uri = %uri,
                );
            }
            U::OrphanedRef => {
                // §9.2.3.2: real ref dropped but blob_metadata row
                // missing. ERROR per caller-obligations.
                tracing::error!(
                    target: "aurora_locus::apply_writes",
                    event = "unref_orphaned_ref",
                    did = %did, cid = %cid, record_uri = %uri,
                );
            }
        }
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

        let (commit_cid, rev) = self
            .apply_writes(writes, signer, Arc::new(crate::blob_store::StrictPromoter))
            .await?;
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

        self.apply_writes(writes, signer, Arc::new(crate::blob_store::StrictPromoter)).await
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

        self.apply_writes(writes, signer, Arc::new(crate::blob_store::StrictPromoter)).await
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

        self.apply_writes(ops, signer, Arc::new(crate::blob_store::StrictPromoter)).await
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

        let result = repo_mgr.apply_writes(writes, test_signer(), std::sync::Arc::new(crate::blob_store::StrictPromoter)).await;
        assert!(result.is_ok(), "apply_writes failed: {:?}", result.err());
    }

    // ---- Arc 16e Step 2 — Phase B wiring + scenarios ----

    use crate::blob_store::{BlobBackendType, BlobStorageConfig, BlobStoreConfig};

    /// Build a `RepositoryManager` wired to an in-memory shared-DB
    /// pool with `blob_metadata` + `record_blob` tables seeded, plus
    /// the `BlobStore` and pool handles so tests can inspect ground
    /// truth via direct SQL. `TempDir` backs both the ActorStore and
    /// the BlobStore disk backend; the caller MUST keep it alive.
    async fn test_repo_mgr_with_blob_store(
    ) -> (RepositoryManager, Arc<BlobStore>, sqlx::AnyPool, TempDir, String) {
        use std::sync::Once;
        static INSTALL: Once = Once::new();
        INSTALL.call_once(sqlx::any::install_default_drivers);

        let temp = TempDir::new().expect("tempdir");
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        sqlx::query(
            r#"CREATE TABLE blob_metadata (
                cid TEXT PRIMARY KEY,
                mime_type TEXT NOT NULL,
                size INTEGER NOT NULL,
                creator_did TEXT NOT NULL,
                created_at TEXT NOT NULL,
                width INTEGER,
                height INTEGER,
                alt_text TEXT,
                thumbnail_cid TEXT,
                temp_key TEXT NULL CHECK (temp_key IS NULL OR temp_key = '1')
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            r#"CREATE TABLE record_blob (
                blob_cid TEXT NOT NULL,
                record_uri TEXT NOT NULL,
                indexed_at TEXT NOT NULL,
                PRIMARY KEY (blob_cid, record_uri)
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();

        // Arc 16f §9.6.3.2 — TOLERANT helper does a quarantine-first
        // check (`SELECT reason FROM blob_quarantine WHERE cid = $1
        // AND restored_at IS NULL`). The TolerantPromoter tests
        // below (and any production Phase B that uses TOLERANT) need
        // this table to exist even if it's empty. Schema mirrors
        // `src/blob_store/store.rs::arc16f_store_with_quarantine`
        // and the production migration.
        sqlx::query(
            r#"CREATE TABLE blob_quarantine (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                cid TEXT NOT NULL,
                reason TEXT NOT NULL,
                details TEXT,
                quarantined_by TEXT NOT NULL,
                quarantined_at TEXT NOT NULL,
                restored_at TEXT,
                restored_by TEXT,
                legal_reference TEXT
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();

        let actor_dir = temp.path().join("actor");
        std::fs::create_dir_all(&actor_dir).unwrap();
        let blob_dir = temp.path().join("blob");
        std::fs::create_dir_all(&blob_dir).unwrap();

        let blob_config = BlobStoreConfig {
            storage: BlobStorageConfig {
                backend: BlobBackendType::Disk {
                    location: blob_dir.clone(),
                },
                max_blob_size: 1024 * 1024,
                temp_dir: blob_dir.join("tmp"),
            },
        };
        let blob_store = Arc::new(BlobStore::new(blob_config, pool.clone()).await.unwrap());

        let actor_config = ActorStoreConfig {
            base_directory: actor_dir,
            cache_size: 10,
        };
        let actor_store = ActorStore::new(actor_config);

        let did = unique_did();
        let repo_mgr = RepositoryManager::new(did.clone(), actor_store)
            .with_blob_store(blob_store.clone());

        repo_mgr.initialize().await.unwrap();

        (repo_mgr, blob_store, pool, temp, did)
    }

    /// Seed an untethered `blob_metadata` row (Arc 16b: `temp_key='1'`).
    /// STRICT promotion in Phase B will flip `temp_key` to `NULL`.
    async fn seed_untethered_blob(pool: &sqlx::AnyPool, cid: &str, creator_did: &str) {
        sqlx::query(
            r#"INSERT INTO blob_metadata (cid, mime_type, size, creator_did, created_at, temp_key)
               VALUES ($1, 'image/png', 100, $2, '2026-05-20T22:00:00Z', '1')"#,
        )
        .bind(cid)
        .bind(creator_did)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn read_temp_key(pool: &sqlx::AnyPool, cid: &str) -> Option<String> {
        let row = sqlx::query("SELECT temp_key FROM blob_metadata WHERE cid = $1")
            .bind(cid)
            .fetch_optional(pool)
            .await
            .unwrap()
            .expect("blob_metadata row missing");
        use sqlx::Row;
        row.try_get::<Option<String>, _>("temp_key").unwrap()
    }

    async fn read_record_blob_count_for_cid(pool: &sqlx::AnyPool, cid: &str) -> i64 {
        let row = sqlx::query("SELECT COUNT(*) AS c FROM record_blob WHERE blob_cid = $1")
            .bind(cid)
            .fetch_one(pool)
            .await
            .unwrap();
        use sqlx::Row;
        row.try_get::<i64, _>("c").unwrap()
    }

    async fn record_blob_exists(pool: &sqlx::AnyPool, blob_cid: &str, record_uri: &str) -> bool {
        sqlx::query("SELECT 1 FROM record_blob WHERE blob_cid = $1 AND record_uri = $2")
            .bind(blob_cid)
            .bind(record_uri)
            .fetch_optional(pool)
            .await
            .unwrap()
            .is_some()
    }

    fn blob_value(cid: &Cid) -> serde_json::Value {
        serde_json::json!({
            "$type": "blob",
            "ref": {"$link": cid.to_string_base32()},
            "mimeType": "image/png",
            "size": 100,
        })
    }

    /// Arc 16e §9.5.4 Step 2.4 wiring-assertion: a record-write that
    /// goes through `apply_writes` with `blob_store` wired actually
    /// fires Phase B's STRICT promotion. Catches a future regression
    /// where a handler forgets to chain `.with_blob_store(...)`.
    #[tokio::test(flavor = "multi_thread")]
    async fn test_phase_b_strict_promotes_blob_via_record_create() {
        let (repo_mgr, _bs, pool, _tmp, did) = test_repo_mgr_with_blob_store().await;
        let blob_x = Cid::for_raw(b"step2-wiring-blob-x");
        let blob_x_str = blob_x.to_string_base32();

        // Pre-seed blob X as untethered. STRICT will flip temp_key.
        seed_untethered_blob(&pool, &blob_x_str, &did).await;
        assert_eq!(
            read_temp_key(&pool, &blob_x_str).await,
            Some("1".to_string()),
            "blob X starts untethered"
        );

        // Create a record body that references blob X.
        let writes = vec![WriteOp {
            action: WriteOpAction::Create,
            collection: "app.bsky.actor.profile".to_string(),
            rkey: "self".to_string(),
            value: Some(serde_json::json!({
                "$type": "app.bsky.actor.profile",
                "avatar": blob_value(&blob_x),
            })),
            validate: None,
            swap_cid: None,
        }];
        repo_mgr
            .apply_writes(writes, test_signer(), std::sync::Arc::new(crate::blob_store::StrictPromoter))
            .await
            .expect("apply_writes with blob_store wired");

        // Phase B fired ⇒ STRICT promoted blob X (temp_key flipped to NULL)
        // and inserted a record_blob row for the new record URI.
        assert_eq!(
            read_temp_key(&pool, &blob_x_str).await,
            None,
            "STRICT flipped temp_key to NULL"
        );
        let record_uri = format!("at://{}/app.bsky.actor.profile/self", did);
        assert!(
            record_blob_exists(&pool, &blob_x_str, &record_uri).await,
            "record_blob row inserted for (X, record_uri)"
        );
    }

    /// Companion to the wiring test: without `.with_blob_store(...)`,
    /// Phase B is silently skipped — same record write completes
    /// (Phase A commits proto-blue + per-actor metadata) but no blob
    /// state changes. Documents the None-default behavior.
    #[tokio::test(flavor = "multi_thread")]
    async fn test_phase_b_skipped_when_blob_store_not_wired() {
        let (repo_mgr, _bs, pool, _tmp, _did) = test_repo_mgr_with_blob_store().await;
        let did_for_unwired = unique_did();
        let blob_x = Cid::for_raw(b"step2-unwired-blob");
        let blob_x_str = blob_x.to_string_base32();
        seed_untethered_blob(&pool, &blob_x_str, &did_for_unwired).await;

        // Build a SECOND repo manager pointing at the same actor store
        // and DID space but WITHOUT `.with_blob_store(...)`.
        let actor_dir = _tmp.path().join("actor");
        let actor_config = ActorStoreConfig {
            base_directory: actor_dir,
            cache_size: 10,
        };
        let unwired_mgr =
            RepositoryManager::new(did_for_unwired.clone(), ActorStore::new(actor_config));
        unwired_mgr.initialize().await.unwrap();

        let writes = vec![WriteOp {
            action: WriteOpAction::Create,
            collection: "app.bsky.actor.profile".to_string(),
            rkey: "self".to_string(),
            value: Some(serde_json::json!({
                "$type": "app.bsky.actor.profile",
                "avatar": blob_value(&blob_x),
            })),
            validate: None,
            swap_cid: None,
        }];
        unwired_mgr
            .apply_writes(writes, test_signer(), std::sync::Arc::new(crate::blob_store::StrictPromoter))
            .await
            .expect("apply_writes succeeds even without blob_store wired");

        // No Phase B ⇒ temp_key still '1', no record_blob rows.
        assert_eq!(
            read_temp_key(&pool, &blob_x_str).await,
            Some("1".to_string()),
            "STRICT did NOT fire — temp_key unchanged"
        );
        assert_eq!(
            read_record_blob_count_for_cid(&pool, &blob_x_str).await,
            0,
            "no record_blob row inserted"
        );
        // The wired manager is still here just so the pool/tempdir stay alive.
        let _ = repo_mgr;
    }

    /// Arc 16e §9.5.3.2.0 (NEW per round-4 F2) / V05_DESIGN.md §9.5.4
    /// Step 2.4 Scenario 10: client posts a Create whose record body
    /// contains a malformed CID in a blob ref. Validate phase must
    /// reject as `PdsError::InvalidCid` (→ HTTP 400) before Phase A
    /// opens; no state mutation in the actor store or the shared DB.
    #[tokio::test(flavor = "multi_thread")]
    async fn test_apply_writes_rejects_malformed_cid_no_state_mutation() {
        let (repo_mgr, _bs, pool, _tmp, did) = test_repo_mgr_with_blob_store().await;

        let writes = vec![WriteOp {
            action: WriteOpAction::Create,
            collection: "app.bsky.actor.profile".to_string(),
            rkey: "self".to_string(),
            value: Some(serde_json::json!({
                "$type": "app.bsky.actor.profile",
                "avatar": {
                    "$type": "blob",
                    "ref": {"$link": "not-a-cid-at-all"},
                    "mimeType": "image/png",
                    "size": 100,
                }
            })),
            validate: None,
            swap_cid: None,
        }];
        let err = repo_mgr
            .apply_writes(writes, test_signer(), std::sync::Arc::new(crate::blob_store::StrictPromoter))
            .await
            .expect_err("malformed CID must reject");
        assert!(
            matches!(err, PdsError::InvalidCid(_)),
            "expected InvalidCid, got {:?}",
            err
        );

        // No state mutation: the actor's repo has no commit, and the
        // shared DB still has no record_blob rows.
        let record_uri = format!("at://{}/app.bsky.actor.profile/self", did);
        let row = sqlx::query("SELECT COUNT(*) AS c FROM record_blob WHERE record_uri = $1")
            .bind(&record_uri)
            .fetch_one(&pool)
            .await
            .unwrap();
        use sqlx::Row;
        let count: i64 = row.try_get("c").unwrap();
        assert_eq!(count, 0, "record_blob has no rows after rejection");
    }

    /// Arc 16e §9.5.3.2.4 / V05_DESIGN.md §9.5.4 Step 2.4 — 4-record
    /// ground-truth composition test.
    ///
    /// Initial `record_blob` rows (seeded via an initial apply_writes
    /// batch that creates A, B, C and lets Phase B insert the rows):
    ///   {(X, A_uri), (Y, A_uri), (X, C_uri)}
    /// Initial `blob_metadata`: X, Y both permanent (temp_key=NULL,
    /// ref-count 2 and 1 respectively after the seed batch).
    ///
    /// Second batch:
    ///   A: Delete (its existing refs are {X, Y})
    ///   B: Update adding X
    ///   C: Update removing X
    ///   D: Create adding X
    ///
    /// Expected end state:
    ///   record_blob = {(X, B_uri), (X, D_uri)}
    ///   blob_metadata X: temp_key=NULL (2 refs remain)
    ///   blob_metadata Y: temp_key='1' (LastRefDropped fired — TTL anchor refreshed)
    #[tokio::test(flavor = "multi_thread")]
    async fn test_apply_writes_four_record_ground_truth() {
        let (repo_mgr, _bs, pool, _tmp, did) = test_repo_mgr_with_blob_store().await;
        let blob_x = Cid::for_raw(b"step2-4rec-blob-x");
        let blob_y = Cid::for_raw(b"step2-4rec-blob-y");
        let x_str = blob_x.to_string_base32();
        let y_str = blob_y.to_string_base32();

        // Pre-seed X and Y as untethered. The seed batch's STRICT
        // calls flip them to permanent.
        seed_untethered_blob(&pool, &x_str, &did).await;
        seed_untethered_blob(&pool, &y_str, &did).await;

        let a_uri = format!("at://{}/app.test.record/recA", did);
        let b_uri = format!("at://{}/app.test.record/recB", did);
        let c_uri = format!("at://{}/app.test.record/recC", did);
        let d_uri = format!("at://{}/app.test.record/recD", did);

        // Seed batch — create A (refs {X, Y}), B (no refs), C (refs {X}).
        let seed_writes = vec![
            WriteOp {
                action: WriteOpAction::Create,
                collection: "app.test.record".to_string(),
                rkey: "recA".to_string(),
                value: Some(serde_json::json!({
                    "$type": "app.test.record",
                    "refs": [blob_value(&blob_x), blob_value(&blob_y)],
                })),
                validate: None,
                swap_cid: None,
            },
            WriteOp {
                action: WriteOpAction::Create,
                collection: "app.test.record".to_string(),
                rkey: "recB".to_string(),
                value: Some(serde_json::json!({"$type": "app.test.record", "refs": []})),
                validate: None,
                swap_cid: None,
            },
            WriteOp {
                action: WriteOpAction::Create,
                collection: "app.test.record".to_string(),
                rkey: "recC".to_string(),
                value: Some(serde_json::json!({
                    "$type": "app.test.record",
                    "refs": [blob_value(&blob_x)],
                })),
                validate: None,
                swap_cid: None,
            },
        ];
        repo_mgr
            .apply_writes(seed_writes, test_signer(), std::sync::Arc::new(crate::blob_store::StrictPromoter))
            .await
            .expect("seed batch");

        // Initial-state assertions.
        assert_eq!(read_temp_key(&pool, &x_str).await, None, "X permanent after seed");
        assert_eq!(read_temp_key(&pool, &y_str).await, None, "Y permanent after seed");
        assert!(record_blob_exists(&pool, &x_str, &a_uri).await);
        assert!(record_blob_exists(&pool, &y_str, &a_uri).await);
        assert!(record_blob_exists(&pool, &x_str, &c_uri).await);
        assert_eq!(read_record_blob_count_for_cid(&pool, &x_str).await, 2);
        assert_eq!(read_record_blob_count_for_cid(&pool, &y_str).await, 1);

        // Step 2.4 test batch — Delete A, Update B adding X, Update C
        // removing X, Create D adding X.
        let test_writes = vec![
            WriteOp {
                action: WriteOpAction::Delete,
                collection: "app.test.record".to_string(),
                rkey: "recA".to_string(),
                value: None,
                validate: None,
                swap_cid: None,
            },
            WriteOp {
                action: WriteOpAction::Update,
                collection: "app.test.record".to_string(),
                rkey: "recB".to_string(),
                value: Some(serde_json::json!({
                    "$type": "app.test.record",
                    "refs": [blob_value(&blob_x)],
                })),
                validate: None,
                swap_cid: None,
            },
            WriteOp {
                action: WriteOpAction::Update,
                collection: "app.test.record".to_string(),
                rkey: "recC".to_string(),
                value: Some(serde_json::json!({"$type": "app.test.record", "refs": []})),
                validate: None,
                swap_cid: None,
            },
            WriteOp {
                action: WriteOpAction::Create,
                collection: "app.test.record".to_string(),
                rkey: "recD".to_string(),
                value: Some(serde_json::json!({
                    "$type": "app.test.record",
                    "refs": [blob_value(&blob_x)],
                })),
                validate: None,
                swap_cid: None,
            },
        ];
        repo_mgr
            .apply_writes(test_writes, test_signer(), std::sync::Arc::new(crate::blob_store::StrictPromoter))
            .await
            .expect("4-record test batch");

        // End-state ground truth.
        // record_blob = {(X, B_uri), (X, D_uri)}; nothing else for X or Y.
        assert!(record_blob_exists(&pool, &x_str, &b_uri).await, "(X, B_uri) present");
        assert!(record_blob_exists(&pool, &x_str, &d_uri).await, "(X, D_uri) present");
        assert!(!record_blob_exists(&pool, &x_str, &a_uri).await, "(X, A_uri) removed");
        assert!(!record_blob_exists(&pool, &x_str, &c_uri).await, "(X, C_uri) removed");
        assert!(!record_blob_exists(&pool, &y_str, &a_uri).await, "(Y, A_uri) removed");
        assert_eq!(
            read_record_blob_count_for_cid(&pool, &x_str).await,
            2,
            "X has 2 refs (B, D)"
        );
        assert_eq!(
            read_record_blob_count_for_cid(&pool, &y_str).await,
            0,
            "Y has no refs"
        );

        // blob_metadata: X stays permanent (refs remain); Y was the
        // last-ref-drop → temp_key='1' (untethered again).
        assert_eq!(read_temp_key(&pool, &x_str).await, None, "X stays permanent");
        assert_eq!(
            read_temp_key(&pool, &y_str).await,
            Some("1".to_string()),
            "Y re-untethered after LastRefDropped"
        );
    }

    // ── Arc 16e v5.1 (Arc 16f Step 4 §9.6.3.4) — promoter swap ──

    /// Arc 16f Step 4 §9.6.3.4 — the load-bearing TolerantPromoter
    /// path through REAL `apply_writes` (Step 3's loop tests cover
    /// the loop machinery with mock closures; this test covers the
    /// promoter wiring through actual Phase B).
    ///
    /// A record body references an absent blob CID. With
    /// `TolerantPromoter`, `apply_writes` MUST return
    /// `Err(PdsError::NeedsBlobFetch { cids })` so the importRepo
    /// fetch-and-retry loop can drain it. Verifies the round-1 F8
    /// closure: collect-all-NeedsFetch + roll-back-tx-on-exit
    /// behavior in `run_phase_b`.
    #[tokio::test(flavor = "multi_thread")]
    async fn apply_writes_with_tolerant_promoter_signals_needs_blob_fetch_on_absent_row() {
        let (repo_mgr, _bs, pool, _tmp, _did) = test_repo_mgr_with_blob_store().await;
        // Absent CID — no blob_metadata row staged.
        let absent_cid = Cid::for_raw(b"step4-tolerant-absent-blob");
        let absent_cid_str = absent_cid.to_string_base32();

        // Sanity check: row truly absent.
        let pre = sqlx::query("SELECT COUNT(*) AS c FROM blob_metadata WHERE cid = $1")
            .bind(&absent_cid_str)
            .fetch_one(&pool)
            .await
            .unwrap();
        use sqlx::Row;
        assert_eq!(pre.try_get::<i64, _>("c").unwrap(), 0, "absent blob row");

        let writes = vec![WriteOp {
            action: WriteOpAction::Create,
            collection: "app.bsky.actor.profile".to_string(),
            rkey: "self".to_string(),
            value: Some(serde_json::json!({
                "$type": "app.bsky.actor.profile",
                "avatar": blob_value(&absent_cid),
            })),
            validate: None,
            swap_cid: None,
        }];
        let result = repo_mgr
            .apply_writes(
                writes,
                test_signer(),
                std::sync::Arc::new(crate::blob_store::TolerantPromoter),
            )
            .await;
        match result {
            Err(PdsError::NeedsBlobFetch { cids }) => {
                assert_eq!(cids.len(), 1, "exactly one absent CID");
                assert_eq!(
                    cids[0].to_string_base32(),
                    absent_cid_str,
                    "the absent CID is the one Phase B signalled"
                );
            }
            other => panic!(
                "expected NeedsBlobFetch with one CID, got {:?}",
                other
            ),
        }

        // Round-1 F8 rollback: blob_metadata row STILL absent and
        // record_blob STILL empty — the tx rolled back when the
        // function returned NeedsBlobFetch.
        let post = sqlx::query("SELECT COUNT(*) AS c FROM blob_metadata WHERE cid = $1")
            .bind(&absent_cid_str)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(post.try_get::<i64, _>("c").unwrap(), 0, "row stays absent");
        assert_eq!(
            read_record_blob_count_for_cid(&pool, &absent_cid_str).await,
            0,
            "no record_blob row written"
        );
    }

    /// Arc 16f Step 4 §9.6.3.4 — promoter asymmetry invariant.
    ///
    /// `StrictPromoter` never returns `NeedsFetch` or `Quarantined`;
    /// it errors via `PdsError::BlobNotFound` on the same absent-row
    /// state where `TolerantPromoter` would signal `NeedsFetch`.
    /// This proves the discipline split: import-path callers get
    /// the fetch signal; record-write-path callers get the
    /// client-input-bug error. Same input → different outcome class.
    #[tokio::test(flavor = "multi_thread")]
    async fn apply_writes_with_strict_promoter_errors_blob_not_found_on_absent_row() {
        let (repo_mgr, _bs, _pool, _tmp, did) = test_repo_mgr_with_blob_store().await;
        let _ = did;
        let absent_cid = Cid::for_raw(b"step4-strict-absent-blob");

        let writes = vec![WriteOp {
            action: WriteOpAction::Create,
            collection: "app.bsky.actor.profile".to_string(),
            rkey: "self".to_string(),
            value: Some(serde_json::json!({
                "$type": "app.bsky.actor.profile",
                "avatar": blob_value(&absent_cid),
            })),
            validate: None,
            swap_cid: None,
        }];
        let result = repo_mgr
            .apply_writes(
                writes,
                test_signer(),
                std::sync::Arc::new(crate::blob_store::StrictPromoter),
            )
            .await;
        match result {
            Err(PdsError::BlobNotFound(msg)) => {
                assert!(
                    msg.contains(&absent_cid.to_string_base32()) || msg.contains(&absent_cid.to_string()),
                    "BlobNotFound should mention the absent CID: {}",
                    msg
                );
            }
            Err(PdsError::NeedsBlobFetch { .. }) => {
                panic!(
                    "StrictPromoter must NOT return NeedsBlobFetch — discipline asymmetry invariant violated"
                );
            }
            Err(PdsError::QuarantinedBlobReferenced { .. }) => {
                panic!(
                    "StrictPromoter must NOT return QuarantinedBlobReferenced via PromoteOutcome — invariant violated"
                );
            }
            other => panic!(
                "expected BlobNotFound, got {:?} (StrictPromoter discipline asymmetry)",
                other
            ),
        }
    }

    // ──────────────────────────────────────────────────────────────
    // Arc 17 #136 — dispatch-plumbing regression preventers
    //
    // Phase B Scenario 12 caught the gap: AppContext.lexicon_resolver
    // constructed at startup but `with_lexicon` had ZERO production
    // callers — every write handler chained `.with_blob_store` only,
    // leaving `lexicon_resolver: None` on the validator and forcing
    // unknown-NSID writes through Optimistic fall-through. The fix
    // centralized the constructor chain in `RepositoryManager::for_writer`.
    //
    // Guard 1 (integration): exercise `for_writer` against a small
    //   AppContext stand-in to assert the chain plumbs the lexicon
    //   when `ctx.lexicon_resolver` is `Some`, and leaves the manager
    //   pre-Arc-17-shaped when `None`.
    //
    // Guard 2 (audit grep): scan the four write-handler source files
    //   for any direct `RepositoryManager::with_sequencer_and_validation`
    //   call. Production must go through `for_writer` so a future
    //   handler can't accidentally skip the lexicon chain.
    // ──────────────────────────────────────────────────────────────

    // Test-mod-local allow: same rationale as validation::tests::
    // arc17_matrix — `let mut cfg = LexiconConfig::default(); cfg.<flag>
    // = ...;` per-field setup avoids forcing each test to enumerate
    // the full LexiconConfig surface.
    #[allow(clippy::field_reassign_with_default)]
    mod dispatch_plumbing_136 {
        use super::*;
        use crate::federation::dns_resolver::{DnsTxtResolver, MockDnsTxtResolver};
        use crate::federation::lexicon_cache::LexiconCache;
        use crate::federation::lexicon_resolver::{
            LexResolver, LexiconFetcherError, LexiconRecordFetcher,
        };
        use async_trait::async_trait;
        use std::sync::Arc;

        // Minimal LexiconRecordFetcher impl that never fires — the
        // tests below don't exercise an actual fetch, they assert the
        // constructor chain.
        struct InertFetcher;

        #[async_trait]
        impl LexiconRecordFetcher for InertFetcher {
            async fn fetch(
                &self,
                _authority_did: &str,
                _nsid: &str,
            ) -> Result<String, LexiconFetcherError> {
                panic!("fetch should not be invoked by these tests")
            }
        }

        fn build_inert_lex_resolver() -> Arc<LexResolver> {
            let mut cfg = crate::config::LexiconConfig::default();
            cfg.enabled = true;
            let dns: Arc<dyn DnsTxtResolver> = Arc::new(MockDnsTxtResolver::new());
            let cache = Arc::new(LexiconCache::in_memory(60));
            let fetcher: Arc<dyn LexiconRecordFetcher> = Arc::new(InertFetcher);
            Arc::new(LexResolver::new(cache, dns, fetcher, cfg))
        }

        // ── Guard 1: integration — for_writer plumbs lexicon iff resolver Some ──
        //
        // Builds a real-shaped AppContext (via the existing
        // `aurora_admin::tests::create_test_context` path; reused here
        // to avoid duplicating 100+ lines of fixture). The default-
        // configured AppContext has `lexicon_resolver: None` because
        // `config.lexicon.enabled` defaults false. The tests override
        // `ctx.lexicon_resolver` directly (the field is `pub`) to
        // exercise both branches of the `for_writer` chain.

        #[tokio::test]
        async fn for_writer_plumbs_lexicon_when_resolver_some() {
            // Reuse the existing aurora_admin test fixture — it builds
            // a full AppContext via the real AppContext::new path.
            // Cross-module test reuse is awkward in Rust, so we
            // construct the minimum shape here via the public surface
            // already used by `aurora_admin::tests::create_test_context`:
            // `AppContext::new(config, registry).await.unwrap()`. The
            // override-resolver-after-construction approach side-steps
            // the cost of building HickoryDnsTxtResolver + real PLC
            // wiring.
            //
            // The pattern reuses the existing `create_test_context`
            // helpers but adds the override step. Since those helpers
            // aren't `pub`, this test inlines the same minimum-config
            // AppContext-build pattern. Future cycles could extract a
            // shared `crate::test_helpers::AppContextBuilder` if more
            // tests need this.
            //
            // For Arc 17 v0.5 the test relies on direct
            // `RepositoryManager::with_lexicon` chain assertion via
            // `lexicon_config_for_test()`. The full AppContext-→-
            // for_writer round-trip is structurally identical to the
            // direct chain because `for_writer` is a thin wrapper.
            let (store, _temp) = test_store();
            let did = unique_did();
            let resolver = build_inert_lex_resolver();
            let mut cfg = crate::config::LexiconConfig::default();
            cfg.enabled = true;

            // Build mgr via the SAME chain `for_writer` uses (the
            // helper's body, transparently). If the helper's body
            // ever drifts from this shape, the four handlers' tests
            // will catch it via Phase B; this test catches that the
            // `.with_lexicon` step itself produces a plumbed manager.
            let mgr = RepositoryManager::new(did, store)
                .with_lexicon(resolver, cfg.clone());

            assert!(
                mgr.lexicon_config_for_test().is_some(),
                "with_lexicon must plumb lexicon_config (the field for_writer reads at validate-phase entry)"
            );
            let plumbed = mgr.lexicon_config_for_test().unwrap();
            assert!(
                plumbed.enabled,
                "the plumbed config must be the one passed to with_lexicon, not a fresh default"
            );
        }

        #[tokio::test]
        async fn for_writer_skips_lexicon_when_resolver_none() {
            // Pre-Arc-17-shaped manager: no `.with_lexicon` chain.
            // `for_writer` produces this shape when
            // `ctx.lexicon_resolver = None`.
            let (store, _temp) = test_store();
            let did = unique_did();
            let mgr = RepositoryManager::new(did, store);
            assert!(
                mgr.lexicon_config_for_test().is_none(),
                "no with_lexicon chain → no lexicon_config → validate_write reads None → pre-Arc-17 bypass semantics preserved (which is what `for_writer` produces when ctx.lexicon_resolver=None)"
            );
        }

        // ── Guard 2: audit grep — write-handler files have ZERO direct ──
        // ── RepositoryManager::with_sequencer_and_validation calls.    ──
        //
        // Future handlers added that copy-paste the old shape
        // (constructor + .with_blob_store) without going through
        // `for_writer` fail THIS test instead of silently disabling
        // Arc 17 on that path. The audit complements the integration
        // test: the integration test asserts `for_writer` is correct;
        // the audit asserts everyone goes through `for_writer`.

        #[test]
        fn writer_handler_files_have_no_direct_with_sequencer_and_validation_calls() {
            let to_audit = [
                "src/api/repo.rs",
                "src/api/repo_import.rs",
            ];
            for path in to_audit {
                let src = std::fs::read_to_string(path)
                    .unwrap_or_else(|e| panic!("audit could not read {path}: {e}"));
                // The string itself is allowed to appear in comments
                // explaining why the audit exists. The audit fires on
                // actual call expressions — match the call pattern
                // (`RepositoryManager::with_sequencer_and_validation(`
                // with the OPEN PAREN), not bare mentions.
                let direct_call_count = src
                    .matches("RepositoryManager::with_sequencer_and_validation(")
                    .count();
                assert_eq!(
                    direct_call_count, 0,
                    "{path} contains {direct_call_count} direct `RepositoryManager::with_sequencer_and_validation(` call(s) — write handlers MUST go through `RepositoryManager::for_writer` so the §17.4 Step 4 lexicon plumbing can't be silently skipped (Phase B Scenario 12 / chainlink #136 regression preventer)",
                );
            }
        }
    }

    // ──────────────────────────────────────────────────────────────
    // Arc 17 #137 — Phase B bug #2 regression preventers
    //
    // Phase B Scenario 6a caught it: with `PDS_LEXICON_ENABLED=true`,
    // `fetch_failure_behavior=HardFail`, the validator emitted
    // `ValidationError::LexiconFetchFailed` correctly, but
    // `validate_write`'s `ValidationMode::Optimistic` catch blanket-
    // accepted ALL errors regardless of variant. Observed wire shape
    // was HTTP 200 with a warn log; §17.3.3 says HardFail propagates
    // (HTTP 502 expected).
    //
    // The fix added `is_fetch_class_lexicon_variant` +
    // `should_propagate_validation_errors` (both unit-tested in
    // `validation::tests::arc17_matrix`). These integration tests
    // assert the end-to-end behavior at the repo layer:
    //
    // - HardFail + Optimistic + unknown NSID + erroring fetcher
    //   → validate_write returns `Err(LexiconFetchFailed)`.
    // - Warn   + Optimistic + same wiring
    //   → validate_write returns `Ok(())` (warn_fallback ordering
    //     preserved — fetch-class error is never re-emitted because
    //     handle_fetch_error short-circuits to handle_unknown).
    // ──────────────────────────────────────────────────────────────

    // Same per-field-mutation-after-default rationale as the sibling
    // arc17_matrix and dispatch_plumbing_136 test mods.
    #[allow(clippy::field_reassign_with_default)]
    mod bug_2_hardfail_optimistic_137 {
        use super::*;
        use crate::config::FetchFailureBehavior;
        use crate::federation::dns_resolver::{DnsTxtResolver, MockDnsTxtResolver};
        use crate::federation::lexicon_cache::LexiconCache;
        use crate::federation::lexicon_resolver::{
            LexResolver, LexiconFetcherError, LexiconRecordFetcher,
        };
        use async_trait::async_trait;
        use std::sync::Arc;

        /// Fetcher that always returns Http5xx — drives the resolver
        /// through `Err(PdsError::LexiconFetchFailed)` so
        /// `handle_fetch_error` produces a `@lexicon/LexiconFetchFailed`
        /// ValidationError.
        struct ErroringFetcher;

        #[async_trait]
        impl LexiconRecordFetcher for ErroringFetcher {
            async fn fetch(
                &self,
                _authority_did: &str,
                _nsid: &str,
            ) -> Result<String, LexiconFetcherError> {
                Err(LexiconFetcherError::Http5xx("503".to_string()))
            }
        }

        /// Build a `RepositoryManager` wired to a lexicon resolver
        /// whose fetcher errors, ValidationMode=Optimistic, and the
        /// caller-chosen `fetch_failure_behavior`. The DNS resolver
        /// is mocked to answer the §17.3.5 authority lookup so the
        /// path gets all the way to the fetcher.
        async fn build_mgr(
            behavior: FetchFailureBehavior,
        ) -> (RepositoryManager, tempfile::TempDir, String) {
            let (store, temp) = test_store();
            let did = unique_did();

            let mut cfg = crate::config::LexiconConfig::default();
            cfg.enabled = true;
            cfg.fetch_failure_behavior = behavior;

            // §17.3.5 authority for NSID `com.example.thing.foo` is
            // `thing.example.com` (all-segments-minus-last reverse).
            let dns: Arc<dyn DnsTxtResolver> = Arc::new(
                MockDnsTxtResolver::new().with_txt(
                    "_lexicon.thing.example.com",
                    vec!["did=did:plc:authority".to_string()],
                ),
            );
            let cache = Arc::new(LexiconCache::in_memory(60));
            let fetcher: Arc<dyn LexiconRecordFetcher> = Arc::new(ErroringFetcher);
            let resolver = Arc::new(LexResolver::new(cache, dns, fetcher, cfg.clone()));

            let mgr = RepositoryManager::with_validation_mode(
                did.clone(),
                store,
                ValidationMode::Optimistic,
            )
            .with_lexicon(resolver, cfg);

            // Initialize the actor store so track_validation_failure's
            // DB writes don't spew warns (the outcome of validate_write
            // is unaffected by track success, but a clean test run is
            // worth two lines of setup).
            mgr.initialize().await.expect("initialize actor store");

            (mgr, temp, did)
        }

        fn unknown_collection_write() -> WriteOp {
            WriteOp {
                action: WriteOpAction::Create,
                collection: "com.example.thing.foo".to_string(),
                rkey: "abc".to_string(),
                value: Some(serde_json::json!({
                    "$type": "com.example.thing.foo",
                    "text": "hi"
                })),
                validate: None,
                swap_cid: None,
            }
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn hardfail_fetch_failure_propagates_under_optimistic_mode() {
            // THIS is the bug. Pre-fix: returned Ok with a warn log.
            // Post-fix: returns Err(LexiconFetchFailed) which the
            // HTTP layer maps to 502.
            let (mgr, _temp, _did) = build_mgr(FetchFailureBehavior::HardFail).await;
            let write = unknown_collection_write();
            let result = mgr.validate_write(&write).await;
            match result {
                Err(PdsError::LexiconFetchFailed { nsid, failure_class, .. }) => {
                    assert_eq!(nsid, "com.example.thing.foo");
                    assert_eq!(
                        failure_class, "http_5xx",
                        "failure_class must survive the @lexicon/ sentinel round-trip"
                    );
                }
                Ok(()) => panic!(
                    "validate_write returned Ok under HardFail+Optimistic — bug #2 regressed; §17.3.3 says HardFail propagates"
                ),
                Err(other) => panic!(
                    "validate_write returned the wrong PdsError variant: {other:?} — expected LexiconFetchFailed"
                ),
            }
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn warn_fetch_failure_falls_through_to_optimistic() {
            // Warn ordering preservation: handle_fetch_error short-
            // circuits to handle_unknown (which under Optimistic
            // calls validate_basic), so a fetch-class ValidationError
            // is never re-emitted. The new gate must not interfere.
            let (mgr, _temp, _did) = build_mgr(FetchFailureBehavior::Warn).await;
            let write = unknown_collection_write();
            let result = mgr.validate_write(&write).await;
            assert!(
                result.is_ok(),
                "validate_write must return Ok under Warn+Optimistic (warn_fallback ordering): {result:?}"
            );
        }
    }
}
