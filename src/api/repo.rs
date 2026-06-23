/// com.atproto.repo.* endpoints
use crate::{
    actor_store::{RepositoryManager, WriteOp, WriteOpAction},
    api::{labels::LabelView, middleware},
    context::AppContext,
    error::{PdsError, PdsResult},
    oauth::AtProtoScope,
};
use axum::{
    extract::{Query, State},
    http::HeaderMap,
    routing::{get, post},
    Json, Router,
};
use proto_blue::common::next_tid;
use serde::{Deserialize, Serialize};

/// Build repository routes
pub fn routes() -> Router<AppContext> {
    Router::new()
        .route("/xrpc/com.atproto.repo.createRecord", post(create_record))
        .route("/xrpc/com.atproto.repo.putRecord", post(put_record))
        .route("/xrpc/com.atproto.repo.deleteRecord", post(delete_record))
        .route("/xrpc/com.atproto.repo.getRecord", get(get_record))
        .route("/xrpc/com.atproto.repo.listRecords", get(list_records))
        .route(
            "/xrpc/com.atproto.repo.listMissingBlobs",
            get(list_missing_blobs),
        )
        .route("/xrpc/com.atproto.repo.describeRepo", get(describe_repo))
        .route("/xrpc/com.atproto.repo.applyWrites", post(apply_writes))
}

/// Request to create a record
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateRecordRequest {
    repo: String, // DID or handle
    collection: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    rkey: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    validate: Option<bool>,
    record: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[allow(dead_code)] // TODO: Implement optimistic concurrency control
    swap_commit: Option<String>,
}

/// Response from creating a record
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateRecordResponse {
    uri: String,
    cid: String,
}

/// Request to update a record
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PutRecordRequest {
    repo: String,
    collection: String,
    rkey: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    validate: Option<bool>,
    record: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[allow(dead_code)] // TODO: Implement optimistic concurrency control
    swap_record: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[allow(dead_code)] // TODO: Implement optimistic concurrency control
    swap_commit: Option<String>,
}

/// Response from updating a record
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PutRecordResponse {
    uri: String,
    cid: String,
}

/// Request to delete a record
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteRecordRequest {
    repo: String,
    collection: String,
    rkey: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[allow(dead_code)] // TODO: Implement optimistic concurrency control
    swap_record: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[allow(dead_code)] // TODO: Implement optimistic concurrency control
    swap_commit: Option<String>,
}

/// Query parameters for getRecord
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetRecordQuery {
    repo: String,
    collection: String,
    rkey: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[allow(dead_code)] // TODO: Implement CID-specific record retrieval
    cid: Option<String>,
}

/// Response from getting a record
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GetRecordResponse {
    uri: String,
    cid: String,
    value: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    labels: Option<Vec<LabelView>>,
}

/// Query parameters for listRecords
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListRecordsQuery {
    repo: String,
    collection: String,
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[allow(dead_code)] // TODO: Implement reverse ordering
    reverse: Option<bool>,
}

fn default_limit() -> i64 {
    50
}

/// Record entry in list response
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecordEntry {
    uri: String,
    cid: String,
    value: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    labels: Option<Vec<LabelView>>,
}

/// Response from listing records
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ListRecordsResponse {
    records: Vec<RecordEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor: Option<String>,
}

/// Query parameters for describeRepo
#[derive(Debug, Deserialize)]
struct DescribeRepoQuery {
    repo: String,
}

/// Response from describing a repo
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DescribeRepoResponse {
    did: String,
    handle: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    did_doc: Option<serde_json::Value>,
    collections: Vec<String>,
    handle_is_correct: bool,
}

/// Request to apply writes
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApplyWritesRequest {
    repo: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[allow(dead_code)] // TODO: Implement record validation
    validate: Option<bool>,
    writes: Vec<WriteOpInput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[allow(dead_code)] // TODO: Implement optimistic concurrency control
    swap_commit: Option<String>,
}

/// A single applyWrites operation as received on the wire (#110, Option A).
///
/// Accepts BOTH shapes and normalizes to the internal [`WriteOp`]:
/// - the atproto-spec `$type`-discriminated shape
///   (`com.atproto.repo.applyWrites#{create,update,delete}`), which standard
///   bsky-PDS-shaped clients send, and
/// - Aurora's legacy flat `{action, collection, rkey, value}` shape, which
///   existing internal consumers (Phase B cookbooks, admin UI, dev scripts)
///   send.
///
/// `#[serde(untagged)]` tries the discriminated variant first (its `$type` tag
/// is a strong discriminator); a body lacking `$type` falls through to flat.
/// Each `writes` entry deserializes independently, so a request that mixes
/// shapes across entries is accepted — flagged but not policed (no deprecation
/// planned for either shape).
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum WriteOpInput {
    Discriminated(WriteOpDiscriminated),
    Flat(WriteOpFlat),
}

/// atproto-spec discriminated applyWrites operation (lexicon shape).
#[derive(Debug, Deserialize)]
#[serde(tag = "$type")]
enum WriteOpDiscriminated {
    #[serde(rename = "com.atproto.repo.applyWrites#create")]
    Create {
        collection: String,
        #[serde(default)]
        rkey: Option<String>,
        value: serde_json::Value,
    },
    #[serde(rename = "com.atproto.repo.applyWrites#update")]
    Update {
        collection: String,
        rkey: String,
        value: serde_json::Value,
    },
    #[serde(rename = "com.atproto.repo.applyWrites#delete")]
    Delete { collection: String, rkey: String },
}

/// Aurora's legacy flat applyWrites operation.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WriteOpFlat {
    action: WriteOpAction,
    collection: String,
    #[serde(default)]
    rkey: Option<String>,
    #[serde(default)]
    value: Option<serde_json::Value>,
    #[serde(default)]
    validate: Option<bool>,
    #[serde(default)]
    swap_cid: Option<String>,
}

impl WriteOpInput {
    /// Normalize either wire shape into the canonical internal [`WriteOp`].
    ///
    /// `create` with an absent rkey gets a server-generated TID (proto-blue
    /// `next_tid`), matching the lexicon (`rkey` optional on create) and the
    /// single-record `createRecord` path. `update`/`delete` require an rkey
    /// (you cannot target a record without one); a flat update/delete missing
    /// rkey is rejected rather than silently mis-targeted. `kryphocron_
    /// authorization` is always `None` here — it is in-process state populated
    /// at internal call sites, never request-bearable (see `WriteOp`).
    fn into_write_op(self) -> PdsResult<WriteOp> {
        match self {
            WriteOpInput::Discriminated(d) => Ok(match d {
                WriteOpDiscriminated::Create {
                    collection,
                    rkey,
                    value,
                } => WriteOp {
                    action: WriteOpAction::Create,
                    collection,
                    rkey: rkey.unwrap_or_else(|| next_tid(None).to_string()),
                    value: Some(value),
                    validate: None,
                    swap_cid: None,
                    kryphocron_authorization: None,
                },
                WriteOpDiscriminated::Update {
                    collection,
                    rkey,
                    value,
                } => WriteOp {
                    action: WriteOpAction::Update,
                    collection,
                    rkey,
                    value: Some(value),
                    validate: None,
                    swap_cid: None,
                    kryphocron_authorization: None,
                },
                WriteOpDiscriminated::Delete { collection, rkey } => WriteOp {
                    action: WriteOpAction::Delete,
                    collection,
                    rkey,
                    value: None,
                    validate: None,
                    swap_cid: None,
                    kryphocron_authorization: None,
                },
            }),
            WriteOpInput::Flat(f) => {
                let rkey = match (f.action, f.rkey) {
                    // create with no rkey → server-generated TID.
                    (WriteOpAction::Create, None) => next_tid(None).to_string(),
                    (_, Some(rkey)) => rkey,
                    // update/delete must name a record.
                    (WriteOpAction::Update, None) | (WriteOpAction::Delete, None) => {
                        return Err(PdsError::Validation(
                            "applyWrites update/delete requires rkey".to_string(),
                        ));
                    }
                };
                Ok(WriteOp {
                    action: f.action,
                    collection: f.collection,
                    rkey,
                    value: f.value,
                    validate: f.validate,
                    swap_cid: f.swap_cid,
                    kryphocron_authorization: None,
                })
            }
        }
    }
}

/// Build the proto-blue Signer that the repository manager uses to sign
/// commits.
///
/// Arc 18 (chainlink #117 / CF3 recon §G): resolve the per-account
/// signing key for `did` and wrap it as `Arc<dyn Signer>`.
///
/// Pre-Arc-18, the four record-write handlers below all signed with
/// the server-wide `ctx.config.authentication.repo_signing_key` (a
/// single key shared across every actor on the instance). This left a
/// signature-chain discontinuity at commit 2 of every repo — genesis
/// was signed by the per-actor key from `plc_keys.atproto_signing_key`
/// ([src/api/account_emit.rs:59-63]) but every subsequent record-write
/// commit was signed by the global key. Latent for Aurora's own paths
/// (which don't verify commit signatures), but federation-visible:
/// any external verifier rotating through the importing DID's
/// published `#atproto` verification method would fail to verify
/// post-genesis commits.
///
/// Caught by Arc 16f Step 5 Phase B Scenario 2 (chainlink #121) on
/// 2026-05-21: importing alice's CAR from instance A into instance B
/// failed `InvalidCommitSignature` despite the signature being
/// genuinely valid for the global key — the published per-account
/// verification method correctly rejected it.
///
/// Mirrors the per-account-signer template inlined at the importRepo
/// handler's CF3 gate ([src/api/repo_import.rs] `account_emit.rs:59`
/// template). Surfaces `Internal` (500) on NotFound — record-write
/// handlers have already passed createSession, so a missing
/// `plc_keys` row is a server-side inconsistency, not an
/// `ActorNotInitialized` (400) condition reserved for importRepo's
/// pre-account-init gate.
pub(crate) async fn create_actor_signer(
    account_manager: &crate::account::AccountManager,
    did: &str,
) -> PdsResult<std::sync::Arc<dyn proto_blue::crypto::Signer>> {
    let key_bytes = account_manager
        .get_atproto_signing_key_bytes(did)
        .await
        .map_err(|e| PdsError::Internal(format!(
            "could not resolve per-account signing key for {}: {}",
            did, e
        )))?;
    let signer = crate::crypto::proto_blue_signer::RepoSigner::from_bytes(&key_bytes)
        .map_err(|e| PdsError::Internal(format!(
            "Failed to construct per-account repo signer for {}: {}",
            did, e
        )))?;
    Ok(std::sync::Arc::new(signer))
}

/// Create a new record (`com.atproto.repo.createRecord`).
///
/// Writes a record to the actor's repo and reconciles blob references
/// in Phase B. Aurora-Locus's lexicons-as-Rust-types convention means
/// the wire contract for this endpoint is this handler's signature
/// (request / response shapes from [`CreateRecordRequest`] /
/// [`CreateRecordResponse`]) plus the error set below.
///
/// # Errors
///
/// Returns `PdsError` mapped to one of the following wire-error codes
/// (see [`crate::error::PdsError`]'s `IntoResponse` for the full
/// HTTP-status mapping):
///
/// - `AuthRequired` (401) — caller is unauthenticated.
/// - `InsufficientScope` / `Forbidden` (403) — OAuth scope check
///   against `AtProtoScope::RepoCreate` failed, or the request's
///   `repo` field disagrees with the authenticated DID.
/// - `RateLimitExceeded` (429) — cross-PDS rate limiter rejected the
///   request.
/// - `InvalidCid` (400) — Arc 16e §9.5.3.5: validate-phase walker
///   ([`extract_blob_cids`](crate::repository::blob_refs::extract_blob_cids))
///   found a malformed or non-DASL CID in a blob ref in the record
///   body. No state mutation occurs (rejection is before Phase A).
/// - `BlobNotFound` (400) — Arc 16e §9.5.3.5: Phase B STRICT could
///   not find a referenced blob's `blob_metadata` row. Per R0c.A
///   spec pin, the wire shape matches bsky-PDS at
///   `packages/pds/src/actor-store/blob/transactor.ts:259-260`.
/// - `Validation` (400) — record-body size limit, lexicon validation,
///   or swap-CID mismatch.
/// - `Database` / `Internal` (500) — sqlx or proto-blue commit
///   failures.
async fn create_record(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(req): Json<CreateRecordRequest>,
) -> PdsResult<Json<CreateRecordResponse>> {
    tracing::info!("create_record: Starting for collection: {}", req.collection);

    // Require authentication (OAuth, local, or cross-PDS) - Phase 6
    let auth = middleware::require_auth_unified(State(ctx.clone()), headers.clone())
        .await
        .map_err(|e| {
            tracing::error!("create_record: Auth failed: {}", e);
            e
        })?;

    // Enforce OAuth scope if using OAuth authentication
    middleware::enforce_scope(&auth, &AtProtoScope::RepoCreate)?;

    let auth_did = auth.did();
    tracing::debug!(
        "create_record: Authenticated as DID: {}, auth_type: {}",
        auth_did,
        if auth.is_oauth() {
            "oauth"
        } else if auth.is_local() {
            "local"
        } else {
            "cross_pds"
        }
    );

    // Apply stricter rate limiting for cross-PDS requests (Phase 4 Security)
    if auth.is_cross_pds() {
        ctx.rate_limiter.check_cross_pds()?;
        tracing::debug!("create_record: Cross-PDS rate limit check passed");
    }

    // Verify repo matches authenticated user
    if req.repo != auth_did {
        tracing::error!(
            "create_record: Repo mismatch - req: {}, auth: {}",
            req.repo,
            auth_did
        );
        return Err(PdsError::Authorization(
            "Cannot create record in another user's repo".to_string(),
        ));
    }

    // Create repository manager with sequencer
    tracing::debug!("create_record: Creating repository manager with sequencer");
    // §17.4 Step 4 + #136 — go through `for_writer` so the Arc 17
    // lexicon resolver + config snapshot get plumbed when
    // `lexicon_resolver` is `Some`. Centralization is load-bearing:
    // pre-#136, this site chained `.with_blob_store` but not
    // `.with_lexicon`, leaving the dispatch ladder's PRIORITY 2 gate
    // empty and forcing every unknown-NSID write through Optimistic
    // fall-through.
    let repo_mgr = RepositoryManager::for_writer(&ctx, auth_did.to_string());

    // Create signer from repo key
    let signer = create_actor_signer(&ctx.account_manager, auth_did).await?;

    // Create the record. Arc 16e §9.5.4 Step 2: blob refs are tracked
    // inside `apply_writes` Phase B via the wired `blob_store` field
    // rather than by this handler — see the `.with_blob_store(...)`
    // builder call above.
    tracing::debug!("create_record: Calling repo_mgr.create_record");
    let (uri, cid, _rev) = repo_mgr
        .create_record(
            &req.collection,
            req.rkey.as_deref(),
            req.record,
            req.validate,
            signer,
        )
        .await
        .map_err(|e| {
            tracing::error!("create_record: Failed to create record: {}", e);
            e
        })?;

    // Invalidate read-after-write cache for this user
    ctx.cache_invalidator.invalidate_did(auth_did).await;
    tracing::debug!("create_record: Invalidated cache for DID: {}", auth_did);

    tracing::info!(
        "create_record: Successfully created record - URI: {}, CID: {}",
        uri,
        cid
    );

    // §5.5.4 Phase C: Pipeline C account-age-activity auto-label rules fire on
    // post creation. Best-effort; scoped to feed posts (the activity signal).
    if req.collection == "app.bsky.feed.post" {
        if let Err(e) =
            crate::api::auto_label_rules::evaluate_pipeline_c(&ctx, auth_did, &uri).await
        {
            tracing::warn!(error = %e, author = auth_did, "auto-label Pipeline C failed");
        }
    }

    Ok(Json(CreateRecordResponse { uri, cid }))
}

/// Update an existing record, or create if it doesn't exist
/// (`com.atproto.repo.putRecord`).
///
/// Per-record Phase B computes the existing-refs / new-refs
/// difference: added CIDs go through STRICT; dropped CIDs go through
/// `unreference_blob`. Wire contract = this handler's signature
/// (request / response shapes from [`PutRecordRequest`] /
/// [`PutRecordResponse`]) plus the error set below.
///
/// # Errors
///
/// - `AuthRequired` (401) — caller is unauthenticated.
/// - `InsufficientScope` / `Forbidden` (403) — OAuth scope check
///   against `AtProtoScope::RepoUpdate` failed, or `repo` disagrees
///   with the authenticated DID.
/// - `RateLimitExceeded` (429) — cross-PDS rate limiter rejected.
/// - `InvalidCid` (400) — Arc 16e §9.5.3.5: validate-phase walker
///   found a malformed or non-DASL CID in a blob ref in the new
///   record body. No state mutation (rejection is before Phase A).
/// - `BlobNotFound` (400) — Arc 16e §9.5.3.5: Phase B STRICT could
///   not find a newly-referenced blob's `blob_metadata` row.
/// - `Validation` (400) — record-body size, lexicon validation, or
///   swap-CID mismatch.
/// - `NotFound` (404) — swap-CID supplied for a record that does
///   not exist.
/// - `Database` / `Internal` (500) — sqlx or proto-blue commit
///   failures.
async fn put_record(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(req): Json<PutRecordRequest>,
) -> PdsResult<Json<PutRecordResponse>> {
    // Require authentication (OAuth, local, or cross-PDS) - Phase 6
    let auth = middleware::require_auth_unified(State(ctx.clone()), headers.clone()).await?;

    // Enforce OAuth scope if using OAuth authentication
    middleware::enforce_scope(&auth, &AtProtoScope::RepoUpdate)?;

    let auth_did = auth.did();

    // Apply stricter rate limiting for cross-PDS requests (Phase 4 Security)
    if auth.is_cross_pds() {
        ctx.rate_limiter.check_cross_pds()?;
    }

    // Verify repo matches authenticated user
    if req.repo != auth_did {
        return Err(PdsError::Authorization(
            "Cannot update record in another user's repo".to_string(),
        ));
    }

    // Create repository manager via §17.4-Step-4 / #136 helper —
    // plumbs the lexicon resolver + config when enabled.
    let repo_mgr = RepositoryManager::for_writer(&ctx, auth_did.to_string());

    // Create signer from repo key
    let signer = create_actor_signer(&ctx.account_manager, auth_did).await?;

    // Update the record. Arc 16e §9.5.4 Step 2: blob-ref add/drop is
    // computed in Phase B via `read_existing_refs` + set differences;
    // see the `.with_blob_store(...)` builder call above.
    let (cid, _rev) = repo_mgr
        .update_record(&req.collection, &req.rkey, req.record, req.validate, signer)
        .await?;

    let uri = format!("at://{}/{}/{}", auth_did, req.collection, req.rkey);

    // Invalidate read-after-write cache for this user
    ctx.cache_invalidator.invalidate_did(auth_did).await;

    Ok(Json(PutRecordResponse { uri, cid }))
}

/// Delete a record (`com.atproto.repo.deleteRecord`).
///
/// Per-record Phase B reads the existing refs and runs
/// `unreference_blob` for each. Delete records carry no new CIDs, so
/// neither `InvalidCid` nor `BlobNotFound` surface from the
/// Arc 16e wiring on this path (`unreference_blob`'s six-variant
/// `UnreferenceOutcome` is log-and-continue, never an error
/// propagation). Wire contract = this handler's signature
/// (request shape from [`DeleteRecordRequest`]) plus the error set
/// below.
///
/// # Errors
///
/// - `AuthRequired` (401) — caller is unauthenticated.
/// - `InsufficientScope` / `Forbidden` (403) — OAuth scope check
///   against `AtProtoScope::RepoDelete` failed, or `repo` disagrees
///   with the authenticated DID.
/// - `RateLimitExceeded` (429) — cross-PDS rate limiter rejected.
/// - `Validation` (400) — swap-CID mismatch.
/// - `NotFound` (404) — swap-CID supplied for a record that does
///   not exist, or delete of a non-existent record (depending on
///   the underlying store's contract).
/// - `Database` / `Internal` (500) — sqlx or proto-blue commit
///   failures.
async fn delete_record(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(req): Json<DeleteRecordRequest>,
) -> PdsResult<Json<serde_json::Value>> {
    // Require authentication (OAuth, local, or cross-PDS) - Phase 6
    let auth = middleware::require_auth_unified(State(ctx.clone()), headers.clone()).await?;

    // Enforce OAuth scope if using OAuth authentication
    middleware::enforce_scope(&auth, &AtProtoScope::RepoDelete)?;

    let auth_did = auth.did();

    // Apply stricter rate limiting for cross-PDS requests (Phase 4 Security)
    if auth.is_cross_pds() {
        ctx.rate_limiter.check_cross_pds()?;
    }

    // Verify repo matches authenticated user
    if req.repo != auth_did {
        return Err(PdsError::Authorization(
            "Cannot delete record from another user's repo".to_string(),
        ));
    }

    // §17.4-Step-4 / #136 — delete writes don't reach the lexicon
    // path (`validate_write` early-returns on `write.value = None`),
    // but route through `for_writer` anyway so the audit grep
    // (#136-guard-2) has a uniform rule.
    let repo_mgr = RepositoryManager::for_writer(&ctx, auth_did.to_string());

    // Create signer from repo key
    let signer = create_actor_signer(&ctx.account_manager, auth_did).await?;

    // Delete the record. Arc 16e §9.5.4 Step 2: blob refs for the
    // record are unreferenced in Phase B via the wired `blob_store`;
    // see the `.with_blob_store(...)` builder call above.
    repo_mgr
        .delete_record(&req.collection, &req.rkey, signer)
        .await?;

    // Invalidate read-after-write cache for this user
    ctx.cache_invalidator.invalidate_did(auth_did).await;

    Ok(Json(serde_json::json!({})))
}

/// Get a record
async fn get_record(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Query(query): Query<GetRecordQuery>,
) -> PdsResult<Json<GetRecordResponse>> {
    // Get the DID (could be handle resolution in the future)
    let did = &query.repo;

    // Create repository manager
    let repo_mgr = RepositoryManager::with_validation_mode(
        did.clone(),
        (*ctx.actor_store).clone(),
        ctx.config.validation_mode,
    );

    // Get the record
    let uri = format!("at://{}/{}/{}", did, query.collection, query.rkey);
    let record = repo_mgr.get_record(&uri).await?;

    match record {
        Some(value) => {
            // Extract CID from the returned value
            let cid = value
                .get("cid")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();

            let mut record_value = value
                .get("value")
                .cloned()
                .unwrap_or(serde_json::Value::Null);

            // v0.9 Arc D (#237a) — decode-on-read for private-tier records.
            // `feed.postPrivate` content is encoded at rest (#236); an
            // authorized reader fetching it here gets decoded plaintext, a
            // non-authorized/anonymous reader gets the encoded form unchanged
            // (transparent — atproto's getRecord-is-public contract + the
            // friction model are both preserved). Aurora-Locus owns the
            // authorization decision (architecture (Y)); the substrate codec is
            // the at-rest layer only. Other NSIDs (public records, audience/
            // block/mute structural records) pass through untouched.
            if query.collection == crate::api::kryphocron_endpoints::NSID_POST_PRIVATE {
                // Optional reader auth: a valid session identifies the reader;
                // no/invalid credentials -> anonymous (treated as non-member).
                let reader_did = middleware::require_auth_unified(State(ctx.clone()), headers)
                    .await
                    .ok()
                    .map(|auth| auth.did().to_string());
                if matches!(
                    crate::kryphocron_content::authorize_private_read(
                        &ctx,
                        reader_did.as_deref(),
                        did,
                        &record_value,
                    )
                    .await,
                    crate::kryphocron_content::ReadAuthz::Authorized
                ) {
                    // Decode in place. Codec skew surfaces as 410 (the record is
                    // valid but undecodable here); other decode failures surface
                    // as 500. A non-encoded (legacy `text`) record is a no-op.
                    crate::kryphocron_content::decode_private_content(
                        &ctx,
                        did,
                        &query.collection,
                        &query.rkey,
                        &mut record_value,
                    )
                    .await?;
                }
                // Non-authorized: leave the encoded form as stored.
            }

            // Fetch labels for this record
            let labels = ctx
                .label_manager
                .get_labels(&uri)
                .await
                .ok()
                .map(|lbls| lbls.into_iter().map(LabelView::from).collect());

            Ok(Json(GetRecordResponse {
                uri,
                cid,
                value: record_value,
                labels,
            }))
        }
        None => Err(PdsError::NotFound(format!("Record not found: {}", uri))),
    }
}

/// List records in a collection
async fn list_records(
    State(ctx): State<AppContext>,
    Query(query): Query<ListRecordsQuery>,
) -> PdsResult<Json<ListRecordsResponse>> {
    // Get the DID
    let did = &query.repo;

    // Create repository manager
    let repo_mgr = RepositoryManager::with_validation_mode(
        did.clone(),
        (*ctx.actor_store).clone(),
        ctx.config.validation_mode,
    );

    // Fetch limit + 1 to determine if there are more records
    let fetch_limit = query.limit + 1;
    let records = repo_mgr
        .list_records(&query.collection, fetch_limit, query.cursor.as_deref())
        .await?;

    // Determine if we have more records and calculate cursor
    let has_more = records.len() as i64 > query.limit;
    let records_to_return = if has_more {
        &records[0..query.limit as usize]
    } else {
        &records[..]
    };

    // Convert to response format and fetch labels
    let mut entries = Vec::new();
    let mut next_cursor: Option<String> = None;

    for rec in records_to_return {
        let uri = rec
            .get("uri")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let cid = rec
            .get("cid")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let value = rec.get("value").cloned().unwrap_or(serde_json::Value::Null);

        // Extract rkey from URI for cursor (format: at://did/collection/rkey)
        if let Some(rkey) = uri.split('/').next_back() {
            next_cursor = Some(rkey.to_string());
        }

        // Fetch labels for this record
        let labels = ctx
            .label_manager
            .get_labels(&uri)
            .await
            .ok()
            .map(|lbls| lbls.into_iter().map(LabelView::from).collect());

        entries.push(RecordEntry {
            uri,
            cid,
            value,
            labels,
        });
    }

    Ok(Json(ListRecordsResponse {
        records: entries,
        cursor: if has_more { next_cursor } else { None },
    }))
}

/// Describe a repository
async fn describe_repo(
    State(ctx): State<AppContext>,
    Query(query): Query<DescribeRepoQuery>,
) -> PdsResult<Json<DescribeRepoResponse>> {
    // Get the DID
    let did = &query.repo;

    // Create repository manager
    let repo_mgr = RepositoryManager::with_validation_mode(
        did.clone(),
        (*ctx.actor_store).clone(),
        ctx.config.validation_mode,
    );

    // Get description with account manager and identity resolver
    let desc = repo_mgr
        .describe_repo(
            Some(&ctx.account_manager),
            Some(ctx.identity_resolver.as_ref()),
        )
        .await?;

    Ok(Json(DescribeRepoResponse {
        did: desc
            .get("did")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        handle: desc
            .get("handle")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        did_doc: desc.get("didDoc").cloned(),
        collections: desc
            .get("collections")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        handle_is_correct: desc
            .get("handleIsCorrect")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
    }))
}

/// Apply a batch of writes (`com.atproto.repo.applyWrites`).
///
/// Performs atomic batch operations with full validation of all
/// operations before execution, duplicate detection, size limit
/// enforcement, and all-or-nothing atomicity. Arc 16e §9.5.3.2.0
/// gives this endpoint the same validate-phase walker as the single-
/// record write handlers: a malformed CID anywhere in the batch
/// rejects the WHOLE batch before Phase A opens, so partial state
/// mutation is structurally impossible.
///
/// Wire contract = this handler's signature (request shape from
/// [`ApplyWritesRequest`]) plus the error set below.
///
/// # Errors
///
/// - `AuthRequired` (401) — caller is unauthenticated.
/// - `InsufficientScope` / `Forbidden` (403) — OAuth scope check
///   against `AtProtoScope::RepoAll` failed, or `repo` disagrees
///   with the authenticated DID.
/// - `RateLimitExceeded` (429) — cross-PDS rate limiter rejected.
/// - `InvalidCid` (400) — Arc 16e §9.5.3.5: validate-phase walker
///   found a malformed or non-DASL CID in a blob ref in any
///   Create/Update record body in the batch. Aborts the whole
///   batch with zero state mutation.
/// - `BlobNotFound` (400) — Arc 16e §9.5.3.5: Phase B STRICT could
///   not find a referenced blob's `blob_metadata` row for any
///   Create/Update in the batch.
/// - `Validation` (400) — batch size limit (>200 ops), duplicate
///   operations, record size, lexicon validation, swap-CID
///   mismatch, or per-op shape errors.
/// - `NotFound` (404) — swap-CID supplied for a record that does
///   not exist.
/// - `Database` / `Internal` (500) — sqlx or proto-blue commit
///   failures.
async fn apply_writes(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(req): Json<ApplyWritesRequest>,
) -> PdsResult<Json<serde_json::Value>> {
    // Require authentication (OAuth, local, or cross-PDS) - Phase 6
    let auth = middleware::require_auth_unified(State(ctx.clone()), headers.clone()).await?;

    // Enforce OAuth scope if using OAuth authentication
    // apply_writes can do create/update/delete, so it requires RepoAll or Write scope
    middleware::enforce_scope(&auth, &AtProtoScope::RepoAll)?;

    let auth_did = auth.did();

    // Apply stricter rate limiting for cross-PDS requests
    if auth.is_cross_pds() {
        ctx.rate_limiter.check_cross_pds()?;
    }

    // Verify repo matches authenticated user
    if req.repo != auth_did {
        return Err(PdsError::Authorization(
            "Cannot apply writes to another user's repo".to_string(),
        ));
    }

    // §17.4-Step-4 / #136 — batch writes route the same way: each
    // per-write validate dispatch goes through the lexicon resolver
    // when plumbed, and the §17.3.4 `validate_imports` override
    // fires at validate-phase entry for each write in the batch.
    let repo_mgr = RepositoryManager::for_writer(&ctx, auth_did.to_string());

    // Prepare writes (converts to PreparedWrite format)
    // Normalize both accepted wire shapes (#110) into the canonical WriteOp.
    let writes = req
        .writes
        .into_iter()
        .map(WriteOpInput::into_write_op)
        .collect::<PdsResult<Vec<WriteOp>>>()?;
    let prepared = repo_mgr.prepare_writes(writes)?;

    tracing::info!(
        "Applying batch of {} operations for {}",
        prepared.len(),
        auth_did
    );

    // Create signer from repo key
    let signer = create_actor_signer(&ctx.account_manager, auth_did).await?;

    // Apply batch atomically (includes validation)
    let (commit_cid, rev) = repo_mgr.apply_batch_writes(prepared, signer).await?;

    tracing::info!(
        "Successfully committed batch for {} (rev: {})",
        auth_did,
        rev
    );

    Ok(Json(serde_json::json!({
        "commit": {
            "cid": commit_cid,
            "rev": rev,
        }
    })))
}

/// Query parameters for listMissingBlobs
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListMissingBlobsQuery {
    #[serde(default = "default_missing_blobs_limit")]
    limit: i64,
    #[serde(default)]
    cursor: Option<String>,
}

fn default_missing_blobs_limit() -> i64 {
    500
}

/// A blob that is referenced by a record but not yet uploaded
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecordBlob {
    cid: String,
    record_uri: String,
}

/// Response for listMissingBlobs
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ListMissingBlobsResponse {
    blobs: Vec<RecordBlob>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor: Option<String>,
}

/// List missing blobs endpoint
///
/// Returns blobs that are referenced by records but have not yet been uploaded.
/// This is useful for resuming failed uploads or identifying incomplete records.
async fn list_missing_blobs(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Query(query): Query<ListMissingBlobsQuery>,
) -> PdsResult<Json<ListMissingBlobsResponse>> {
    // Require authentication
    let auth = middleware::require_auth_unified(State(ctx.clone()), headers.clone()).await?;
    let auth_did = auth.did();

    // Validate limit (1-1000)
    let limit = query.limit.clamp(1, 1000);

    // Query for missing blobs
    let missing = ctx
        .blob_store
        .list_missing_blobs(auth_did, limit + 1, query.cursor.as_deref())
        .await?;

    // Determine if there's more data (pagination)
    let has_more = missing.len() > limit as usize;
    let results: Vec<_> = missing.into_iter().take(limit as usize).collect();

    // Build cursor from last item
    let cursor = if has_more {
        results.last().map(|(cid, _)| cid.clone())
    } else {
        None
    };

    // Convert to response format
    let blobs = results
        .into_iter()
        .map(|(cid, record_uri)| RecordBlob { cid, record_uri })
        .collect();

    Ok(Json(ListMissingBlobsResponse { blobs, cursor }))
}

#[cfg(test)]
mod apply_writes_shape_tests {
    //! #110: applyWrites accepts both the atproto-spec `$type`-discriminated
    //! shape and Aurora's legacy flat shape; both normalize to `WriteOp`.
    use super::*;

    fn norm(json: &str) -> WriteOp {
        serde_json::from_str::<WriteOpInput>(json)
            .expect("deserialize WriteOpInput")
            .into_write_op()
            .expect("normalize to WriteOp")
    }

    #[test]
    fn both_shapes_normalize_equivalently() {
        // create
        let disc = norm(
            r#"{"$type":"com.atproto.repo.applyWrites#create","collection":"app.bsky.feed.post","rkey":"rk1","value":{"text":"hi"}}"#,
        );
        let flat = norm(
            r#"{"action":"create","collection":"app.bsky.feed.post","rkey":"rk1","value":{"text":"hi"}}"#,
        );
        assert_eq!(disc.action, WriteOpAction::Create);
        assert_eq!(flat.action, WriteOpAction::Create);
        assert_eq!(disc.collection, flat.collection);
        assert_eq!(disc.rkey, "rk1");
        assert_eq!(disc.rkey, flat.rkey);
        assert_eq!(disc.value, flat.value);

        // update
        let disc = norm(
            r#"{"$type":"com.atproto.repo.applyWrites#update","collection":"c","rkey":"rk2","value":{"a":1}}"#,
        );
        let flat = norm(r#"{"action":"update","collection":"c","rkey":"rk2","value":{"a":1}}"#);
        assert_eq!(disc.action, WriteOpAction::Update);
        assert_eq!(flat.action, WriteOpAction::Update);
        assert_eq!(disc.rkey, flat.rkey);
        assert_eq!(disc.value, flat.value);

        // delete — discriminated #delete has no `value` field at all.
        let disc =
            norm(r#"{"$type":"com.atproto.repo.applyWrites#delete","collection":"c","rkey":"rk3"}"#);
        let flat = norm(r#"{"action":"delete","collection":"c","rkey":"rk3"}"#);
        assert_eq!(disc.action, WriteOpAction::Delete);
        assert_eq!(flat.action, WriteOpAction::Delete);
        assert_eq!(disc.rkey, "rk3");
        assert_eq!(flat.rkey, "rk3");
        assert!(disc.value.is_none());
    }

    #[test]
    fn create_without_rkey_server_generates_and_update_delete_require_it() {
        // create with no rkey → server-generated TID (both shapes).
        let disc = norm(
            r#"{"$type":"com.atproto.repo.applyWrites#create","collection":"c","value":{"x":1}}"#,
        );
        let flat = norm(r#"{"action":"create","collection":"c","value":{"x":1}}"#);
        assert!(!disc.rkey.is_empty(), "discriminated create rkey server-generated");
        assert!(!flat.rkey.is_empty(), "flat create rkey server-generated");

        // flat update/delete without rkey is rejected (cannot target a record).
        let err = serde_json::from_str::<WriteOpInput>(r#"{"action":"delete","collection":"c"}"#)
            .expect("deserialize")
            .into_write_op();
        assert!(err.is_err(), "flat delete without rkey must error");
    }
}
