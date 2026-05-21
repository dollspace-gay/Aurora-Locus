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

    info!(
        target: "import_repo",
        event = "import_repo_starting",
        did = %importing_did,
        "importRepo handler entry"
    );

    // CF3 — signer resolution gate (account_emit.rs:59 template).
    // Existence-check + per-account key resolution in one. Resolve
    // BEFORE acquiring the lock so a missing actor fails fast
    // without holding it.
    let signer: Arc<dyn Signer> =
        match ctx.account_manager.get_atproto_signing_key_bytes(&importing_did).await {
            Ok(key_bytes) => Arc::new(
                RepoSigner::from_bytes(&key_bytes).map_err(|e| {
                    PdsError::Internal(format!(
                        "importRepo signer construction failed: {}",
                        e
                    ))
                })?,
            ),
            Err(PdsError::NotFound(_)) => {
                emit_rejected(&importing_did, "ActorNotInitialized", 0, &[]);
                return Err(PdsError::ActorNotInitialized);
            }
            Err(other) => return Err(other),
        };

    // Single-flight lock keyed on importing_did. Acquired AFTER
    // signer resolution so missing accounts fail fast.
    let _lock_guard = match import_lock_registry().try_acquire(&importing_did) {
        Some(g) => g,
        None => {
            warn!(
                target: "import_repo",
                did = %importing_did,
                "importRepo concurrent mutation rejected"
            );
            emit_rejected(&importing_did, "ConcurrentMutation", 0, &[]);
            return Err(PdsError::ConcurrentMutation);
        }
    };

    // accepting_imports drain switch — checked INSIDE the lock
    // so operators can flip the switch and let in-flight imports
    // finish without serving new ones.
    if !ctx.config.service.accepting_imports {
        emit_rejected(&importing_did, "ServiceUnavailable", 0, &[]);
        let status = axum::http::StatusCode::SERVICE_UNAVAILABLE;
        let payload = serde_json::json!({
            "error": "ServiceUnavailable",
            "message": "importRepo is disabled (service.accepting_imports = false)",
        });
        return Ok((status, Json(payload)).into_response());
    }

    // CAR body read with streaming size-bound enforcement
    // (round-1 F21). Chunks accumulate; first chunk that pushes
    // the total past `max_import_size` aborts the read.
    let car_bytes = read_body_with_cap(body, ctx.config.service.max_import_size).await?;
    let car_size = car_bytes.len();
    debug!(
        target: "import_repo",
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
    let signing_did_key =
        public_did_key_for(&ctx, &importing_did).await?;

    let verified_diff = match verify_diff_car(
        &car_bytes,
        None, // No prior-repo snapshot — Aurora's v0.5 importRepo
              // applies imported commits as additive diffs against
              // an empty MST. v0.6+ may pass the current repo's
              // VerifiedRepo here to support incremental import.
        Some(&importing_did),
        Some(&signing_did_key),
    ) {
        Ok(d) => d,
        Err(e) => {
            let msg = e.to_string();
            // proto-blue's RepoError covers structural failures
            // (CAR decode, MST load) AND signature failures.
            // Disambiguate via the error message — proto-blue
            // signature failures carry "signature" in the
            // displayed string per its error vocabulary. Cheap
            // string match is sufficient for the wire-error
            // routing; production tooling discriminates via the
            // error name in the JSON envelope, not by
            // string-matching the message.
            let lower = msg.to_lowercase();
            let err = if lower.contains("signature") || lower.contains("signing") {
                emit_rejected(&importing_did, "InvalidCommitSignature", car_size, &[]);
                PdsError::InvalidCommitSignature
            } else {
                emit_rejected(&importing_did, "InvalidCar", car_size, &[]);
                PdsError::InvalidCar(format!("CAR decode failed: {}", msg))
            };
            return Err(err);
        }
    };

    // DID match: CAR root commit's DID MUST equal the authenticated
    // importing_did. The verify_diff_car call above already passes
    // `expected_did = Some(&importing_did)`, which proto-blue
    // enforces — so a mismatch here would surface as a decode
    // error above. This second check is defensive (catches future
    // proto-blue changes that might relax the check).
    if verified_diff.repo.commit.did != importing_did {
        emit_rejected(&importing_did, "InvalidCar", car_size, &[]);
        return Err(PdsError::InvalidCar(format!(
            "CAR root commit DID {} does not match importing DID {}",
            verified_diff.repo.commit.did, importing_did
        )));
    }

    // Diff → WriteOp + blob_cid extraction.
    let (writes, blob_cids) = diff_to_writes(&verified_diff, &importing_did)?;
    let prepared_write_count = writes.len();
    let validate_phase_cid_count = blob_cids.len();
    debug!(
        target: "import_repo",
        did = %importing_did,
        prepared_write_count,
        validate_phase_cid_count,
        "importRepo diff prepared"
    );

    // Validate-phase quarantine + DASL gate (round-1 F1).
    validate_phase_blob_check(&ctx, &importing_did, &blob_cids).await?;

    // Pooled reqwest::Client (round-1 F11) — single instance per
    // handler invocation, threaded to all fetch calls.
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(
            ctx.config.service.blob_fetch_timeout_seconds,
        ))
        .build()
        .map_err(|e| {
            PdsError::Internal(format!("reqwest client construction failed: {}", e))
        })?;

    // RepositoryManager is not Clone (holds a sync RecordValidator);
    // rebuild it on each loop iteration. Construction is cheap —
    // just struct field setup, no I/O — and lets the closure satisfy
    // the FnMut bound without needing Arc<Manager> indirection.
    let writes_for_loop = writes.clone();
    let signer_for_loop = signer.clone();
    let did_for_loop = importing_did.clone();
    let actor_store_for_loop = (*ctx.actor_store).clone();
    let sequencer_for_loop = ctx.sequencer.clone();
    let validation_mode_for_loop = ctx.config.validation_mode;
    let blob_store_for_loop = ctx.blob_store.clone();
    let started = std::time::Instant::now();

    // HttpOriginBlobFetcher is zero-sized; construct fresh per
    // stage call inside the closure rather than capturing by-move.
    let blob_store_for_stage = ctx.blob_store.clone();
    let did_for_stage = importing_did.clone();
    let client_for_stage = client.clone();
    let ctx_for_stage = ctx.clone();
    let outcome = import_with_fetch_retry(
        ctx.config.service.blob_fetch_max_retries,
        // do_writes: Arc 16f Step 4 flipped this from the v5
        // `apply_writes(writes, signer)` call to the v5.1
        // `apply_writes(writes, signer, Arc::new(TolerantPromoter))`
        // form. TolerantPromoter signals NeedsBlobFetch on row-absent
        // CIDs, which `import_with_fetch_retry` consumes via the
        // already-tested NeedsBlobFetch branch.
        || {
            let writes = writes_for_loop.clone();
            let signer = signer_for_loop.clone();
            let mgr = RepositoryManager::with_sequencer_and_validation(
                did_for_loop.clone(),
                actor_store_for_loop.clone(),
                sequencer_for_loop.clone(),
                validation_mode_for_loop,
            )
            .with_blob_store(blob_store_for_loop.clone());
            async move {
                mgr.apply_writes(
                    writes,
                    signer,
                    Arc::new(crate::blob_store::TolerantPromoter),
                )
                .await
            }
        },
        // stage_one: fetch + stage + commit per-CID. Captures the
        // pooled client + blob store + DID; the trait shim
        // (`OriginBlobFetcher`) lets Phase B and Step 4 tests swap in
        // mock fetchers without touching this seam.
        |cid: Cid| {
            let client = client_for_stage.clone();
            let blob_store = blob_store_for_stage.clone();
            let did = did_for_stage.clone();
            let ctx_inner = ctx_for_stage.clone();
            async move {
                let fetcher = crate::federation::blob_fetch::HttpOriginBlobFetcher;
                fetch_and_stage_one(&ctx_inner, &client, &blob_store, &did, &cid, &fetcher)
                    .await
            }
        },
    )
    .await;

    let total_duration_ms = started.elapsed().as_millis() as u64;

    match outcome {
        Ok((commit_cid, rev)) => {
            info!(
                target: "import_repo",
                event = "import_repo_complete",
                did = %importing_did,
                car_size,
                prepared_write_count,
                fetched_blob_count = 0u32, // TOLERANT path lands in Step 4 — STRICT today
                fetch_round_count = 0u32,
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
        Err(err) => {
            let kind = error_kind_label(&err);
            emit_rejected(&importing_did, kind, car_size, &[]);
            warn!(
                target: "import_repo",
                event = "import_repo_rejected",
                did = %importing_did,
                car_size,
                error = %err,
                total_duration_ms,
                "importRepo handler rejected"
            );
            Err(err)
        }
    }
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
                    target: "import_repo",
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
    pub(crate) fn try_acquire(self: &'static Self, did: &str) -> Option<ImportLockGuard> {
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
        target: "import_repo",
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
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn test_cid() -> Cid {
        use std::str::FromStr;
        Cid::from_str("bafkreigh2akiscaildcqabsyg3dfr6chu3fgpregiymsck7e7aqa4s52zy")
            .expect("valid CIDv1 raw multibase")
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
