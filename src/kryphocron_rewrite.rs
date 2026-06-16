//! v0.9 Arc D (#224) — rewrite-on-rotate background job.
//!
//! kryphocron 0.3 ships **no** rewrite-on-rotate driver (the substrate
//! provides the `ContentCodec` + `RotationOracle` primitives; the corpus walk
//! is the host's, per design §6.4.2). This module is that host-side job: an
//! operator triggers it via `triggerRotation`, and Aurora-Locus walks every
//! local account's `tools.kryphocron.feed.postPrivate` records, decoding each
//! under its write-time generation ([#237a's decode primitive][dec]) and
//! re-encoding it under the post-`force_rotation()` generation ([#236's encode
//! primitive][enc]), then re-storing it. The record's at-URI is stable; its
//! CID changes (the encoded bytes change) — exactly the design's "CIDs change,
//! at-URIs stable" contract.
//!
//! ## Mechanism
//!
//! Re-storing a re-encoded record is an `Update` [`WriteOp`] through
//! [`RepositoryManager::apply_writes`] under the account's own signer — the
//! only way to change a record's CID while preserving MST + commit integrity
//! and firehose contiguity. Updates are **batched per account** (≤ the
//! `apply_writes` 200-op cap) so a rewrite emits one `#commit` per batch
//! rather than one per record. A full rewrite re-signs every account's repo
//! and emits commit churn — expected per the design's bulk-op framing
//! (progress bar; "takes some time depending on record volume").
//!
//! ## Single-flight
//!
//! At most one rewrite runs per deployment ([`RewriteJob::try_start`] rejects a
//! concurrent trigger; the §6.4.2 "rotation already in progress" guard). A
//! crash mid-walk leaves a partial rewrite, which is safe: every record carries
//! its own generation and decodes correctly regardless, so the pass is
//! best-effort and retryable (design §7.2.3 "operators retry").
//!
//! Mid-walk **cancellation** is deferred to #225, where it lands together with
//! its only trigger — the `cancelRotation` XRPC — rather than shipping an
//! unused cancel hook here (the codebase's lib/bin dead-code discipline).
//!
//! ## Host-vocabulary bookkeeping (§16 D1)
//!
//! Rewrite events are **Aurora-Locus host vocabulary**, not substrate audit
//! events (kryphocron 0.3 has no host-emittable rewrite-event variants). Each
//! run appends `started` / `terminated` records to
//! `<data-dir>/aurora-locus/rewrite-history.log` (peer to the design's
//! `rotation-history.log` / `block-cascade.log`), and the completion timestamp
//! is persisted to `<data-dir>/aurora-locus/last-rewrite-completed.state` for
//! `getRotationStatus` to surface on the Laquna page.
//!
//! [dec]: crate::kryphocron_content::decode_private_content_with_hooks
//! [enc]: crate::kryphocron_content::encode_private_content_with_hooks

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use kryphocron::encryption::{RotationContext, RotationOracle as _};

use crate::actor_store::{repository::WriteOpAction, RepositoryManager, WriteOp};
use crate::api::kryphocron_endpoints::NSID_POST_PRIVATE;
use crate::api::repo::create_actor_signer;
use crate::context::AppContext;
use crate::error::{PdsError, PdsResult};
use crate::kryphocron::{CapabilityClass, KryphocronWriteAuthorization};
use crate::kryphocron_content::{
    decode_private_content_with_hooks, encode_private_content_with_hooks, ContentAuditSink,
    DecodeOutcome,
};

/// Max `Update` ops batched into one `apply_writes` commit (the `apply_writes`
/// per-commit cap is 200; 100 leaves headroom and bounds commit size).
const REWRITE_BATCH: usize = 100;
/// Accounts fetched per `list_accounts` page.
const ACCOUNT_PAGE: i64 = 200;
/// Records fetched per `list_records` page.
const RECORD_PAGE: i64 = 500;

/// Terminal outcome of a rewrite-on-rotate run (host vocabulary, §16 D1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RewriteOutcome {
    /// The walk finished across all accounts.
    Completed,
    /// The walk failed (e.g. account enumeration error); a partial rewrite may
    /// have committed. Safe to retry.
    Failed,
}

impl RewriteOutcome {
    fn as_str(self) -> &'static str {
        match self {
            RewriteOutcome::Completed => "completed",
            RewriteOutcome::Failed => "failed",
        }
    }
}

/// Live state of the rewrite job. The single-flight `running` flag + the
/// per-run counters the job needs to drive itself and write its terminal
/// bookkeeping. The observability *reads* over this (`getRotationProgress` /
/// `getRotationStatus`) land in #225 alongside their XRPCs — #224 ships the
/// write side (the job + its `rewrite-history.log` / `last-rewrite-completed`
/// artifacts); #225 ships the read side.
struct State {
    running: bool,
    processed: u64,
    rewritten: u64,
    generation_mark: Option<String>,
    started_at: Option<SystemTime>,
}

/// The deployment's single rewrite-on-rotate job: shared state + the
/// single-flight guard. Held in [`AppContext`].
pub struct RewriteJob {
    state: RwLock<State>,
    data_dir: PathBuf,
}

impl RewriteJob {
    /// Construct over the deployment `data_dir`.
    pub(crate) fn new(data_dir: PathBuf) -> Self {
        RewriteJob {
            state: RwLock::new(State {
                running: false,
                processed: 0,
                rewritten: 0,
                generation_mark: None,
                started_at: None,
            }),
            data_dir,
        }
    }

    /// The single-flight guard: mark the job running under the state lock and
    /// stamp the post-rotation generation. Returns `false` if a run is already
    /// in progress (no state change). Split out from [`Self::try_start`] so the
    /// guard is unit-testable without spawning the walk.
    fn begin(&self, generation_mark: Option<String>) -> bool {
        let mut st = self.state.write().expect("rewrite state lock not poisoned");
        if st.running {
            return false;
        }
        st.running = true;
        st.processed = 0;
        st.rewritten = 0;
        st.started_at = Some(SystemTime::now());
        st.generation_mark = generation_mark.clone();
        drop(st);
        append_history(&self.data_dir, "started", generation_mark.as_deref(), 0, 0, None);
        true
    }

    /// Start a rewrite-on-rotate run: rotate the slug, then spawn the corpus
    /// walk on a background task. Single-flight — returns `false` (no rotation,
    /// no spawn) if a run is already in progress, which the `triggerRotation`
    /// XRPC maps to the §6.4.2 "rotation already in progress" rejection.
    ///
    /// The slug rotation (`force_rotation()`) happens **inside** the guard, so a
    /// rejected trigger does not rotate the generation.
    pub(crate) fn try_start(self: &Arc<Self>, ctx: AppContext) -> bool {
        // Rotate + capture the new generation under the running-guard so a
        // concurrent trigger can neither double-rotate nor double-spawn.
        let generation_mark = {
            let _guard = self.state.read().expect("rewrite state lock not poisoned");
            if _guard.running {
                return false;
            }
            drop(_guard);
            if let Some(oracle) = &ctx.kryphocron_rotation_oracle {
                oracle.force_rotation();
                oracle
                    .current_generation(&RotationContext::for_install_probe())
                    .map(|m| m.to_string())
            } else {
                None
            }
        };
        if !self.begin(generation_mark) {
            return false;
        }

        let job = Arc::clone(self);
        tokio::spawn(async move {
            let outcome = job.run(&ctx).await;
            job.finish(outcome);
        });
        true
    }

    /// The corpus walk: every local account's `postPrivate` records, decoded
    /// then re-encoded under the new generation, batched per account.
    async fn run(&self, ctx: &AppContext) -> RewriteOutcome {
        let Some(hooks) = ctx.kryphocron_at_rest_hooks.clone() else {
            tracing::error!(
                target: "aurora_locus::kryphocron",
                "rewrite-on-rotate: no at-rest hooks installed; aborting",
            );
            return RewriteOutcome::Failed;
        };
        let sink = ContentAuditSink;

        let mut account_cursor: Option<String> = None;
        loop {
            let accounts = match ctx
                .account_manager
                .list_accounts(account_cursor.as_deref(), ACCOUNT_PAGE)
                .await
            {
                Ok(a) => a,
                Err(e) => {
                    tracing::error!(
                        target: "aurora_locus::kryphocron",
                        error = %e,
                        "rewrite-on-rotate: account enumeration failed; aborting",
                    );
                    return RewriteOutcome::Failed;
                }
            };
            if accounts.is_empty() {
                break;
            }
            let page_len = accounts.len();
            let next_cursor = accounts.last().map(|a| a.did.clone());

            for account in &accounts {
                if let Err(e) = self
                    .rewrite_account(ctx, hooks.as_ref(), &sink, &account.did)
                    .await
                {
                    // Per-account failure (missing signer for a non-hosted key,
                    // a mid-walk delete, a transient DB error) skips the account
                    // and continues — one account must not abort the whole run.
                    tracing::warn!(
                        target: "aurora_locus::kryphocron",
                        did = %account.did,
                        error = %e,
                        "rewrite-on-rotate: account skipped",
                    );
                }
            }

            if page_len < ACCOUNT_PAGE as usize {
                break;
            }
            account_cursor = next_cursor;
        }
        RewriteOutcome::Completed
    }

    /// Re-encode one account's `postPrivate` corpus, batching `Update`s.
    async fn rewrite_account(
        &self,
        ctx: &AppContext,
        hooks: &dyn kryphocron::encryption::AtRestHooks,
        sink: &ContentAuditSink,
        did: &str,
    ) -> PdsResult<()> {
        let signer = create_actor_signer(&ctx.account_manager, did).await?;
        let repo_mgr = RepositoryManager::for_writer(ctx, did.to_string());

        let mut record_cursor: Option<String> = None;
        let mut batch: Vec<WriteOp> = Vec::new();
        loop {
            let records = ctx
                .actor_store
                .list_records(did, NSID_POST_PRIVATE, RECORD_PAGE, record_cursor.as_deref())
                .await?;
            if records.is_empty() {
                break;
            }
            let page_len = records.len();
            let last_rkey = records.last().map(|r| r.rkey.clone());

            for rec in &records {
                self.bump_processed();
                match prepare_record_rewrite(ctx, hooks, sink, did, rec).await {
                    Ok(Some(write)) => {
                        batch.push(write);
                        if batch.len() >= REWRITE_BATCH {
                            let taken = std::mem::take(&mut batch);
                            self.flush_batch(&repo_mgr, &signer, taken).await?;
                        }
                    }
                    Ok(None) => {} // not encoded / nothing to rewrite — skip
                    Err(e) => {
                        // A single bad record (codec skew, malformed block) is
                        // skipped; it must not abort the account.
                        tracing::warn!(
                            target: "aurora_locus::kryphocron",
                            uri = %rec.uri,
                            error = %e,
                            "rewrite-on-rotate: record skipped",
                        );
                    }
                }
            }

            if page_len < RECORD_PAGE as usize {
                break;
            }
            record_cursor = last_rkey;
        }
        if !batch.is_empty() {
            self.flush_batch(&repo_mgr, &signer, batch).await?;
        }
        Ok(())
    }

    async fn flush_batch(
        &self,
        repo_mgr: &RepositoryManager,
        signer: &Arc<dyn proto_blue::crypto::Signer>,
        batch: Vec<WriteOp>,
    ) -> PdsResult<()> {
        let n = batch.len() as u64;
        repo_mgr
            .apply_writes(
                batch,
                signer.clone(),
                Arc::new(crate::blob_store::StrictPromoter),
            )
            .await?;
        let mut st = self.state.write().expect("rewrite state lock not poisoned");
        st.rewritten += n;
        Ok(())
    }

    fn bump_processed(&self) {
        let mut st = self.state.write().expect("rewrite state lock not poisoned");
        st.processed += 1;
    }

    /// Record terminal state + host-vocabulary bookkeeping at run end. Clears
    /// the running flag and persists the completion timestamp +
    /// `rewrite-history.log` `terminated` record (the read side — surfacing
    /// these on the Laquna page — is #225).
    fn finish(&self, outcome: RewriteOutcome) {
        let now = SystemTime::now();
        let (generation_mark, processed, rewritten, started_at) = {
            let mut st = self.state.write().expect("rewrite state lock not poisoned");
            st.running = false;
            (st.generation_mark.clone(), st.processed, st.rewritten, st.started_at)
        };
        write_last_completed(&self.data_dir, now);
        let duration_ms = started_at
            .and_then(|s| now.duration_since(s).ok())
            .map(|d| d.as_millis());
        append_history(
            &self.data_dir,
            "terminated",
            generation_mark.as_deref(),
            processed,
            rewritten,
            Some((outcome, duration_ms)),
        );
        tracing::info!(
            target: "aurora_locus::kryphocron",
            event = "rewrite_on_rotate_terminated",
            outcome = outcome.as_str(),
            rewritten,
            generation = generation_mark.as_deref().unwrap_or("-"),
            "rewrite-on-rotate run finished",
        );
    }
}

/// Decode one stored `postPrivate` record under its write-time generation and
/// re-encode it under the oracle's current (post-`force_rotation`) generation,
/// returning the `Update` [`WriteOp`] to re-store it — or `None` when the
/// record is not encoded (legacy `text` / non-encoded; out of rewrite scope).
async fn prepare_record_rewrite(
    ctx: &AppContext,
    hooks: &dyn kryphocron::encryption::AtRestHooks,
    sink: &ContentAuditSink,
    did: &str,
    rec: &crate::actor_store::models::Record,
) -> PdsResult<Option<WriteOp>> {
    let Some(block) = ctx.actor_store.get_block(did, &rec.cid).await? else {
        return Ok(None);
    };
    let lex = proto_blue::lex_cbor::decode(&block)
        .map_err(|e| PdsError::Internal(format!("rewrite: decode record block: {e}")))?;
    let mut value = proto_blue::lex_json::lex_to_json(&lex);

    let rewritten = rewrite_value(hooks, sink, did, &rec.rkey, &mut value).await?;
    if !rewritten {
        return Ok(None);
    }
    Ok(Some(WriteOp {
        action: WriteOpAction::Update,
        collection: NSID_POST_PRIVATE.to_string(),
        rkey: rec.rkey.clone(),
        value: Some(value),
        validate: None,
        swap_cid: None,
        kryphocron_authorization: Some(KryphocronWriteAuthorization::DedicatedEndpoint {
            capability_class: CapabilityClass::User,
        }),
    }))
}

/// The pure decode-then-re-encode core (no store): given a record value JSON,
/// decode its `encodedContent` (write-time generation) and re-encode under the
/// oracle's current generation, mutating `value` in place. Returns `true` when
/// the record was re-encoded, `false` when it carried no `encodedContent` (a
/// legacy `text` / non-encoded record — left untouched, out of rewrite scope).
async fn rewrite_value(
    hooks: &dyn kryphocron::encryption::AtRestHooks,
    sink: &ContentAuditSink,
    did: &str,
    rkey: &str,
    value: &mut serde_json::Value,
) -> PdsResult<bool> {
    match decode_private_content_with_hooks(hooks, did, NSID_POST_PRIVATE, rkey, value).await? {
        DecodeOutcome::NotEncoded => return Ok(false),
        DecodeOutcome::Decoded => {}
    }
    // Re-encode under the oracle's current (post-force_rotation) generation.
    let encoded =
        encode_private_content_with_hooks(hooks, sink, did, NSID_POST_PRIVATE, rkey, value).await?;
    Ok(encoded)
}

// ---- host-vocabulary bookkeeping files (under <data-dir>/aurora-locus/) ----

fn last_completed_path(data_dir: &Path) -> PathBuf {
    data_dir
        .join("aurora-locus")
        .join("last-rewrite-completed.state")
}

fn history_path(data_dir: &Path) -> PathBuf {
    data_dir.join("aurora-locus").join("rewrite-history.log")
}

/// Persist the completion timestamp (unix millis). Best-effort host bookkeeping.
fn write_last_completed(data_dir: &Path, at: SystemTime) {
    let path = last_completed_path(data_dir);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(ms) = at.duration_since(SystemTime::UNIX_EPOCH) {
        if let Err(e) = std::fs::write(&path, ms.as_millis().to_string()) {
            tracing::warn!(
                target: "aurora_locus::kryphocron",
                path = %path.display(),
                error = %e,
                "rewrite-on-rotate: failed to persist last-rewrite-completed",
            );
        }
    }
}

/// Append a `started` / `terminated` record to `rewrite-history.log` (JSONL).
/// Best-effort: a logging failure never affects the rewrite itself. (The read
/// side — `listRotations` reconstructing the operator-triggered track from this
/// log + `read_last_completed` for `getRotationStatus` — is #225.)
fn append_history(
    data_dir: &Path,
    kind: &str,
    generation: Option<&str>,
    processed: u64,
    rewritten: u64,
    terminal: Option<(RewriteOutcome, Option<u128>)>,
) {
    let path = history_path(data_dir);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let at_ms = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let mut entry = serde_json::json!({
        "at": at_ms,
        "kind": kind,
        "generation": generation,
        "processed": processed,
        "rewritten": rewritten,
    });
    if let Some((outcome, duration_ms)) = terminal {
        entry["outcome"] = serde_json::Value::String(outcome.as_str().to_string());
        if let Some(ms) = duration_ms {
            entry["durationMs"] = serde_json::json!(ms as u64);
        }
    }
    let line = format!("{entry}\n");
    match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        Ok(mut f) => {
            let _ = f.write_all(line.as_bytes());
        }
        Err(e) => tracing::warn!(
            target: "aurora_locus::kryphocron",
            path = %path.display(),
            error = %e,
            "rewrite-on-rotate: failed to append rewrite-history.log",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "aurora-locus-rewrite-{}-{}",
            std::process::id(),
            tag
        ))
    }

    struct DirGuard(PathBuf);
    impl Drop for DirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Hooks built around Aurora-Locus's own rotation oracle (which supports
    /// `force_rotation`), so a test can rotate the generation between encode
    /// and re-encode. Returns the hooks + the oracle + a dir guard.
    fn al_hooks(
        tag: &str,
    ) -> (
        Box<dyn kryphocron::encryption::AtRestHooks>,
        Arc<crate::kryphocron_rotation::AuroraLocusStandardRotationOracle>,
        DirGuard,
    ) {
        use kryphocron::encryption::RotationOracle;
        let dir = tmp_dir(tag);
        let oracle = Arc::new(
            crate::kryphocron_rotation::AuroraLocusStandardRotationOracle::for_data_dir(
                &dir,
                crate::kryphocron_rotation::Cadence::from_setting("daily"),
            )
            .expect("oracle"),
        );
        let hooks = kryphocron::encryption::DefaultAtRestHooks::builder(dir.clone())
            .with_rotation_oracle(oracle.clone() as Arc<dyn RotationOracle>)
            .build()
            .expect("hooks");
        (Box::new(hooks), oracle, DirGuard(dir))
    }

    const DID: &str = "did:plc:exampleexampleexample";
    const RKEY: &str = "3kabcdefghij2";

    #[tokio::test]
    async fn rewrite_value_re_encodes_under_new_generation_preserving_plaintext() {
        let (hooks, oracle, _g) = al_hooks("reencode");
        let sink = ContentAuditSink;

        // Encode under generation A.
        let mut record = serde_json::json!({ "$type": NSID_POST_PRIVATE, "text": "secret payload" });
        encode_private_content_with_hooks(hooks.as_ref(), &sink, DID, NSID_POST_PRIVATE, RKEY, &mut record)
            .await
            .unwrap();
        let bytes_a = record["encodedContent"]["$bytes"].as_str().unwrap().to_string();
        let gen_a = record["encodedContentGeneration"].as_str().map(str::to_string);

        // Rotate to generation B, then rewrite.
        oracle.force_rotation();
        let rewritten = rewrite_value(hooks.as_ref(), &sink, DID, RKEY, &mut record)
            .await
            .unwrap();
        assert!(rewritten, "an encoded record must be re-encoded");

        let bytes_b = record["encodedContent"]["$bytes"].as_str().unwrap().to_string();
        let gen_b = record["encodedContentGeneration"].as_str().map(str::to_string);
        // The generation mark moved and the encoded bytes changed...
        assert_ne!(gen_a, gen_b, "generation must advance across the rewrite");
        assert_ne!(bytes_a, bytes_b, "encoded bytes must change under the new generation");

        // ...but the record still decodes to the original plaintext.
        let outcome =
            decode_private_content_with_hooks(hooks.as_ref(), DID, NSID_POST_PRIVATE, RKEY, &mut record)
                .await
                .unwrap();
        assert_eq!(outcome, DecodeOutcome::Decoded);
        assert_eq!(record["text"].as_str(), Some("secret payload"));
    }

    #[tokio::test]
    async fn rewrite_value_skips_non_encoded_record() {
        let (hooks, _oracle, _g) = al_hooks("skip");
        let sink = ContentAuditSink;
        // A legacy text-only record (no encodedContent) is out of rewrite scope.
        let mut record = serde_json::json!({ "$type": NSID_POST_PRIVATE, "text": "legacy" });
        let before = record.clone();
        let rewritten = rewrite_value(hooks.as_ref(), &sink, DID, RKEY, &mut record)
            .await
            .unwrap();
        assert!(!rewritten);
        assert_eq!(record, before, "non-encoded record left untouched");
    }

    /// Read the private `running` flag — tests are in-module, so they observe
    /// state directly (the `getRotationProgress` accessor is #225).
    fn running(job: &RewriteJob) -> bool {
        job.state.read().unwrap().running
    }

    #[test]
    fn begin_is_single_flight() {
        let _g = DirGuard(tmp_dir("singleflight"));
        let job = RewriteJob::new(tmp_dir("singleflight"));
        assert!(!running(&job));
        assert!(job.begin(Some("laquna/1/aa".into())), "first begin starts");
        assert!(!job.begin(Some("laquna/1/bb".into())), "second begin is rejected");
        assert!(running(&job));
        assert_eq!(
            job.state.read().unwrap().generation_mark.as_deref(),
            Some("laquna/1/aa")
        );
    }

    #[test]
    fn finish_clears_running_and_persists_bookkeeping() {
        let dir = tmp_dir("persist");
        let _g = DirGuard(dir.clone());
        let job = RewriteJob::new(dir.clone());
        assert!(job.begin(Some("laquna/2/cd".into())));
        job.finish(RewriteOutcome::Completed);

        assert!(!running(&job), "finish clears the running flag");

        // last-rewrite-completed.state is persisted (read side surfaces it in #225).
        assert!(last_completed_path(&dir).exists());

        // rewrite-history.log carries the started + terminated records.
        let log = std::fs::read_to_string(history_path(&dir)).unwrap();
        assert!(log.contains("\"kind\":\"started\""));
        assert!(log.contains("\"kind\":\"terminated\""));
        assert!(log.contains("\"outcome\":\"completed\""));
    }
}
