//! Kryphocron on the write paths: registry routing, host rules, the
//! private-tier gates, the `feed.postPrivate` pipeline, the dedicated
//! procedures, and the audit writer and rotation oracle refresh.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use locus_kryphocron::audit::{AuditEvent, AuditSinks};
use locus_kryphocron::{Encoder, Kind, PipelineError, SharedOracle, nsid, rules};
use locus_repo::Action;
use locus_repo::records;
use locus_store::kryphocron::{PermissionRow, SOURCE_MEMBERSHIP, SOURCE_PROBE};
use proto_blue_lex_data::LexValue;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::error::{XrpcError, XrpcResult};
use crate::repo;
use crate::state::AppState;

/// The codec, rotation oracle and audit sinks one node shares.
pub struct Kr {
    pub encoder: Encoder,
    pub audit: AuditSinks,
    /// Whether this node sees prism up.
    pub prism: crate::audience::PrismHealth,
    /// Wakes the audience sync job (prism back, a mapping to re-point).
    pub audience_sync_wake: tokio::sync::Notify,
    /// Short-lived membership answers (probes, misses, remote blocks).
    pub membership: crate::membership::Caches,
}

impl std::fmt::Debug for Kr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Kr").finish_non_exhaustive()
    }
}

impl Kr {
    /// The node's kryphocron state and the audit events its writer drains.
    #[must_use]
    pub fn new(correlation_key: [u8; 32]) -> (Arc<Self>, mpsc::Receiver<AuditEvent>) {
        let (audit, rx) = AuditSinks::new(correlation_key);
        let kr = Self {
            encoder: Encoder::new(Arc::new(SharedOracle::default())),
            audit,
            prism: crate::audience::PrismHealth::default(),
            audience_sync_wake: tokio::sync::Notify::new(),
            membership: crate::membership::Caches::default(),
        };
        (Arc::new(kr), rx)
    }
}

// ---------------------------------------------------------------- errors

fn unsupported() -> XrpcError {
    XrpcError::named(
        "UnsupportedNamespace",
        "This server doesn't have the private tier enabled",
    )
}

fn forbidden() -> XrpcError {
    XrpcError::new(
        StatusCode::FORBIDDEN,
        "Forbidden",
        "This account may not make private-tier writes",
    )
}

/// A retryable refusal.
fn retry(message: impl Into<String>) -> XrpcError {
    XrpcError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "TemporarilyUnavailable",
        message,
    )
}

fn pipeline_error(e: PipelineError) -> XrpcError {
    match e {
        PipelineError::Invalid(m) => XrpcError::invalid_request(m),
        PipelineError::Retry(m) => retry(m),
    }
}

// ---------------------------------------------------------------- the gate

/// What prism said, or why it said nothing.
#[derive(Debug)]
enum PrismAnswer {
    Membership {
        status: String,
        private_indexed: bool,
        private_read: bool,
        valid_for_secs: i64,
    },
    /// A named error answer (`AccountTakedown`, `NotEnrolled`, ...).
    Refused(String),
    /// The call reached nothing (connection refused, DNS): an outage.
    Unreachable,
    /// An answer that maps to nothing (429, 5xx, timeout).
    Unmapped(String),
}

/// `app.nearhorizon.actor.getMembership`, as the account (it answers for
/// the caller only, and may enrol an eligible caller, so it's called only
/// when the account itself starts a private-tier action).
async fn get_membership(s: &Arc<AppState>, did: &str) -> PrismAnswer {
    use crate::audience::Prism;
    match crate::audience::call(s, did, "app.nearhorizon.actor.getMembership", &[], None).await {
        Prism::Answer {
            status: 200, body, ..
        } => PrismAnswer::Membership {
            status: body["status"].as_str().unwrap_or("notEligible").to_owned(),
            private_indexed: body["permissions"]["privateIndexed"].as_bool() == Some(true),
            private_read: body["permissions"]["privateRead"].as_bool() == Some(true),
            valid_for_secs: body["validForSecs"].as_i64().unwrap_or(60).clamp(5, 3600),
        },
        a @ Prism::Answer { status, .. } => match a.error_name() {
            Some(name @ ("AccountTakedown" | "NotEnrolled" | "NotReady" | "Reindexing"))
                if (400..500).contains(&status) || status == 503 =>
            {
                PrismAnswer::Refused(name.to_owned())
            }
            _ => PrismAnswer::Unmapped(format!("prism answered {status}")),
        },
        Prism::Unreachable => PrismAnswer::Unreachable,
        Prism::Timeout => PrismAnswer::Unmapped("timeout".into()),
    }
}

/// `AccountTakedown` from any prism call: both rows say `takedown` for an
/// hour, so the write gate refuses too.
pub(crate) async fn record_takedown(s: &AppState, did: &str, prism_did: &str) -> XrpcResult<()> {
    let now = locus_core::time::now_ms();
    for source in [SOURCE_MEMBERSHIP, SOURCE_PROBE] {
        s.store
            .put_kr_permission(&PermissionRow {
                did: did.to_owned(),
                source: source.to_owned(),
                prism_did: prism_did.to_owned(),
                status: "takedown".into(),
                private_indexed: false,
                private_read: false,
                expires_at: now + 3_600_000,
                checked_at: now,
            })
            .await?;
    }
    Ok(())
}

/// The private-tier gates for a non-delete write: an AppView configured
/// and the host switch on (else `UnsupportedNamespace`), no local block,
/// and the AppView's permission (`privateIndexed`), else `Forbidden`.
pub(crate) async fn gate(s: &Arc<AppState>, did: &str) -> XrpcResult<()> {
    let Some(appview) = crate::pipethrough::appview(s) else {
        return Err(unsupported());
    };
    if !s.snapshot().bool("kryphocron.private_tier_enabled") {
        return Err(unsupported());
    }
    if s.store.private_tier_blocked(did).await? {
        return Err(forbidden());
    }
    // An account that writes privately is polled in the background.
    s.store.ensure_audience_sync(did).await?;
    let prism_did = appview.1.clone();
    let now = locus_core::time::now_ms();
    let last = s
        .store
        .kr_permission(did, SOURCE_MEMBERSHIP)
        .await?
        .filter(|r| r.prism_did == prism_did);
    let row = match last {
        Some(r) if r.expires_at > now => r,
        last => match get_membership(s, did).await {
            PrismAnswer::Membership {
                status,
                private_indexed,
                private_read,
                valid_for_secs,
            } => {
                let row = PermissionRow {
                    did: did.to_owned(),
                    source: SOURCE_MEMBERSHIP.to_owned(),
                    prism_did,
                    status,
                    private_indexed,
                    private_read,
                    expires_at: now + valid_for_secs * 1000,
                    checked_at: now,
                };
                s.store.put_kr_permission(&row).await?;
                row
            }
            PrismAnswer::Refused(name) if name == "AccountTakedown" => {
                record_takedown(s, did, &prism_did).await?;
                return Err(forbidden());
            }
            PrismAnswer::Refused(_) => return Err(forbidden()),
            // During an outage the last answer stands, expired or not.
            PrismAnswer::Unreachable => last.ok_or_else(forbidden)?,
            PrismAnswer::Unmapped(why) => {
                tracing::info!(did, "getMembership gave no answer: {why}");
                return Err(retry(
                    "The AppView couldn't confirm private-tier access; try again",
                ));
            }
        },
    };
    if row.status == "takedown" || !row.private_indexed {
        return Err(forbidden());
    }
    Ok(())
}

// ---------------------------------------------------------------- writes

/// What a write does, for the gates (deletes are never gated).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    Write,
    Delete,
}

/// Checks a plaintext post against the lexicon (the stored, encoded form
/// can't show what the content was).
fn validate_plain(
    rkey: &str,
) -> impl Fn(&BTreeMap<String, LexValue>) -> Result<(), String> + Sync + '_ {
    move |plain| {
        records::prepare_record(
            nsid::POST_PRIVATE,
            rkey,
            LexValue::Map(plain.clone()),
            Some(true),
            usize::MAX,
        )
        .map(|_| ())
        .map_err(|e| e.0)
    }
}

/// The audience a private post names must be one of the writer's own, held
/// in the private audience store (refreshed from prism on a miss). A mapped
/// local URI is replaced by the prism URI it maps to.
async fn check_audience(
    s: &Arc<AppState>,
    did: &str,
    record: &mut BTreeMap<String, LexValue>,
) -> XrpcResult<()> {
    let uri = record
        .get("audienceList")
        .and_then(LexValue::as_str)
        .unwrap_or_default()
        .to_owned();
    let stored = crate::audience::writer_audience(s, did, &uri).await?;
    record.insert("audienceList".into(), LexValue::String(stored));
    Ok(())
}

/// Every write path's kryphocron step, before the record is prepared:
/// unregistered collections refused; for creates and updates the host
/// rules, the private-tier gates and, for `feed.postPrivate`, the encode
/// pipeline. Returns the record to store.
pub(crate) async fn prepare_write(
    s: &Arc<AppState>,
    did: &str,
    collection: &str,
    rkey: &str,
    value: Option<LexValue>,
    op: Op,
) -> XrpcResult<Option<LexValue>> {
    let private = match locus_kryphocron::classify(collection) {
        Kind::Other => return Ok(value),
        Kind::Unregistered => {
            return Err(XrpcError::invalid_request(format!(
                "Unknown kryphocron collection: {collection}"
            )));
        }
        Kind::Record { private } => private,
    };
    if op == Op::Delete {
        return Ok(value);
    }
    if collection == nsid::AUDIENCE {
        return Err(XrpcError::invalid_request(
            "Audiences aren't stored in repositories; use tools.kryphocron.policy.manageAudience",
        ));
    }
    let Some(LexValue::Map(mut map)) = value else {
        return Err(XrpcError::invalid_request(
            "Invalid record: expected an object",
        ));
    };
    rules::check_record(did, collection, rkey, &map)
        .map_err(|e| XrpcError::invalid_request(e.0))?;
    if private {
        gate(s, did).await?;
    }
    if collection != nsid::POST_PRIVATE {
        return Ok(Some(LexValue::Map(map)));
    }
    if map.contains_key("reply") {
        // A reply goes under its parent's audience (participatePrivate's
        // checks), never one of the writer's own by choice.
        let audience = crate::membership::check_reply(s, did, &map).await?;
        map.insert("audienceList".into(), LexValue::String(audience));
    } else {
        check_audience(s, did, &mut map).await?;
    }
    let validate = validate_plain(rkey);
    let out =
        s.kr.encoder
            .prepare_post(&s.kr.audit, did, rkey, map, &validate)
            .await
            .map_err(pipeline_error)?;
    Ok(Some(LexValue::Map(out)))
}

// ---------------------------------------------------------------- procedures

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatePostPrivateInput {
    repo: String,
    rkey: Option<String>,
    validate: Option<bool>,
    record: Value,
}

type St = State<Arc<AppState>>;

/// `tools.kryphocron.feed.createPostPrivate`: a private post in the
/// writer's own repo, under one of the writer's own audiences.
pub async fn create_post_private(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<CreatePostPrivateInput>,
) -> XrpcResult<Json<Value>> {
    let account = repo::writer(&s, &headers, &input.repo, &|p| {
        p.assert_repo(nsid::POST_PRIVATE, "create")
    })
    .await?;
    crate::ratelimit::limit_repo_write(&s, &account.did, 3).await?;
    let rkey = input.rkey.clone().unwrap_or_else(|| s.repo.tids().next());
    let value = records::parse_json(&input.record).map_err(|e| XrpcError::invalid_request(e.0))?;
    let value = prepare_write(
        &s,
        &account.did,
        nsid::POST_PRIVATE,
        &rkey,
        Some(value),
        Op::Write,
    )
    .await?
    .expect("a write keeps its record");
    let rec = records::prepare_record(
        nsid::POST_PRIVATE,
        &rkey,
        value,
        input.validate,
        repo::record_limit(&s),
    )
    .map_err(|e| XrpcError::invalid_request(e.0))?;
    let write = repo::prepared(Action::Create, nsid::POST_PRIVATE, &rkey, Some(&rec), None);
    repo::commit(&s, &account, vec![write], None, false).await?;
    Ok(Json(json!({
        "uri": repo::uri(&account.did, nsid::POST_PRIVATE, &rkey),
        "cid": rec.cid.to_string_base32(),
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdatePostPrivateInput {
    repo: String,
    rkey: String,
    audience_list: String,
    swap_record: Option<String>,
}

/// `tools.kryphocron.feed.updatePostPrivate`: changes only `audienceList`.
/// The seed doesn't include the audience, so `encodedContent` is kept byte
/// for byte; only the CID changes. Replies are refused.
pub async fn update_post_private(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<UpdatePostPrivateInput>,
) -> XrpcResult<Json<Value>> {
    let account = repo::writer(&s, &headers, &input.repo, &|p| {
        p.assert_repo(nsid::POST_PRIVATE, "update")
    })
    .await?;
    crate::ratelimit::limit_repo_write(&s, &account.did, 2).await?;
    let missing = || XrpcError::invalid_request("The post doesn't exist");
    let (cid, _) = s
        .store
        .record_entry(&account.did, nsid::POST_PRIVATE, &input.rkey)
        .await?
        .ok_or_else(missing)?;
    if let Some(swap) = &input.swap_record
        && *swap != cid
    {
        return Err(XrpcError::named(
            "InvalidSwap",
            format!("Record was at {cid}"),
        ));
    }
    let bytes = s
        .store
        .block(&account.did, &cid)
        .await?
        .ok_or_else(|| XrpcError::internal("a record's block is missing"))?;
    let Ok(LexValue::Map(mut map)) = proto_blue_lex_cbor::decode(&bytes) else {
        return Err(XrpcError::internal("a stored record doesn't decode"));
    };
    if map.contains_key("reply") {
        return Err(XrpcError::invalid_request(
            "A reply's audience is its thread's; it can't be changed",
        ));
    }
    map.insert(
        "audienceList".into(),
        LexValue::String(input.audience_list.clone()),
    );
    rules::check_record(&account.did, nsid::POST_PRIVATE, &input.rkey, &map)
        .map_err(|e| XrpcError::invalid_request(e.0))?;
    gate(&s, &account.did).await?;
    check_audience(&s, &account.did, &mut map).await?;
    let rec = records::prepare_record(
        nsid::POST_PRIVATE,
        &input.rkey,
        LexValue::Map(map),
        None,
        repo::record_limit(&s),
    )
    .map_err(|e| XrpcError::invalid_request(e.0))?;
    let current = proto_blue_lex_data::Cid::from_str_multibase(&cid)
        .map_err(|_| XrpcError::internal("a stored CID doesn't parse"))?;
    let write = repo::prepared(
        Action::Update,
        nsid::POST_PRIVATE,
        &input.rkey,
        Some(&rec),
        Some(Some(current)),
    );
    match repo::commit(&s, &account, vec![write], None, false).await {
        Ok(_) => {}
        Err(e) if e.message == "no changes" => {}
        Err(e) => return Err(e),
    }
    Ok(Json(json!({
        "uri": repo::uri(&account.did, nsid::POST_PRIVATE, &input.rkey),
        "cid": rec.cid.to_string_base32(),
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletePostPrivateInput {
    repo: String,
    rkey: String,
}

/// `tools.kryphocron.feed.deletePostPrivate`: never gated, so a user can
/// always delete their own private posts.
pub async fn delete_post_private(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<DeletePostPrivateInput>,
) -> XrpcResult<Json<Value>> {
    let account = repo::writer(&s, &headers, &input.repo, &|p| {
        p.assert_repo(nsid::POST_PRIVATE, "delete")
    })
    .await?;
    crate::ratelimit::limit_repo_write(&s, &account.did, 1).await?;
    let write = repo::prepared(
        Action::DeleteIfPresent,
        nsid::POST_PRIVATE,
        &input.rkey,
        None,
        None,
    );
    match repo::commit(&s, &account, vec![write], None, false).await {
        Ok(_) => Ok(Json(json!({}))),
        Err(e) if e.message == "no changes" => Ok(Json(json!({}))),
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------- background

/// Drains audit events into `kr_audit`, in batches.
pub async fn run_audit_writer(
    store: locus_store::Store,
    mut rx: mpsc::Receiver<AuditEvent>,
    cancel: CancellationToken,
) {
    loop {
        let first = tokio::select! {
            () = cancel.cancelled() => None,
            e = rx.recv() => e,
        };
        let Some(first) = first else {
            // Drain what's left before stopping.
            let mut rest = Vec::new();
            while let Ok(e) = rx.try_recv() {
                rest.push(row(e));
            }
            if !rest.is_empty()
                && let Err(e) = store.append_kr_audit(&rest).await
            {
                tracing::error!("kryphocron audit rows lost at shutdown: {e}");
            }
            return;
        };
        let mut batch = vec![row(first)];
        while batch.len() < 500 {
            match rx.try_recv() {
                Ok(e) => batch.push(row(e)),
                Err(_) => break,
            }
        }
        if let Err(e) = store.append_kr_audit(&batch).await {
            // The fallback: the log, which can't fail.
            for r in &batch {
                tracing::warn!(
                    sink = r.sink,
                    event = r.event,
                    "kryphocron audit row not stored: {e}"
                );
            }
        }
    }
}

fn row(e: AuditEvent) -> locus_store::kryphocron::AuditRow {
    locus_store::kryphocron::AuditRow {
        at: e.at,
        sink: e.sink.to_owned(),
        event: e.event,
        correlation_id: e.correlation_id,
        detail_json: serde_json::to_string(&e.detail).unwrap_or_else(|_| "\"\"".into()),
    }
}

/// Reads the current generation into this node's oracle. A fresh install
/// gets its first generation here (nodes racing to make it each add one;
/// the newest is current for all of them).
pub async fn refresh_oracle(s: &AppState) -> locus_core::CoreResult<()> {
    let mark = match s.store.current_generation().await? {
        Some((mark, _)) => mark,
        None => {
            let now = locus_core::time::now_ms();
            let secs = u64::try_from(now / 1000).unwrap_or(0);
            s.store
                .add_generation(&locus_kryphocron::rotation::new_mark(secs), now)
                .await?;
            s.store
                .current_generation()
                .await?
                .map(|(m, _)| m)
                .ok_or_else(|| locus_core::CoreError::Storage("no rotation generation".into()))?
        }
    };
    s.kr.encoder.oracle().set(&mark);
    Ok(())
}

// ---------------------------------------------------------------- rotation

/// Most ops, and record bytes, in one rotation commit: headroom under the
/// 2 MB event cap.
const ROTATION_OPS: usize = 200;
const ROTATION_BYTES: usize = 1_500_000;

/// Marks dated more than this far ahead are due whatever the generation.
const FUTURE_SKEW_SECS: u64 = 300;

/// What one rotation pass did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RotationReport {
    pub rewritten: u64,
    pub flagged: u64,
    pub commits: u64,
}

/// Starts a new generation when the current one is older than the period
/// (under the job's fence), then refreshes this node's oracle.
pub async fn ensure_generation(
    s: &AppState,
    lease: &locus_core::lease::LeaseToken,
) -> locus_core::CoreResult<()> {
    let now = locus_core::time::now_ms();
    let period = s.snapshot().int("kryphocron.rotation_period_days") * 86_400_000;
    let due = match s.store.current_generation().await? {
        Some((_, created)) => now - created >= period,
        None => true,
    };
    if due {
        let mark = locus_kryphocron::rotation::new_mark(u64::try_from(now / 1000).unwrap_or(0));
        let mut tx = s.store.begin().await?;
        tx.fence(lease).await?;
        tx.add_generation(&mark, now).await?;
        tx.commit().await?;
        s.kr.audit
            .record_locus("rotation.generation", format!("generation={mark}"));
        tracing::info!(generation = mark, "new rotation generation");
    }
    refresh_oracle(s).await
}

/// Where in the period an account's stale records are rewritten, so the
/// firehose doesn't spike when a generation starts.
fn spread_offset(did: &str, period_ms: i64) -> i64 {
    use sha2::Digest;
    let h = sha2::Sha256::digest(did.as_bytes());
    let n = u64::from_be_bytes(h[..8].try_into().expect("8 bytes"));
    i64::try_from(n % u64::try_from(period_ms.max(1)).unwrap_or(1)).unwrap_or(0)
}

/// One pass of the rewrite-on-rotate job: every active account's due
/// private posts are re-encoded under the current generation, in batched
/// commits with CAS against the CIDs read, so a concurrent user write is
/// never clobbered (that record is found again next pass). Records that
/// can't be rewritten are flagged, one by one.
pub async fn rotation_pass(
    s: &AppState,
    lease: &locus_core::lease::LeaseToken,
    cancel: &CancellationToken,
) -> locus_core::CoreResult<RotationReport> {
    ensure_generation(s, lease).await?;
    let mut report = RotationReport::default();
    let Some(current) = s.kr.encoder.oracle().current() else {
        return Ok(report);
    };
    let Some((_, created_at)) = s.store.current_generation().await? else {
        return Ok(report);
    };
    let snap = s.snapshot();
    let period = snap.int("kryphocron.rotation_period_days") * 86_400_000;
    let per_minute = snap
        .int("kryphocron.rotation_max_commits_per_minute")
        .max(1);
    let pause = Duration::from_millis(u64::try_from(60_000 / per_minute).unwrap_or(500));
    let now = locus_core::time::now_ms();
    let future = locus_kryphocron::rotation::mark_floor(
        u64::try_from(now / 1000).unwrap_or(0) + FUTURE_SKEW_SECS,
    );
    let mut after = String::new();
    loop {
        let dids = s
            .store
            .rotation_due_accounts(&current, &future, &after, 100)
            .await?;
        let Some(last) = dids.last() else {
            break;
        };
        after.clone_from(last);
        for did in dids {
            if cancel.is_cancelled() {
                return Ok(report);
            }
            let active = s
                .store
                .account_by_did(&did)
                .await?
                .is_some_and(|a| a.status == locus_store::accounts::Status::Active);
            if !active {
                continue;
            }
            let urgent_only = now < created_at + spread_offset(&did, period);
            loop {
                let mut due = s
                    .store
                    .rotation_due_records(&did, &current, urgent_only, ROTATION_OPS as i64)
                    .await?;
                if due.len() < ROTATION_OPS {
                    let more = s
                        .store
                        .rotation_future_dated(&did, &future, (ROTATION_OPS - due.len()) as i64)
                        .await?;
                    due.extend(more);
                }
                if due.is_empty() {
                    break;
                }
                match rotate_batch(s, &did, &due, &mut report).await? {
                    BatchOutcome::Committed => {
                        report.commits += 1;
                        tokio::time::sleep(pause).await;
                    }
                    // Only flags this time: the loop continues with the rest.
                    BatchOutcome::NothingToCommit => {}
                    // A concurrent write, a write block, a stale oracle: try
                    // this account again next pass.
                    BatchOutcome::Later => break,
                }
            }
        }
    }
    if report.rewritten > 0 || report.flagged > 0 {
        s.kr.audit.record_locus(
            "rotation.pass",
            format!(
                "generation={current} rewritten={} flagged={} commits={}",
                report.rewritten, report.flagged, report.commits
            ),
        );
    }
    Ok(report)
}

enum BatchOutcome {
    Committed,
    NothingToCommit,
    Later,
}

/// Rewrites one batch of an account's due records in one commit.
async fn rotate_batch(
    s: &AppState,
    did: &str,
    due: &[locus_store::kryphocron::PrivateRecord],
    report: &mut RotationReport,
) -> locus_core::CoreResult<BatchOutcome> {
    let mut writes = Vec::new();
    let mut bytes = 0usize;
    let mut rewritten = 0u64;
    for rec in due {
        let flag = |why: &'static str| async move {
            s.store
                .flag_private_record(did, &rec.collection, &rec.rkey, why)
                .await
        };
        let Some((cid, _)) = s
            .store
            .record_entry(did, &rec.collection, &rec.rkey)
            .await?
        else {
            // Gone meanwhile; the index follows the record index.
            continue;
        };
        let Some(block) = s.store.block(did, &cid).await? else {
            continue;
        };
        let Ok(LexValue::Map(map)) = proto_blue_lex_cbor::decode(&block) else {
            flag("malformed").await?;
            report.flagged += 1;
            continue;
        };
        let rewritten_map = if rec.legacy_text {
            // An imported plaintext record: the full pipeline, without the
            // lexicon check (it's signed history) or blob checks.
            let skip = |_: &BTreeMap<String, LexValue>| Ok(());
            s.kr.encoder
                .prepare_post(&s.kr.audit, did, &rec.rkey, map, &skip)
                .await
        } else {
            s.kr.encoder
                .reencode(&s.kr.audit, did, &rec.rkey, map)
                .await
        };
        let value = match rewritten_map {
            Ok(m) => LexValue::Map(m),
            Err(PipelineError::Invalid(_)) => {
                flag("undecodable").await?;
                report.flagged += 1;
                continue;
            }
            Err(PipelineError::Retry(_)) => return Ok(BatchOutcome::Later),
        };
        let Ok(prepared) =
            records::prepare_record(&rec.collection, &rec.rkey, value, None, usize::MAX)
        else {
            flag("invalid").await?;
            report.flagged += 1;
            continue;
        };
        bytes += prepared.bytes.len();
        if !writes.is_empty() && bytes > ROTATION_BYTES {
            break;
        }
        let read = proto_blue_lex_data::Cid::from_str_multibase(&cid)
            .map_err(|_| locus_core::CoreError::Storage("a stored CID doesn't parse".into()))?;
        let mut write = repo::prepared(
            Action::Update,
            &rec.collection,
            &rec.rkey,
            Some(&prepared),
            Some(Some(read)),
        );
        write.lenient_blobs = rec.legacy_text;
        writes.push(write);
        rewritten += 1;
    }
    if writes.is_empty() {
        return Ok(BatchOutcome::NothingToCommit);
    }
    let key = s
        .signing_key(did)
        .await
        .map_err(|e| locus_core::CoreError::Storage(e.message))?;
    let outcome = s
        .repo
        .commit(locus_repo::CommitRequest {
            did,
            writes,
            swap_commit: None,
            signer: &key,
            dedupe_backlinks: false,
        })
        .await;
    match outcome {
        Ok(o) => {
            repo::finish_moves(s, &o.moves).await;
            report.rewritten += rewritten;
            Ok(BatchOutcome::Committed)
        }
        Err(e) => {
            tracing::info!(did, "rotation commit deferred: {e}");
            Ok(BatchOutcome::Later)
        }
    }
}

/// `tools.kryphocron.actor.participatePrivate`: a private reply under the
/// parent post's audience, after the reply checks (every refusal the same
/// `Forbidden`).
pub async fn participate_private(
    state: St,
    headers: HeaderMap,
    Json(input): Json<CreatePostPrivateInput>,
) -> XrpcResult<Json<Value>> {
    let has_parent = input.record["reply"]["parent"]["uri"].is_string();
    if !has_parent || !input.record["audienceList"].is_string() {
        return Err(XrpcError::invalid_request(
            "A reply needs reply.parent and audienceList",
        ));
    }
    create_post_private(state, headers, Json(input)).await
}
