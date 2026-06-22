//! Repository rebuild — reconstruction, verification, and atomic swap
//! (Arc H §7.4.1 / #289, #290).
//!
//! Reconstructs an account's canonical repo state by replaying its sequencer
//! history: walk the commit events ascending, fold each `#commit` event's CAR
//! block slice into one accumulating [`BlockMap`], then verify that the head
//! commit and its MST resolve via proto-blue's [`verify_repo`] — the substrate
//! primitive purpose-built for this ("used when the blocks came from a stream
//! of firehose commit events"). `verify_repo` loads the head commit, checks the
//! DID (and, when a key is supplied, the signature), then loads the MST from the
//! block map, tolerating extra/dead blocks (only the reachable closure must be
//! present) — so accumulating every delta and rooting at the head yields the
//! canonical current state.
//!
//! ## Two halves
//!
//! - **#289 (non-destructive)**: [`reconstruct_and_verify`] reconstructs +
//!   verifies in memory and returns the [`VerifiedRepo`]. It touches no live
//!   repo state. Used by `preRebuildCheck`'s `deep` mode.
//! - **#290 (destructive)**: [`atomic_swap`] replaces an account's live repo
//!   storage with a reconstructed [`VerifiedRepo`] in ONE per-DID SQLite
//!   transaction (wipe blocks + records, insert the reconstructed block set,
//!   set the new root, rebuild the record index from the MST leaves). The
//!   [`RebuildRegistry`] drives this as a background [`RebuildJob`] with
//!   progress, cancellation, and per-DID single-flight, and emits the
//!   [`RepoRebuilt`](crate::admin::events::ModerationEventType::RepoRebuilt)
//!   audit event on success.
//!
//! ## Shadow-then-swap
//!
//! Reconstruction + verification happen entirely in memory; the live repo is
//! touched exactly once, at the swap. A rebuild that fails to reconstruct,
//! fails verification, or is cancelled before the swap leaves the original repo
//! byte-for-byte untouched — the right posture for possibly-corrupt input. The
//! swap itself is one SQLite transaction: it either commits whole or rolls back
//! whole, so there is no torn intermediate state to recover from.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::str::FromStr;
use std::time::SystemTime;

use crate::actor_store::ActorStore;
use crate::admin::events::{LogEventParams, ModerationEventLogger, ModerationEventType};
use crate::context::AppContext;
use crate::error::{PdsError, PdsResult};
use crate::sequencer::Sequencer;
use proto_blue::lex_data::Cid;
use proto_blue::repo::car as pb_car;
use proto_blue::repo::{parse_data_key, verify_repo, BlockMap, VerifiedRepo};

// ---------------------------------------------------------------------------
// Reconstruction (#289) — non-destructive
// ---------------------------------------------------------------------------

/// Outcome of an in-memory history walk. Separates a clean completion (with the
/// accumulated blocks + head) from a caller-requested cancellation, so the
/// rebuild job can stop a long walk responsively without writing anything.
pub enum AccumulateOutcome {
    /// The walk reached the end of history. Carries the accumulated block set,
    /// the head commit CID (`None` when the account has no commit events), and
    /// the number of commit events folded in.
    Completed {
        blocks: BlockMap,
        head_commit_cid: Option<String>,
        commit_count: u64,
    },
    /// The `cancel` flag was observed at a commit boundary; the walk stopped
    /// early. Nothing was written (reconstruction is in-memory) so the live
    /// repo is untouched.
    Cancelled,
}

/// Walk `did`'s sequencer history ascending and fold every `#commit` event's
/// CAR delta into one accumulating [`BlockMap`], tracking the latest commit CID
/// as the head. `on_commit` is invoked with the running commit count after each
/// commit (progress reporting); `cancel` is polled at each commit boundary so a
/// long walk can be stopped cleanly.
///
/// Non-destructive: reads the sequencer, accumulates in memory, writes nothing.
pub async fn accumulate_history(
    sequencer: &Sequencer,
    did: &str,
    cancel: &AtomicBool,
    mut on_commit: impl FnMut(u64),
) -> PdsResult<AccumulateOutcome> {
    let mut blocks = BlockMap::new();
    let mut head_commit_cid: Option<String> = None;
    let mut commit_count: u64 = 0;
    let mut cursor = 0i64;

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(AccumulateOutcome::Cancelled);
        }
        let (events, last_seq) = sequencer.commit_events_after(did, cursor, None).await?;
        match last_seq {
            None => break, // end of history
            Some(s) => cursor = s,
        }
        for (seq, evt) in events {
            if cancel.load(Ordering::Relaxed) {
                return Ok(AccumulateOutcome::Cancelled);
            }
            head_commit_cid = Some(evt.commit.clone());
            let (_roots, delta) = pb_car::read_car(&evt.blocks).map_err(|e| {
                PdsError::InvalidCar(format!("rebuild: CAR decode failed at seq {}: {}", seq, e))
            })?;
            blocks.add_map(&delta);
            commit_count += 1;
            on_commit(commit_count);
        }
    }

    Ok(AccumulateOutcome::Completed {
        blocks,
        head_commit_cid,
        commit_count,
    })
}

/// Verify an accumulated block set resolves into a coherent repo rooted at
/// `head_commit_cid`, via proto-blue's [`verify_repo`]. `signing_did_key`:
/// `None` runs structural verification only (DID match + MST resolution);
/// `Some(did_key)` additionally verifies the head commit's signature.
///
/// Errors when the head CID is malformed or `verify_repo` rejects the assembled
/// state (a missing reachable block, MST inconsistency, DID/signature mismatch)
/// — i.e. replay would NOT produce a coherent repo.
pub fn verify_reconstructed(
    blocks: BlockMap,
    head_commit_cid: &str,
    did: &str,
    signing_did_key: Option<&str>,
) -> PdsResult<VerifiedRepo> {
    let root = Cid::from_str(head_commit_cid).map_err(|e| {
        PdsError::Internal(format!(
            "rebuild: malformed head commit CID '{}': {}",
            head_commit_cid, e
        ))
    })?;
    verify_repo(blocks, &root, Some(did), signing_did_key).map_err(|e| {
        PdsError::Internal(format!(
            "rebuild: reconstructed repo for {} failed verification: {}",
            did, e
        ))
    })
}

/// Reconstruct `did`'s canonical repo from its full sequencer history and
/// verify it resolves. Returns the [`VerifiedRepo`] (head commit + MST + the
/// accumulated block set), or `None` when the account has no commit history
/// (nothing to rebuild).
///
/// `signing_did_key`: `None` runs structural verification only — what the
/// non-destructive preflight needs; #290's actual rebuild passes the account's
/// `did:key` for full signature verification. Non-destructive; mutates nothing.
pub async fn reconstruct_and_verify(
    sequencer: &Sequencer,
    did: &str,
    signing_did_key: Option<&str>,
) -> PdsResult<Option<VerifiedRepo>> {
    // No cancellation for the preflight path: a never-set flag + a no-op
    // progress sink.
    let never = AtomicBool::new(false);
    match accumulate_history(sequencer, did, &never, |_| {}).await? {
        AccumulateOutcome::Completed {
            blocks,
            head_commit_cid,
            ..
        } => {
            let Some(head) = head_commit_cid else {
                return Ok(None);
            };
            Ok(Some(verify_reconstructed(blocks, &head, did, signing_did_key)?))
        }
        // Unreachable with a never-set cancel flag; surfaced explicitly rather
        // than panicking should the contract ever change.
        AccumulateOutcome::Cancelled => Err(PdsError::Internal(
            "rebuild: reconstruct_and_verify walk cancelled without a cancel request".to_string(),
        )),
    }
}

// ---------------------------------------------------------------------------
// Atomic swap (#290) — destructive
// ---------------------------------------------------------------------------

/// Atomically replace `did`'s live repo storage with `verified`'s reconstructed
/// state, in ONE per-DID SQLite transaction. Wipes the existing `record` index
/// and `repo_block` set, inserts every reconstructed block, sets `repo_root` to
/// the reconstructed head, and rebuilds the `record` index from the MST leaves.
/// Returns the number of records written.
///
/// The whole operation is one transaction: any error rolls the entire swap back
/// and the original repo is untouched (the FK `record.cid → repo_block.cid`
/// means a record whose value block is absent from the reconstructed set fails
/// the insert and aborts the swap — an incompleteness the rebuild must not
/// paper over). This is the only point at which a rebuild mutates live state.
pub async fn atomic_swap(
    store: &ActorStore,
    did: &str,
    verified: &VerifiedRepo,
) -> PdsResult<u64> {
    let pool = store.open_db(did).await?;
    let mut tx = pool.begin().await.map_err(PdsError::Database)?;

    // Wipe the existing repo state. Records first, then blocks (either order is
    // safe — the FK cascades — but explicit ordering documents intent).
    sqlx::query("DELETE FROM record")
        .execute(&mut *tx)
        .await
        .map_err(PdsError::Database)?;
    sqlx::query("DELETE FROM repo_block")
        .execute(&mut *tx)
        .await
        .map_err(PdsError::Database)?;

    // Insert the reconstructed block set (commit + MST nodes + record blobs).
    for (cid, content) in verified.blocks.iter() {
        ActorStore::put_block_in_tx(&mut tx, &cid.to_string(), content).await?;
    }

    // Set the new head.
    let rev = verified.rev();
    ActorStore::update_repo_root_in_tx(&mut tx, did, &verified.commit_cid.to_string(), rev).await?;

    // Rebuild the record index from the MST leaves: each leaf is an
    // `at://<did>/<collection>/<rkey>` → record-block-CID mapping.
    let mut records_written: u64 = 0;
    for leaf in verified.mst.leaves() {
        let dk = parse_data_key(&leaf.key).map_err(|e| {
            PdsError::Internal(format!("rebuild: malformed MST key '{}': {}", leaf.key, e))
        })?;
        let uri = format!("at://{}/{}/{}", did, dk.collection, dk.rkey);
        ActorStore::put_record_in_tx(
            &mut tx,
            &uri,
            &leaf.value.to_string(),
            &dk.collection,
            &dk.rkey,
            rev,
        )
        .await?;
        records_written += 1;
    }

    tx.commit().await.map_err(PdsError::Database)?;
    Ok(records_written)
}

// ---------------------------------------------------------------------------
// Background job + registry (#290)
// ---------------------------------------------------------------------------

/// The phase a [`RebuildJob`] is in, surfaced by `getRebuildProgress`. Walking
/// and accumulating are a single loop (each commit is walked and its CAR folded
/// in together), reported as `Walking`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebuildPhase {
    /// Job created, not yet started running.
    Pending,
    /// Walking the sequencer history + accumulating CAR deltas in memory.
    Walking,
    /// Running the accumulated block set through `verify_repo`.
    Verifying,
    /// Applying the atomic swap (the single destructive transaction).
    Swapping,
    /// Swap committed; the rebuild succeeded.
    Completed,
    /// The rebuild failed (reconstruction, verification, or swap error). The
    /// original repo is untouched unless the failure was inside the swap
    /// transaction, which rolls back whole.
    Failed,
    /// Cancelled before the swap; the original repo is untouched.
    Cancelled,
}

impl RebuildPhase {
    /// Wire string for `getRebuildProgress`.
    pub fn as_str(self) -> &'static str {
        match self {
            RebuildPhase::Pending => "pending",
            RebuildPhase::Walking => "walking",
            RebuildPhase::Verifying => "verifying",
            RebuildPhase::Swapping => "swapping",
            RebuildPhase::Completed => "completed",
            RebuildPhase::Failed => "failed",
            RebuildPhase::Cancelled => "cancelled",
        }
    }

    /// Whether this is a terminal phase (the job is no longer making progress).
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            RebuildPhase::Completed | RebuildPhase::Failed | RebuildPhase::Cancelled
        )
    }
}

/// A live snapshot of a rebuild job, returned by `getRebuildProgress`.
#[derive(Debug, Clone)]
pub struct RebuildProgress {
    pub job_id: String,
    pub did: String,
    pub phase: RebuildPhase,
    /// Total commit events to walk (from the metadata preflight); `0` until the
    /// preflight completes or when the account had no history.
    pub commits_total: u64,
    /// Commit events folded in so far (advances during `Walking`).
    pub commits_processed: u64,
    /// Records written by the swap (set when `Swapping` completes).
    pub records_written: u64,
    /// The account's head commit CID before the swap (`None` if uninitialised).
    pub head_before: Option<String>,
    /// The reconstructed head commit CID after a successful swap.
    pub head_after: Option<String>,
    /// Diagnostic on a `Failed` job.
    pub error: Option<String>,
    /// Whether cancellation has been requested.
    pub cancel_requested: bool,
    pub started_at: Option<SystemTime>,
    pub finished_at: Option<SystemTime>,
}

/// Mutable interior of a [`RebuildJob`].
struct RebuildState {
    phase: RebuildPhase,
    commits_total: u64,
    commits_processed: u64,
    records_written: u64,
    head_before: Option<String>,
    head_after: Option<String>,
    error: Option<String>,
    started_at: Option<SystemTime>,
    finished_at: Option<SystemTime>,
}

/// Terminal outcome of a [`RebuildJob::run`].
enum RebuildOutcome {
    Completed,
    Cancelled,
    Failed(String),
}

/// One background repository rebuild. Holds its own progress state + cancel
/// flag; driven by [`RebuildRegistry`]. The job's identity (`did`, who
/// triggered it, the required rationale) is immutable for its lifetime.
pub struct RebuildJob {
    job_id: String,
    did: String,
    triggered_by: String,
    rationale: String,
    state: RwLock<RebuildState>,
    cancel: AtomicBool,
}

impl RebuildJob {
    fn new(job_id: String, did: String, triggered_by: String, rationale: String) -> Self {
        RebuildJob {
            job_id,
            did,
            triggered_by,
            rationale,
            state: RwLock::new(RebuildState {
                phase: RebuildPhase::Pending,
                commits_total: 0,
                commits_processed: 0,
                records_written: 0,
                head_before: None,
                head_after: None,
                error: None,
                started_at: None,
                finished_at: None,
            }),
            cancel: AtomicBool::new(false),
        }
    }

    /// A live snapshot of the job's progress.
    pub fn progress(&self) -> RebuildProgress {
        let st = self.state.read().expect("rebuild state lock not poisoned");
        RebuildProgress {
            job_id: self.job_id.clone(),
            did: self.did.clone(),
            phase: st.phase,
            commits_total: st.commits_total,
            commits_processed: st.commits_processed,
            records_written: st.records_written,
            head_before: st.head_before.clone(),
            head_after: st.head_after.clone(),
            error: st.error.clone(),
            cancel_requested: self.cancel.load(Ordering::Relaxed),
            started_at: st.started_at,
            finished_at: st.finished_at,
        }
    }

    /// Request cancellation. Returns `true` if the job was still in flight (the
    /// flag is now set and the walk/phase-boundary will observe it), `false` if
    /// it had already reached a terminal phase ("nothing to cancel"). The swap
    /// itself is atomic, so a cancel that lands during the swap is a no-op — the
    /// transaction commits or rolls back whole regardless.
    pub fn request_cancel(&self) -> bool {
        let terminal = self
            .state
            .read()
            .expect("rebuild state lock not poisoned")
            .phase
            .is_terminal();
        if terminal {
            return false;
        }
        self.cancel.store(true, Ordering::Relaxed);
        true
    }

    fn set_phase(&self, phase: RebuildPhase) {
        self.state.write().expect("rebuild state lock not poisoned").phase = phase;
    }

    /// The rebuild pipeline: preflight (for the commit total) → walk + accumulate
    /// → verify → atomic swap. Cancellation is observed at each commit boundary
    /// and between phases; the swap is the point of no return (atomic).
    async fn run(&self, ctx: &AppContext) -> RebuildOutcome {
        {
            let mut st = self.state.write().expect("rebuild state lock not poisoned");
            st.started_at = Some(SystemTime::now());
            st.phase = RebuildPhase::Walking;
        }

        // Head before the swap, for the audit trail (None if uninitialised).
        let head_before = ctx
            .actor_store
            .get_repo_root(&self.did)
            .await
            .ok()
            .map(|r| r.cid);
        if let Some(h) = &head_before {
            self.state.write().expect("rebuild state lock not poisoned").head_before = Some(h.clone());
        }

        // Full signature verification needs the account's published signing key
        // (the same resolution `importRepo` uses for `verify_diff_car`).
        let signing_key = match public_did_key_for(ctx, &self.did).await {
            Ok(k) => k,
            Err(e) => return RebuildOutcome::Failed(format!("signing-key resolution failed: {e}")),
        };

        // The commit total (for "walking N/M") — the cheap metadata preflight,
        // distinct from the block-decoding accumulation walk below.
        match ctx.sequencer.rebuild_preflight(&self.did).await {
            Ok(Some(pf)) => {
                self.state.write().expect("rebuild state lock not poisoned").commits_total =
                    pf.commit_count;
            }
            Ok(None) => {
                return RebuildOutcome::Failed(format!(
                    "no sequencer history for {} — nothing to rebuild",
                    self.did
                ))
            }
            Err(e) => return RebuildOutcome::Failed(format!("preflight failed: {e}")),
        }

        // Phase 1: walk + accumulate (cancellable at each commit boundary).
        let acc = accumulate_history(&ctx.sequencer, &self.did, &self.cancel, |n| {
            self.state
                .write()
                .expect("rebuild state lock not poisoned")
                .commits_processed = n;
        })
        .await;
        let (blocks, head, commit_count) = match acc {
            Ok(AccumulateOutcome::Completed {
                blocks,
                head_commit_cid,
                commit_count,
            }) => match head_commit_cid {
                Some(h) => (blocks, h, commit_count),
                None => {
                    return RebuildOutcome::Failed(format!(
                        "no commit history for {} — nothing to rebuild",
                        self.did
                    ))
                }
            },
            Ok(AccumulateOutcome::Cancelled) => return RebuildOutcome::Cancelled,
            Err(e) => return RebuildOutcome::Failed(format!("history walk failed: {e}")),
        };

        // Phase boundary: a cancel observed here stops before any verification.
        if self.cancel.load(Ordering::Relaxed) {
            return RebuildOutcome::Cancelled;
        }

        // Phase 2: verify (full signature check with the resolved key).
        self.set_phase(RebuildPhase::Verifying);
        let verified = match verify_reconstructed(blocks, &head, &self.did, Some(&signing_key)) {
            Ok(v) => v,
            Err(e) => return RebuildOutcome::Failed(e.to_string()),
        };

        // Phase boundary: last chance to cancel before the destructive swap.
        if self.cancel.load(Ordering::Relaxed) {
            return RebuildOutcome::Cancelled;
        }

        // Phase 3: atomic swap (point of no return — the transaction is whole).
        self.set_phase(RebuildPhase::Swapping);
        let records_written = match atomic_swap(&ctx.actor_store, &self.did, &verified).await {
            Ok(n) => n,
            Err(e) => return RebuildOutcome::Failed(format!("atomic swap failed: {e}")),
        };

        let head_after = verified.commit_cid.to_string();
        {
            let mut st = self.state.write().expect("rebuild state lock not poisoned");
            st.records_written = records_written;
            st.head_after = Some(head_after.clone());
        }
        // `commit_count` is the number of commits actually replayed (the audit's
        // `rebuiltCommitCount`); `commits_total` from the preflight drove the
        // walking-progress display and should equal it for a healthy history.
        let rebuilt_commit_count = commit_count;

        // Audit the successful swap (Category C — own short tx). Best-effort:
        // the swap has already committed, so an audit-write failure is logged
        // but does not un-rebuild the repo.
        emit_repo_rebuilt(
            ctx,
            &self.did,
            rebuilt_commit_count,
            head_before.as_deref(),
            &head_after,
            &self.triggered_by,
            &self.rationale,
        )
        .await;

        RebuildOutcome::Completed
    }

    /// Stamp the terminal phase + finish time.
    fn finish(&self, outcome: RebuildOutcome) {
        let mut st = self.state.write().expect("rebuild state lock not poisoned");
        st.finished_at = Some(SystemTime::now());
        match outcome {
            RebuildOutcome::Completed => st.phase = RebuildPhase::Completed,
            RebuildOutcome::Cancelled => st.phase = RebuildPhase::Cancelled,
            RebuildOutcome::Failed(e) => {
                st.phase = RebuildPhase::Failed;
                st.error = Some(e);
            }
        }
    }
}

/// Emit the [`RepoRebuilt`](ModerationEventType::RepoRebuilt) host-vocabulary
/// audit event for a successful swap. Best-effort: a logging failure is recorded
/// but never reverses the (already-committed) rebuild.
async fn emit_repo_rebuilt(
    ctx: &AppContext,
    did: &str,
    rebuilt_commit_count: u64,
    head_before: Option<&str>,
    head_after: &str,
    triggered_by: &str,
    rationale: &str,
) {
    let logger = ModerationEventLogger::new(ctx.account_db.clone());
    let details = serde_json::json!({
        "rebuiltCommitCount": rebuilt_commit_count,
        "headCommitCidBefore": head_before,
        "headCommitCidAfter": head_after,
        "rationale": rationale,
    });
    if let Err(e) = logger
        .log_event(LogEventParams {
            event_type: ModerationEventType::RepoRebuilt,
            actor_did: triggered_by,
            subject_did: Some(did),
            subject_uri: None,
            subject_cid: Some(head_after),
            details,
            meta: None,
        })
        .await
    {
        tracing::error!(
            target: "aurora_locus::rebuild",
            did = %did,
            error = %e,
            "repo rebuild succeeded but RepoRebuilt audit emit failed",
        );
    }

    // #303 — the tamper-evident operator-decision chain (read by getAuditTrail /
    // the #mod/audit page). A repo rebuild is an operator-initiated destructive
    // action gated behind a typed-confirm + rationale, so the decision (and its
    // rationale) must land in the chain, not only the moderation_event feed.
    // Best-effort + post-commit, mirroring the moderation_event emit above: a
    // chain-emit failure is logged and never reverses the already-committed
    // rebuild.
    let subject = crate::admin::defs::Subject::Repo { did: did.to_string() };
    if let Err(e) = crate::admin::audit_chain::insert_chain_entry_pool(
        &ctx.account_db,
        ctx.config.database.backend,
        crate::admin::audit_chain::AppendEntryParams {
            source: "manual",
            payload: None,
            actor_did: triggered_by,
            action: "repo.rebuild",
            subject: Some(&subject),
            rationale,
            snapshot_id: None,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    {
        tracing::error!(
            target: "aurora_locus::rebuild",
            did = %did,
            error = %e,
            "repo rebuild audit-chain emit failed (moderation_event still recorded)",
        );
    }
}

/// Resolve `did`'s published signing key as a `did:key:z...` string — the same
/// `plc_keys.atproto_signing_key` → `did:key` conversion `importRepo` uses for
/// `verify_diff_car`'s `signing_did_key`. Used to verify the reconstructed
/// head commit's signature during a rebuild.
async fn public_did_key_for(ctx: &AppContext, did: &str) -> PdsResult<String> {
    use proto_blue::crypto::Keypair as _;
    let key_bytes = ctx.account_manager.get_atproto_signing_key_bytes(did).await?;
    let kp = proto_blue::crypto::K256Keypair::from_private_key(&key_bytes).map_err(|e| {
        PdsError::Internal(format!(
            "rebuild: did:key construction failed for {}: {}",
            did, e
        ))
    })?;
    Ok(kp.did())
}

/// Interior of [`RebuildRegistry`]. `by_id` retains jobs (including terminal
/// ones) so `getRebuildProgress` can report on a finished rebuild; `active_did`
/// is the single-flight guard — a DID with an in-flight job rejects a new one.
#[derive(Default)]
struct RegistryInner {
    by_id: HashMap<String, Arc<RebuildJob>>,
    active_did: HashMap<String, String>,
}

/// The deployment's registry of repository-rebuild jobs. Enforces per-DID
/// single-flight (one rebuild per account at a time) and maps job-ids to jobs
/// for progress/cancel. Held in [`AppContext`].
#[derive(Default)]
pub struct RebuildRegistry {
    inner: Mutex<RegistryInner>,
}

impl RebuildRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new rebuild job for `did` under the single-flight guard, or
    /// return [`PdsError::Conflict`] if one is already in flight for the same
    /// DID. Synchronous and spawn-free — [`Self::start`] calls this then spawns
    /// the run. Split out so the single-flight invariant is testable without
    /// background-task timing.
    fn try_register(
        &self,
        did: String,
        triggered_by: String,
        rationale: String,
    ) -> PdsResult<Arc<RebuildJob>> {
        let mut inner = self.inner.lock().expect("rebuild registry lock not poisoned");
        if let Some(active_id) = inner.active_did.get(&did) {
            return Err(PdsError::Conflict(format!(
                "a repository rebuild is already in flight for {} (job {})",
                did, active_id
            )));
        }
        let job_id = uuid::Uuid::new_v4().to_string();
        let job = Arc::new(RebuildJob::new(
            job_id.clone(),
            did.clone(),
            triggered_by,
            rationale,
        ));
        inner.by_id.insert(job_id.clone(), Arc::clone(&job));
        inner.active_did.insert(did, job_id);
        Ok(job)
    }

    /// Start a rebuild for `did`: register a job, spawn its background run, and
    /// return the job-id. Per-DID single-flight — returns
    /// [`PdsError::Conflict`] (→ 409) if a rebuild is already in flight for the
    /// same DID. `rationale` is required (high-impact destructive action) and
    /// carried into the [`RepoRebuilt`](ModerationEventType::RepoRebuilt) audit.
    pub fn start(
        self: &Arc<Self>,
        ctx: AppContext,
        did: String,
        triggered_by: String,
        rationale: String,
    ) -> PdsResult<String> {
        let job = self.try_register(did, triggered_by, rationale)?;
        let job_id = job.job_id.clone();

        let registry = Arc::clone(self);
        tokio::spawn(async move {
            registry.drive(job, &ctx).await;
        });
        Ok(job_id)
    }

    /// Run a registered job to its terminal state, then free its DID so a
    /// subsequent rebuild can start (the job stays in `by_id` for progress
    /// reads). Shared by [`Self::start`] (spawned) and [`Self::run_one`]
    /// (awaited inline).
    async fn drive(&self, job: Arc<RebuildJob>, ctx: &AppContext) {
        let did = job.did.clone();
        let outcome = job.run(ctx).await;
        job.finish(outcome);
        self.inner
            .lock()
            .expect("rebuild registry lock not poisoned")
            .active_did
            .remove(&did);
    }

    /// Register + run a rebuild to completion inline (awaited, not spawned),
    /// returning its terminal progress. Per-DID single-flight — returns
    /// [`PdsError::Conflict`] if a rebuild is already in flight for the same
    /// DID. The bulk-repair job (#292) drives per-account rebuilds through this
    /// so each runs under the same single-flight lock as an operator-triggered
    /// `rebuildRepo`, without polling.
    pub async fn run_one(
        self: &Arc<Self>,
        ctx: &AppContext,
        did: String,
        triggered_by: String,
        rationale: String,
    ) -> PdsResult<RebuildProgress> {
        let job = self.try_register(did, triggered_by, rationale)?;
        self.drive(Arc::clone(&job), ctx).await;
        Ok(job.progress())
    }

    /// Look up a job's live progress by id, or `None` if the id is unknown.
    pub fn progress(&self, job_id: &str) -> Option<RebuildProgress> {
        self.inner
            .lock()
            .expect("rebuild registry lock not poisoned")
            .by_id
            .get(job_id)
            .map(|j| j.progress())
    }

    /// Request cancellation of a job by id. Returns `Some(true)` if the job was
    /// in flight (now cancelling), `Some(false)` if it had already finished
    /// ("nothing to cancel"), or `None` if the id is unknown.
    pub fn request_cancel(&self, job_id: &str) -> Option<bool> {
        self.inner
            .lock()
            .expect("rebuild registry lock not poisoned")
            .by_id
            .get(job_id)
            .map(|j| j.request_cancel())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sequencer::{events::CommitEvent, Sequencer, SequencerConfig};
    use proto_blue::crypto::{Keypair, P256Keypair};
    use proto_blue::lex_cbor::cid_for_lex;
    use proto_blue::lex_data::LexValue;
    use proto_blue::repo::commit::{sign_commit, UnsignedCommit};
    use proto_blue::repo::{blocks_to_car, BlockMap, MstNode};

    async fn test_sequencer() -> Sequencer {
        use std::sync::Once;
        static INSTALL: Once = Once::new();
        INSTALL.call_once(sqlx::any::install_default_drivers);
        let db = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE repo_seq (seq INTEGER PRIMARY KEY AUTOINCREMENT, did TEXT NOT NULL, \
             event_type TEXT NOT NULL, event BLOB NOT NULL, invalidated INTEGER NOT NULL DEFAULT 0, \
             sequenced_at TEXT NOT NULL)",
        )
        .execute(&db)
        .await
        .unwrap();
        Sequencer::new(db, SequencerConfig::default())
    }

    /// Build a real one-record repo: an MST with one record, a signed commit,
    /// serialized to a CAR. Returns (did, did:key, head_commit_cid, car_bytes).
    fn build_repo_car(rev: &str) -> (String, String, String, Vec<u8>) {
        let kp = P256Keypair::generate();
        let did = kp.did().replace("did:key:", "did:plc:");
        let mut blocks = BlockMap::new();
        let value = LexValue::String("hello".to_string());
        let rec_cid = cid_for_lex(&value).unwrap();
        blocks.add_value(&value).unwrap();
        let mst = MstNode::empty().add("app.bsky.feed.post/abc", rec_cid).unwrap();
        let (mst_root, mst_blocks) = mst.get_all_blocks().unwrap();
        blocks.add_map(&mst_blocks);
        let unsigned = UnsignedCommit::new(did.clone(), mst_root, rev.to_string(), None);
        let signed = sign_commit(&unsigned, &kp).unwrap();
        let commit_cid = blocks.add_value(&signed.to_lex_value()).unwrap();
        let car = blocks_to_car(Some(&commit_cid), &blocks).unwrap();
        (did, kp.did(), commit_cid.to_string(), car)
    }

    #[tokio::test]
    async fn reconstruct_verifies_a_real_repo() {
        let seq = test_sequencer().await;
        let (did, did_key, head_cid, car) = build_repo_car("3jzfcijpj2z2a");
        seq.sequence_commit(CommitEvent::new(
            did.clone(),
            head_cid.clone(),
            "3jzfcijpj2z2a".to_string(),
            None,
            None,
            car,
            vec![],
        ))
        .await
        .unwrap();

        // Full verification (with the signing key) reconstructs the canonical repo.
        let verified = reconstruct_and_verify(&seq, &did, Some(&did_key))
            .await
            .unwrap()
            .expect("history present");
        assert_eq!(
            verified.commit_cid.to_string(),
            head_cid,
            "reconstructed head matches the sequenced commit"
        );
        assert_eq!(verified.commit.did, did);
        // The MST resolves to the one record we put in.
        assert_eq!(verified.mst.leaves().len(), 1);
    }

    #[tokio::test]
    async fn reconstruct_none_for_unknown_account() {
        let seq = test_sequencer().await;
        assert!(reconstruct_and_verify(&seq, "did:plc:nobody", None)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn reconstruct_errors_on_incomplete_history() {
        // A commit event whose CAR omits the MST/record blocks (head commit CID
        // points at blocks not present) → verify_repo can't resolve → error.
        // Models a corrupt/incomplete history the preflight must flag.
        let seq = test_sequencer().await;
        let did = "did:plc:broken";
        // A real CID (so it parses) whose block is NOT in the CAR → MissingBlock.
        let absent = cid_for_lex(&LexValue::String("absent-head".to_string())).unwrap();
        seq.sequence_commit(CommitEvent::new(
            did.to_string(),
            absent.to_string(),
            "3jzfcijpj2z2a".to_string(),
            None,
            None,
            // empty CAR — no blocks
            blocks_to_car(None, &BlockMap::new()).unwrap(),
            vec![],
        ))
        .await
        .unwrap();
        assert!(
            reconstruct_and_verify(&seq, did, None).await.is_err(),
            "incomplete history (head block absent) must fail verification"
        );
    }

    #[tokio::test]
    async fn accumulate_observes_cancel() {
        let seq = test_sequencer().await;
        let (did, _did_key, head_cid, car) = build_repo_car("3jzfcijpj2z2a");
        seq.sequence_commit(CommitEvent::new(
            did.clone(),
            head_cid,
            "3jzfcijpj2z2a".to_string(),
            None,
            None,
            car,
            vec![],
        ))
        .await
        .unwrap();
        // A pre-set cancel flag stops the walk before the first page.
        let cancelled = AtomicBool::new(true);
        let out = accumulate_history(&seq, &did, &cancelled, |_| {})
            .await
            .unwrap();
        assert!(matches!(out, AccumulateOutcome::Cancelled));
    }

    #[test]
    fn phase_wire_strings_and_terminality() {
        assert_eq!(RebuildPhase::Walking.as_str(), "walking");
        assert_eq!(RebuildPhase::Swapping.as_str(), "swapping");
        assert!(RebuildPhase::Completed.is_terminal());
        assert!(RebuildPhase::Cancelled.is_terminal());
        assert!(RebuildPhase::Failed.is_terminal());
        assert!(!RebuildPhase::Walking.is_terminal());
        assert!(!RebuildPhase::Pending.is_terminal());
    }

    #[test]
    fn job_cancel_only_while_non_terminal() {
        let job = RebuildJob::new(
            "job-1".to_string(),
            "did:plc:x".to_string(),
            "did:plc:op".to_string(),
            "investigating corruption".to_string(),
        );
        assert!(job.request_cancel(), "a pending job can be cancelled");
        assert!(job.progress().cancel_requested);
        // After it reaches terminal, cancel is a no-op ("nothing to cancel").
        job.finish(RebuildOutcome::Completed);
        assert!(!job.request_cancel());
    }

    #[test]
    fn registry_progress_and_cancel_lookup_by_id() {
        // Unknown ids return None for both progress and cancel.
        let reg = RebuildRegistry::new();
        assert!(reg.progress("nope").is_none());
        assert!(reg.request_cancel("nope").is_none());
    }

    #[test]
    fn registry_single_flight_per_did() {
        let reg = RebuildRegistry::new();
        let j1 = reg
            .try_register("did:plc:a".to_string(), "did:plc:op".to_string(), "r".to_string())
            .expect("first registration for a DID succeeds");
        // A second registration for the SAME did is rejected (single-flight).
        assert!(matches!(
            reg.try_register("did:plc:a".to_string(), "did:plc:op".to_string(), "r".to_string()),
            Err(PdsError::Conflict(_))
        ));
        // A different did is independent.
        assert!(reg
            .try_register("did:plc:b".to_string(), "did:plc:op".to_string(), "r".to_string())
            .is_ok());
        // The registered job is reachable by id for progress.
        assert!(reg.progress(&j1.job_id).is_some());
        assert_eq!(reg.progress(&j1.job_id).unwrap().phase, RebuildPhase::Pending);
    }

    /// The §7.4.1 verification gate, captured as code: feed a known-good
    /// account through reconstruct → atomic swap, and assert the post-swap
    /// live store state equals what walking the sequencer history fresh
    /// produces (the reconstructed `VerifiedRepo`). The store is pre-seeded
    /// with divergent stale state to prove the swap *replaces* the repo rather
    /// than merging into it. This locks the substrate's correctness criterion;
    /// a regression here means a rebuild no longer yields the canonical repo.
    #[tokio::test]
    async fn end_to_end_rebuild_matches_reconstructed_state() {
        use crate::actor_store::{ActorStore, ActorStoreConfig};
        use std::collections::BTreeSet;
        use tempfile::TempDir;

        // A known-good account: one signed commit with one record, sequenced.
        let seq = test_sequencer().await;
        let (did, did_key, head_cid, car) = build_repo_car("3jzfcijpj2z2a");
        seq.sequence_commit(CommitEvent::new(
            did.clone(),
            head_cid.clone(),
            "3jzfcijpj2z2a".to_string(),
            None,
            None,
            car,
            vec![],
        ))
        .await
        .unwrap();

        // A live actor store seeded with STALE, divergent state — a different
        // root, a block and a record that the rebuild must obliterate.
        let temp = TempDir::new().unwrap();
        let store = ActorStore::new(ActorStoreConfig {
            base_directory: temp.path().to_path_buf(),
            cache_size: 10,
        });
        store.create(&did).await.unwrap();
        let stale_cid = cid_for_lex(&LexValue::String("stale-block".to_string()))
            .unwrap()
            .to_string();
        store.put_block(&did, &stale_cid, b"stale").await.unwrap();
        store
            .update_repo_root(&did, &stale_cid, "3aaaaaaaaaaaa")
            .await
            .unwrap();
        store
            .put_record(
                &did,
                &format!("at://{}/app.bsky.feed.post/stale", did),
                &stale_cid,
                "app.bsky.feed.post",
                "stale",
                "3aaaaaaaaaaaa",
            )
            .await
            .unwrap();

        // Reconstruct from history (full signature verification) + atomic swap.
        let verified = reconstruct_and_verify(&seq, &did, Some(&did_key))
            .await
            .unwrap()
            .expect("history present");
        let n = atomic_swap(&store, &did, &verified).await.unwrap();
        assert_eq!(n, 1, "exactly the one MST-leaf record is rebuilt");

        // GATE: post-swap root == reconstructed head.
        let root = store.get_repo_root(&did).await.unwrap();
        assert_eq!(root.cid, head_cid, "post-swap head == reconstructed commit");
        assert_eq!(root.rev, verified.rev(), "post-swap rev == reconstructed rev");

        // GATE: post-swap block set == reconstructed block set (no stale block).
        let store_blocks: BTreeSet<String> = store
            .get_all_blocks(&did)
            .await
            .unwrap()
            .into_iter()
            .map(|(c, _)| c)
            .collect();
        let recon_blocks: BTreeSet<String> =
            verified.blocks.iter().map(|(c, _)| c.to_string()).collect();
        assert_eq!(store_blocks, recon_blocks, "blocks == reconstructed set");
        assert!(
            !store_blocks.contains(&stale_cid),
            "the stale block was obliterated by the swap"
        );

        // GATE: post-swap record index == MST leaves (no stale record).
        let recs = store.list_all_records(&did).await.unwrap();
        assert_eq!(recs.len(), 1, "exactly one record after rebuild");
        assert_eq!(recs[0].uri, format!("at://{}/app.bsky.feed.post/abc", did));
        assert_eq!(
            recs[0].cid,
            verified.mst.leaves()[0].value.to_string(),
            "record points at the MST leaf's value CID"
        );
        assert!(
            !recs.iter().any(|r| r.rkey == "stale"),
            "the stale record was obliterated by the swap"
        );

        // GATE: the rebuild is deterministic — a second pass reproduces the
        // identical canonical state.
        let verified2 = reconstruct_and_verify(&seq, &did, Some(&did_key))
            .await
            .unwrap()
            .expect("history present");
        atomic_swap(&store, &did, &verified2).await.unwrap();
        let root2 = store.get_repo_root(&did).await.unwrap();
        assert_eq!(root2.cid, head_cid, "rebuild is idempotent / deterministic");

        drop(temp);
    }
}
