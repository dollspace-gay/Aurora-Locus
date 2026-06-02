//! v0.7 arc 2 step 5 — dedicated user-class kryphocron endpoints.
//!
//! Four XRPC procedures per the arc 2 kickoff §5:
//!
//! - `tools.kryphocron.feed.createPostPrivate` — capability
//!   `EditPrivatePost` (user class). Creates a
//!   `tools.kryphocron.feed.postPrivate` record.
//! - `tools.kryphocron.feed.deletePostPrivate` — capability
//!   `DeletePrivatePost` (user class). Deletes a
//!   `tools.kryphocron.feed.postPrivate` record.
//! - `tools.kryphocron.actor.participatePrivate` — capability
//!   `ParticipatePrivate` (user class). Creates a
//!   `tools.kryphocron.feed.postPrivate` record positioned as a
//!   reply to an existing private post — the reply parent's
//!   `audienceList` governs the bind pipeline's audience-oracle
//!   check at step 7. (The exact reply-vs-create split for this
//!   capability is deferred to step 7's wiring; step 5 ships the
//!   endpoint that routes a participation post into the bind
//!   pipeline under the `ParticipatePrivate`-class
//!   authorization.)
//! - `tools.kryphocron.policy.manageAudience` — capability
//!   `ManageAudience` (user class). Creates / updates a
//!   `tools.kryphocron.policy.audience` record.
//!
//! Each handler authenticates via Aurora-Locus's existing
//! `require_auth_unified` flow, enforces the appropriate OAuth
//! scope, builds a `RepositoryManager` via `for_writer` (which
//! plumbs the shared pool needed for the relay-race lent-tx
//! mechanism), constructs a `WriteOp` with
//! `kryphocron_authorization:
//! Some(KryphocronWriteAuthorization::DedicatedEndpoint {
//! capability_class: CapabilityClass::User })`, and routes through
//! `apply_writes`.
//!
//! Inside `apply_writes`, the step-5-restructured validate loop
//! threads the lent shared tx into `validate_write`, whose
//! kryphocron-prefix dispatcher routes the `Some(_)`-authorized
//! write into `bind_pipeline`. Step 4 wired the framework + 5
//! match arms; this is the first production code path that
//! constructs a `Some(_)` authorization, so it's the first commit
//! where `bind_pipeline` is reachable from the production write
//! path. Step 7 will add the per-capability bind-pipeline stages
//! (oracle calls, audit-emit payloads). Until step 7 ships, the
//! `DedicatedEndpoint` arm just emits the
//! `kryphocron_bind_pipeline_authorized` tracing event and returns
//! Ok.

use std::sync::Arc;

use axum::{extract::State, http::HeaderMap, routing::post, Json, Router};
use proto_blue::common::next_tid;
use serde::{Deserialize, Serialize};

use crate::{
    actor_store::{repository::WriteOpAction, RepositoryManager, WriteOp},
    api::{middleware, repo::create_actor_signer},
    context::AppContext,
    error::{PdsError, PdsResult},
    kryphocron::{CapabilityClass, KryphocronWriteAuthorization},
    oauth::AtProtoScope,
};

/// Record collection NSIDs the four dedicated endpoints write to.
/// Held as constants so the deny-map population
/// ([`crate::kryphocron::build_deny_map`]) can route the same
/// names through the `RequiresDedicatedEndpoint` rewrite without
/// duplicating string literals.
pub(crate) const NSID_POST_PRIVATE: &str = "tools.kryphocron.feed.postPrivate";
pub(crate) const NSID_AUDIENCE: &str = "tools.kryphocron.policy.audience";

/// XRPC procedure NSIDs (the dedicated-endpoint paths the
/// `RequiresDedicatedEndpoint` suggested_endpoint field points to).
pub(crate) const PROC_CREATE_POST_PRIVATE: &str = "tools.kryphocron.feed.createPostPrivate";
pub(crate) const PROC_DELETE_POST_PRIVATE: &str = "tools.kryphocron.feed.deletePostPrivate";
pub(crate) const PROC_PARTICIPATE_PRIVATE: &str = "tools.kryphocron.actor.participatePrivate";
pub(crate) const PROC_MANAGE_AUDIENCE: &str = "tools.kryphocron.policy.manageAudience";

/// Build the four route bindings. Plain `.route(...)` per the arc
/// 2 recon R2 finding: user-class endpoints aren't advertised in
/// `RouteRegistry` (which is admin-tier only).
pub fn routes() -> Router<AppContext> {
    Router::new()
        .route(
            &format!("/xrpc/{PROC_CREATE_POST_PRIVATE}"),
            post(create_post_private),
        )
        .route(
            &format!("/xrpc/{PROC_DELETE_POST_PRIVATE}"),
            post(delete_post_private),
        )
        .route(
            &format!("/xrpc/{PROC_PARTICIPATE_PRIVATE}"),
            post(participate_private),
        )
        .route(
            &format!("/xrpc/{PROC_MANAGE_AUDIENCE}"),
            post(manage_audience),
        )
}

/// Common request shape for the three create-style endpoints:
/// the importing repo's DID, an optional rkey, and the record body
/// the substrate's lexicons will validate.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateLikeRequest {
    repo: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    rkey: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    validate: Option<bool>,
    record: serde_json::Value,
}

/// Delete-shaped request: repo + rkey of the record to delete.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteLikeRequest {
    repo: String,
    rkey: String,
}

/// Bsky-style createRecord response: uri + cid.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WriteResponse {
    uri: String,
    cid: String,
}

/// Helper — verify caller is authenticated, OAuth scope is OK,
/// and `req.repo` matches the authenticated DID. Returns the DID
/// for the handler to use downstream.
async fn authenticated_did_for_repo(
    ctx: &AppContext,
    headers: HeaderMap,
    requested_repo: &str,
    scope: AtProtoScope,
) -> PdsResult<String> {
    let auth = middleware::require_auth_unified(State(ctx.clone()), headers).await?;
    middleware::enforce_scope(&auth, &scope)?;
    if auth.is_cross_pds() {
        ctx.rate_limiter.check_cross_pds()?;
    }
    let auth_did = auth.did();
    if requested_repo != auth_did {
        return Err(PdsError::Authorization(
            "Cannot write into another user's repo via kryphocron \
             dedicated endpoint"
                .to_string(),
        ));
    }
    Ok(auth_did.to_string())
}

/// Helper — apply a single kryphocron-authorized create `WriteOp`
/// and return the (uri, commit_cid) response. Wraps the
/// `repo_mgr.apply_writes` call so the four create-style endpoints
/// share the same plumbing.
async fn apply_single_create(
    ctx: &AppContext,
    auth_did: &str,
    collection: &str,
    rkey: Option<String>,
    record: serde_json::Value,
    validate: Option<bool>,
) -> PdsResult<WriteResponse> {
    let rkey = rkey.unwrap_or_else(|| next_tid(None).to_string());
    let repo_mgr = RepositoryManager::for_writer(ctx, auth_did.to_string());
    let signer = create_actor_signer(&ctx.account_manager, auth_did).await?;

    let write = WriteOp {
        action: WriteOpAction::Create,
        collection: collection.to_string(),
        rkey: rkey.clone(),
        value: Some(record),
        validate,
        swap_cid: None,
        kryphocron_authorization: Some(KryphocronWriteAuthorization::DedicatedEndpoint {
            capability_class: CapabilityClass::User,
        }),
    };

    let (commit_cid, _rev) = repo_mgr
        .apply_writes(
            vec![write],
            signer,
            Arc::new(crate::blob_store::StrictPromoter),
        )
        .await?;

    let uri = format!("at://{auth_did}/{collection}/{rkey}");
    Ok(WriteResponse {
        uri,
        cid: commit_cid,
    })
}

/// `tools.kryphocron.feed.createPostPrivate` — create a
/// `tools.kryphocron.feed.postPrivate` record under the
/// `EditPrivatePost` (user-class) capability. The bind pipeline
/// runs during `apply_writes`'s validate loop; the audit-emit
/// payload arrives at step 7.
async fn create_post_private(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(req): Json<CreateLikeRequest>,
) -> PdsResult<Json<WriteResponse>> {
    let auth_did =
        authenticated_did_for_repo(&ctx, headers, &req.repo, AtProtoScope::RepoCreate).await?;
    let resp = apply_single_create(
        &ctx,
        &auth_did,
        NSID_POST_PRIVATE,
        req.rkey,
        req.record,
        req.validate,
    )
    .await?;
    Ok(Json(resp))
}

/// `tools.kryphocron.feed.deletePostPrivate` — delete a
/// `tools.kryphocron.feed.postPrivate` record under the
/// `DeletePrivatePost` (user-class) capability. Constructs the
/// delete `WriteOp` with the same `DedicatedEndpoint`
/// authorization so the dispatcher routes through
/// `bind_pipeline` rather than the deny-map.
async fn delete_post_private(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(req): Json<DeleteLikeRequest>,
) -> PdsResult<Json<serde_json::Value>> {
    let auth_did =
        authenticated_did_for_repo(&ctx, headers, &req.repo, AtProtoScope::RepoDelete).await?;
    let repo_mgr = RepositoryManager::for_writer(&ctx, auth_did.clone());
    let signer = create_actor_signer(&ctx.account_manager, &auth_did).await?;

    let write = WriteOp {
        action: WriteOpAction::Delete,
        collection: NSID_POST_PRIVATE.to_string(),
        rkey: req.rkey.clone(),
        value: None,
        validate: None,
        swap_cid: None,
        kryphocron_authorization: Some(KryphocronWriteAuthorization::DedicatedEndpoint {
            capability_class: CapabilityClass::User,
        }),
    };

    repo_mgr
        .apply_writes(
            vec![write],
            signer,
            Arc::new(crate::blob_store::StrictPromoter),
        )
        .await?;

    Ok(Json(serde_json::json!({})))
}

/// `tools.kryphocron.actor.participatePrivate` — participate in a
/// private thread (write a reply / participation marker) under the
/// `ParticipatePrivate` (user-class) capability. Step 5 ships the
/// endpoint that routes the WriteOp through `bind_pipeline`; the
/// exact reply-vs-create split + the parent-post audience-oracle
/// integration arrive at step 7 when the bind pipeline stages
/// land. The endpoint accepts the same `record` body shape as
/// `createPostPrivate` (a `postPrivate` record) and writes to
/// `tools.kryphocron.feed.postPrivate`.
async fn participate_private(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(req): Json<CreateLikeRequest>,
) -> PdsResult<Json<WriteResponse>> {
    let auth_did =
        authenticated_did_for_repo(&ctx, headers, &req.repo, AtProtoScope::RepoCreate).await?;
    let resp = apply_single_create(
        &ctx,
        &auth_did,
        NSID_POST_PRIVATE,
        req.rkey,
        req.record,
        req.validate,
    )
    .await?;
    Ok(Json(resp))
}

/// `tools.kryphocron.policy.manageAudience` — create or update a
/// `tools.kryphocron.policy.audience` record under the
/// `ManageAudience` (user-class) capability. Step 5 ships the
/// create path; the cascade machinery for audience-update cascade-
/// reassign (per `v07_DESIGN.md` §7a) is the post-arc-2 cycle's
/// concern.
async fn manage_audience(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(req): Json<CreateLikeRequest>,
) -> PdsResult<Json<WriteResponse>> {
    let auth_did =
        authenticated_did_for_repo(&ctx, headers, &req.repo, AtProtoScope::RepoCreate).await?;
    let resp = apply_single_create(
        &ctx,
        &auth_did,
        NSID_AUDIENCE,
        req.rkey,
        req.record,
        req.validate,
    )
    .await?;
    Ok(Json(resp))
}
