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

    // #236 — encode-on-write floor. Fix the rkey up-front (so the
    // at-rest content context's rkey matches the stored record), then
    // run the record's `text` through the installed codec before it
    // reaches the write path. No-op when kryphocron is disabled.
    let rkey = req.rkey.unwrap_or_else(|| next_tid(None).to_string());
    let mut record = req.record;
    crate::kryphocron_content::encode_private_content(
        &ctx,
        &auth_did,
        NSID_POST_PRIVATE,
        &rkey,
        &mut record,
    )
    .await?;

    let resp = apply_single_create(
        &ctx,
        &auth_did,
        NSID_POST_PRIVATE,
        Some(rkey),
        record,
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
/// private thread under the `ParticipatePrivate` (user-class)
/// capability.
///
/// **Reply-vs-create discrimination.** The endpoint REQUIRES the
/// supplied `record` to carry a `reply.parent.uri` field (the
/// `postPrivate` lexicon's reply ref per
/// `tools.kryphocron.feed.postPublic#replyRef`). Posts without a
/// reply parent are rejected — that shape is what
/// `createPostPrivate` is for (capability `EditPrivatePost`).
/// The presence of `reply.parent.uri` is the structural
/// discriminator the design uses to identify "participating in
/// someone else's thread" vs "creating a fresh post".
///
/// **Audience-oracle integration.** Before the bind pipeline
/// runs, the host-side audience check inspects the parent post's
/// `audienceList` and verifies the requester DID is in the
/// referenced audience. On rejection the handler emits
/// `KryphocronAudienceCheckDenied` (per design §4) in its own
/// short tx on the shared account DB and returns HTTP 403. The
/// 403's body is intentionally coarse-grained (per design §10
/// "BindDenied diagnostic ambiguity") to avoid leaking audience
/// membership info.
///
/// **Local vs cross-DID parents.** Arc 2 step 7 implements the
/// check for parents whose owner DID is a local account. Cross-
/// DID parents (the writer is participating in a thread hosted
/// on another PDS) are deferred with a `tracing::warn!` —
/// federation-backed audience read-through is post-arc-2 work.
/// The deferred path allows the participation; the warning makes
/// the deferred check visible in operator logs so the gap
/// doesn't sit silent.
async fn participate_private(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(req): Json<CreateLikeRequest>,
) -> PdsResult<Json<WriteResponse>> {
    let auth_did =
        authenticated_did_for_repo(&ctx, headers, &req.repo, AtProtoScope::RepoCreate).await?;

    // Reply-vs-create discrimination: extract the reply parent URI.
    // Reject if absent — participatePrivate is for participating
    // in someone else's thread, not for fresh posts.
    let parent_uri = req
        .record
        .get("reply")
        .and_then(|r| r.get("parent"))
        .and_then(|p| p.get("uri"))
        .and_then(|u| u.as_str())
        .ok_or_else(|| {
            PdsError::Validation(
                "participatePrivate requires record.reply.parent.uri \
                 (the post being participated in)"
                    .to_string(),
            )
        })?
        .to_string();

    match check_participate_audience(&ctx, &parent_uri, &auth_did).await? {
        ParticipateAudienceOutcome::Allowed => {}
        ParticipateAudienceOutcome::DeferredCrossDid { parent_owner } => {
            tracing::warn!(
                target: "aurora_locus::kryphocron",
                event = "participate_private_audience_check_deferred",
                requester_did = %auth_did,
                parent_uri = %parent_uri,
                parent_owner = %parent_owner,
                reason = "cross_did_audience_lookup_not_yet_wired",
                "audience-oracle check deferred — parent owner is \
                 not a local actor; arc 2 step 7 ships local-DID \
                 lookup only, federation-backed lookup is post-arc-2",
            );
        }
        ParticipateAudienceOutcome::Denied(payload) => {
            let mut tx = ctx
                .account_db
                .begin()
                .await
                .map_err(PdsError::Database)?;
            crate::kryphocron_audit::emit_audience_check_denied_in_tx(
                &mut tx, &auth_did, payload,
            )
            .await?;
            tx.commit().await.map_err(PdsError::Database)?;
            return Err(PdsError::Authorization(
                "Audience check denied".to_string(),
            ));
        }
    }

    // #236 — encode-on-write floor (same seam as create_post_private),
    // applied after the audience pre-check passes and before the write.
    let rkey = req.rkey.unwrap_or_else(|| next_tid(None).to_string());
    let mut record = req.record;
    crate::kryphocron_content::encode_private_content(
        &ctx,
        &auth_did,
        NSID_POST_PRIVATE,
        &rkey,
        &mut record,
    )
    .await?;

    let resp = apply_single_create(
        &ctx,
        &auth_did,
        NSID_POST_PRIVATE,
        Some(rkey),
        record,
        req.validate,
    )
    .await?;
    Ok(Json(resp))
}

/// Outcome of the host-side audience-oracle pre-check that runs
/// before `participatePrivate` invokes the bind pipeline. See
/// [`check_participate_audience`].
#[derive(Debug)]
enum ParticipateAudienceOutcome {
    /// Requester is in the parent's audience — proceed.
    Allowed,
    /// Parent owner is not a local actor; cross-DID audience
    /// lookup is deferred to a post-arc-2 cycle. The
    /// participation is allowed; the deferred check is logged
    /// via tracing.
    DeferredCrossDid { parent_owner: String },
    /// Parent post or audience is misconfigured, or the
    /// requester is not a member. The handler emits the
    /// audience-check-denied event and returns HTTP 403.
    Denied(crate::kryphocron_audit::AudienceCheckDeniedPayload),
}

/// Host-side audience check for participatePrivate per
/// `v07_DESIGN.md` §3 "Where audience enforcement lives". Reads
/// the parent post record's `audienceList` field, looks up the
/// referenced audience record, and checks the requester DID
/// against the audience's member list.
///
/// Arc 2 step 7 implements the check for parents whose owner DID
/// is a local account. Cross-DID parents return
/// `DeferredCrossDid` — federation-backed audience read-through
/// is post-arc-2 work documented in `participate_private`'s
/// rustdoc.
async fn check_participate_audience(
    ctx: &AppContext,
    parent_uri: &str,
    requester_did: &str,
) -> PdsResult<ParticipateAudienceOutcome> {
    let parent_owner = parse_at_uri_did(parent_uri).ok_or_else(|| {
        PdsError::Validation(format!("invalid parent URI: {parent_uri}"))
    })?;

    if !ctx.actor_store.exists(&parent_owner).await {
        return Ok(ParticipateAudienceOutcome::DeferredCrossDid { parent_owner });
    }

    let parent_record = match ctx
        .actor_store
        .get_record(&parent_owner, parent_uri)
        .await?
    {
        Some(r) => r,
        None => {
            return Ok(ParticipateAudienceOutcome::Denied(
                build_denied_payload(parent_uri, requester_did, None, "unknown"),
            ));
        }
    };

    let parent_block_bytes = match ctx
        .actor_store
        .get_block(&parent_owner, &parent_record.cid)
        .await?
    {
        Some(b) => b,
        None => {
            return Ok(ParticipateAudienceOutcome::Denied(
                build_denied_payload(parent_uri, requester_did, None, "unknown"),
            ));
        }
    };

    let parent_lex = proto_blue::lex_cbor::decode(&parent_block_bytes).map_err(|e| {
        PdsError::Internal(format!("decode parent post block: {e}"))
    })?;
    let parent_json = proto_blue::lex_json::lex_to_json(&parent_lex);
    let audience_uri = parent_json
        .get("audienceList")
        .and_then(|v| v.get("uri").or(Some(v)))
        .and_then(|v| v.as_str())
        .map(String::from);

    let audience_uri = match audience_uri {
        Some(uri) => uri,
        None => {
            return Ok(ParticipateAudienceOutcome::Denied(
                build_denied_payload(parent_uri, requester_did, None, "unknown"),
            ));
        }
    };

    let audience_owner = parse_at_uri_did(&audience_uri).ok_or_else(|| {
        PdsError::Validation(format!("invalid audience URI: {audience_uri}"))
    })?;

    if !ctx.actor_store.exists(&audience_owner).await {
        return Ok(ParticipateAudienceOutcome::DeferredCrossDid {
            parent_owner: audience_owner,
        });
    }

    let audience_record = match ctx
        .actor_store
        .get_record(&audience_owner, &audience_uri)
        .await?
    {
        Some(r) => r,
        None => {
            return Ok(ParticipateAudienceOutcome::Denied(build_denied_payload(
                parent_uri,
                requester_did,
                Some(audience_uri.clone()),
                "unknown",
            )));
        }
    };

    let audience_block_bytes = match ctx
        .actor_store
        .get_block(&audience_owner, &audience_record.cid)
        .await?
    {
        Some(b) => b,
        None => {
            return Ok(ParticipateAudienceOutcome::Denied(build_denied_payload(
                parent_uri,
                requester_did,
                Some(audience_uri.clone()),
                "unknown",
            )));
        }
    };

    let audience_lex = proto_blue::lex_cbor::decode(&audience_block_bytes).map_err(|e| {
        PdsError::Internal(format!("decode audience record block: {e}"))
    })?;
    let audience_json = proto_blue::lex_json::lex_to_json(&audience_lex);
    let mode = audience_json
        .get("mode")
        .and_then(|v| v.as_str())
        .unwrap_or("list")
        .to_string();

    // Arc 2 step 7 implements list-mode membership check. Other
    // modes (`everyone`, `followers`, `following`, `nobody`)
    // need follow-graph / public-default semantics that are
    // post-arc-2 work; for those modes we conservatively defer
    // to `NoAudienceConfigured` so the request fails closed
    // until the per-mode logic ships.
    if mode != "list" {
        return Ok(ParticipateAudienceOutcome::Denied(build_denied_payload(
            parent_uri,
            requester_did,
            Some(audience_uri),
            &mode,
        )));
    }
    let is_member = audience_json
        .get("members")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().any(|m| m.as_str() == Some(requester_did)))
        .unwrap_or(false);

    if is_member {
        Ok(ParticipateAudienceOutcome::Allowed)
    } else {
        Ok(ParticipateAudienceOutcome::Denied(
            crate::kryphocron_audit::AudienceCheckDeniedPayload {
                capability_attempted: "ParticipatePrivate".to_string(),
                subject_uri: parent_uri.to_string(),
                requester_did: requester_did.to_string(),
                audience_uri: Some(audience_uri),
                audience_mode: mode,
                audience_check_result: crate::kryphocron_audit::AudienceCheckResult::NotInAudience,
                trace_id: crate::kryphocron_audit::synthesize_trace_id(),
                payload_completeness: crate::kryphocron_audit::PayloadCompleteness::Full,
            },
        ))
    }
}

/// Build a denial payload for the `NoAudienceConfigured` path.
/// Used when the parent post is missing, its block isn't
/// fetchable, the audience URI isn't extractable, or the audience
/// record/block isn't fetchable.
fn build_denied_payload(
    parent_uri: &str,
    requester_did: &str,
    audience_uri: Option<String>,
    audience_mode: &str,
) -> crate::kryphocron_audit::AudienceCheckDeniedPayload {
    crate::kryphocron_audit::AudienceCheckDeniedPayload {
        capability_attempted: "ParticipatePrivate".to_string(),
        subject_uri: parent_uri.to_string(),
        requester_did: requester_did.to_string(),
        audience_uri,
        audience_mode: audience_mode.to_string(),
        audience_check_result: crate::kryphocron_audit::AudienceCheckResult::NoAudienceConfigured,
        trace_id: crate::kryphocron_audit::synthesize_trace_id(),
        payload_completeness: crate::kryphocron_audit::PayloadCompleteness::Full,
    }
}

/// Extract the owner DID from an `at://<did>/<collection>/<rkey>`
/// URI. Returns `None` if the URI doesn't match the expected
/// shape. `pub(crate)` so the read-side authorization resolver
/// ([`crate::kryphocron_content`]) can reuse it (#237a).
pub(crate) fn parse_at_uri_did(uri: &str) -> Option<String> {
    let after_scheme = uri.strip_prefix("at://")?;
    let did = after_scheme.split('/').next()?;
    if did.starts_with("did:") {
        Some(did.to_string())
    } else {
        None
    }
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
