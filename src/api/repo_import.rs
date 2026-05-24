//! Arc 16f §9.6.3.1 — `com.atproto.repo.importRepo` handler.
//!
//! Ties together Step 1 (TOLERANT helper + promoter), Step 2
//! (origin-blob-fetch primitive), and the existing
//! `apply_writes` commit machinery. The handler:
//!
//! 1. Authenticates and resolves the importing DID's per-account
//!    signer via the [`account_emit.rs:59`] template (CF3 Path 1).
//! 2. Acquires a single-flight lock keyed on the importing DID
//!    (in-process actor-keyed mutex — see
//!    [`SingleFlightImportLock`] for the v0.5 posture vs the
//!    v0.6+ cross-process Postgres-advisory variant).
//! 3. Re-checks the `accepting_imports` drain switch.
//! 4. Streams the CAR body with a `max_import_size` cap;
//!    decodes via [`proto_blue_repo::verify_diff_car`] with
//!    `signing_did_key = Some(<importing DID's signing-key
//!    did:key>)` — that single call covers both structural
//!    verification AND commit-chain signature verification
//!    (CF1: §9.6.3.1 step 4 is subsumed).
//! 5. Validates DID match between the CAR's root commit and
//!    the importing DID.
//! 6. Walks extracted blob CIDs against `blob_quarantine` +
//!    [`Cid::is_dasl_compliant`] *before* any Phase A commit
//!    (atomic-reject closure for round-1 F1).
//! 7. Converts the diff into `Vec<WriteOp>` with
//!    `validate: Some(false)` (records pre-validated by origin).
//! 8. Runs the §9.6.3.5 fetch-and-retry loop via
//!    [`import_with_fetch_retry`]. The loop drives apply_writes
//!    through a closure so Step 4's promoter parameter drops in
//!    cleanly without changing loop logic — see [skydeval]'s
//!    sequencing note in the closure-injection seam below.
//!
//! ## CF5 anonymous origin fetch
//!
//! The fetch-retry loop calls Step 2's
//! [`crate::federation::blob_fetch::fetch_blob_from_origin`]
//! as-is. That primitive sends NO `Authorization` header —
//! the CF5 invariant pinned in
//! [`docs/internal/v05-recon/V05_ARC16F_CF5_RECON.md`].
//!
//! ## Sequencing — Step 4 promoter seam
//!
//! Arc 16f Step 4 extends `apply_writes` to take an
//! `Arc<dyn BlobPromoter>` parameter and signal `NeedsBlobFetch`
//! when TOLERANT's [`crate::blob_store::store::verify_blob_tolerant_or_signal`]
//! reports a row-absent CID. Until that lands, today's
//! `apply_writes` always uses STRICT semantics (returns
//! `BlobNotFound` for un-staged blobs, never `NeedsBlobFetch`).
//!
//! [`import_with_fetch_retry`] takes the apply_writes call as
//! a `FnMut() -> Future` closure. Production callers wrap
//! `repo_mgr.apply_writes(writes.clone(), signer.clone())` in
//! a closure today; when Step 4 lands, the same closure body
//! gains `Arc::new(TolerantPromoter)` as the third arg with
//! no other change to this module.
//!
//! ## Lock posture (in-process for v0.5)
//!
//! [`SingleFlightImportLock`] is an in-process actor-keyed
//! mutex map. This handles single-process deployments
//! (the documented v0.5 posture). Multi-process HA deployments
//! sharing one Postgres would need the kickoff's
//! `pg_try_advisory_lock(SHA256("aurora-locus.import_repo." +
//! did))` variant — tracked as a v0.6+ hardening follow-up
//! against this module (see [skydeval]'s Step 3 report-back).

#![allow(dead_code)] // Route is wired below; some helpers are exposed for Step 4/5 wiring.

use crate::actor_store::{RepositoryManager, WriteOp, WriteOpAction};
use crate::api::middleware;
use crate::blob_store::quarantine::BlobQuarantine;
use crate::blob_store::store::QuarantinePublicReason;
use crate::context::AppContext;
use crate::crypto::proto_blue_signer::RepoSigner;
use crate::error::{PdsError, PdsResult};
use crate::oauth::AtProtoScope;
use crate::repository::blob_refs::extract_blob_cids;
use axum::{
    body::Body,
    extract::State,
    http::HeaderMap,
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use futures::StreamExt;
use proto_blue::crypto::{Keypair, Signer};
use proto_blue::lex_data::Cid;
use proto_blue::repo::{verify_diff_car, VerifiedDiff};
use serde::Serialize;
use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};
use tracing::{debug, info, warn};

// ============================================================
// Public route registration
// ============================================================

/// Build the importRepo route. Merged into `src/api/repo.rs`'s
/// top-level `routes()` via `.merge()` so the kickoff's
/// "register `/xrpc/com.atproto.repo.importRepo`" contract is
/// fulfilled without enlarging `repo.rs` (already 1100+ LOC).
pub fn routes() -> Router<AppContext> {
    Router::new().route("/xrpc/com.atproto.repo.importRepo", post(import_repo))
}

// ============================================================
// Handler
// ============================================================

/// `com.atproto.repo.importRepo` — apply an exported CAR to the
/// authenticated actor's local repository.
///
/// # Errors
///
/// Aurora-owned error vocabulary
/// (`tools.aurora.repo.importRepo` namespace per CF2 — rustdoc
/// + `docs/AURORA_ENDPOINT_INVENTORY.md`, NOT a JSON lexicon):
///
/// - `ActorNotInitialized` (400) — no `plc_keys` row for the
///   authenticated DID (createAccount prerequisite missing).
/// - `ConcurrentMutation` (409) — another importRepo for the
///   same DID is in flight. Try-acquire + fail-fast.
/// - `InvalidCar` (400) — CAR root DID doesn't match authed
///   DID, CAR body fails structural decode, or body exceeds
///   `service.max_import_size`.
/// - `InvalidCommitSignature` (400) — `verify_diff_car`
///   rejected the commit chain against the importing DID's
///   signing key (CF1: same call as decode).
/// - `InvalidCid` (400) — a record body references a malformed
///   or non-DASL-compliant CID (Arc 16e validate-phase walker).
/// - `QuarantinedBlobReferenced` (400) — a record body
///   references a blob CID present in `blob_quarantine` at
///   validate-phase. Wire payload carries coarse
///   `public_reason` only (round-1 F20).
/// - `BlobTooLarge` (413) — fetched blob from origin exceeded
///   `service.max_blob_fetch_size`.
/// - `OriginFetchClientError` (502) — origin PDS returned 4xx
///   for a blob fetch (durable; no retry).
/// - `OriginFetchExhausted` (502) — fetch-and-retry loop
///   exceeded `service.blob_fetch_max_retries`, or one or
///   more CIDs failed terminally inside a round.
/// - `503 Service Unavailable` — `service.accepting_imports`
///   is `false` (operator drain switch).
async fn import_repo(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    body: Body,
) -> Result<axum::response::Response, PdsError> {
    let auth = middleware::require_auth_unified(State(ctx.clone()), headers.clone()).await?;
    middleware::enforce_scope(&auth, &AtProtoScope::RepoAll)?;
    let importing_did = auth.did().to_string();

    // §9.6.3.9 unified forensic-rejection emit. Inner body returns
    // `PdsResult<Response>`; every `?`-propagation surfaces here as a
    // single `emit_rejected` call. Pre-refactor, 6 inner Err paths
    // skipped the forensic emit (Phase B Scenario 11 discovery), which
    // left Scenarios 6/14/10 unable to assert `import_repo_rejected`
    // even when the wire response was correct. Outer-level emit
    // restores the canonical-trail invariant: every Err path produces
    // exactly one rejected event. The `accepting_imports=false` drain
    // path is the one inline-emit exception inside `import_repo_inner`
    // because it returns `Ok(503)` (the wire-shape design choice
    // documented at §9.6.3.1 step 2), so the outer Err arm doesn't
    // see it.
    match import_repo_inner(&ctx, body, &importing_did).await {
        Ok(resp) => Ok(resp),
        Err(err) => {
            let kind = error_kind_label(&err);
            // §9.6.3.5 / #129 — surface aggregated per-CID failures into
            // the forensic log when the inner path collected them. Without
            // this extract, the wire response correctly ships
            // `per_cid_failures` (e.g. 3 entries) while the structured log
            // event reports `per_cid_failure_count: 0`, leaving operators
            // grepping logs blind to the failure shape that the client
            // already saw.
            let failures_for_log: &[(Cid, String)] = match &err {
                PdsError::OriginFetchExhausted { per_cid_failures } => {
                    per_cid_failures.as_slice()
                }
                _ => &[],
            };
            emit_rejected(&importing_did, kind, 0, failures_for_log);
            Err(err)
        }
    }
}

async fn import_repo_inner(
    ctx: &AppContext,
    body: Body,
    importing_did: &str,
) -> PdsResult<axum::response::Response> {
    // CF3 — signer resolution gate (account_emit.rs:59 template).
    // Resolve BEFORE acquiring the lock so a missing actor fails fast
    // without holding it.
    let signer: Arc<dyn Signer> =
        match ctx.account_manager.get_atproto_signing_key_bytes(importing_did).await {
            Ok(key_bytes) => Arc::new(
                RepoSigner::from_bytes(&key_bytes).map_err(|e| {
                    PdsError::Internal(format!(
                        "importRepo signer construction failed: {}",
                        e
                    ))
                })?,
            ),
            Err(PdsError::NotFound(_)) => return Err(PdsError::ActorNotInitialized),
            Err(other) => return Err(other),
        };

    // Single-flight lock keyed on importing_did. Acquired AFTER signer
    // resolution so missing accounts fail fast.
    let _lock_guard = match import_lock_registry().try_acquire(importing_did) {
        Some(g) => g,
        None => {
            warn!(did = %importing_did, "importRepo concurrent mutation rejected");
            return Err(PdsError::ConcurrentMutation);
        }
    };

    // accepting_imports drain switch — special: returns Ok(503), not
    // Err, so this is the one path that inline-emits rejected because
    // the outer handler's Err arm won't see it. Checked INSIDE the
    // lock so operators can flip the switch and let in-flight imports
    // finish without serving new ones.
    if !ctx.config.service.accepting_imports {
        emit_rejected(importing_did, "ServiceUnavailable", 0, &[]);
        let status = axum::http::StatusCode::SERVICE_UNAVAILABLE;
        let payload = serde_json::json!({
            "error": "ServiceUnavailable",
            "message": "importRepo is disabled (service.accepting_imports = false)",
        });
        return Ok((status, Json(payload)).into_response());
    }

    // Arc 16f Step 3 v5.2 (chainlink #123) — ensure the per-actor
    // SQLite store exists. Idempotent: `CREATE TABLE IF NOT EXISTS` +
    // `create_dir_all`. Materialises the store on first import for a
    // seeded-but-uninitialised DID (the v0.5 federation-into-fresh-
    // instance case). Without it, apply_writes → proto-blue Repo →
    // SqliteRepoStorage::put_block → ActorStore::open_db fails
    // NotFound for any DID whose actor store wasn't initialised
    // through createAccount.
    ctx.actor_store.create(importing_did).await?;

    // CAR body read with streaming size-bound enforcement (round-1
    // F21). Chunks accumulate; first chunk that pushes the total
    // past `max_import_size` aborts the read.
    let car_bytes = read_body_with_cap(body, ctx.config.service.max_import_size).await?;
    let car_size = car_bytes.len();
    debug!(
        did = %importing_did,
        car_size,
        "importRepo CAR body buffered"
    );

    // CF1 — verify_diff_car decode + commit-chain sig verify in
    // ONE call. proto-blue 0.3.2's verify_diff_car takes
    // `signing_did_key: Option<&str>`; when Some, the call verifies
    // signatures against that did:key. The §9.6.3.1 step 4 explicit
    // verification step is SUBSUMED.
    //
    // For Step 3 self-import (auth.did == importing_did): pull the
    // signing key did:key from the same account_manager surface as
    // the local signer. This works because Aurora's accounts are
    // PLC-rooted: the per-account `plc_keys.atproto_signing_key`
    // bytes are the very key whose did:key form is published in
    // the DID document at `verificationMethods.atproto`. When v0.6+
    // adds cross-account import (a different DID importing into a
    // local actor — currently unsupported by the entry-gate
    // self-only constraint), this should switch to the importing
    // DID's PLC-resolved did:key history.
    let signing_did_key = public_did_key_for(ctx, importing_did).await?;

    let verified_diff = match verify_diff_car(
        &car_bytes,
        None, // No prior-repo snapshot — Aurora's v0.5 importRepo
              // applies imported commits as additive diffs against
              // an empty MST. v0.6+ may pass the current repo's
              // VerifiedRepo here to support incremental import.
        Some(importing_did),
        Some(&signing_did_key),
    ) {
        Ok(d) => d,
        Err(e) => {
            // proto-blue's RepoError covers structural failures (CAR
            // decode, MST load) AND signature failures. Disambiguate
            // via the error message — proto-blue signature failures
            // carry "signature" in the displayed string per its
            // error vocabulary. (Chainlink #120 tracks the v0.6+
            // upstream-discriminated-variant cleanup.)
            let msg = e.to_string();
            let lower = msg.to_lowercase();
            return Err(if lower.contains("signature") || lower.contains("signing") {
                PdsError::InvalidCommitSignature
            } else {
                PdsError::InvalidCar(format!("CAR decode failed: {}", msg))
            });
        }
    };

    // DID match: CAR root commit's DID MUST equal the authenticated
    // importing_did. The verify_diff_car call above already passes
    // `expected_did = Some(importing_did)`, which proto-blue
    // enforces — so a mismatch here would surface as a decode
    // error above. This second check is defensive (catches future
    // proto-blue changes that might relax the check).
    if verified_diff.repo.commit.did != importing_did {
        return Err(PdsError::InvalidCar(format!(
            "CAR root commit DID {} does not match importing DID {}",
            verified_diff.repo.commit.did, importing_did
        )));
    }

    // Diff → WriteOp + blob_cid extraction.
    let (writes, blob_cids) = diff_to_writes(&verified_diff, importing_did)?;
    let prepared_write_count = writes.len();
    let validate_phase_cid_count = blob_cids.len();

    // Validate-phase quarantine + DASL gate (round-1 F1). Returns
    // `Err(QuarantinedBlobReferenced)` on hit; outer handler emits
    // rejected.
    validate_phase_blob_check(ctx, importing_did, &blob_cids).await?;

    // §9.6.3.9 import_repo_starting — fires HERE, after all gates +
    // validate-phase have passed. All four design-spec fields are now
    // in scope (importing_did + car_size_bytes + prepared_write_count
    // + validate_phase_cid_count). Crossing this emit means the
    // import body is actually entering the apply_writes loop;
    // rejections before this point produce `rejected`-only forensic
    // trails (no `starting`). Discovered during Phase B Scenario 11
    // (chainlink #121, 2026-05-21) — the prior at-handler-entry
    // placement violated the design's "starting = crossed-validation-
    // threshold" invariant.
    info!(
        event = "import_repo_starting",
        did = %importing_did,
        car_size_bytes = car_size,
        prepared_write_count,
        validate_phase_cid_count,
        "importRepo entering apply_writes loop"
    );

    // Pooled reqwest::Client (round-1 F11) — single instance per
    // handler invocation, used for the pre-fetch loop below.
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(
            ctx.config.service.blob_fetch_timeout_seconds,
        ))
        .build()
        .map_err(|e| {
            PdsError::Internal(format!("reqwest client construction failed: {}", e))
        })?;

    let started = std::time::Instant::now();

    // Step 3 v5.3 (chainlink #124) — PRE-FETCH missing blobs BEFORE
    // apply_writes. Closes the multi-blob retry-into-Phase-A-collision
    // surfaced by Phase B Scenario 13: §9.6.3.5's "Phase A re-commit
    // on retry is idempotent" claim applies to Aurora's `put_record`
    // SQL but NOT to proto-blue's `Repo::apply_writes` MST commit
    // (which rejects Create-on-existing-key). After the first
    // apply_writes attempt's Phase A committed records to per-actor
    // SQLite, a NeedsBlobFetch retry would re-run Phase A against a
    // repo that ALREADY contains those keys, failing
    // `Key already exists: <collection>/<rkey>`.
    //
    // By staging all missing blobs BEFORE apply_writes, the
    // TolerantPromoter Phase B inner loop sees every CID present and
    // returns `Done` for each — no NeedsBlobFetch fires on the happy
    // path, no retry, no Phase A collision. TolerantPromoter stays
    // wired as defense-in-depth for the narrow race window where a
    // blob's metadata row vanishes between pre-fetch and Phase B
    // (concurrent admin action — vanishingly rare); a post-pre-fetch
    // NeedsBlobFetch surfaces as Internal (signals a real bug).
    // Arc 16f Step 3 v5.4 (chainlink #127) — COLLECT-ALL-FAILURES.
    // Pre-v5.4 the pre-fetch loop used `?`-propagation, short-
    // circuiting on the first per-CID failure — silently superseding
    // §9.6.3.5's drain-all-CIDs-then-aggregate invariant. Restored
    // here so federation operators see the full failure scope (X, Y,
    // Z all missing from origin) in one envelope rather than
    // retrying to rediscover one CID at a time.
    //
    // Mechanics: iterate every absent blob_cid, collect Ok/Err per
    // CID into pre_fetch_results, then aggregate via
    // aggregate_per_cid_failures. Successful pre-fetches stage to
    // blob_metadata + disk normally; if the round ends with non-empty
    // failures, the import returns `OriginFetchExhausted`. Successful
    // fetches in a failed round persist as untethered blob_metadata
    // rows (will be reused on retry via the get_metadata-found-present
    // skip, or cleaned by Arc 16d row-sweep if abandoned). Same Option
    // A dual-DB posture already accepted everywhere else in v0.5.
    //
    // The `?` on `get_metadata` is preserved — those are Database/Io
    // errors, not per-CID origin failures, and should fail-fast.
    let fetcher = crate::federation::blob_fetch::HttpOriginBlobFetcher;
    let mut pre_fetch_results: Vec<(Cid, PdsResult<()>)> = Vec::new();
    for cid in &blob_cids {
        let cid_str = cid.to_string();
        let already_present = ctx.blob_store.get_metadata(&cid_str).await?.is_some();
        if already_present {
            continue;
        }
        let result = fetch_and_stage_one(
            ctx,
            &client,
            &ctx.blob_store,
            importing_did,
            cid,
            &fetcher,
        )
        .await;
        pre_fetch_results.push((cid.clone(), result));
    }
    let fetched_blob_count = aggregate_per_cid_failures(pre_fetch_results)?;

    // apply_writes runs ONCE with all blobs staged. TolerantPromoter
    // returns Done for each present blob; no NeedsBlobFetch on the
    // happy path.
    //
    // §17.4-Step-4 / #136 — importRepo is THE site where §17.3.4's
    // `validate_imports` override fires. Without `for_writer` (which
    // plumbs the lexicon resolver + config snapshot), the
    // `lexicon_config = None` branch at validate-phase entry leaves
    // the override unreachable from the import path — every CAR-
    // imported record bypasses validation regardless of what
    // `PDS_LEXICON_VALIDATE_IMPORTS` is set to. Phase B Scenario 16
    // depends on this.
    let mgr = RepositoryManager::for_writer(ctx, importing_did.to_string());

    let outcome = mgr
        .apply_writes(writes, signer, Arc::new(crate::blob_store::TolerantPromoter))
        .await;

    let total_duration_ms = started.elapsed().as_millis() as u64;

    let (commit_cid, rev) = match outcome {
        Ok(success) => success,
        Err(PdsError::NeedsBlobFetch { cids }) => {
            // Defense-in-depth: pre-fetch staged all required blobs,
            // so a post-pre-fetch NeedsBlobFetch indicates concurrent
            // blob_metadata mutation (e.g. admin quarantine racing the
            // import) OR a pre-fetch logic bug. Surface as Internal
            // rather than retrying — a second apply_writes would hit
            // the Phase A retry-collision this v5.3 fix exists to
            // prevent.
            return Err(PdsError::Internal(format!(
                "apply_writes returned NeedsBlobFetch after pre-fetch staged {} blob(s); \
                 likely concurrent blob_metadata mutation. unstaged cids: {:?}",
                fetched_blob_count, cids
            )));
        }
        Err(other) => return Err(other),
    };

    // §9.6.3.9 import_repo_complete — fires on success path only.
    // Apply-time Err propagates to the outer `import_repo` handler
    // which emits `import_repo_rejected` via the unified outer arm.
    //
    // fetched_blob_count: number of blobs the pre-fetch loop pulled
    // from origin (excludes already-locally-present blobs that
    // skipped the fetch). Post-v5.3 (chainlink #124) this is
    // accurate-by-construction; closes chainlink #122 (the prior
    // hardcoded placeholder).
    //
    // fetch_round_count: 1 if the pre-fetch ran (legacy field name
    // preserved for operator-dashboard compat; the retry-loop
    // accounting it tracked pre-v5.3 is no longer meaningful since
    // the import is a single pass now).
    info!(
        event = "import_repo_complete",
        did = %importing_did,
        car_size_bytes = car_size,
        prepared_write_count,
        fetched_blob_count,
        fetch_round_count = 1u32,
        total_duration_ms,
        %commit_cid,
        %rev,
        "importRepo handler complete"
    );
    Ok(Json(serde_json::json!({
        "commit": { "cid": commit_cid, "rev": rev }
    }))
    .into_response())
}

// ============================================================
// Helpers
// ============================================================

/// Read the request body into a single `Vec<u8>`, aborting if the
/// running total exceeds `max_import_size`. `None` cap disables the
/// check (dev posture). On overflow returns `PdsError::InvalidCar`
/// with a size-mentioning message — see the variant doc-comment for
/// the v0.6+ 413-vs-400 split.
async fn read_body_with_cap(
    body: Body,
    max_import_size: Option<u64>,
) -> PdsResult<Vec<u8>> {
    let mut buf: Vec<u8> = Vec::new();
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| {
            PdsError::Internal(format!("body stream read failed: {}", e))
        })?;
        let next_len = buf.len() as u64 + chunk.len() as u64;
        if let Some(cap) = max_import_size {
            if next_len > cap {
                return Err(PdsError::InvalidCar(format!(
                    "CAR body exceeds max_import_size (cap {} bytes, would be {} bytes)",
                    cap, next_len
                )));
            }
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

/// Resolve the `did:key` form of the importing DID's signing key
/// for `verify_diff_car`'s `signing_did_key` argument.
///
/// v0.5 posture: self-import only (the entry gate constrains
/// `importing_did == auth.did`). The signing key bytes come from
/// the same `plc_keys` row that the local signer was constructed
/// from; converting to `did:key` form matches the publication
/// format in the DID document's `verificationMethods.atproto`.
///
/// v0.6+ cross-account import would resolve this via the
/// importing DID's PLC-resolved signing-key history (multi-key
/// rotation aware) — see [skydeval]'s Step 3 report-back.
async fn public_did_key_for(ctx: &AppContext, did: &str) -> PdsResult<String> {
    let key_bytes = ctx.account_manager.get_atproto_signing_key_bytes(did).await?;
    let kp = proto_blue::crypto::K256Keypair::from_private_key(&key_bytes).map_err(|e| {
        PdsError::Internal(format!(
            "importRepo public_did_key construction failed for {}: {}",
            did, e
        ))
    })?;
    Ok(kp.did())
}

/// Convert a [`VerifiedDiff`] into the matching `Vec<WriteOp>` for
/// `apply_writes` and the union of all blob CIDs referenced by
/// every Create/Update record body in the batch.
///
/// Each diff entry maps to one `WriteOp` with `validate: Some(false)`
/// (records were pre-validated by the origin PDS; re-running
/// lexicon validation would slow imports without catching anything
/// the origin didn't already accept) and `swap_cid: None`.
fn diff_to_writes(
    verified_diff: &VerifiedDiff,
    importing_did: &str,
) -> PdsResult<(Vec<WriteOp>, Vec<Cid>)> {
    let _ = importing_did; // Reserved for future per-write provenance logging.

    let mut writes: Vec<WriteOp> = Vec::with_capacity(
        verified_diff.diff.adds.len()
            + verified_diff.diff.updates.len()
            + verified_diff.diff.deletes.len(),
    );
    let mut all_blob_cids: Vec<Cid> = Vec::new();

    for add in verified_diff.diff.adds.values() {
        let (collection, rkey) = split_record_key(&add.key)?;
        let value_lex = verified_diff
            .repo
            .get_record(&add.cid)
            .map_err(|e| PdsError::InvalidCar(format!("CAR record block load failed: {}", e)))?
            .ok_or_else(|| {
                PdsError::InvalidCar(format!(
                    "CAR record block missing for key {} (cid {})",
                    add.key, add.cid
                ))
            })?;
        let value_json = lex_to_json(&value_lex)?;
        let cids = extract_blob_cids(&value_json)?;
        all_blob_cids.extend(cids);
        writes.push(WriteOp {
            action: WriteOpAction::Create,
            collection,
            rkey,
            value: Some(value_json),
            validate: Some(false),
            swap_cid: None,
        });
    }

    for upd in verified_diff.diff.updates.values() {
        let (collection, rkey) = split_record_key(&upd.key)?;
        let value_lex = verified_diff
            .repo
            .get_record(&upd.cid)
            .map_err(|e| PdsError::InvalidCar(format!("CAR record block load failed: {}", e)))?
            .ok_or_else(|| {
                PdsError::InvalidCar(format!(
                    "CAR record block missing for key {} (cid {})",
                    upd.key, upd.cid
                ))
            })?;
        let value_json = lex_to_json(&value_lex)?;
        let cids = extract_blob_cids(&value_json)?;
        all_blob_cids.extend(cids);
        writes.push(WriteOp {
            action: WriteOpAction::Update,
            collection,
            rkey,
            value: Some(value_json),
            validate: Some(false),
            swap_cid: None,
        });
    }

    for del in verified_diff.diff.deletes.values() {
        let (collection, rkey) = split_record_key(&del.key)?;
        writes.push(WriteOp {
            action: WriteOpAction::Delete,
            collection,
            rkey,
            value: None,
            validate: Some(false),
            swap_cid: None,
        });
    }

    // Dedupe blob CIDs — a single blob can be referenced from
    // multiple records in one diff, and `extract_blob_cids` doesn't
    // deduplicate across records.
    let mut seen: HashSet<String> = HashSet::new();
    all_blob_cids.retain(|c| seen.insert(c.to_string()));

    Ok((writes, all_blob_cids))
}

fn split_record_key(key: &str) -> PdsResult<(String, String)> {
    let (collection, rkey) = key.split_once('/').ok_or_else(|| {
        PdsError::InvalidCar(format!(
            "CAR record key {} is not in `<collection>/<rkey>` form",
            key
        ))
    })?;
    Ok((collection.to_string(), rkey.to_string()))
}

fn lex_to_json(value: &proto_blue::lex_data::LexValue) -> PdsResult<serde_json::Value> {
    // `proto_blue::lex_json::lex_to_json` returns a JSON Value directly
    // (infallible at the type level — LexValue is by-construction a
    // valid lex shape). Wrapper kept as `PdsResult` so a future
    // signature change to fallible doesn't break callers.
    Ok(proto_blue::lex_json::lex_to_json(value))
}

/// Validate-phase blob check (round-1 F1 closure). DASL compliance
/// is enforced by `extract_blob_cids` upstream (its walker rejects
/// non-DASL CIDs with `PdsError::InvalidCid`), so by the time this
/// runs the CIDs are known-compliant. The remaining gate is the
/// blob_quarantine read-only check per CID; any hit aborts BEFORE
/// any Phase A commit fires.
///
/// Reads the operator-internal `blob_quarantine.reason` column and
/// maps to the coarse [`QuarantinePublicReason`] for the wire
/// payload (round-1 F20 — operator-internal reason NEVER reaches
/// the client).
async fn validate_phase_blob_check(
    ctx: &AppContext,
    importing_did: &str,
    blob_cids: &[Cid],
) -> PdsResult<()> {
    // `get_quarantine` already filters `restored_at IS NULL`, so a
    // `Some(_)` outcome means an active quarantine hit.
    let quarantine = BlobQuarantine::new(ctx.account_db.clone());
    for cid in blob_cids {
        let cid_str = cid.to_string();
        match quarantine.get_quarantine(&cid_str).await {
            Ok(Some(rec)) => {
                warn!(
                                did = %importing_did,
                    cid = %cid_str,
                    reason_class = ?rec.reason,
                    "importRepo validate-phase: CID quarantined"
                );
                return Err(PdsError::QuarantinedBlobReferenced {
                    cid: cid.clone(),
                    public_reason:
                        QuarantinePublicReason::from_internal_reason_str(rec.reason.as_str()),
                });
            }
            Ok(None) => {}
            Err(e) => {
                return Err(PdsError::Internal(format!(
                    "validate-phase quarantine query failed for {}: {}",
                    cid_str, e
                )));
            }
        }
    }
    Ok(())
}

// ============================================================
// Fetch-and-retry loop (§9.6.3.5) — closure-injected for testability
// ============================================================

/// §9.6.3.5 outer fetch-and-retry loop with collect-all-failures
/// semantics within a single round.
///
/// Both apply_writes and fetch-and-stage are closure-injected so
/// the loop itself doesn't touch `AppContext`. Production callers
/// capture `AppContext` inside the closures; tests inject mocks
/// that don't reach any AppContext-internal state. The closures'
/// return types match the underlying primitives'.
///
/// Outer loop bound is `max_retries`. Each iteration:
/// 1. Call `do_writes_attempt()`.
/// 2. On `Ok` → return success.
/// 3. On `Err(NeedsBlobFetch { cids })`:
///    - Drain ALL `cids` via `stage_one(cid)` — no `?` propagation;
///      collect per-CID results.
///    - If any CID failed in this round → terminate with
///      [`PdsError::OriginFetchExhausted`].
///    - If `retry_count >= max_retries` → terminate with
///      `OriginFetchExhausted` carrying the most recent CID set.
///    - Otherwise increment `retry_count` and retry.
/// 4. On any other `Err(...)` → propagate.
pub(crate) async fn import_with_fetch_retry<F, Fut, G, GFut>(
    max_retries: u32,
    mut do_writes_attempt: F,
    mut stage_one: G,
) -> PdsResult<(String, String)>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = PdsResult<(String, String)>>,
    G: FnMut(Cid) -> GFut,
    GFut: std::future::Future<Output = PdsResult<()>>,
{
    let mut retry_count: u32 = 0;
    loop {
        match do_writes_attempt().await {
            Ok(success) => return Ok(success),
            Err(PdsError::NeedsBlobFetch { cids }) => {
                if cids.is_empty() {
                    // Defense-in-depth: empty NeedsBlobFetch is a
                    // promoter bug. Surface as 500 so it's visible.
                    return Err(PdsError::Internal(
                        "apply_writes returned NeedsBlobFetch with no CIDs".to_string(),
                    ));
                }
                if retry_count >= max_retries {
                    let exhausted: Vec<(Cid, String)> = cids
                        .into_iter()
                        .map(|c| {
                            (
                                c,
                                format!(
                                    "outer fetch-retry budget exhausted after {} rounds",
                                    max_retries
                                ),
                            )
                        })
                        .collect();
                    return Err(PdsError::OriginFetchExhausted {
                        per_cid_failures: exhausted,
                    });
                }
                let mut per_cid_results: Vec<(Cid, PdsResult<()>)> =
                    Vec::with_capacity(cids.len());
                for cid in &cids {
                    let result = stage_one(cid.clone()).await;
                    per_cid_results.push((cid.clone(), result));
                }
                let failures: Vec<(Cid, String)> = per_cid_results
                    .into_iter()
                    .filter_map(|(c, r)| r.err().map(|e| (c, e.to_string())))
                    .collect();
                if !failures.is_empty() {
                    return Err(PdsError::OriginFetchExhausted {
                        per_cid_failures: failures,
                    });
                }
                retry_count += 1;
            }
            Err(other) => return Err(other),
        }
    }
}

/// Arc 16f Step 3 v5.4 (chainlink #127) — drain a vec of per-CID
/// fetch results and either return the success count or surface
/// `OriginFetchExhausted` with the structured `per_cid_failures`
/// payload per §9.6.3.5.
///
/// Pulled out as a helper so the collect-all-failures aggregation
/// logic is unit-testable in isolation from the production
/// pre-fetch loop's async I/O. Inputs are content-free
/// `(Cid, PdsResult<()>)` tuples; outputs are either an
/// `Ok(success_count)` for the all-clean case or
/// `Err(PdsError::OriginFetchExhausted { per_cid_failures })` with
/// ONLY the failed CIDs listed (not short-circuit-first-failure).
/// Successful fetches in a failed round are still counted in the
/// production code path; this helper just answers "should we
/// surface OriginFetchExhausted with the per-CID context?"
fn aggregate_per_cid_failures(
    results: Vec<(Cid, PdsResult<()>)>,
) -> PdsResult<u32> {
    let mut success_count: u32 = 0;
    let mut per_cid_failures: Vec<(Cid, String)> = Vec::new();
    for (cid, result) in results {
        match result {
            Ok(()) => success_count += 1,
            Err(e) => per_cid_failures.push((cid, e.to_string())),
        }
    }
    if !per_cid_failures.is_empty() {
        return Err(PdsError::OriginFetchExhausted { per_cid_failures });
    }
    Ok(success_count)
}

/// One-CID fetch + stage + commit per §9.6.3.5 pseudocode. Reuses
/// Arc 16c's `stage_blob` + `commit_blob` pipeline so the fetched
/// bytes land in the same final-position-then-metadata ordering as
/// uploadBlob.
async fn fetch_and_stage_one(
    ctx: &AppContext,
    client: &reqwest::Client,
    blob_store: &Arc<crate::blob_store::BlobStore>,
    importing_did: &str,
    cid: &Cid,
    fetcher: &dyn crate::federation::blob_fetch::OriginBlobFetcher,
) -> PdsResult<()> {
    let bytes = fetcher.fetch(ctx, client, importing_did, cid).await?;
    let cid_str = cid.to_string();
    let mime = crate::blob_store::mime::detect_mime_type_from_data(&bytes)
        .unwrap_or("application/octet-stream");
    let _staged = blob_store.stage_blob(bytes, Some(mime), importing_did).await?;
    blob_store.commit_blob(&cid_str).await?;
    Ok(())
}

// ============================================================
// Single-flight lock (in-process actor-keyed mutex, v0.5 posture)
// ============================================================

/// Process-global lock registry. The OnceLock pattern lets every
/// importRepo invocation hit the same in-memory state without
/// threading a registry handle through `AppContext` (which would
/// expand the test-fixture surface considerably). For multi-process
/// HA deployments — out of scope for v0.5 — the kickoff's
/// `pg_try_advisory_lock(SHA-256("aurora-locus.import_repo." +
/// did))` variant replaces this. Tracked as a v0.6+ hardening
/// chainlink.
fn import_lock_registry() -> &'static SingleFlightImportLock {
    static REGISTRY: OnceLock<SingleFlightImportLock> = OnceLock::new();
    REGISTRY.get_or_init(SingleFlightImportLock::new)
}

pub(crate) struct SingleFlightImportLock {
    in_flight: Mutex<HashSet<String>>,
}

impl SingleFlightImportLock {
    pub(crate) fn new() -> Self {
        Self {
            in_flight: Mutex::new(HashSet::new()),
        }
    }

    /// Try-acquire. Returns `Some(guard)` if the DID was free; the
    /// guard's `Drop` impl releases the slot. Returns `None` if
    /// another importRepo for the same DID is in flight — caller
    /// should return `PdsError::ConcurrentMutation`.
    pub(crate) fn try_acquire(&'static self, did: &str) -> Option<ImportLockGuard> {
        let mut set = self.in_flight.lock().expect("lock poisoned");
        if set.contains(did) {
            return None;
        }
        set.insert(did.to_string());
        Some(ImportLockGuard {
            registry: self,
            did: did.to_string(),
        })
    }
}

pub(crate) struct ImportLockGuard {
    registry: &'static SingleFlightImportLock,
    did: String,
}

impl Drop for ImportLockGuard {
    fn drop(&mut self) {
        let mut set = self.registry.in_flight.lock().expect("lock poisoned");
        set.remove(&self.did);
    }
}

// ============================================================
// Forensic logging (§9.6.3.9)
// ============================================================

#[derive(Serialize)]
struct RejectedEvent<'a> {
    event: &'a str,
    did: &'a str,
    rejection_reason: &'a str,
    car_size_bytes: usize,
    per_cid_failures: &'a [(Cid, String)],
}

fn emit_rejected(
    did: &str,
    rejection_reason: &str,
    car_size: usize,
    per_cid_failures: &[(Cid, String)],
) {
    let evt = RejectedEvent {
        event: "import_repo_rejected",
        did,
        rejection_reason,
        car_size_bytes: car_size,
        per_cid_failures,
    };
    info!(
        event = evt.event,
        did = %evt.did,
        rejection_reason = %evt.rejection_reason,
        car_size_bytes = evt.car_size_bytes,
        per_cid_failure_count = evt.per_cid_failures.len(),
        "importRepo rejected"
    );
    // Per Arc 16e §9.5.3.1.3 — flush stdout so structured emissions
    // land immediately even if the process aborts shortly after.
    use std::io::Write as _;
    let _ = std::io::stdout().lock().flush();
}

fn error_kind_label(err: &PdsError) -> &'static str {
    match err {
        PdsError::ActorNotInitialized => "ActorNotInitialized",
        PdsError::ConcurrentMutation => "ConcurrentMutation",
        PdsError::InvalidCar(_) => "InvalidCar",
        PdsError::InvalidCommitSignature => "InvalidCommitSignature",
        PdsError::InvalidCid(_) => "InvalidCid",
        PdsError::QuarantinedBlobReferenced { .. } => "QuarantinedBlobReferenced",
        PdsError::BlobTooLarge { .. } => "BlobTooLarge",
        PdsError::OriginFetchClientError { .. } => "OriginFetchClientError",
        PdsError::OriginFetchExhausted { .. } => "OriginFetchExhausted",
        PdsError::Validation(_) => "Validation",
        PdsError::Authorization(_) | PdsError::Authentication(_) => "AuthError",
        _ => "Internal",
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    
    use std::sync::atomic::{AtomicU32, Ordering};

    fn test_cid() -> Cid {
        use std::str::FromStr;
        Cid::from_str("bafkreigh2akiscaildcqabsyg3dfr6chu3fgpregiymsck7e7aqa4s52zy")
            .expect("valid CIDv1 raw multibase")
    }

    // ------------------------------------------------------------
    // v5.4 pre-fetch aggregate-per-cid-failures (chainlink #127)
    // ------------------------------------------------------------

    /// Phase B Scenario 5b scope-trace (2026-05-22) found that v5.3's
    /// pre-fetch loop short-circuited on the first per-CID failure
    /// via `?`-propagation — silently superseding §9.6.3.5's
    /// drain-all-CIDs-then-aggregate invariant. v5.4 restores
    /// collect-all-failures via the `aggregate_per_cid_failures`
    /// helper. These tests pin three invariants:
    ///   1. All Ok results → returns success count.
    ///   2. All Err results → returns `OriginFetchExhausted` listing
    ///      ALL CIDs (no truncation).
    ///   3. Mixed Ok/Err → returns `OriginFetchExhausted` with
    ///      ONLY the failed CIDs (success count is preserved as
    ///      production-side state; the failure surface lists what
    ///      went wrong, not what worked).
    ///
    /// Aggregation property is content-free — no async, no AppContext,
    /// no live fetch — so this is unit-testable in isolation.
    #[test]
    fn aggregate_per_cid_failures_all_ok_returns_success_count() {
        let cid_a = Cid::for_raw(b"agg-test-a");
        let cid_b = Cid::for_raw(b"agg-test-b");
        let cid_c = Cid::for_raw(b"agg-test-c");
        let results = vec![
            (cid_a, Ok(())),
            (cid_b, Ok(())),
            (cid_c, Ok(())),
        ];
        let count = aggregate_per_cid_failures(results).expect("all-ok must succeed");
        assert_eq!(count, 3, "all 3 CIDs counted as successful fetches");
    }

    #[test]
    fn aggregate_per_cid_failures_all_err_surfaces_origin_fetch_exhausted_with_all_cids() {
        let cid_a = Cid::for_raw(b"agg-fail-a");
        let cid_b = Cid::for_raw(b"agg-fail-b");
        let cid_c = Cid::for_raw(b"agg-fail-c");
        let results = vec![
            (
                cid_a.clone(),
                Err(PdsError::OriginFetchClientError {
                    cid: cid_a.clone(),
                    status_or_reason: "origin returned 503".to_string(),
                }),
            ),
            (
                cid_b.clone(),
                Err(PdsError::OriginFetchClientError {
                    cid: cid_b.clone(),
                    status_or_reason: "exhausted after 4 attempts: connection refused".to_string(),
                }),
            ),
            (
                cid_c.clone(),
                Err(PdsError::OriginFetchClientError {
                    cid: cid_c.clone(),
                    status_or_reason: "origin returned 404".to_string(),
                }),
            ),
        ];
        let err = aggregate_per_cid_failures(results).expect_err("all-err must surface");
        match err {
            PdsError::OriginFetchExhausted { per_cid_failures } => {
                assert_eq!(
                    per_cid_failures.len(),
                    3,
                    "ALL 3 failed CIDs must land in per_cid_failures \
                     (collect-all-failures invariant — chainlink #127). \
                     Short-circuit-first-failure would return only 1."
                );
                // Each reason carries through verbatim (no truncation,
                // no canonicalisation). Operator dashboards key on the
                // raw reason string.
                let reasons: Vec<&str> =
                    per_cid_failures.iter().map(|(_, r)| r.as_str()).collect();
                assert!(reasons.iter().any(|r| r.contains("503")));
                assert!(reasons.iter().any(|r| r.contains("exhausted")));
                assert!(reasons.iter().any(|r| r.contains("404")));
            }
            other => panic!("expected OriginFetchExhausted, got {:?}", other),
        }
    }

    #[test]
    fn aggregate_per_cid_failures_mixed_lists_only_failed_cids() {
        // The §9.6.3.5 invariant Scenario 5b was designed to test:
        // some CIDs succeed in the round, some fail; the wire surface
        // lists ONLY the failures, NOT the successes.
        let cid_failed_a = Cid::for_raw(b"agg-mixed-a-fail");
        let cid_succeeded_b = Cid::for_raw(b"agg-mixed-b-succ");
        let cid_failed_c = Cid::for_raw(b"agg-mixed-c-fail");
        let results = vec![
            (
                cid_failed_a.clone(),
                Err(PdsError::OriginFetchClientError {
                    cid: cid_failed_a.clone(),
                    status_or_reason: "origin returned 503".to_string(),
                }),
            ),
            (cid_succeeded_b.clone(), Ok(())),
            (
                cid_failed_c.clone(),
                Err(PdsError::OriginFetchClientError {
                    cid: cid_failed_c.clone(),
                    status_or_reason: "origin returned 503".to_string(),
                }),
            ),
        ];
        let err = aggregate_per_cid_failures(results).expect_err("mixed must surface");
        match err {
            PdsError::OriginFetchExhausted { per_cid_failures } => {
                assert_eq!(
                    per_cid_failures.len(),
                    2,
                    "ONLY the 2 failed CIDs land in per_cid_failures \
                     (success cid_b is preserved as production state, \
                     not in the wire failure surface)"
                );
                // No-success-leakage check: the successful CID must
                // not appear in any failure-tuple position.
                for (failed_cid, reason) in &per_cid_failures {
                    assert_ne!(
                        failed_cid, &cid_succeeded_b,
                        "successful CID must not appear in per_cid_failures"
                    );
                    assert!(
                        reason.contains("503"),
                        "failure reason must be the actual failure, not synthesised: {}",
                        reason
                    );
                }
            }
            other => panic!("expected OriginFetchExhausted, got {:?}", other),
        }
    }

    // ------------------------------------------------------------
    // CF1 signature-verification routing (chainlink #120 brittleness)
    // ------------------------------------------------------------

    /// Phase B Scenario 7 discovered (2026-05-22) that the `dd`-corrupt
    /// weak form CANNOT reach `verify_diff_car`'s signature-verification
    /// path — proto-blue rejects the byte-flipped CAR at the CID-hash
    /// integrity check first (`RepoError::CidMismatch`), well before
    /// signature verification runs. "Structurally valid + wrong
    /// signature" is unreachable via byte corruption alone.
    ///
    /// To exercise the SIGNATURE path proper, this unit test
    /// constructs a structurally-valid CAR signed by key_A then asks
    /// `verify_diff_car` to verify it against key_B's `did:key` (a
    /// different, valid key). proto-blue should reject with
    /// `RepoError::InvalidSignature`, whose `Display` impl is
    /// `"Invalid signature on commit"`. Aurora's routing at
    /// [`import_repo_inner`]'s `verify_diff_car` Err arm string-matches
    /// the `Display` text for `signature`/`signing` → routes to
    /// `PdsError::InvalidCommitSignature`. If proto-blue's `Display`
    /// wording changes in a future bump (chainlink #120), the routing
    /// silently degrades to `InvalidCar` and federation verification
    /// loses its wire-distinguishable signal — this test fails loud.
    ///
    /// Covers what the weak-form Phase B couldn't: the end-to-end
    /// SDK→wire-code routing for the wrong-signer case.
    #[tokio::test(flavor = "multi_thread")]
    async fn verify_diff_car_wrong_key_displays_signature_keyword_for_aurora_routing() {
        use proto_blue::crypto::{K256Keypair, Keypair};
        use proto_blue::lex_cbor::cid_for_lex;
        use proto_blue::lex_data::LexValue;
        use proto_blue::repo::{
            blocks_to_car, sign_commit, BlockMap, MstNode, UnsignedCommit,
        };

        // Build a CAR signed by key_A with key_A's DID embedded.
        let kp_a = K256Keypair::generate();
        let mut mst = MstNode::empty();
        let mut blocks = BlockMap::new();
        let record_key = "app.bsky.feed.post/3jzfcijpj2z2a";
        let record_value = LexValue::String("payload".into());
        let record_cid = cid_for_lex(&record_value).unwrap();
        blocks.add_value(&record_value).unwrap();
        mst = mst.add(record_key, record_cid).unwrap();
        let (mst_root, mst_blocks) = mst.get_all_blocks().unwrap();
        blocks.add_map(&mst_blocks);
        let unsigned =
            UnsignedCommit::new(kp_a.did(), mst_root, "3jzfcijpj2z2a".to_string(), None);
        let signed = sign_commit(&unsigned, &kp_a).unwrap();
        let commit_cid = signed.cid().unwrap();
        blocks.set(commit_cid.clone(), signed.to_cbor().unwrap());
        let car = blocks_to_car(Some(&commit_cid), &blocks).unwrap();

        // Verify against a DIFFERENT key's did:key. proto-blue must
        // reject because the embedded signature was made with key_A
        // but verification expects key_B.
        let kp_b = K256Keypair::generate();
        let wrong_did_key = kp_b.did();
        assert_ne!(kp_a.did(), wrong_did_key, "two distinct keys");

        let result = verify_diff_car(
            &car,
            None,
            Some(&kp_a.did()),
            Some(&wrong_did_key),
        );
        let err = result.expect_err("wrong-key CAR must be rejected");
        let display = err.to_string();
        let lower = display.to_lowercase();

        // The load-bearing routing assertion: Aurora at
        // import_repo_inner's verify_diff_car Err arm matches on
        // `lower.contains("signature") || lower.contains("signing")`
        // to route to PdsError::InvalidCommitSignature. proto-blue's
        // RepoError::InvalidSignature Display ("Invalid signature on
        // commit") satisfies this — but if a future proto-blue bump
        // changes the wording (e.g. "signed payload rejected"),
        // Aurora silently routes wrong-key failures to InvalidCar,
        // producing wire-indistinguishable confusion at the federation
        // surface. This test fails loud on that change.
        assert!(
            lower.contains("signature") || lower.contains("signing"),
            "proto-blue's wrong-key error Display must contain `signature` \
             or `signing` for Aurora's `verify_diff_car` Err routing to \
             map it to InvalidCommitSignature. Got: {}\n\nIf proto-blue's \
             Display wording changed, update import_repo_inner's string-\
             match heuristic (chainlink #120) — or take the upstream-\
             discriminated-variant fix.",
            display
        );
    }

    // ------------------------------------------------------------
    // Forensic tracing target convention (post-bare-target regression)
    // ------------------------------------------------------------

    /// Regression test for the Phase B Scenario 2 forensic-logging
    /// bypass discovered 2026-05-21: `tracing::info!` / `warn!` /
    /// `debug!` invocations in this module previously used
    /// `target: "import_repo"` — a bare-string target that doesn't
    /// match the standard `aurora_locus=info` `EnvFilter` and is
    /// silently dropped. The fix: remove the `target:` override
    /// entirely, so emits use the default module-path target
    /// (`aurora_locus::api::repo_import`) which matches the
    /// `aurora_locus=info` prefix rule.
    ///
    /// This test reads this file's source at compile time and asserts
    /// no `target: "..."` override exists with a target that does
    /// not start with `aurora_locus` (the prefix the standard
    /// RUST_LOG filter matches). A future contributor who
    /// re-introduces `target: "import_repo"` (or any other
    /// non-aurora_locus target) gets a loud test failure rather
    /// than silently broken production observability.
    #[test]
    fn no_tracing_target_overrides_outside_aurora_locus_namespace() {
        let src = include_str!("repo_import.rs");
        let mut violations: Vec<String> = Vec::new();
        for (lineno_zero, line) in src.lines().enumerate() {
            // Skip comment lines (including the doc-comment ON THIS
            // TEST, which legitimately contains the bad literal as
            // documentation). Aurora-Locus's formatted style puts
            // tracing-macro args at uniform indentation; the line's
            // first non-whitespace char distinguishes code from
            // comment.
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            // Match a `target: "..."` macro-arg form on a CODE line.
            if let Some(after) = line.split("target: \"").nth(1) {
                if let Some(value) = after.split('"').next() {
                    if !value.starts_with("aurora_locus") {
                        violations.push(format!(
                            "line {}: target=\"{}\"",
                            lineno_zero + 1,
                            value
                        ));
                    }
                }
            }
        }
        assert!(
            violations.is_empty(),
            "repo_import.rs has tracing target overrides outside the \
             aurora_locus::* namespace. These bypass the standard \
             `aurora_locus=info` EnvFilter and would silently drop \
             forensic events. Remove the `target:` override (default \
             module-path target matches the filter) or use an \
             `aurora_locus::*`-prefixed target.\n\nViolations:\n{}",
            violations.join("\n")
        );
    }

    // ------------------------------------------------------------
    // §9.6.3 v5.3 — pre-fetch eliminates retry-into-Phase-A-collision
    // ------------------------------------------------------------

    /// Regression test for the Phase B Scenario 13 discovery
    /// (chainlink #124, 2026-05-22): multi-blob imports produced
    /// "Key already exists" on proto-blue's MST.add because the
    /// `import_with_fetch_retry` loop re-ran `apply_writes`'s Phase A
    /// after the first attempt's MST commit had persisted. The v5.3
    /// fix moves blob staging out of the retry loop and BEFORE
    /// `apply_writes`, so the handler calls apply_writes exactly once
    /// per import with all blobs already locally staged.
    ///
    /// This test asserts the structural property: `import_repo_inner`
    /// does NOT invoke `import_with_fetch_retry`. The retry-loop
    /// function still exists for the existing in-isolation unit
    /// tests (it's defensible code that just isn't used in
    /// production anymore), but if it ever reappears in the
    /// production handler body, the retry-into-Phase-A-collision
    /// bug-class returns.
    #[test]
    fn import_repo_inner_does_not_invoke_retry_loop() {
        let src = include_str!("repo_import.rs");
        let inner_marker = "async fn import_repo_inner(";
        let inner_start = src
            .find(inner_marker)
            .expect("import_repo_inner not found in repo_import.rs");
        // Find the next top-level `async fn` declaration as the
        // function boundary (sufficient because all helpers below
        // import_repo_inner are `fn` or `async fn` at module scope).
        let after_inner = &src[inner_start + inner_marker.len()..];
        let inner_end = after_inner
            .find("\nasync fn ")
            .or_else(|| after_inner.find("\nfn "))
            .unwrap_or(after_inner.len());
        let inner_body = &after_inner[..inner_end];

        assert!(
            !inner_body.contains("import_with_fetch_retry("),
            "Step 3 v5.3 (chainlink #124): import_repo_inner must NOT invoke \
             import_with_fetch_retry. Pre-fetch architecture moves blob \
             staging in front of apply_writes so the handler calls apply_writes \
             exactly once per import; re-introducing the retry loop reopens \
             the Phase A retry-collision on multi-blob imports (proto-blue's \
             MST.add rejects Create-on-existing-key after Phase A of attempt \
             1 has already persisted to per-actor SQLite)."
        );
    }

    // ------------------------------------------------------------
    // §9.6.3.9 import_repo_starting placement + payload invariants
    // ------------------------------------------------------------

    /// Asserts V05_DESIGN.md §9.6.3.9's `import_repo_starting` design:
    ///
    /// **Placement**: the emit fires AFTER `validate_phase_blob_check`
    /// returns (and therefore after every other gate too). Crossing
    /// `starting` means the import body has passed all validation
    /// gates and is entering the apply_writes loop. Refused imports
    /// (signer NotFound, lock contention, accepting_imports=false,
    /// decode failure, signature failure, quarantine, etc.) produce
    /// a `rejected`-only forensic trail with NO `starting` line.
    ///
    /// **Payload**: the emit carries all four design-spec fields —
    /// `importing_did`, `car_size_bytes`, `prepared_write_count`,
    /// `validate_phase_cid_count` (§9.6.3.9 verbatim). Pre-fix the
    /// emit at handler entry only carried `did`, violating the spec.
    ///
    /// Discovered by Phase B Scenario 11 (chainlink #121, 2026-05-21):
    /// a refused-at-`accepting_imports=false` import emitted
    /// `starting + rejected` instead of `rejected` only, because the
    /// pre-fix code emitted at handler entry before any gate ran.
    #[test]
    fn import_repo_starting_design_invariants() {
        let src = include_str!("repo_import.rs");
        let lines: Vec<&str> = src.lines().collect();

        // Skip comment lines (including this doc-comment, which
        // legitimately contains the literals as documentation).
        let code_line_match = |needle: &str| -> Option<usize> {
            lines.iter().enumerate().find_map(|(i, line)| {
                let trimmed = line.trim_start();
                if trimmed.starts_with("//") {
                    return None;
                }
                if line.contains(needle) {
                    Some(i)
                } else {
                    None
                }
            })
        };

        let starting_idx = code_line_match("event = \"import_repo_starting\"")
            .expect("import_repo_starting emit not found in repo_import.rs");
        let validate_idx = code_line_match("validate_phase_blob_check(")
            .expect("validate_phase_blob_check invocation not found");

        assert!(
            starting_idx > validate_idx,
            "§9.6.3.9 invariant violated: import_repo_starting (line {}) must fire \
             AFTER validate_phase_blob_check (line {}). Pre-validate-phase \
             placement emits `starting` for refused-at-validate imports, breaking \
             the design's `starting = crossed-validation-threshold` semantic. \
             Phase B Scenario 11 (chainlink #121) discovery.",
            starting_idx + 1,
            validate_idx + 1
        );

        // The info! block follows event= for ~5-10 lines.
        let block: String = lines[starting_idx..]
            .iter()
            .take(10)
            .copied()
            .collect::<Vec<_>>()
            .join("\n");
        for field in &["car_size_bytes", "prepared_write_count", "validate_phase_cid_count"] {
            assert!(
                block.contains(field),
                "§9.6.3.9 invariant violated: import_repo_starting must carry \
                 design-spec field `{}`. Pre-fix emit at handler entry only \
                 carried `did`, dropping the other 3 fields. Source block:\n{}",
                field,
                block
            );
        }
    }

    // ------------------------------------------------------------
    // SingleFlightImportLock
    // ------------------------------------------------------------

    fn lock_for_tests() -> &'static SingleFlightImportLock {
        // Standalone instance per test — sidesteps process-globals
        // bleeding across tests when run in parallel. Cast to
        // 'static via Box::leak — fine for tests; the instance
        // outlives the test runner.
        Box::leak(Box::new(SingleFlightImportLock::new()))
    }

    #[test]
    fn single_flight_lock_blocks_concurrent_acquisition_for_same_did() {
        let lock = lock_for_tests();
        let g1 = lock.try_acquire("did:plc:alice").expect("first acquire ok");
        assert!(
            lock.try_acquire("did:plc:alice").is_none(),
            "second acquire for same DID must fail"
        );
        drop(g1);
        assert!(
            lock.try_acquire("did:plc:alice").is_some(),
            "after drop, re-acquire must succeed"
        );
    }

    #[test]
    fn single_flight_lock_permits_distinct_dids_in_parallel() {
        let lock = lock_for_tests();
        let _alice = lock.try_acquire("did:plc:alice").expect("alice");
        let _bob = lock.try_acquire("did:plc:bob").expect("bob");
        // Both held simultaneously — drop on scope exit.
    }

    // ------------------------------------------------------------
    // import_with_fetch_retry — closure-injected loop
    // ------------------------------------------------------------
    //
    // The loop signature now takes BOTH apply_writes and
    // stage_one as closures, so tests don't need AppContext at
    // all. The fetch-and-stage path (which DOES need a live
    // BlobStore) is covered by Phase B (Step 5).

    #[tokio::test]
    async fn fetch_retry_loop_returns_immediately_on_success() {
        let attempts = AtomicU32::new(0);
        let stage_calls = AtomicU32::new(0);
        let result = import_with_fetch_retry(
            3,
            || {
                attempts.fetch_add(1, Ordering::SeqCst);
                async { Ok(("commit-cid".to_string(), "rev-1".to_string())) }
            },
            |_cid: Cid| {
                stage_calls.fetch_add(1, Ordering::SeqCst);
                async { Ok(()) }
            },
        )
        .await;
        let (cid, rev) = result.expect("success");
        assert_eq!(cid, "commit-cid");
        assert_eq!(rev, "rev-1");
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_eq!(stage_calls.load(Ordering::SeqCst), 0, "no fetch on success");
    }

    #[tokio::test]
    async fn fetch_retry_loop_exhausts_with_collect_all_failures() {
        // do_writes_attempt always returns NeedsBlobFetch with 2 CIDs;
        // stage_one fails every CID. After max_retries=2 the loop
        // terminates with OriginFetchExhausted containing BOTH CIDs'
        // failure reasons aggregated (no short-circuit on first).
        let cid_a = test_cid();
        let cid_b = test_cid();
        let attempts = AtomicU32::new(0);
        let stage_calls = AtomicU32::new(0);
        let cid_a_clone = cid_a.clone();
        let cid_b_clone = cid_b.clone();
        let result = import_with_fetch_retry(
            2,
            || {
                attempts.fetch_add(1, Ordering::SeqCst);
                let cids = vec![cid_a_clone.clone(), cid_b_clone.clone()];
                async move { Err(PdsError::NeedsBlobFetch { cids }) }
            },
            |cid: Cid| {
                stage_calls.fetch_add(1, Ordering::SeqCst);
                async move {
                    Err(PdsError::OriginFetchClientError {
                        cid,
                        status_or_reason: "origin returned 404".to_string(),
                    })
                }
            },
        )
        .await;
        match result {
            Err(PdsError::OriginFetchExhausted { per_cid_failures }) => {
                assert_eq!(
                    per_cid_failures.len(),
                    2,
                    "per_cid_failures should aggregate BOTH CIDs from the round"
                );
                for (_, reason) in &per_cid_failures {
                    assert!(
                        reason.contains("404") || reason.contains("origin"),
                        "failure reason should carry through: {}",
                        reason
                    );
                }
            }
            other => panic!("expected OriginFetchExhausted, got {:?}", other),
        }
        // ONE apply_writes attempt before stage failures aborted the
        // round. ALL cids were tried (collect-all): stage_calls == 2.
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_eq!(stage_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn fetch_retry_loop_succeeds_after_one_fetch_round() {
        // First do_writes returns NeedsBlobFetch[cid_a]; stage_one
        // succeeds; second do_writes returns Ok.
        let cid_a = test_cid();
        let attempts = AtomicU32::new(0);
        let stage_calls = AtomicU32::new(0);
        let cid_a_clone = cid_a.clone();
        let result = import_with_fetch_retry(
            3,
            || {
                let n = attempts.fetch_add(1, Ordering::SeqCst);
                let cids = vec![cid_a_clone.clone()];
                async move {
                    if n == 0 {
                        Err(PdsError::NeedsBlobFetch { cids })
                    } else {
                        Ok(("commit-cid".to_string(), "rev-2".to_string()))
                    }
                }
            },
            |_cid: Cid| {
                stage_calls.fetch_add(1, Ordering::SeqCst);
                async { Ok(()) }
            },
        )
        .await;
        let (cid, rev) = result.expect("success after one fetch round");
        assert_eq!(cid, "commit-cid");
        assert_eq!(rev, "rev-2");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(stage_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn fetch_retry_loop_exhausts_outer_budget_with_persistent_needs_fetch() {
        // Every do_writes call returns NeedsBlobFetch even though
        // every stage succeeds — pathological case where TOLERANT
        // keeps signalling new fetches forever. The outer max_retries
        // budget catches the runaway.
        let cid_a = test_cid();
        let attempts = AtomicU32::new(0);
        let cid_a_clone = cid_a.clone();
        let result = import_with_fetch_retry(
            2,
            || {
                attempts.fetch_add(1, Ordering::SeqCst);
                let cids = vec![cid_a_clone.clone()];
                async move { Err(PdsError::NeedsBlobFetch { cids }) }
            },
            |_cid: Cid| async { Ok(()) },
        )
        .await;
        match result {
            Err(PdsError::OriginFetchExhausted { per_cid_failures }) => {
                assert_eq!(per_cid_failures.len(), 1);
                assert!(
                    per_cid_failures[0].1.contains("budget exhausted"),
                    "expected exhausted-budget reason: {}",
                    per_cid_failures[0].1
                );
            }
            other => panic!("expected OriginFetchExhausted, got {:?}", other),
        }
        // 1 initial + 2 retries = 3 do_writes attempts.
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn fetch_retry_loop_propagates_non_needs_fetch_errors() {
        let attempts = AtomicU32::new(0);
        let result = import_with_fetch_retry(
            3,
            || {
                attempts.fetch_add(1, Ordering::SeqCst);
                async {
                    Err::<(String, String), _>(PdsError::Validation(
                        "deliberate non-fetch error".to_string(),
                    ))
                }
            },
            |_cid: Cid| async { Ok(()) },
        )
        .await;
        match result {
            Err(PdsError::Validation(msg)) => {
                assert!(msg.contains("deliberate non-fetch error"));
            }
            other => panic!("expected Validation passthrough, got {:?}", other),
        }
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn fetch_retry_loop_rejects_empty_needs_fetch_as_internal() {
        let result = import_with_fetch_retry(
            3,
            || async { Err::<(String, String), _>(PdsError::NeedsBlobFetch { cids: vec![] }) },
            |_cid: Cid| async { Ok(()) },
        )
        .await;
        match result {
            Err(PdsError::Internal(msg)) => {
                assert!(msg.contains("no CIDs"));
            }
            other => panic!("expected Internal for empty NeedsBlobFetch, got {:?}", other),
        }
    }

    // ------------------------------------------------------------
    // diff → WriteOp + blob_cid extraction
    // ------------------------------------------------------------

    #[test]
    fn split_record_key_parses_collection_slash_rkey() {
        let (c, r) = split_record_key("app.bsky.feed.post/3jc7abc").unwrap();
        assert_eq!(c, "app.bsky.feed.post");
        assert_eq!(r, "3jc7abc");
    }

    #[test]
    fn split_record_key_rejects_missing_slash() {
        let err = split_record_key("noslash").unwrap_err();
        match err {
            PdsError::InvalidCar(msg) => {
                assert!(msg.contains("not in"));
            }
            other => panic!("expected InvalidCar, got {:?}", other),
        }
    }
}
