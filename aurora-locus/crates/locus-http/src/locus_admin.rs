//! Locus's own operator methods, under `app.nearhorizon.locus`, grouped by
//! the operator role they need: `.viewer.*`, `.moderator.*`, `.admin.*` and
//! `.owner.*` (each needs that role or higher, from the group prefix). The
//! console and the CLI both use these, so anything one can do the other
//! can. Operator tokens only: the legacy admin password doesn't reach
//! them.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::routing::{get, post};
use locus_auth::password;
use locus_core::locus_nsid;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::admin_auth::{self, Caller, Role, Surface};
use crate::error::{XrpcError, XrpcResult};
use crate::state::AppState;

type St = State<Arc<AppState>>;

/// The role a method's group needs.
fn group_role(lxm: &str) -> Role {
    let rest = lxm.strip_prefix(locus_core::LOCUS_NS).unwrap_or(lxm);
    match rest.trim_start_matches('.').split('.').next() {
        Some("owner") => Role::Owner,
        Some("admin") => Role::Admin,
        Some("moderator") => Role::Moderator,
        _ => Role::Viewer,
    }
}

async fn op(s: &AppState, headers: &HeaderMap, lxm: &str) -> XrpcResult<Caller> {
    admin_auth::require(s, headers, lxm, Surface::Locus, group_role(lxm)).await
}

/// Every locus operator route.
pub fn routes() -> axum::Router<Arc<AppState>> {
    macro_rules! at {
        ($name:literal) => {
            concat!("/xrpc/", locus_nsid!($name))
        };
    }
    axum::Router::new()
        .route(at!("describeCapabilities"), get(describe_capabilities))
        .route(
            at!("shard.setCallbackToken"),
            post(crate::shard::set_callback_token),
        )
        .route(
            at!("shard.revokeSessions"),
            post(crate::shard::revoke_sessions),
        )
        .route(
            at!("shard.setVerifyKeys"),
            post(crate::shard::set_verify_keys),
        )
        .route(
            at!("shard.setPrivateTierBlock"),
            post(crate::shard::set_private_tier_block),
        )
        .route(at!("viewer.getSettings"), get(get_settings))
        .route(at!("viewer.getAuditLog"), get(get_audit_log))
        .route(at!("viewer.listJobs"), get(list_jobs))
        .route(
            at!("viewer.account.getImportStatus"),
            get(get_import_status),
        )
        .route(at!("viewer.kryphocron.getStatus"), get(kryphocron_status))
        .route(at!("viewer.kryphocron.getAuditLog"), get(kryphocron_audit))
        .route(
            at!("moderator.account.setPrivateTierBlock"),
            post(set_private_tier_block),
        )
        .route(at!("moderator.review.listQueue"), get(list_review_queue))
        .route(at!("moderator.review.resolve"), post(resolve_review))
        .route(at!("admin.setSetting"), post(set_setting))
        .route(at!("admin.kryphocron.rotateNow"), post(rotate_now))
        .route(at!("admin.requestCrawl"), post(request_crawl))
        .route(at!("owner.listOperators"), get(list_operators))
        .route(at!("owner.createOperator"), post(create_operator))
        .route(at!("owner.updateOperator"), post(update_operator))
        .route(at!("owner.createApiToken"), post(create_api_token))
        .route(at!("owner.revokeApiToken"), post(revoke_api_token))
        .route(at!("owner.keys.show"), get(crate::keys::show))
        .route(
            at!("owner.hostname.prepare"),
            post(crate::hostnames::prepare),
        )
        .route(
            at!("owner.hostname.confirm"),
            post(crate::hostnames::confirm),
        )
        .route(at!("owner.hostname.cancel"), post(crate::hostnames::cancel))
        .route(at!("owner.hostname.close"), post(crate::hostnames::close))
        .route(at!("viewer.hostname.status"), get(crate::hostnames::status))
        .route(
            at!("admin.migrateBlobs.start"),
            post(crate::blob_migration::start),
        )
        .route(
            at!("viewer.migrateBlobs.status"),
            get(crate::blob_migration::status),
        )
        .route(
            at!("admin.migrateBlobs.cancel"),
            post(crate::blob_migration::cancel),
        )
        .route(
            at!("admin.migrateBlobs.acceptMissing"),
            post(crate::blob_migration::accept_missing),
        )
        .route(at!("owner.backup.create"), post(crate::backups::create))
        .route(at!("viewer.backup.list"), get(crate::backups::list))
        .route(at!("owner.keys.setSecret"), post(crate::keys::set_secret))
        .route(
            at!("owner.keys.rotateServiceKey"),
            post(crate::keys::rotate_service_key),
        )
        .route(at!("owner.keys.import"), post(crate::keys::import))
        .route(
            at!("admin.keys.rotateAccountKeys"),
            post(crate::keys::rotate_account_keys),
        )
        .route(
            at!("admin.identity.publish"),
            post(crate::keys::publish_identity),
        )
        .route(at!("owner.keys.backup"), post(crate::keys::backup))
        .route(
            at!("owner.keys.setPlcRotationKey"),
            post(crate::keys::set_plc_rotation_key),
        )
        .route(
            at!("owner.keys.dropPlcKey"),
            post(crate::keys::drop_plc_key),
        )
}

// ---------------------------------------------------------------- ungated

/// What this server is: a locus, its version and namespace, the roles.
pub async fn describe_capabilities(State(s): St) -> Json<Value> {
    Json(json!({
        "locus": true,
        "version": env!("CARGO_PKG_VERSION"),
        "namespace": locus_core::LOCUS_NS,
        "roles": ["owner", "admin", "moderator", "viewer"],
        "role": match s.role {
            locus_core::bootstrap::Role::Pds => "pds",
            locus_core::bootstrap::Role::Shard => "shard",
            locus_core::bootstrap::Role::Entryway => "entryway",
        },
        "shardCallbacks": s.entryway.is_some(),
        "kryphocron": {
            "privateTier": crate::pipethrough::appview(&s).is_some() && s.snapshot().bool("kryphocron.private_tier_enabled"),
            "mediaGate": crate::media_gate::gate_on(&s),
        },
    }))
}

// ---------------------------------------------------------------- viewer

/// Every setting: value, default, kind and description. Secrets aren't
/// settings, so nothing here is sensitive.
pub async fn get_settings(State(s): St, headers: HeaderMap) -> XrpcResult<Json<Value>> {
    op(&s, &headers, locus_nsid!("viewer.getSettings")).await?;
    let snap = s.snapshot();
    let settings: Vec<Value> = locus_core::settings::REGISTRY
        .iter()
        .map(|d| {
            json!({
                "key": d.key,
                "value": snap.get(d.key),
                "default": d.default_value(),
                "env": d.env,
                "description": d.description,
            })
        })
        .collect();
    Ok(Json(
        json!({"version": snap.version(), "settings": settings}),
    ))
}

#[derive(Deserialize)]
pub struct PageParams {
    cursor: Option<String>,
    limit: Option<i64>,
}

pub async fn get_audit_log(
    State(s): St,
    headers: HeaderMap,
    Query(p): Query<PageParams>,
) -> XrpcResult<Json<Value>> {
    op(&s, &headers, locus_nsid!("viewer.getAuditLog")).await?;
    let limit = p.limit.unwrap_or(100).clamp(1, 1000);
    let before = p.cursor.as_deref().and_then(|c| c.parse().ok());
    let entries = s.store.audit_entries(before, limit).await?;
    let mut out = json!({"entries": entries.iter().map(|e| json!({
        "id": e.id,
        "at": locus_core::time::iso8601(e.at),
        "actor": e.actor,
        "action": e.action,
        "target": e.target,
        "detail": e.detail_json.as_deref().and_then(|d| serde_json::from_str::<Value>(d).ok()),
    })).collect::<Vec<_>>()});
    if entries.len() as i64 == limit
        && let Some(last) = entries.last()
    {
        out["cursor"] = json!(last.id.to_string());
    }
    Ok(Json(out))
}

pub async fn list_jobs(State(s): St, headers: HeaderMap) -> XrpcResult<Json<Value>> {
    op(&s, &headers, locus_nsid!("viewer.listJobs")).await?;
    let iso = |t: Option<i64>| t.map(locus_core::time::iso8601);
    let jobs: Vec<Value> = s
        .store
        .job_states()
        .await?
        .into_iter()
        .map(|j| json!({
            "name": j.name, "runs": j.runs, "lastRunAt": iso(j.last_run_at), "lastOkAt": iso(j.last_ok_at),
            "nextRunAt": iso(j.next_run_at), "lastError": j.last_error,
        }))
        .collect();
    Ok(Json(json!({"jobs": jobs})))
}

#[derive(Deserialize)]
pub struct DidParam {
    did: String,
}

/// Any account's latest import (the CLI's `account import-status`).
pub async fn get_import_status(
    State(s): St,
    headers: HeaderMap,
    Query(p): Query<DidParam>,
) -> XrpcResult<Json<Value>> {
    op(&s, &headers, locus_nsid!("viewer.account.getImportStatus")).await?;
    crate::import::import_status(&s, &p.did).await.map(Json)
}

/// The private tier at a glance: switches, the AppView's availability, the
/// rotation generation, records rotation couldn't rewrite.
pub async fn kryphocron_status(State(s): St, headers: HeaderMap) -> XrpcResult<Json<Value>> {
    op(&s, &headers, locus_nsid!("viewer.kryphocron.getStatus")).await?;
    let snap = s.snapshot();
    let generation = s.store.current_generation().await?;
    let flagged = s.store.flagged_private_records(100).await?;
    Ok(Json(json!({
        "appview": crate::pipethrough::appview(&s).map(|(_, d)| d),
        "privateTierEnabled": snap.bool("kryphocron.private_tier_enabled"),
        "mediaGate": {"setting": snap.bool("kryphocron.private_media_gated"), "effective": crate::media_gate::gate_on(&s)},
        "prism": {"down": s.kr.prism.is_down(), "changedAt": match s.kr.prism.changed_at() {
            0 => Value::Null,
            t => json!(locus_core::time::iso8601(t)),
        }},
        "rotation": {
            "periodDays": snap.int("kryphocron.rotation_period_days"),
            "generation": generation.as_ref().map(|(g, _)| g),
            "generatedAt": generation.map(|(_, t)| locus_core::time::iso8601(t)),
        },
        "flaggedRecords": flagged.iter().map(|r| json!({
            "uri": format!("at://{}/{}/{}", r.did, r.collection, r.rkey), "why": r.flagged,
        })).collect::<Vec<_>>(),
        "auditRejected": s.kr.audit.rejected(),
    })))
}

pub async fn kryphocron_audit(
    State(s): St,
    headers: HeaderMap,
    Query(p): Query<PageParams>,
) -> XrpcResult<Json<Value>> {
    op(&s, &headers, locus_nsid!("viewer.kryphocron.getAuditLog")).await?;
    let rows = s
        .store
        .kr_audit(p.limit.unwrap_or(100).clamp(1, 1000))
        .await?;
    Ok(Json(json!({"events": rows.iter().map(|r| json!({
        "at": locus_core::time::iso8601(r.at), "sink": r.sink, "event": r.event,
        "correlationId": r.correlation_id, "detail": serde_json::from_str::<Value>(&r.detail_json).unwrap_or(Value::Null),
    })).collect::<Vec<_>>()})))
}

// ---------------------------------------------------------------- moderator

#[derive(Deserialize)]
pub struct BlockInput {
    did: String,
    blocked: bool,
}

/// The local block on an account's private-tier writes (the AppView's
/// permission is the AppView's; locus can only add a block).
pub async fn set_private_tier_block(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<BlockInput>,
) -> XrpcResult<Json<Value>> {
    let who = op(
        &s,
        &headers,
        locus_nsid!("moderator.account.setPrivateTierBlock"),
    )
    .await?;
    if crate::shard::block_is_entryways(&s).await? {
        return Err(XrpcError::invalid_request(
            "This shard's private-tier blocks are set on the entryway",
        ));
    }
    s.store
        .account_by_did(&input.did)
        .await?
        .ok_or_else(|| XrpcError::invalid_request("Account not found"))?;
    s.store
        .set_private_tier_blocked(&input.did, input.blocked)
        .await?;
    // On an entryway, the account's locus shard is told.
    crate::entryway::queue_block(&s, &input.did, input.blocked).await?;
    admin_auth::audit(
        &s,
        &who,
        "account.setPrivateTierBlock",
        Some(&input.did),
        Some(json!({"blocked": input.blocked})),
    )
    .await;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
pub struct QueueParams {
    status: Option<String>,
    limit: Option<i64>,
}

/// Blobs a scanner matched (matches can be false positives).
pub async fn list_review_queue(
    State(s): St,
    headers: HeaderMap,
    Query(p): Query<QueueParams>,
) -> XrpcResult<Json<Value>> {
    op(&s, &headers, locus_nsid!("moderator.review.listQueue")).await?;
    let rows = s
        .store
        .reviews(
            p.status.as_deref().unwrap_or("open"),
            p.limit.unwrap_or(100).clamp(1, 1000),
        )
        .await?;
    Ok(Json(json!({"reviews": rows.iter().map(|r| json!({
        "id": r.id, "did": r.did, "cid": r.cid, "createdAt": locus_core::time::iso8601(r.created_at),
        "source": r.source, "result": r.result, "reason": r.reason, "status": r.status,
        "resolvedBy": r.resolved_by, "note": r.note,
    })).collect::<Vec<_>>()})))
}

#[derive(Deserialize)]
pub struct ResolveInput {
    id: i64,
    /// `cleared` (a false positive: released) or `reported`.
    status: String,
    note: Option<String>,
}

/// Resolves a review. A cleared match is released (no longer held or
/// quarantined); a reported one stays quarantined.
pub async fn resolve_review(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<ResolveInput>,
) -> XrpcResult<Json<Value>> {
    let who = op(&s, &headers, locus_nsid!("moderator.review.resolve")).await?;
    if !matches!(input.status.as_str(), "cleared" | "reported") {
        return Err(XrpcError::invalid_request("status is cleared or reported"));
    }
    let review = s
        .store
        .reviews("open", 100_000)
        .await?
        .into_iter()
        .find(|r| r.id == input.id)
        .ok_or_else(|| XrpcError::invalid_request("No open review with that id"))?;
    if !s
        .store
        .resolve_review(input.id, &input.status, &who.actor, input.note.as_deref())
        .await?
    {
        return Err(XrpcError::invalid_request("No open review with that id"));
    }
    if input.status == "cleared" {
        s.store
            .release_quarantined_blob(&review.did, &review.cid)
            .await?;
        crate::repo::place_blob(&s, &review.did, &review.cid).await;
    }
    admin_auth::audit(
        &s,
        &who,
        "review.resolve",
        Some(&format!("{}/{}", review.did, review.cid)),
        Some(json!({"id": input.id, "status": input.status, "note": input.note})),
    )
    .await;
    Ok(Json(json!({})))
}

// ---------------------------------------------------------------- admin

#[derive(Deserialize)]
pub struct SettingInput {
    key: String,
    value: Value,
}

/// Changes a setting (validated; audited with before and after).
pub async fn set_setting(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<SettingInput>,
) -> XrpcResult<Json<Value>> {
    let who = op(&s, &headers, locus_nsid!("admin.setSetting")).await?;
    let def = locus_core::settings::def(&input.key)
        .map_err(|e| XrpcError::invalid_request(e.to_string()))?;
    let value = def
        .validate(input.value)
        .map_err(|e| XrpcError::invalid_request(e.to_string()))?;
    if input.key == "kryphocron.private_media_gated"
        && value == json!(true)
        && s.snapshot().text("blobs.scanner") == "none"
    {
        return Err(XrpcError::invalid_request(
            "The media gate needs a scanner: configure blobs.scanner first",
        ));
    }
    if input.key == "blobs.scanner"
        && value == json!("none")
        && s.snapshot().bool("kryphocron.private_media_gated")
    {
        return Err(XrpcError::invalid_request(
            "Turn the media gate off before removing the scanner",
        ));
    }
    let before = s.snapshot().get(&input.key).clone();
    let version = s
        .settings
        .set(&input.key, value.clone(), &who.actor)
        .await?;
    admin_auth::audit(
        &s,
        &who,
        "setting.change",
        Some(&input.key),
        Some(json!({"before": before, "after": value})),
    )
    .await;
    Ok(Json(json!({"version": version})))
}

/// Asks every configured relay to crawl this PDS now.
pub async fn request_crawl(State(s): St, headers: HeaderMap) -> XrpcResult<Json<Value>> {
    let who = op(&s, &headers, locus_nsid!("admin.requestCrawl")).await?;
    let crawlers: Vec<String> = s
        .snapshot()
        .list("sync.crawlers")
        .into_iter()
        .map(str::to_owned)
        .collect();
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| XrpcError::internal(e.to_string()))?;
    let mut results = Vec::new();
    for c in &crawlers {
        let r = locus_sync::request_crawl(&http, c, &s.hostname).await;
        results.push(json!({"crawler": c, "ok": r.is_ok(), "error": r.err()}));
    }
    admin_auth::audit(
        &s,
        &who,
        "network.requestCrawl",
        None,
        Some(json!({"crawlers": crawlers})),
    )
    .await;
    Ok(Json(json!({"results": results})))
}

/// Starts a new rotation generation now (the rotation job re-encodes
/// under it, spread over the period).
pub async fn rotate_now(State(s): St, headers: HeaderMap) -> XrpcResult<Json<Value>> {
    let who = op(&s, &headers, locus_nsid!("admin.kryphocron.rotateNow")).await?;
    let now = locus_core::time::now_ms();
    let current = s.store.current_generation().await?.map(|(g, _)| g);
    let mark = locus_kryphocron::rotation::next_mark(
        current.as_deref(),
        u64::try_from(now / 1000).unwrap_or(0),
    );
    s.store.add_generation(&mark, now).await?;
    crate::kryphocron::refresh_oracle(&s).await?;
    admin_auth::audit(
        &s,
        &who,
        "kryphocron.rotateNow",
        None,
        Some(json!({"generation": mark})),
    )
    .await;
    Ok(Json(json!({"generation": mark})))
}

// ---------------------------------------------------------------- owner

pub async fn list_operators(State(s): St, headers: HeaderMap) -> XrpcResult<Json<Value>> {
    op(&s, &headers, locus_nsid!("owner.listOperators")).await?;
    let mut out = Vec::new();
    for o in s.store.operators().await? {
        let tokens: Vec<Value> = s.store.operator_tokens(&o.username).await?.iter().map(|t| json!({
            "id": t.id, "name": t.name, "createdAt": locus_core::time::iso8601(t.created_at),
            "lastUsedAt": t.last_used_at.map(locus_core::time::iso8601),
            "expiresAt": t.expires_at.map(locus_core::time::iso8601),
        })).collect();
        out.push(json!({
            "username": o.username, "role": o.role, "disabled": o.disabled, "totp": o.totp_secret.is_some(),
            "createdAt": locus_core::time::iso8601(o.created_at), "tokens": tokens,
        }));
    }
    Ok(Json(json!({"operators": out})))
}

#[derive(Deserialize)]
pub struct OperatorInput {
    username: String,
    password: String,
    role: String,
}

fn check_username(u: &str) -> XrpcResult<()> {
    let ok = (1..=64).contains(&u.len())
        && u.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b));
    if ok {
        Ok(())
    } else {
        Err(XrpcError::invalid_request(
            "Usernames are 1–64 letters, digits, '-', '_' or '.'",
        ))
    }
}

pub async fn create_operator(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<OperatorInput>,
) -> XrpcResult<Json<Value>> {
    let who = op(&s, &headers, locus_nsid!("owner.createOperator")).await?;
    check_username(&input.username)?;
    let role =
        Role::parse(&input.role).ok_or_else(|| XrpcError::invalid_request("Unknown role"))?;
    if input.password.len() < 12 || input.password.len() > password::MAX_NEW_PASSWORD {
        return Err(XrpcError::invalid_request(
            "Operator passwords are at least 12 characters",
        ));
    }
    let hash = password::hash(&input.password).map_err(XrpcError::internal)?;
    if !s
        .store
        .create_operator(&input.username, &hash, role.as_str())
        .await?
    {
        return Err(XrpcError::invalid_request("That username is taken"));
    }
    admin_auth::audit(
        &s,
        &who,
        "operator.create",
        Some(&input.username),
        Some(json!({"role": role.as_str()})),
    )
    .await;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
pub struct UpdateOperatorInput {
    username: String,
    role: Option<String>,
    password: Option<String>,
    disabled: Option<bool>,
}

/// Changes an operator; the last enabled owner can't be demoted or
/// disabled.
pub async fn update_operator(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<UpdateOperatorInput>,
) -> XrpcResult<Json<Value>> {
    let who = op(&s, &headers, locus_nsid!("owner.updateOperator")).await?;
    let current = s
        .store
        .operator(&input.username)
        .await?
        .ok_or_else(|| XrpcError::invalid_request("No such operator"))?;
    let role = input
        .role
        .as_deref()
        .map(|r| Role::parse(r).ok_or_else(|| XrpcError::invalid_request("Unknown role")))
        .transpose()?;
    let loses_owner = current.role == "owner"
        && !current.disabled
        && (role.is_some_and(|r| r != Role::Owner) || input.disabled == Some(true));
    if loses_owner && s.store.count_owners().await? <= 1 {
        return Err(XrpcError::invalid_request(
            "There must be at least one owner",
        ));
    }
    let hash = match &input.password {
        Some(p) if p.len() < 12 || p.len() > password::MAX_NEW_PASSWORD => {
            return Err(XrpcError::invalid_request(
                "Operator passwords are at least 12 characters",
            ));
        }
        Some(p) => Some(password::hash(p).map_err(XrpcError::internal)?),
        None => None,
    };
    s.store
        .update_operator(
            &input.username,
            role.map(Role::as_str),
            hash.as_deref(),
            input.disabled,
        )
        .await?;
    // A new password or a disabled account ends the operator's console
    // sign-ins.
    if hash.is_some() || input.disabled == Some(true) {
        s.store
            .delete_operator_sessions(&input.username, None)
            .await?;
    }
    admin_auth::audit(
        &s,
        &who,
        "operator.update",
        Some(&input.username),
        Some(json!({"role": input.role, "disabled": input.disabled, "password": input.password.is_some()})),
    )
    .await;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenInput {
    operator: String,
    name: String,
    expires_in_days: Option<i64>,
}

/// A new API token; its value is shown once.
pub async fn create_api_token(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<TokenInput>,
) -> XrpcResult<Json<Value>> {
    let who = op(&s, &headers, locus_nsid!("owner.createApiToken")).await?;
    s.store
        .operator(&input.operator)
        .await?
        .ok_or_else(|| XrpcError::invalid_request("No such operator"))?;
    let token = admin_auth::new_token();
    let id = hex::encode(rand::random::<[u8; 8]>());
    let expires = input
        .expires_in_days
        .map(|d| locus_core::time::now_ms() + d.clamp(1, 3650) * 86_400_000);
    s.store
        .create_operator_token(
            &id,
            &admin_auth::token_hash(&token),
            &input.operator,
            &input.name,
            expires,
        )
        .await?;
    admin_auth::audit(
        &s,
        &who,
        "operator.createToken",
        Some(&input.operator),
        Some(json!({"id": id, "name": input.name})),
    )
    .await;
    Ok(Json(json!({"id": id, "token": token})))
}

#[derive(Deserialize)]
pub struct RevokeInput {
    id: String,
}

pub async fn revoke_api_token(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<RevokeInput>,
) -> XrpcResult<Json<Value>> {
    let who = op(&s, &headers, locus_nsid!("owner.revokeApiToken")).await?;
    if !s.store.revoke_operator_token(&input.id).await? {
        return Err(XrpcError::invalid_request("No such token"));
    }
    admin_auth::audit(&s, &who, "operator.revokeToken", Some(&input.id), None).await;
    Ok(Json(json!({})))
}
