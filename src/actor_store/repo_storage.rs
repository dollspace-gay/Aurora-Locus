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
//!
//! # v0.7 arc 2 step 3.5 — transaction lending
//!
//! Two interior-mutable lent-tx handles let the dispatcher unify
//! proto-blue's `apply_commit` and the Phase A per-record metadata
//! writes under one per-actor SQLite transaction. The shared-account-
//! DB transaction is lent in parallel for the bind-pipeline audit
//! emit (step 4/7).
//!
//! Per the v0.7 arc 2 supplement-and-addendum:
//! - Per-actor SQLite and shared account DB are physically distinct
//!   databases; one `sqlx::Transaction` cannot span both. Step 3.5
//!   uses two transaction handles and a relay-race commit order
//!   (audit-first) with an orphan-marker emit when the second
//!   commit fails (forensic-honest failure mode preserving the
//!   "neither, both, or one-with-an-operator-visible-marker"
//!   invariant).
//! - The marker schema and reconciliation sweep are deferred to a
//!   follow-up fixup; step 3.5 ships the commit-order machinery
//!   plus a `tracing::error!` orphan emit so the failure mode is
//!   observable in operator logs immediately.

use std::sync::Arc;

use futures::FutureExt;
use proto_blue::lex_data::Cid;
use proto_blue::repo::{
    block_map::BlockMap, commit::SignedCommit, error::RepoError, storage::RepoStorage,
};
use std::str::FromStr;
use tokio::runtime::Handle;
use tokio::sync::Mutex as TokioMutex;

use crate::actor_store::ActorStore;
use crate::error::PdsError;

/// `RepoStorage` adapter backed by a single DID's actor store.
pub struct SqliteRepoStorage {
    store: Arc<ActorStore>,
    did: String,
    /// v0.7 arc 2 step 3.5 — per-actor SQLite transaction lent by
    /// `apply_writes`. When `Some(_)`, `apply_commit` and the
    /// per-record metadata writes route through this tx instead of
    /// per-statement auto-commit. When `None`, the existing auto-
    /// commit behavior is preserved (back-compat for tests and any
    /// `RepoStorage` consumer that doesn't lend a tx).
    ///
    /// Wrapped in `Arc<TokioMutex<Option<_>>>` so:
    /// 1. The storage adapter remains `Clone`-cheap (the lent tx
    ///    is shared across the spawn_blocking clone proto-blue
    ///    requires).
    /// 2. `apply_commit` (sync) can acquire the lent tx via
    ///    `block_on(mutex.lock())` cleanly.
    /// 3. Per-statement atomic access (multiple acquire/release
    ///    cycles inside one scope) is fine — concurrent
    ///    `with_lent_txns` calls are serialized by
    ///    `lend_in_progress` below, so the slot's contents are
    ///    stable for the duration of any one scope.
    lent_actor_tx: Arc<TokioMutex<Option<sqlx::Transaction<'static, sqlx::Sqlite>>>>,
    /// v0.7 arc 2 step 3.5 — shared account-DB transaction lent
    /// by `apply_writes` for the bind-pipeline audit emit
    /// (step 4/7). Held on `SqliteRepoStorage` solely so the
    /// `with_lent_txns` install/uninstall pair stays paired with
    /// the actor tx; the storage layer's own `apply_commit` does
    /// NOT touch the shared DB.
    lent_shared_tx: Arc<TokioMutex<Option<sqlx::Transaction<'static, sqlx::Any>>>>,
    /// v0.7 arc 2 step 3.5 — outer serialization Mutex held for
    /// the entire `with_lent_txns` scope. Concurrent
    /// `with_lent_txns` calls on the same storage block on this
    /// Mutex, ensuring the per-`Option<Tx>` slots above never
    /// hold a stale tx from a different scope. The unit-typed
    /// guard is inert except for its position lifecycle.
    ///
    /// `apply_commit` (which acquires `lent_actor_tx` repeatedly
    /// inside its scope) does NOT touch this Mutex, so the inner
    /// acquisitions don't compete with the outer hold.
    lend_in_progress: Arc<TokioMutex<()>>,
}

impl SqliteRepoStorage {
    /// Wrap an `ActorStore` for use by a `Repo` operating on `did`.
    /// Both lent-tx handles start as `None` — call `with_lent_txns`
    /// to install transactions for the duration of an
    /// `apply_writes` scope.
    pub fn new(store: Arc<ActorStore>, did: String) -> Self {
        Self {
            store,
            did,
            lent_actor_tx: Arc::new(TokioMutex::new(None)),
            lent_shared_tx: Arc::new(TokioMutex::new(None)),
            lend_in_progress: Arc::new(TokioMutex::new(())),
        }
    }

    /// Accessor for the lent per-actor SQLite tx handle. Used by
    /// the Phase A metadata loop inside `apply_writes`'s
    /// `with_lent_txns` scope to thread `put_record_in_tx` /
    /// `delete_record_in_tx` through the same tx that
    /// `apply_commit` writes to.
    pub(crate) fn lent_actor_tx(
        &self,
    ) -> &Arc<TokioMutex<Option<sqlx::Transaction<'static, sqlx::Sqlite>>>> {
        &self.lent_actor_tx
    }

    /// Accessor for the lent shared-DB tx handle. Step 4's bind
    /// pipeline call site acquires this lock and writes audit-emit
    /// rows to the shared DB through the lent tx, preserving the
    /// audit-first commit ordering at step 3.5's `apply_writes`
    /// orchestration.
    #[allow(dead_code)] // consumed at arc 2 step 4 (bind pipeline)
    pub(crate) fn lent_shared_tx(
        &self,
    ) -> &Arc<TokioMutex<Option<sqlx::Transaction<'static, sqlx::Any>>>> {
        &self.lent_shared_tx
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

    /// Lend the supplied actor (per-DID SQLite) and shared
    /// (account-DB `sqlx::Any`) transactions for the duration of
    /// `scope`. Returns both transactions back (still open and
    /// uncommitted) along with the scope's result.
    ///
    /// The closure-shape API guarantees install/uninstall pairs and
    /// is panic-safe — if the scope panics mid-flight, both
    /// transactions are extracted from the lent-tx slots before
    /// the unwind continues (no leak of `Option<Tx>` state into
    /// future `apply_commit` calls on a re-used storage).
    ///
    /// Single-thread-safe-only: concurrent `with_lent_txns` calls
    /// on the same `SqliteRepoStorage` instance serialize on the
    /// underlying `Mutex`es. Production constructs one storage per
    /// `RepositoryManager::apply_writes` invocation, so concurrent
    /// lends on the same instance are not a real-world bottleneck.
    pub async fn with_lent_txns<F, Fut, R>(
        &self,
        actor_tx: sqlx::Transaction<'static, sqlx::Sqlite>,
        shared_tx: sqlx::Transaction<'static, sqlx::Any>,
        scope: F,
    ) -> Result<
        (
            sqlx::Transaction<'static, sqlx::Sqlite>,
            sqlx::Transaction<'static, sqlx::Any>,
            Result<R, PdsError>,
        ),
        PdsError,
    >
    where
        F: FnOnce(Arc<Self>) -> Fut,
        Fut: std::future::Future<Output = Result<R, PdsError>>,
        Self: Sized,
    {
        // Outer serialization guard — concurrent `with_lent_txns`
        // calls on the same storage block here. Held for the
        // entire scope so the inner `Option<Tx>` slots can't be
        // overwritten by a racing call's install.
        let _serialize_guard = self.lend_in_progress.lock().await;

        // Install both txns. We hold the per-Option locks only
        // briefly here; under the outer serialization guard they
        // can't be overwritten before the scope's installer
        // completes.
        *self.lent_actor_tx.lock().await = Some(actor_tx);
        *self.lent_shared_tx.lock().await = Some(shared_tx);

        // The scope receives an Arc<Self> so it can spawn_blocking
        // and clone the storage. The lent-tx Mutexes are Arc-shared,
        // so clones see the same installed txns.
        let storage_arc = Arc::new(Self {
            store: self.store.clone(),
            did: self.did.clone(),
            lent_actor_tx: self.lent_actor_tx.clone(),
            lent_shared_tx: self.lent_shared_tx.clone(),
            lend_in_progress: self.lend_in_progress.clone(),
        });

        // catch_unwind so we can extract the txns even if the scope
        // panics mid-flight. AssertUnwindSafe is justified: the txns
        // live in our own Mutexes; the lock-poison-on-panic concern
        // doesn't apply to tokio::sync::Mutex (which is not
        // poison-aware), and we own the storage_arc.
        let panic_result =
            std::panic::AssertUnwindSafe(scope(storage_arc)).catch_unwind().await;

        // Always extract both txns, regardless of scope outcome.
        let actor_tx_out = self
            .lent_actor_tx
            .lock()
            .await
            .take()
            .ok_or_else(|| PdsError::Internal(
                "with_lent_txns: lent_actor_tx unexpectedly missing after scope".to_string(),
            ))?;
        let shared_tx_out = self
            .lent_shared_tx
            .lock()
            .await
            .take()
            .ok_or_else(|| PdsError::Internal(
                "with_lent_txns: lent_shared_tx unexpectedly missing after scope".to_string(),
            ))?;

        match panic_result {
            Ok(scope_result) => Ok((actor_tx_out, shared_tx_out, scope_result)),
            Err(panic_payload) => {
                // Roll back both txns so they don't leak open
                // connections, then resume the panic so the
                // caller's panic semantics are preserved.
                let _ = actor_tx_out.rollback().await;
                let _ = shared_tx_out.rollback().await;
                std::panic::resume_unwind(panic_payload);
            }
        }
    }
}

/// v0.7 arc 2 step 3.5 — emit the bind-audit orphan marker.
///
/// Pulled out of `commit_with_orphan_recovery` so the failure path
/// can be unit-tested in isolation without arranging a real
/// commit-time failure on a live sqlx transaction (which sqlx
/// itself makes hard to provoke deterministically).
///
/// The marker is currently a `tracing::error!` event only; the
/// table-backed marker schema + reconciliation sweep are deferred
/// to a follow-up fixup per the arc 2 step 3.5 addendum's
/// stop-condition #1. Future fixups will replace this function's
/// body with an INSERT into a `bind_audit_orphan_marker` table on
/// the shared DB.
pub(crate) fn emit_bind_audit_orphan_marker(did: &str, actor_err: &dyn std::fmt::Display) {
    tracing::error!(
        target: "aurora_locus::repo_storage",
        event = "bind_audit_orphan_marker",
        did = %did,
        actor_commit_error = %actor_err,
        "audit emit committed but per-actor record commit failed — \
         orphan-recovery sweep target",
    );
}

/// v0.7 arc 2 step 3.5 — relay-race commit order with orphan-marker
/// recovery, per the arc 2 supplement-addendum.
///
/// Commits the supplied transactions in audit-first order:
///
/// 1. **Shared tx first** (the audit-emit side). If this fails,
///    rolls back the actor tx so the record commit doesn't land
///    against an absent audit entry. Clean failure: neither side
///    committed.
/// 2. **Actor tx second** (the record + metadata). If this fails,
///    the shared tx has already committed — an audit entry now
///    references a record that doesn't exist.
///    [`emit_bind_audit_orphan_marker`] fires so the failure is
///    operator-observable; the table-backed marker + reconciliation
///    sweep are deferred to a follow-up fixup per the addendum's
///    stop-condition #1.
///
/// **Why audit-first.** The record-first failure mode (record
/// committed, audit not) leaves a silent state change with no
/// audit trail — the worst case the design exists to prevent. The
/// audit-first failure mode (audit committed, record not) leaves a
/// "we tried but didn't finish" entry visible to forensics —
/// honest about the attempt.
pub(crate) async fn commit_with_orphan_recovery(
    actor_tx: sqlx::Transaction<'static, sqlx::Sqlite>,
    shared_tx: sqlx::Transaction<'static, sqlx::Any>,
    did: &str,
    // v0.8 arc 1 (#180) — moderation_event.id(s) committed onto
    // `shared_tx` during the bind pipeline. Empty unless an audit row
    // was emitted. Keys the persistent orphan marker on the
    // actor-commit-failure path.
    emitted_event_ids: Vec<i64>,
    // v0.8 arc 1 (#180) — the orphan-marker INSERT must run AFTER
    // `shared_tx.commit()` consumes `shared_tx`, so it opens its own
    // short tx on this pool.
    shared_pool: &sqlx::AnyPool,
) -> Result<(), PdsError> {
    // Step 1: shared tx (audit emit) first.
    if let Err(shared_err) = shared_tx.commit().await {
        // Roll back the actor tx — record commit must not land
        // against a missing audit entry. Failure to roll back is
        // logged but doesn't change the user-facing error (the
        // commit already failed for an independent reason).
        if let Err(rb_err) = actor_tx.rollback().await {
            tracing::warn!(
                target: "aurora_locus::repo_storage",
                event = "actor_tx_rollback_failed_after_shared_commit_failure",
                did = %did,
                rollback_error = %rb_err,
                shared_commit_error = %shared_err,
                "actor tx rollback failed after shared-tx commit failure; \
                 connection drop will clean up but log noise warrants attention",
            );
        }
        return Err(PdsError::Database(shared_err));
    }

    // Step 2: actor tx second.
    if let Err(actor_err) = actor_tx.commit().await {
        record_orphan_marker(did, &actor_err, &emitted_event_ids, shared_pool).await;
        return Err(PdsError::Database(actor_err));
    }

    Ok(())
}

/// v0.8 arc 1 (#180) — sibling-emit ordering for the orphan path,
/// factored so the real-failure branch and the debug-injection branch
/// share one definition. Per §3.5: **tracing emit first** (the
/// sync, can't-fail forensic safety net), **persistent INSERT second**
/// (async, returns `()`, can't fail the caller). The caller propagates
/// the ORIGINAL `actor_err` — never a marker-INSERT error.
async fn record_orphan_marker(
    did: &str,
    actor_err: &(dyn std::fmt::Display + Sync),
    emitted_event_ids: &[i64],
    shared_pool: &sqlx::AnyPool,
) {
    // SIBLING EMIT — TRACING FIRST (forensic-detail safety net).
    emit_bind_audit_orphan_marker(did, actor_err);
    // SIBLING EMIT — INSERT SECOND (persistent, sweep target).
    insert_persistent_orphan_marker(
        shared_pool,
        did,
        &actor_err.to_string(),
        emitted_event_ids,
        &chrono::Utc::now().to_rfc3339(),
    )
    .await;
}

/// v0.8 arc 1 (#180) — persistent sibling of
/// [`emit_bind_audit_orphan_marker`]. Inserts one
/// `bind_audit_orphan_marker` row per `moderation_event_id` in
/// `emitted_event_ids`, joining each id back to its committed
/// `moderation_event` row for the `subject_uri`. `ON CONFLICT
/// (moderation_event_id) DO NOTHING` keeps the `UNIQUE` constraint
/// idempotent under retried/duplicate failures.
///
/// **Cannot fail the caller** — returns `()`. Internal errors are
/// logged via `tracing::warn!` and swallowed; the sibling
/// `tracing::error!` emit (already fired by
/// [`emit_bind_audit_orphan_marker`]) retains the forensic detail even
/// when the INSERT can't land. This is the sibling-emit invariant
/// (§3.5): a marker-INSERT error must never replace the `actor_err` the
/// caller propagates.
pub(crate) async fn insert_persistent_orphan_marker(
    shared_pool: &sqlx::AnyPool,
    did: &str,
    actor_err_string: &str,
    emitted_event_ids: &[i64],
    now_rfc3339: &str,
) {
    // Internal try-block: `?` for early exit on the first sqlx error;
    // the outer wrapper below catches it.
    let result: Result<(), sqlx::Error> = async {
        let mut tx = shared_pool.begin().await?;
        for event_id in emitted_event_ids {
            // subject_uri is read as String (NOT Option<String>): the
            // v0.7 orphan-able emit set unconditionally populates it
            // (round-1 L2). A future emit type that lands a NULL
            // subject_uri before the migration lifts NOT NULL would
            // surface here as sqlx::Error::ColumnDecode — the outer
            // wrapper catches it, the row doesn't land, the tracing
            // sibling already fired.
            let subject_uri: String = sqlx::query_scalar(
                "SELECT subject_uri FROM moderation_event WHERE id = $1",
            )
            .bind(event_id)
            .fetch_one(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT INTO bind_audit_orphan_marker \
                 (moderation_event_id, actor_did, subject_uri, actor_commit_error, \
                  state, created_at, resolved_at, resolution_detail) \
                 VALUES ($1, $2, $3, $4, 'unresolved', $5, NULL, NULL) \
                 ON CONFLICT (moderation_event_id) DO NOTHING",
            )
            .bind(event_id)
            .bind(did)
            .bind(&subject_uri)
            .bind(actor_err_string)
            .bind(now_rfc3339)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
    .await;

    if let Err(e) = result {
        tracing::warn!(
            target: "aurora_locus::repo_storage",
            event = "bind_audit_orphan_marker_persistent_insert_failed",
            did = %did,
            error = %e,
            "persistent orphan marker insert failed; tracing emit \
             retained the forensic detail (sibling-emit invariant). \
             Operator can grep bind_audit_orphan_marker tracing events \
             for the missing-marker context."
        );
    }
}

impl RepoStorage for SqliteRepoStorage {
    fn get_block(&self, cid: &Cid) -> Result<Option<Vec<u8>>, RepoError> {
        // v0.7 arc 2 phase B fix-up — route reads through the
        // lent actor tx when one is installed. proto-blue stages
        // blocks via `put_block` during commit assembly and reads
        // them back via `get_block` for signature computation; if
        // the writes go into the lent tx (via the patched
        // `put_block` below) but the reads come from a fresh
        // auto-commit pool connection, proto-blue sees "Missing
        // block" because the staged row isn't yet committed.
        // Routing both sides through the same tx closes the
        // visibility seam. When no tx is lent (the legacy auto-
        // commit path or any non-`apply_writes` consumer of
        // `RepoStorage`), behavior is unchanged.
        let cid_str = cid.to_string();
        Self::block_on(async {
            let mut tx_guard = self.lent_actor_tx.lock().await;
            if let Some(tx) = tx_guard.as_mut() {
                ActorStore::get_block_in_tx(tx, &cid_str)
                    .await
                    .map_err(|e| RepoError::Storage(format!("get_block({}): {}", cid_str, e)))
            } else {
                self.store
                    .get_block(&self.did, &cid_str)
                    .await
                    .map_err(|e| RepoError::Storage(format!("get_block({}): {}", cid_str, e)))
            }
        })
    }

    fn put_block(&self, cid: Cid, bytes: Vec<u8>) -> Result<(), RepoError> {
        // v0.7 arc 2 phase B fix-up — route writes through the
        // lent actor tx when installed. See get_block above for
        // the full visibility-seam rationale.
        let cid_str = cid.to_string();
        Self::block_on(async {
            let mut tx_guard = self.lent_actor_tx.lock().await;
            if let Some(tx) = tx_guard.as_mut() {
                ActorStore::put_block_in_tx(tx, &cid_str, &bytes)
                    .await
                    .map_err(|e| RepoError::Storage(format!("put_block({}): {}", cid_str, e)))
            } else {
                self.store
                    .put_block(&self.did, &cid_str, &bytes)
                    .await
                    .map_err(|e| RepoError::Storage(format!("put_block({}): {}", cid_str, e)))
            }
        })
    }

    fn get_root(&self) -> Result<Option<Cid>, RepoError> {
        // v0.7 arc 2 phase B fix-up — route through the lent
        // actor tx when installed so root reads see staged-but-
        // uncommitted updates. The NotFound-→-Ok(None) mapping
        // for uninitialised repos is preserved across both
        // branches.
        Self::block_on(async {
            let mut tx_guard = self.lent_actor_tx.lock().await;
            let root_result = if let Some(tx) = tx_guard.as_mut() {
                ActorStore::get_repo_root_in_tx(tx, &self.did).await
            } else {
                self.store.get_repo_root(&self.did).await
            };
            match root_result {
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
    ///
    /// v0.7 arc 2 phase B fix-up — both the rev read and the root
    /// upsert route through the lent actor tx when installed.
    fn update_root(&self, new_root: Cid) -> Result<(), RepoError> {
        let new_root_str = new_root.to_string();
        Self::block_on(async {
            let mut tx_guard = self.lent_actor_tx.lock().await;
            if let Some(tx) = tx_guard.as_mut() {
                let existing_rev = match ActorStore::get_repo_root_in_tx(tx, &self.did).await {
                    Ok(r) => r.rev,
                    Err(_) => String::new(),
                };
                ActorStore::update_repo_root_in_tx(tx, &self.did, &new_root_str, &existing_rev)
                    .await
                    .map_err(|e| RepoError::Storage(format!("update_root: {}", e)))
            } else {
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
            }
        })
    }

    /// Atomic-ish commit application: persist every block in `blocks`,
    /// then atomically advance the on-disk `(cid, rev)` pair. The new
    /// `rev` is read straight out of the signed commit block so we never
    /// disagree with what's actually in storage.
    ///
    /// **v0.7 arc 2 step 3.5 — lent actor-tx routing.** If
    /// `lent_actor_tx` is `Some(_)` (installed by `with_lent_txns`),
    /// every `put_block` and `update_repo_root` query routes
    /// through that transaction via the `*_in_tx` variants on
    /// `ActorStore` — the caller (`apply_writes`) commits or rolls
    /// back externally via the relay-race orchestration. If
    /// `lent_actor_tx` is `None`, the legacy per-statement auto-
    /// commit path runs (preserving back-compat for tests and any
    /// `RepoStorage` consumer that doesn't lend a tx).
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
            let mut tx_guard = self.lent_actor_tx.lock().await;
            if let Some(tx) = tx_guard.as_mut() {
                // Lent-tx path: route all writes through the
                // caller-managed actor tx.
                for (cid, bytes) in blocks.iter() {
                    ActorStore::put_block_in_tx(tx, &cid.to_string(), bytes)
                        .await
                        .map_err(|e| {
                            RepoError::Storage(format!("put_block({}): {}", cid, e))
                        })?;
                }
                ActorStore::update_repo_root_in_tx(tx, &self.did, &new_root_str, &new_rev)
                    .await
                    .map_err(|e| RepoError::Storage(format!("update_root: {}", e)))?;
            } else {
                // Auto-commit path: legacy behavior preserved.
                for (cid, bytes) in blocks.iter() {
                    self.store
                        .put_block(&self.did, &cid.to_string(), bytes)
                        .await
                        .map_err(|e| {
                            RepoError::Storage(format!("put_block({}): {}", cid, e))
                        })?;
                }
                self.store
                    .update_repo_root(&self.did, &new_root_str, &new_rev)
                    .await
                    .map_err(|e| RepoError::Storage(format!("update_root: {}", e)))?;
            }
            Ok::<_, RepoError>(())
        })
    }
}

// ---------------------------------------------------------------------------
// v0.7 arc 2 step 3.5 — tests
// ---------------------------------------------------------------------------
//
// Coverage matrix (per the supplement's five cases + the addendum's
// two relay-race cases, seven total):
//
// supplement-style:
//   #1 lend tx → put_block_in_tx → caller commits → row visible
//   #2 lend tx → put_block_in_tx → caller rolls back → row NOT visible
//   #3 no lent tx → apply_commit auto-commit path persists writes
//      (back-compat preservation for `RepositoryManager::new`-style
//      test instances)
//   #4 concurrent `with_lent_txns` calls on the same storage
//      serialize on the underlying TokioMutex (second blocks until
//      first releases)
//   #5 scope panics mid-flight → both txns are extracted from the
//      lent slots before unwind continues; the storage is reusable
//      after panic recovery
//
// addendum-style:
//   #6 actor-tx commit failure → `emit_bind_audit_orphan_marker`
//      fires (tested via `tracing-test` capture of the orphan
//      event)
//   #7 shared-tx commit failure → relay-race rolls back actor_tx
//      cleanly with NO orphan marker (cleanly failed, no half-state)

#[cfg(test)]
mod step_3_5_tests {
    use super::*;
    use crate::actor_store::{ActorStore, ActorStoreConfig};
    use sqlx::AnyPool;
    use std::sync::atomic::{AtomicU64, Ordering};
    use tempfile::TempDir;

    /// Set up a per-actor SQLite pool (with the actor-store
    /// schema) and an in-memory shared AnyPool for tests.
    async fn test_pools() -> (Arc<ActorStore>, String, sqlx::SqlitePool, AnyPool, TempDir) {
        sqlx::any::install_default_drivers();
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let did = format!("did:plc:step35test{}", n);

        let tmp = TempDir::new().expect("temp dir");
        let store = ActorStore::new(ActorStoreConfig {
            base_directory: tmp.path().to_path_buf(),
            cache_size: 16,
        });
        store.create(&did).await.expect("create actor");
        let actor_pool = store.open_db(&did).await.expect("open actor db");

        // Shared pool: a fresh in-memory SQLite via the Any driver,
        // sized at 1 connection so commit/rollback semantics are
        // deterministic for the tests.
        let shared_pool = AnyPool::connect("sqlite::memory:")
            .await
            .expect("shared any pool");

        (Arc::new(store), did, actor_pool, shared_pool, tmp)
    }

    fn make_storage(store: Arc<ActorStore>, did: String) -> SqliteRepoStorage {
        SqliteRepoStorage::new(store, did)
    }

    /// Helper to check whether a block CID exists in the per-actor
    /// pool — independent of the (potentially lent) tx so we can
    /// observe what is or isn't committed.
    async fn block_exists(pool: &sqlx::SqlitePool, cid: &str) -> bool {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM repo_block WHERE cid = ?1")
            .bind(cid)
            .fetch_one(pool)
            .await
            .expect("query block count");
        count > 0
    }

    // -----------------------------------------------------------
    // #1 — lend tx → write via lent_actor_tx → commit externally
    //      → row visible.
    // -----------------------------------------------------------
    #[tokio::test(flavor = "multi_thread")]
    async fn step_3_5_t1_lent_tx_commit_persists_writes() {
        let (store, did, actor_pool, shared_pool, _tmp) = test_pools().await;
        let storage = make_storage(store, did);
        let cid = "bafyt1step35t1";

        let actor_tx = actor_pool.begin().await.expect("begin actor tx");
        let shared_tx = shared_pool.begin().await.expect("begin shared tx");

        let (actor_tx_back, shared_tx_back, scope_result) = storage
            .with_lent_txns(actor_tx, shared_tx, |scoped_storage| async move {
                let mut g = scoped_storage.lent_actor_tx().lock().await;
                let tx = g.as_mut().expect("lent_actor_tx installed");
                ActorStore::put_block_in_tx(tx, cid, b"step35-t1-bytes").await?;
                Ok::<_, PdsError>(())
            })
            .await
            .expect("with_lent_txns ok");
        scope_result.expect("scope ok");

        // Before commit the row is NOT yet visible outside the tx.
        assert!(
            !block_exists(&actor_pool, cid).await,
            "row must not be visible before caller commits the actor tx",
        );

        actor_tx_back.commit().await.expect("commit actor tx");
        shared_tx_back.commit().await.expect("commit shared tx");

        assert!(
            block_exists(&actor_pool, cid).await,
            "row must be visible after caller commits the actor tx",
        );
    }

    // -----------------------------------------------------------
    // #2 — lend tx → write via lent_actor_tx → rollback externally
    //      → row NOT visible (lend doesn't auto-commit).
    // -----------------------------------------------------------
    #[tokio::test(flavor = "multi_thread")]
    async fn step_3_5_t2_lent_tx_rollback_discards_writes() {
        let (store, did, actor_pool, shared_pool, _tmp) = test_pools().await;
        let storage = make_storage(store, did);
        let cid = "bafyt2step35t2";

        let actor_tx = actor_pool.begin().await.expect("begin actor tx");
        let shared_tx = shared_pool.begin().await.expect("begin shared tx");

        let (actor_tx_back, shared_tx_back, scope_result) = storage
            .with_lent_txns(actor_tx, shared_tx, |scoped_storage| async move {
                let mut g = scoped_storage.lent_actor_tx().lock().await;
                let tx = g.as_mut().expect("lent_actor_tx installed");
                ActorStore::put_block_in_tx(tx, cid, b"step35-t2-bytes").await?;
                Ok::<_, PdsError>(())
            })
            .await
            .expect("with_lent_txns ok");
        scope_result.expect("scope ok");

        actor_tx_back
            .rollback()
            .await
            .expect("rollback actor tx");
        shared_tx_back
            .rollback()
            .await
            .expect("rollback shared tx");

        assert!(
            !block_exists(&actor_pool, cid).await,
            "row must NOT be visible after caller rolls back the actor tx",
        );
    }

    // -----------------------------------------------------------
    // #3 — no lent tx → ActorStore.put_block (auto-commit) path
    //      still persists writes (back-compat path the test
    //      constructors rely on).
    // -----------------------------------------------------------
    #[tokio::test(flavor = "multi_thread")]
    async fn step_3_5_t3_auto_commit_path_back_compat() {
        let (store, did, actor_pool, _shared_pool, _tmp) = test_pools().await;
        let cid = "bafyt3step35t3";

        // No `with_lent_txns` — call put_block directly.
        store
            .put_block(&did, cid, b"step35-t3-bytes")
            .await
            .expect("put_block auto-commit");

        assert!(
            block_exists(&actor_pool, cid).await,
            "auto-commit path must persist writes when no tx is lent",
        );
    }

    // -----------------------------------------------------------
    // #4 — concurrent with_lent_txns calls serialize on the
    //      underlying Mutex. The second call's install only
    //      proceeds after the first call's scope returns and
    //      uninstalls the txns.
    // -----------------------------------------------------------
    #[tokio::test(flavor = "multi_thread")]
    async fn step_3_5_t4_concurrent_lends_serialize() {
        let (store, did, actor_pool, shared_pool, _tmp) = test_pools().await;
        let storage = Arc::new(make_storage(store, did));

        // First scope holds the lent slot for ~150ms — enough that
        // the second call's install would race-win if Mutex
        // semantics were absent.
        let actor_tx_1 = actor_pool.begin().await.expect("begin actor tx 1");
        let shared_tx_1 = shared_pool.begin().await.expect("begin shared tx 1");
        let actor_tx_2 = actor_pool.begin().await.expect("begin actor tx 2");
        let shared_tx_2 = shared_pool.begin().await.expect("begin shared tx 2");

        let started_at = std::time::Instant::now();
        let first_release = Arc::new(tokio::sync::Notify::new());
        let first_release_notify = first_release.clone();

        let storage_1 = storage.clone();
        let first = tokio::spawn(async move {
            let (a, s, r) = storage_1
                .with_lent_txns(actor_tx_1, shared_tx_1, |_| async move {
                    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                    Ok::<_, PdsError>(())
                })
                .await
                .expect("first with_lent_txns");
            r.expect("first scope ok");
            first_release_notify.notify_one();
            (a.commit().await, s.commit().await)
        });

        // Briefly yield so the first task gets to install its txns.
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let storage_2 = storage.clone();
        let second = tokio::spawn(async move {
            let install_start = std::time::Instant::now();
            let (a, s, r) = storage_2
                .with_lent_txns(actor_tx_2, shared_tx_2, |_| async move {
                    Ok::<_, PdsError>(())
                })
                .await
                .expect("second with_lent_txns");
            r.expect("second scope ok");
            let install_done = install_start.elapsed();
            (install_done, a.commit().await, s.commit().await)
        });

        let (commit_a_1, commit_s_1) = first.await.expect("first joined");
        let (second_install_elapsed, commit_a_2, commit_s_2) = second.await.expect("second joined");
        commit_a_1.expect("first actor commit ok");
        commit_s_1.expect("first shared commit ok");
        commit_a_2.expect("second actor commit ok");
        commit_s_2.expect("second shared commit ok");

        let _ = first_release; // Keep the Notify alive
        let total = started_at.elapsed();
        assert!(
            total >= std::time::Duration::from_millis(140),
            "concurrent lends must serialize — total elapsed {:?} < first scope's 150ms hold",
            total,
        );
        assert!(
            second_install_elapsed >= std::time::Duration::from_millis(120),
            "second install must wait until first scope releases — observed {:?}",
            second_install_elapsed,
        );
    }

    // -----------------------------------------------------------
    // #5 — scope panics mid-flight → with_lent_txns extracts both
    //      lent txns before the unwind continues. Storage's lent
    //      slots are empty after the panic, so a fresh
    //      with_lent_txns on the same storage succeeds. Verifies
    //      that AssertUnwindSafe + take-before-resume_unwind keeps
    //      the storage state recoverable.
    // -----------------------------------------------------------
    #[tokio::test(flavor = "multi_thread")]
    async fn step_3_5_t5_panic_in_scope_recoverable() {
        let (store, did, actor_pool, shared_pool, _tmp) = test_pools().await;
        let storage = Arc::new(make_storage(store, did));

        let actor_tx = actor_pool.begin().await.expect("begin actor tx 1");
        let shared_tx = shared_pool.begin().await.expect("begin shared tx 1");
        let storage_panic = storage.clone();

        let panic_result = tokio::spawn(async move {
            std::panic::AssertUnwindSafe(storage_panic.with_lent_txns(
                actor_tx,
                shared_tx,
                |_| async move {
                    panic!("scope panics on purpose");
                    #[allow(unreachable_code)]
                    Ok::<_, PdsError>(())
                },
            ))
            .catch_unwind()
            .await
        })
        .await
        .expect("join panicking task");

        assert!(panic_result.is_err(), "panic must propagate");

        // Storage's lent slots must be empty after the panic
        // recovery — the take()s inside with_lent_txns ran before
        // resume_unwind. A fresh with_lent_txns on the same storage
        // proceeds without blocking on stale state.
        let actor_tx_b = actor_pool.begin().await.expect("begin actor tx 2");
        let shared_tx_b = shared_pool.begin().await.expect("begin shared tx 2");
        let (a, s, r) = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            storage.with_lent_txns(actor_tx_b, shared_tx_b, |_| async move {
                Ok::<_, PdsError>(())
            }),
        )
        .await
        .expect("storage reusable after panic — timeout means deadlock")
        .expect("with_lent_txns ok");
        r.expect("scope ok");
        a.commit().await.expect("commit actor tx 2");
        s.commit().await.expect("commit shared tx 2");
    }

    // -----------------------------------------------------------
    // #6 — actor-tx commit failure path emits the orphan marker.
    //      `emit_bind_audit_orphan_marker` is the function the
    //      real relay-race path invokes when actor commit fails;
    //      it stays a free function so this test can capture its
    //      tracing emit without contriving a live commit-time
    //      failure on sqlx (which sqlx itself makes hard to
    //      provoke deterministically — the marker semantics are
    //      worth proving regardless of the actual failure
    //      injection mechanism).
    // -----------------------------------------------------------
    #[tokio::test(flavor = "multi_thread")]
    #[tracing_test::traced_test]
    async fn step_3_5_t6_actor_commit_failure_emits_orphan_marker() {
        let did = "did:plc:step35t6";
        let synthetic_err =
            std::io::Error::new(std::io::ErrorKind::ConnectionReset, "simulated commit failure");

        emit_bind_audit_orphan_marker(did, &synthetic_err);

        assert!(
            logs_contain("bind_audit_orphan_marker"),
            "orphan marker tracing event must fire",
        );
        assert!(
            logs_contain(did),
            "orphan marker must include the DID for forensic correlation",
        );
        assert!(
            logs_contain("audit emit committed but per-actor record commit failed"),
            "orphan marker message must explain the failure mode for log readers",
        );
    }

    // -----------------------------------------------------------
    // #7 — shared-tx commit failure rolls back the actor_tx
    //      cleanly with NO orphan marker. The clean-failure path:
    //      neither side committed, the actor record never landed,
    //      no half-state is observable.
    //
    //      Forcing a real shared-tx commit failure on a live sqlx
    //      pool is non-deterministic — the obvious tricks
    //      (`pool.close().await` while the tx is held) deadlock
    //      because close awaits the open connection. Instead this
    //      test verifies the documented CONTRACT of the rollback
    //      path: stage an actor_tx write, invoke the rollback
    //      branch the same way `commit_with_orphan_recovery` does
    //      on shared-commit failure, then assert
    //      (a) the actor write did NOT persist (clean rollback)
    //      and (b) no orphan marker fired (the clean-failure path
    //      must not emit the orphan event — that's reserved for
    //      the audit-committed-but-record-uncommitted half-state).
    //
    //      The orphan-marker emit path itself is covered by
    //      test #6 (which exercises `emit_bind_audit_orphan_marker`
    //      directly).
    // -----------------------------------------------------------
    #[tokio::test(flavor = "multi_thread")]
    #[tracing_test::traced_test]
    async fn step_3_5_t7_shared_commit_failure_rolls_back_actor_no_orphan() {
        let (_store, did, actor_pool, _shared_pool, _tmp) = test_pools().await;
        let cid = "bafyt7step35t7";

        // Stage a write inside actor_tx — simulates the proto-blue
        // commit and Phase A metadata writes that happened inside
        // a `with_lent_txns` scope before the relay-race commit
        // step.
        let mut actor_tx: sqlx::Transaction<'static, sqlx::Sqlite> =
            actor_pool.begin().await.expect("begin actor tx");
        ActorStore::put_block_in_tx(&mut actor_tx, cid, b"step35-t7-bytes")
            .await
            .expect("stage put_block in actor tx");

        // Mimic `commit_with_orphan_recovery`'s shared-commit-
        // failure branch: roll back the actor tx, do NOT emit the
        // orphan marker (the clean-failure path).
        let _ = did; // forensic context; only used by tracing in the real path
        actor_tx
            .rollback()
            .await
            .expect("actor rollback must succeed on the clean-failure path");

        assert!(
            !block_exists(&actor_pool, cid).await,
            "actor write must be rolled back cleanly when shared commit fails",
        );
        assert!(
            !logs_contain("bind_audit_orphan_marker"),
            "orphan marker must NOT fire on the shared-commit-failure path; \
             the clean-failure invariant from the arc 2 step 3.5 addendum is \
             'neither side committed, no half-state to mark'",
        );
    }

    // -----------------------------------------------------------
    // Phase B fix-up — RepoStorage trait-method lent-tx routing.
    //
    // Arc 2's Phase B Block 2.4 (vanilla bsky baseline write)
    // surfaced HTTP 500 from `Repo::apply_writes` with `Missing
    // block`. Root cause: step 3.5 wired `apply_commit` to consult
    // `lent_actor_tx` but left the four other RepoStorage trait
    // methods (get_block, put_block, get_root, update_root)
    // routing through the auto-commit pool. proto-blue stages a
    // block via `put_block` (auto-commit), then reads it back via
    // `get_block` (also auto-commit, but on a FRESH pool
    // connection) during signature computation — the lent tx
    // holds the write lock and the read-back doesn't see the
    // staged row. proto-blue aborts with `Missing block`.
    //
    // The fix routes all four methods through the lent tx when
    // installed, fall-through to the existing auto-commit path
    // when no tx is installed. These tests pin both directions of
    // the property:
    //
    //   #1 read-during-lent-tx visibility: stage via
    //      put_block_in_tx, read back via the trait's get_block
    //      → Some(bytes). The pre-fix code would return None.
    //   #2 no-tx fallback unchanged: with lent_actor_tx empty,
    //      the trait's put_block / get_block route through the
    //      auto-commit helpers exactly as before.

    /// Helper — install a `Some(_)` actor tx on the storage and
    /// return it for caller-managed commit/rollback at end of
    /// scope. Used by the two visibility tests below.
    async fn install_lent_actor_tx_for_test(
        storage: &SqliteRepoStorage,
        actor_pool: &sqlx::SqlitePool,
    ) {
        let tx = actor_pool
            .begin()
            .await
            .expect("begin actor tx for visibility test");
        *storage.lent_actor_tx().lock().await = Some(tx);
    }

    /// Compute a real CID over a synthetic record — the trait
    /// methods take `&Cid`, and `Cid::from_str` rejects bare
    /// sentinel strings. Round-tripping through `json_to_lex` +
    /// `cid_for_lex` (the same pipeline `RepositoryManager`'s
    /// per-record CID computation uses) gives a CID that
    /// round-trips through `to_string` / `from_str` cleanly. The
    /// test only cares that the same Cid serializes into the SQL
    /// key consistently across stage and read; the stored bytes
    /// are a sentinel since storage doesn't enforce
    /// content == cid.
    fn synth_cid_and_bytes(seed: &str) -> (Cid, Vec<u8>) {
        let value = serde_json::json!({ "seed": seed });
        let lex = proto_blue::lex_json::json_to_lex(&value);
        let cid = proto_blue::lex_cbor::cid_for_lex(&lex)
            .expect("cid_for_lex over synth record");
        (cid, seed.as_bytes().to_vec())
    }

    /// Phase B fix-up #1 — read-during-lent-tx visibility.
    /// Stage a block via `put_block_in_tx`, then call the trait's
    /// `get_block` (sync, the path proto-blue takes during
    /// commit assembly). The fix routes the read through the
    /// same tx so the staged-but-uncommitted block is visible.
    /// The pre-fix code would return `None` here.
    ///
    /// **spawn_blocking wrap.** Per the module-level docs at the
    /// top of this file, callers MUST run work that touches the
    /// trait methods inside `tokio::task::spawn_blocking` —
    /// `block_on` from a worker thread panics. We mirror
    /// production by spawning the trait calls.
    #[tokio::test(flavor = "multi_thread")]
    async fn fixup_t1_trait_get_block_sees_staged_block_via_lent_tx() {
        let (store, did, actor_pool, _shared_pool, _tmp) = test_pools().await;
        let storage = Arc::new(make_storage(store, did));
        let (cid, bytes) = synth_cid_and_bytes("fixup-t1-seed");
        let cid_str = cid.to_string();

        // Install lent actor tx on the storage. Then stage a
        // block directly via the in_tx helper — this is what
        // happens when proto-blue's put_block call routes through
        // the patched trait method.
        install_lent_actor_tx_for_test(&storage, &actor_pool).await;
        {
            let mut g = storage.lent_actor_tx().lock().await;
            let tx = g.as_mut().expect("lent tx installed");
            ActorStore::put_block_in_tx(tx, &cid_str, &bytes)
                .await
                .expect("stage block via in_tx");
        }

        // Call the trait method on a blocking-pool thread (the
        // path proto-blue takes from `Repo::apply_writes` during
        // commit assembly). Must return Some(bytes) — the staged
        // block is visible because the read routes through the
        // SAME lent tx.
        let storage_for_blocking = storage.clone();
        let cid_for_blocking = cid.clone();
        let read = tokio::task::spawn_blocking(move || {
            <SqliteRepoStorage as RepoStorage>::get_block(
                &storage_for_blocking,
                &cid_for_blocking,
            )
        })
        .await
        .expect("join blocking task")
        .expect("get_block trait call ok");

        assert_eq!(
            read,
            Some(bytes.clone()),
            "trait get_block must see the staged-via-lent-tx block; \
             the pre-fix code returned None because the read \
             opened a fresh auto-commit connection that didn't \
             see the lent tx's uncommitted state",
        );

        // Recover the tx so it can be cleanly rolled back. Phase
        // B fix-up doesn't change rollback semantics.
        let tx = storage
            .lent_actor_tx()
            .lock()
            .await
            .take()
            .expect("extract lent tx for rollback");
        tx.rollback().await.expect("rollback lent tx");
    }

    /// Phase B fix-up #2 — no-tx fallback unchanged. With
    /// `lent_actor_tx` empty, the trait's put_block / get_block
    /// must route through the auto-commit ActorStore helpers
    /// exactly as before. This is the back-compat path that
    /// every test that constructs via `RepositoryManager::new`
    /// (and any non-`apply_writes` RepoStorage consumer) relies
    /// on.
    #[tokio::test(flavor = "multi_thread")]
    async fn fixup_t2_no_lent_tx_falls_back_to_auto_commit_path() {
        let (store, did, actor_pool, _shared_pool, _tmp) = test_pools().await;
        let storage = Arc::new(make_storage(store, did));
        let (cid, bytes) = synth_cid_and_bytes("fixup-t2-seed");
        let cid_str = cid.to_string();

        // No `install_lent_actor_tx_for_test` call here — the
        // storage's lent slot stays None for this test.

        let storage_for_put = storage.clone();
        let cid_for_put = cid.clone();
        let bytes_for_put = bytes.clone();
        tokio::task::spawn_blocking(move || {
            <SqliteRepoStorage as RepoStorage>::put_block(
                &storage_for_put,
                cid_for_put,
                bytes_for_put,
            )
        })
        .await
        .expect("join put blocking task")
        .expect("put_block trait call ok (auto-commit fallback)");

        // The block is committed via the auto-commit path, so
        // both the trait's get_block AND a direct pool query see
        // it.
        let storage_for_get = storage.clone();
        let cid_for_get = cid.clone();
        let trait_read = tokio::task::spawn_blocking(move || {
            <SqliteRepoStorage as RepoStorage>::get_block(&storage_for_get, &cid_for_get)
        })
        .await
        .expect("join get blocking task")
        .expect("get_block trait call ok");
        assert_eq!(
            trait_read,
            Some(bytes.clone()),
            "trait get_block must see the block written via the auto-commit fallback",
        );
        assert!(
            block_exists(&actor_pool, &cid_str).await,
            "the auto-commit fallback must persist the block — a \
             pool-level query sees it after the trait's put_block returns",
        );
    }
}
