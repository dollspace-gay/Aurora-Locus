//! The audience module: the hidden copy of the AppView's (prism's)
//! audiences, kept in the private audience store and refreshed by polling,
//! and every call locus makes to prism as an account.
//!
//! Prism is authoritative while it's up; during an outage the copy is.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use locus_store::kryphocron::{AudienceRow, PermissionRow, RefreshLimiter, SOURCE_PROBE};
use serde_json::{Value, json};

use crate::error::{XrpcError, XrpcResult};
use crate::state::AppState;

pub const AUDIENCE: &str = "tools.kryphocron.policy.audience";

// ---------------------------------------------------------------- availability

/// This node's view of whether prism is up. Nodes can briefly disagree,
/// which is harmless: what a node does while it sees prism down is synced
/// later by the same rules.
#[derive(Debug, Default)]
pub struct PrismHealth {
    down: AtomicBool,
    changed_at: AtomicI64,
    probing: AtomicBool,
}

impl PrismHealth {
    #[must_use]
    pub fn is_down(&self) -> bool {
        self.down.load(Ordering::SeqCst)
    }

    /// When the state last changed (ms), for the console.
    #[must_use]
    pub fn changed_at(&self) -> i64 {
        self.changed_at.load(Ordering::SeqCst)
    }

    pub(crate) fn set(&self, down: bool) -> bool {
        let was = self.down.swap(down, Ordering::SeqCst);
        if was != down {
            self.changed_at
                .store(locus_core::time::now_ms(), Ordering::SeqCst);
        }
        was != down
    }
}

/// How long one prism call may take.
fn call_timeout(s: &AppState) -> Duration {
    Duration::from_millis(
        u64::try_from(s.snapshot().int("kryphocron.prism_request_timeout_ms")).unwrap_or(10_000),
    )
}

/// An unauthenticated `GET /.well-known/did.json` on prism: any HTTP answer
/// means it's up.
async fn probe_once(s: &AppState, url: &str) -> bool {
    let Ok(client) = crate::pipethrough::client(s, false) else {
        return false;
    };
    client
        .get(format!(
            "{}/.well-known/did.json",
            url.trim_end_matches('/')
        ))
        .timeout(call_timeout(s))
        .send()
        .await
        .is_ok()
}

/// After an unreachable or timed-out call: probe at once, and if the probe
/// fails too, mark prism down and keep probing every 10 s until it answers.
async fn probe_after_failure(s: &std::sync::Arc<AppState>, url: String) {
    if probe_once(s, &url).await {
        if s.kr.prism.set(false) {
            prism_came_back(s).await;
        }
        return;
    }
    s.kr.prism.set(true);
    tracing::warn!("the AppView's private tier is unreachable; using the audience copy");
    if s.kr.prism.probing.swap(true, Ordering::SeqCst) {
        return;
    }
    let st = std::sync::Arc::clone(s);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            let Some((url, _)) = crate::pipethrough::appview(&st) else {
                break;
            };
            if probe_once(&st, &url).await {
                st.kr.prism.set(false);
                tracing::info!("the AppView's private tier answers again");
                prism_came_back(&st).await;
                break;
            }
        }
        st.kr.prism.probing.store(false, Ordering::SeqCst);
    });
}

/// Prism is up again: accounts with work queued during the outage are
/// polled first.
async fn prism_came_back(s: &AppState) {
    if let Err(e) = s.store.poll_queued_audiences_now().await {
        tracing::warn!("couldn't schedule audience syncs: {e}");
    }
    s.kr.audience_sync_wake.notify_one();
}

/// What a prism call came to.
#[derive(Debug, Clone)]
pub enum Prism {
    /// An HTTP answer (any status).
    Answer {
        status: u16,
        body: Value,
        retry_after: Option<u64>,
    },
    /// Nothing reached prism (connection refused, DNS, or prism already
    /// marked down): an outage.
    Unreachable,
    /// Sent, but no answer in time: prism may have acted on it.
    Timeout,
}

impl Prism {
    /// The error name of a 4xx/5xx answer.
    #[must_use]
    pub fn error_name(&self) -> Option<&str> {
        match self {
            Self::Answer { status, body, .. } if *status >= 400 => body["error"].as_str(),
            _ => None,
        }
    }
}

/// Calls a prism method as `did` (a user service JWT, one method per
/// token). `body` makes it a POST.
/// A query to another service as `did` (service auth), with prism's
/// answer kinds but none of its up/down tracking: the block-source AppView.
pub async fn call_service(
    s: &AppState,
    url: &str,
    aud: &str,
    did: &str,
    lxm: &str,
    query: &[(&str, &str)],
) -> Prism {
    let Ok(token) = crate::pipethrough::service_token(s, did, aud, lxm).await else {
        return Prism::Unreachable;
    };
    let Ok(client) = crate::pipethrough::client(s, false) else {
        return Prism::Unreachable;
    };
    let res = client
        .get(format!("{}/xrpc/{lxm}", url.trim_end_matches('/')))
        .query(query)
        .bearer_auth(token)
        .timeout(call_timeout(s))
        .send()
        .await;
    match res {
        Ok(res) => {
            let status = res.status().as_u16();
            let body = res.json().await.unwrap_or(Value::Null);
            Prism::Answer {
                status,
                body,
                retry_after: None,
            }
        }
        Err(e) if e.is_timeout() => Prism::Timeout,
        Err(_) => Prism::Unreachable,
    }
}

pub async fn call(
    s: &std::sync::Arc<AppState>,
    did: &str,
    lxm: &str,
    query: &[(&str, &str)],
    body: Option<Value>,
) -> Prism {
    let Some((url, aud)) = crate::pipethrough::appview(s) else {
        return Prism::Unreachable;
    };
    if s.kr.prism.is_down() {
        return Prism::Unreachable;
    }
    let token = match crate::pipethrough::service_token(s, did, &aud, lxm).await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(did, "couldn't sign a token for prism: {}", e.message);
            return Prism::Unreachable;
        }
    };
    let Ok(client) = crate::pipethrough::client(s, false) else {
        return Prism::Unreachable;
    };
    let endpoint = format!("{}/xrpc/{lxm}", url.trim_end_matches('/'));
    let req = match &body {
        Some(b) => client.post(&endpoint).json(b),
        None => client.get(&endpoint).query(query),
    }
    .bearer_auth(token)
    .timeout(call_timeout(s));
    match req.send().await {
        Ok(res) => {
            let status = res.status().as_u16();
            let retry_after = res
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse().ok());
            let body = res.json().await.unwrap_or(Value::Null);
            Prism::Answer {
                status,
                body,
                retry_after,
            }
        }
        Err(e) => {
            let st = std::sync::Arc::clone(s);
            tokio::spawn(async move { probe_after_failure(&st, url).await });
            if e.is_timeout() {
                Prism::Timeout
            } else {
                Prism::Unreachable
            }
        }
    }
}

// ---------------------------------------------------------------- values

/// An audience as stored: prism's canonical form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Audience {
    pub name: Option<String>,
    /// `list`, `everyone`, `followers`, `following` or `nobody`.
    pub mode: String,
    pub members: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

pub const MODES: [&str; 5] = ["list", "everyone", "followers", "following", "nobody"];

impl Audience {
    /// Reads prism's record (or a client's, already checked).
    #[must_use]
    pub fn from_record(v: &Value) -> Option<Self> {
        let mode = v["mode"].as_str().unwrap_or("list").to_owned();
        if !MODES.contains(&mode.as_str()) {
            return None;
        }
        Some(Self {
            name: v["name"].as_str().map(str::to_owned),
            mode,
            members: v["members"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|m| m.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            created_at: v["createdAt"].as_str()?.to_owned(),
            updated_at: v["updatedAt"].as_str()?.to_owned(),
        })
    }

    /// Prism's canonical record: `mode` left out for `list`, `name` when
    /// there's none.
    #[must_use]
    pub fn to_record(&self) -> Value {
        let mut v = json!({
            "$type": AUDIENCE,
            "members": self.members,
            "createdAt": self.created_at,
            "updatedAt": self.updated_at,
        });
        if self.mode != "list" {
            v["mode"] = json!(self.mode);
        }
        if let Some(n) = &self.name {
            v["name"] = json!(n);
        }
        v
    }

    /// The stored value's DAG-CBOR and its CID (`local_cid`).
    pub fn encode(&self) -> XrpcResult<(Vec<u8>, String)> {
        let lex = proto_blue_lex_json::json_to_lex_with(
            &self.to_record(),
            proto_blue_lex_json::LexParseOptions::strict(),
        )
        .map_err(|e| XrpcError::invalid_request(format!("Invalid audience: {e}")))?;
        let bytes = proto_blue_lex_cbor::encode(&lex)
            .map_err(|e| XrpcError::invalid_request(format!("Invalid audience: {e}")))?;
        let cid = proto_blue_lex_data::Cid::for_cbor(&bytes).to_string_base32();
        Ok((bytes, cid))
    }

    /// The store row for `did`/`rkey`.
    pub fn row(
        &self,
        did: &str,
        rkey: &str,
        origin: &str,
        prism_did: &str,
        prism_cid: Option<String>,
    ) -> XrpcResult<AudienceRow> {
        let (value_cbor, local_cid) = self.encode()?;
        Ok(AudienceRow {
            did: did.to_owned(),
            rkey: rkey.to_owned(),
            name: self.name.clone(),
            mode: self.mode.clone(),
            members_json: serde_json::to_string(&self.members).unwrap_or_else(|_| "[]".into()),
            created_at: self.created_at.clone(),
            updated_at: self.updated_at.clone(),
            origin: origin.to_owned(),
            prism_did: prism_did.to_owned(),
            prism_cid,
            local_cid,
            value_cbor,
            pending: "none".into(),
            row_version: 1,
        })
    }

    /// A stored row's audience.
    #[must_use]
    pub fn of_row(r: &AudienceRow) -> Self {
        Self {
            name: r.name.clone(),
            mode: r.mode.clone(),
            members: serde_json::from_str(&r.members_json).unwrap_or_default(),
            created_at: r.created_at.clone(),
            updated_at: r.updated_at.clone(),
        }
    }
}

/// The rkey of an audience URI owned by `did`.
#[must_use]
pub fn rkey_of<'a>(uri: &'a str, did: &str) -> Option<&'a str> {
    let (authority, collection, rkey) = locus_kryphocron::rules::split_uri(uri)?;
    (authority == did && collection == AUDIENCE && !rkey.is_empty()).then_some(rkey)
}

#[must_use]
pub fn uri(did: &str, rkey: &str) -> String {
    format!("at://{did}/{AUDIENCE}/{rkey}")
}

// ---------------------------------------------------------------- answers

/// A fresh `takedown` row: the owner's audiences answer owner only and
/// get no refresh.
pub async fn owner_taken_down(s: &AppState, did: &str) -> XrpcResult<bool> {
    let now = locus_core::time::now_ms();
    for source in [
        locus_store::kryphocron::SOURCE_MEMBERSHIP,
        locus_store::kryphocron::SOURCE_PROBE,
    ] {
        if let Some(r) = s.store.kr_permission(did, source).await?
            && r.status == "takedown"
            && r.expires_at > now
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// What an error answer from an audience call means for the account:
/// `NotEnrolled` purges the copy and both permission rows; `AccountTakedown`
/// writes `takedown` to both; `NotReady`/`Reindexing` write the probe row.
pub async fn apply_error_answer(s: &AppState, did: &str, answer: &Prism) -> XrpcResult<()> {
    let Some((_, prism_did)) = crate::pipethrough::appview(s) else {
        return Ok(());
    };
    let now = locus_core::time::now_ms();
    match answer.error_name() {
        Some("NotEnrolled") => {
            s.store.purge_audience_copy(did).await?;
            s.store.delete_kr_permissions(did).await?;
        }
        Some("AccountTakedown") => {
            crate::kryphocron::record_takedown(s, did, &prism_did).await?;
        }
        Some(name @ ("NotReady" | "Reindexing")) => {
            let retry = match answer {
                Prism::Answer {
                    retry_after: Some(secs),
                    ..
                } => i64::try_from(*secs).unwrap_or(5) * 1000,
                _ => 5_000,
            };
            s.store
                .put_kr_permission(&PermissionRow {
                    did: did.to_owned(),
                    source: SOURCE_PROBE.to_owned(),
                    prism_did,
                    status: if name == "NotReady" {
                        "notReady".into()
                    } else {
                        "reindexing".into()
                    },
                    private_indexed: true,
                    private_read: false,
                    expires_at: now + retry.max(5_000),
                    checked_at: now,
                })
                .await?;
        }
        _ => {}
    }
    Ok(())
}

// ---------------------------------------------------------------- refreshing

/// What asked for a refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// The owner's own action (a store miss, an unknown-rkey write, a
    /// proxied audience write): one per 5 s, throttled → retryable.
    Owner,
    /// Another account's check: once per `audience_check_ttl` per owner.
    ThirdParty,
    /// After an ambiguous create: bypasses both limiters.
    Bypass,
    /// The background poll.
    Background,
}

/// What a refresh got.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Refreshed {
    /// The last page came back without a cursor.
    pub complete: bool,
    /// The rkey asked for was on a page.
    pub found: bool,
    /// Why it stopped early, if it did.
    pub error: Option<String>,
    /// Pages fetched (the call budget counts them).
    pub pages: u32,
    /// What prism listed, by rkey: its version token and value.
    pub listed: BTreeMap<String, (Option<String>, Audience)>,
    /// When the listing started (database clock).
    pub started_at: i64,
}

impl Refreshed {
    fn stopped(why: &str) -> Self {
        Self {
            error: Some(why.to_owned()),
            ..Self::default()
        }
    }
}

const PAGE: &str = "100";

/// Refreshes `owner`'s copy from `listAudiences`, paging until `want` is
/// found or the listing completes (or, without `want`, to the end).
///
/// Every local write is conditional on the row's version when the listing
/// started, so a concurrent write-through or local edit wins. Local
/// audiences are never compared with the listing. A row missing from the
/// listing is deleted only after a complete listing, and only if it
/// existed before the listing and hasn't changed since.
pub async fn refresh(
    s: &std::sync::Arc<AppState>,
    owner: &str,
    trigger: Trigger,
    want: Option<&str>,
    deadline: Instant,
) -> XrpcResult<Refreshed> {
    let Some((_, prism_did)) = crate::pipethrough::appview(s) else {
        return Ok(Refreshed::stopped("no AppView"));
    };
    if owner_taken_down(s, owner).await? {
        return Ok(Refreshed::stopped("takedown"));
    }
    if s.kr.prism.is_down() {
        return Ok(Refreshed::stopped("prism down"));
    }
    let snap = s.snapshot();
    let allowed = match trigger {
        Trigger::Owner => {
            s.store
                .take_refresh(owner, RefreshLimiter::Owner, 5_000)
                .await?
        }
        Trigger::ThirdParty => {
            s.store
                .take_refresh(
                    owner,
                    RefreshLimiter::ThirdParty,
                    snap.int("kryphocron.audience_check_ttl_s") * 1000,
                )
                .await?
        }
        Trigger::Bypass | Trigger::Background => true,
    };
    if !allowed {
        return Ok(Refreshed::stopped("throttled"));
    }
    let listing_deadline = Instant::now()
        + Duration::from_secs(
            u64::try_from(snap.int("kryphocron.audience_listing_deadline_s")).unwrap_or(300),
        );
    let deadline = deadline.min(listing_deadline);

    // A copy that came from another AppView is discarded first, with the
    // permission answers that AppView gave.
    let mut versions = s.store.audience_versions(owner).await?;
    if versions.iter().any(|v| v.prism_did != prism_did) {
        s.store.purge_audience_copy(owner).await?;
        s.store
            .delete_kr_permissions_of_not_from(owner, &prism_did)
            .await?;
        versions.clear();
    }
    let started = s.store.db_now().await?;
    let snapshot: BTreeMap<String, locus_store::kryphocron::AudienceVersion> =
        versions.into_iter().map(|v| (v.rkey.clone(), v)).collect();
    let mut local: BTreeSet<String> = snapshot
        .values()
        .filter(|v| v.origin == "local")
        .map(|v| v.rkey.clone())
        .collect();
    let aliases = s.store.audience_aliases(owner).await?;
    local.extend(aliases.iter().map(|(l, _)| l.clone()));
    s.store.note_listing(owner, started, false, None).await?;

    let mut out = Refreshed {
        started_at: started,
        ..Refreshed::default()
    };
    let mut seen = BTreeSet::new();
    let mut cursor: Option<String> = None;
    loop {
        if Instant::now() >= deadline {
            out.error = Some("deadline".into());
            break;
        }
        let mut query = vec![("limit", PAGE)];
        if let Some(c) = &cursor {
            query.push(("cursor", c.as_str()));
        }
        let answer = call(
            s,
            owner,
            "app.nearhorizon.private.listAudiences",
            &query,
            None,
        )
        .await;
        out.pages += 1;
        // A page that answers past the deadline applies nothing.
        if Instant::now() >= deadline {
            out.error = Some("deadline".into());
            break;
        }
        let (status, body) = match &answer {
            Prism::Answer { status, body, .. } => (*status, body.clone()),
            Prism::Unreachable => {
                out.error = Some("unreachable".into());
                break;
            }
            Prism::Timeout => {
                out.error = Some("timeout".into());
                break;
            }
        };
        if status != 200 {
            apply_error_answer(s, owner, &answer).await?;
            out.error = Some(
                answer
                    .error_name()
                    .map_or_else(|| format!("status {status}"), str::to_owned),
            );
            break;
        }
        for entry in body["audiences"].as_array().into_iter().flatten() {
            let Some(rkey) = entry["uri"].as_str().and_then(|u| rkey_of(u, owner)) else {
                continue;
            };
            let rkey = rkey.to_owned();
            if local.contains(&rkey) {
                // Prism minted a TID equal to a local one: never adopted.
                tracing::warn!(owner, rkey, "prism audience rkey collides with a local one");
                if s.store.audience_alias(owner, &rkey).await?.is_some() {
                    s.store
                        .flag_audience_alias(owner, &rkey, "refused: prism rkey collision")
                        .await?;
                } else {
                    s.store
                        .flag_audience(owner, &rkey, "refused: prism rkey collision")
                        .await?;
                }
                continue;
            }
            let Some(aud) = Audience::from_record(&entry["record"]) else {
                continue;
            };
            seen.insert(rkey.clone());
            if want == Some(rkey.as_str()) {
                out.found = true;
            }
            let prism_cid = entry["cid"].as_str().map(str::to_owned);
            out.listed
                .insert(rkey.clone(), (prism_cid.clone(), aud.clone()));
            match snapshot.get(&rkey) {
                None => {
                    // A deleted URI is never re-minted: a tombstone always wins.
                    if s.store.audience_tombstone(owner, &rkey).await?.is_some() {
                        continue;
                    }
                    let row = aud.row(owner, &rkey, "prism", &prism_did, prism_cid)?;
                    s.store.adopt_audience(&row).await?;
                }
                // Queued edits are the sync rule's to settle.
                Some(v) if v.pending != "none" => {}
                Some(v) if v.prism_cid == prism_cid => {}
                Some(v) => {
                    let row = aud.row(owner, &rkey, "prism", &prism_did, prism_cid)?;
                    s.store.replace_audience_if(&row, v.row_version).await?;
                }
            }
        }
        if out.found && want.is_some() {
            break;
        }
        match body["cursor"].as_str() {
            Some(c) if !c.is_empty() => cursor = Some(c.to_owned()),
            _ => {
                out.complete = true;
                break;
            }
        }
    }

    // Every refresh matches every open create attempt (a create whose answer
    // never came may show up on any page).
    let linked = match_attempts(s, owner).await?;
    crate::audience_sync::after_links(s, owner, &linked).await?;
    if out.complete {
        for v in snapshot.values() {
            if v.origin == "prism" && !seen.contains(&v.rkey) {
                // Deleted in prism (by the owner, or prism's purge).
                s.store
                    .delete_audience(owner, &v.rkey, Some(v.row_version))
                    .await?;
            }
        }
    }
    s.store
        .note_listing(owner, started, out.complete, out.error.as_deref())
        .await?;
    Ok(out)
}

// ---------------------------------------------------------------- lookups

/// Resolves an audience URI to the store row it names: its own rkey, or a
/// mapped local rkey's prism rkey. Returns the URI to store (a mapped local
/// URI is substituted by the prism one) and the row.
pub async fn lookup(
    s: &AppState,
    owner: &str,
    rkey: &str,
) -> XrpcResult<Option<(String, AudienceRow)>> {
    if let Some(row) = s.store.kr_audience(owner, rkey).await? {
        return Ok(Some((uri(owner, rkey), row)));
    }
    if let Some(prism_rkey) = s.store.audience_alias(owner, rkey).await?
        && let Some(row) = s.store.kr_audience(owner, &prism_rkey).await?
    {
        return Ok(Some((uri(owner, &prism_rkey), row)));
    }
    Ok(None)
}

/// The audience a writer's private post names, which must be one of the
/// writer's own in the store. A URI the copy lacks makes the owner's copy
/// refresh from prism first (prism lists newest first, so it pages until
/// the URI turns up or the listing completes). `InvalidRequest` only when
/// a complete listing lacks it; any other outcome is retryable. Returns the
/// URI to store.
pub async fn writer_audience(
    s: &std::sync::Arc<AppState>,
    owner: &str,
    audience_uri: &str,
) -> XrpcResult<String> {
    let missing =
        || XrpcError::invalid_request("audienceList doesn't name one of the writer's audiences");
    let rkey = rkey_of(audience_uri, owner).ok_or_else(missing)?;
    if let Some((u, _)) = lookup(s, owner, rkey).await? {
        return Ok(u);
    }
    let unavailable = || {
        XrpcError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "TemporarilyUnavailable",
            "Couldn't confirm the audience with the AppView; try again",
        )
    };
    let deadline_ms = u64::try_from(s.snapshot().int("http.request_deadline_ms")).unwrap_or(60_000);
    let deadline = Instant::now() + Duration::from_millis(deadline_ms.saturating_sub(1_000));
    let r = refresh(s, owner, Trigger::Owner, Some(rkey), deadline).await?;
    if r.found {
        // Deleted locally meanwhile (a pending delete, or a tombstone).
        return match lookup(s, owner, rkey).await? {
            Some((u, _)) => Ok(u),
            None => Err(missing()),
        };
    }
    if r.complete && r.error.is_none() {
        return Err(missing());
    }
    Err(unavailable())
}

// ---------------------------------------------------------------- writes

/// The prism caps locus enforces before a write is sent.
const MAX_BODY: usize = 512 * 1024;
const MAX_MEMBER_LEN: usize = 256;

/// What a write returns: the audience's URI and the CID of the stored value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    pub uri: String,
    pub cid: String,
}

fn invalid(m: impl Into<String>) -> XrpcError {
    XrpcError::invalid_request(m)
}

fn retry(m: impl Into<String>) -> XrpcError {
    XrpcError::new(StatusCode::SERVICE_UNAVAILABLE, "TemporarilyUnavailable", m)
}

fn forbidden() -> XrpcError {
    XrpcError::new(
        StatusCode::FORBIDDEN,
        "Forbidden",
        "This account may not make private-tier writes",
    )
}

/// The checks prism applies, so a write only fails there for state locus
/// can't see: a known mode (absent = `list`), `members` for `list`, DIDs
/// of at most 256 bytes, de-duplicated, the owner dropped, capped; a name of
/// 1–64 graphemes and at most 640 bytes once trimmed; a body of at most
/// 512 KiB. Other keys are dropped. Returns (name, mode, members).
pub fn check_value(
    s: &AppState,
    owner: &str,
    record: &Value,
) -> XrpcResult<(Option<String>, String, Vec<String>)> {
    use unicode_segmentation::UnicodeSegmentation;
    if serde_json::to_vec(record).map_or(usize::MAX, |b| b.len()) > MAX_BODY {
        return Err(invalid("Audience is too large"));
    }
    if let Some(t) = record.get("$type").and_then(Value::as_str)
        && t != AUDIENCE
    {
        return Err(invalid(format!(
            "Invalid $type: expected {AUDIENCE}, got {t}"
        )));
    }
    let mode = match record.get("mode") {
        None | Some(Value::Null) => "list".to_owned(),
        Some(Value::String(m)) if MODES.contains(&m.as_str()) => m.clone(),
        Some(_) => return Err(invalid("Invalid audience mode")),
    };
    let members = match record.get("members") {
        Some(Value::Array(items)) => {
            let mut out: Vec<String> = Vec::new();
            for m in items {
                let did = m
                    .as_str()
                    .filter(|d| d.len() <= MAX_MEMBER_LEN && proto_blue_syntax::Did::new(d).is_ok())
                    .ok_or_else(|| invalid("Audience members must be DIDs"))?;
                if did != owner && !out.iter().any(|d| d == did) {
                    out.push(did.to_owned());
                }
            }
            out
        }
        None if mode != "list" => Vec::new(),
        None => return Err(invalid("A list audience needs members")),
        Some(_) => return Err(invalid("Audience members must be a list")),
    };
    let cap = usize::try_from(s.snapshot().int("kryphocron.audience_max_members")).unwrap_or(1000);
    if members.len() > cap {
        return Err(invalid(format!("An audience has at most {cap} members")));
    }
    let name = match record.get("name") {
        None | Some(Value::Null) => None,
        Some(Value::String(n)) => {
            let n = n.trim();
            let graphemes = n.graphemes(true).count();
            if !(1..=64).contains(&graphemes) || n.len() > 640 {
                return Err(invalid("An audience name is 1–64 characters"));
            }
            Some(n.to_owned())
        }
        Some(_) => return Err(invalid("Invalid audience name")),
    };
    Ok((name, mode, members))
}

/// What a definite refusal from prism means for the client.
async fn refusal(s: &AppState, owner: &str, answer: &Prism) -> XrpcError {
    if let Err(e) = apply_error_answer(s, owner, answer).await {
        return e;
    }
    let (status, message, retry_after) = match answer {
        Prism::Answer {
            status,
            body,
            retry_after,
        } => (
            *status,
            body["message"].as_str().unwrap_or("Refused").to_owned(),
            *retry_after,
        ),
        _ => (503, "Unavailable".to_owned(), None),
    };
    match answer.error_name() {
        Some("LimitReached" | "InvalidRequest" | "NotReady") => invalid(message),
        Some("InvalidSwap") => XrpcError::named("InvalidSwap", message),
        Some("NotEnrolled" | "AccountTakedown") => forbidden(),
        Some("Reindexing") => retry(format!(
            "The AppView is re-indexing this account; try again in {}s",
            retry_after.unwrap_or(30)
        )),
        _ if status == 429 => XrpcError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "RateLimitExceeded",
            "The AppView is rate limiting this account; try again shortly",
        ),
        _ => retry("The AppView couldn't take the change; try again"),
    }
}

fn now_iso() -> String {
    locus_core::time::iso8601(locus_core::time::now_ms())
}

fn prism_did(s: &AppState) -> XrpcResult<String> {
    crate::pipethrough::appview(s)
        .map(|(_, d)| d)
        .ok_or_else(|| {
            XrpcError::named(
                "UnsupportedNamespace",
                "This server doesn't have the private tier enabled",
            )
        })
}

/// A fresh locus-minted rkey for a local audience, clear of every rkey the
/// copy uses.
async fn local_rkey(s: &AppState, owner: &str) -> XrpcResult<String> {
    loop {
        let rkey = s.repo.tids().next();
        if !s.store.audience_rkey_taken(owner, &rkey).await? {
            return Ok(rkey);
        }
    }
}

/// Creates an audience. While prism is up, prism mints the rkey
/// (write-through); while it's down, a local audience with a locus-minted
/// rkey is created, to be created in prism when it's back.
pub async fn create(
    s: &std::sync::Arc<AppState>,
    owner: &str,
    record: &Value,
) -> XrpcResult<Written> {
    crate::kryphocron::gate(s, owner).await?;
    let (name, mode, members) = check_value(s, owner, record)?;
    let max = s.snapshot().int("kryphocron.audience_max_per_account");
    if s.store.count_audiences(owner).await? >= max {
        return Err(XrpcError::named(
            "LimitReached",
            format!("An account has at most {max} audiences"),
        ));
    }
    let now = now_iso();
    let aud = Audience {
        name,
        mode,
        members,
        created_at: now.clone(),
        updated_at: now,
    };
    let prism_did = prism_did(s)?;
    if s.kr.prism.is_down() {
        return create_local(s, owner, &aud, &prism_did).await;
    }
    let attempt = hex::encode(rand::random::<[u8; 16]>());
    let value = aud.to_record();
    s.store
        .create_audience_attempt(
            &attempt,
            owner,
            &value.to_string(),
            &aud.created_at,
            &aud.updated_at,
            None,
        )
        .await?;
    let answer = call(
        s,
        owner,
        "app.nearhorizon.private.createAudience",
        &[],
        Some(json!({"record": value})),
    )
    .await;
    match &answer {
        Prism::Answer {
            status: 200, body, ..
        } => {
            s.store.delete_attempt(&attempt).await?;
            let rkey = body["uri"]
                .as_str()
                .and_then(|u| rkey_of(u, owner))
                .ok_or_else(|| retry("The AppView's answer was malformed"))?
                .to_owned();
            let row = aud.row(
                owner,
                &rkey,
                "prism",
                &prism_did,
                body["cid"].as_str().map(str::to_owned),
            )?;
            // If this fails, prism has it anyway: the next refresh adopts it.
            if let Err(e) = s.store.put_kr_audience(&row).await {
                tracing::warn!(owner, "audience created in prism but not stored: {e}");
            }
            Ok(Written {
                uri: uri(owner, &rkey),
                cid: row.local_cid,
            })
        }
        Prism::Answer { .. } => {
            s.store.delete_attempt(&attempt).await?;
            Err(refusal(s, owner, &answer).await)
        }
        Prism::Unreachable => {
            s.store.delete_attempt(&attempt).await?;
            create_local(s, owner, &aud, &prism_did).await
        }
        Prism::Timeout => {
            // Prism may have created it: never retried blindly.
            s.store.set_attempt_outcome(&attempt, "unknown").await?;
            let far = Instant::now() + Duration::from_secs(30);
            if let Err(e) = refresh(s, owner, Trigger::Bypass, None, far).await {
                tracing::warn!(
                    owner,
                    "refresh after an ambiguous create failed: {}",
                    e.message
                );
            }
            match s.store.attempt_row(owner, &attempt).await? {
                Some(row) => Ok(Written {
                    uri: uri(owner, &row.rkey),
                    cid: row.local_cid,
                }),
                None => Err(retry(
                    "The outcome is unknown: list your audiences before retrying",
                )),
            }
        }
    }
}

async fn create_local(
    s: &AppState,
    owner: &str,
    aud: &Audience,
    prism_did: &str,
) -> XrpcResult<Written> {
    let rkey = local_rkey(s, owner).await?;
    let row = aud.row(owner, &rkey, "local", prism_did, None)?;
    s.store.put_kr_audience(&row).await?;
    Ok(Written {
        uri: uri(owner, &rkey),
        cid: row.local_cid,
    })
}

/// The row an rkey names, refreshing the owner's copy (owner limit) when
/// the copy lacks it. `Ok(None)` only when a complete listing lacks it.
async fn find_for_write(
    s: &std::sync::Arc<AppState>,
    owner: &str,
    rkey: &str,
) -> XrpcResult<Option<(String, AudienceRow)>> {
    if let Some(found) = lookup(s, owner, rkey).await? {
        return Ok(Some(found));
    }
    let far = Instant::now() + Duration::from_secs(30);
    let r = refresh(s, owner, Trigger::Owner, Some(rkey), far).await?;
    if r.found {
        return lookup(s, owner, rkey).await;
    }
    if r.complete && r.error.is_none() {
        return Ok(None);
    }
    Err(retry(
        "Couldn't confirm the audience with the AppView; try again",
    ))
}

/// Updates an audience (`rkey` may be a mapped local one). `swap` is the
/// client's `swapRecord`, compared with the stored value's CID.
pub async fn update(
    s: &std::sync::Arc<AppState>,
    owner: &str,
    rkey: &str,
    record: &Value,
    swap: Option<&str>,
) -> XrpcResult<Written> {
    crate::kryphocron::gate(s, owner).await?;
    let (name, mode, members) = check_value(s, owner, record)?;
    let prism_did = prism_did(s)?;
    let mut retried = false;
    loop {
        let (u, row) = find_for_write(s, owner, rkey)
            .await?
            .ok_or_else(|| invalid("No such audience"))?;
        if let Some(sw) = swap
            && sw != row.local_cid
        {
            return Err(XrpcError::named(
                "InvalidSwap",
                format!("Audience was at {}", row.local_cid),
            ));
        }
        let aud = Audience {
            name: name.clone(),
            mode: mode.clone(),
            members: members.clone(),
            created_at: row.created_at.clone(),
            updated_at: now_iso(),
        };
        let written = |prism_cid: Option<String>, pending: &str| -> XrpcResult<AudienceRow> {
            let mut new = aud.row(owner, &row.rkey, &row.origin, &prism_did, prism_cid)?;
            new.pending = pending.to_owned();
            Ok(new)
        };
        let conflict = || retry("The audience changed meanwhile; try again");
        // A local audience's whole value is unsynced: its row changes
        // directly, never through `pending`.
        if row.origin == "local" {
            let new = written(None, "none")?;
            if !s.store.write_audience_value(&new, row.row_version).await? {
                return Err(conflict());
            }
            return Ok(Written {
                uri: u,
                cid: new.local_cid,
            });
        }
        if s.kr.prism.is_down() {
            let new = written(row.prism_cid.clone(), "update")?;
            if !s.store.write_audience_value(&new, row.row_version).await? {
                return Err(conflict());
            }
            return Ok(Written {
                uri: u,
                cid: new.local_cid,
            });
        }
        let answer = call(
            s,
            owner,
            "app.nearhorizon.private.updateAudience",
            &[],
            Some(json!({"uri": u, "record": aud.to_record(), "swapCid": row.prism_cid})),
        )
        .await;
        let new = match &answer {
            Prism::Answer {
                status: 200, body, ..
            } => written(body["cid"].as_str().map(str::to_owned), "none")?,
            // Kept pending: the next refresh and sync settle it.
            Prism::Unreachable | Prism::Timeout => written(row.prism_cid.clone(), "update")?,
            a if a.error_name() == Some("InvalidSwap") => {
                let far = Instant::now() + Duration::from_secs(30);
                let r = refresh(s, owner, Trigger::Owner, None, far).await?;
                if r.error.as_deref() == Some("throttled") {
                    return Err(retry("The audience changed in the AppView; try again"));
                }
                if swap.is_none() && !retried {
                    retried = true;
                    continue;
                }
                return Err(XrpcError::named(
                    "InvalidSwap",
                    "The audience changed in the AppView",
                ));
            }
            _ => return Err(refusal(s, owner, &answer).await),
        };
        // A refresh may have moved the row on; prism holds the edit anyway.
        if !s.store.write_audience_value(&new, row.row_version).await? {
            tracing::info!(owner, rkey, "audience row moved on during a write-through");
        }
        return Ok(Written {
            uri: u,
            cid: new.local_cid,
        });
    }
}

/// Deletes an audience. Never gated: with the tier off or the AppView
/// removed, or during an outage, a prism audience's delete is queued.
pub async fn delete(s: &std::sync::Arc<AppState>, owner: &str, rkey: &str) -> XrpcResult<()> {
    let tier_off = crate::pipethrough::appview(s).is_none()
        || !s.snapshot().bool("kryphocron.private_tier_enabled");
    let found = if tier_off {
        lookup(s, owner, rkey).await?
    } else {
        find_for_write(s, owner, rkey).await?
    };
    let Some((u, row)) = found else {
        // Deleting a missing record succeeds, as the reference.
        return Ok(());
    };
    if row.origin == "local" {
        s.store.delete_audience(owner, &row.rkey, None).await?;
        return Ok(());
    }
    if tier_off || s.kr.prism.is_down() {
        s.store.queue_audience_delete(owner, &row.rkey).await?;
        return Ok(());
    }
    let answer = call(
        s,
        owner,
        "app.nearhorizon.private.deleteAudience",
        &[],
        Some(json!({"uri": u})),
    )
    .await;
    match &answer {
        Prism::Answer { status: 200, .. } => {
            s.store.delete_audience(owner, &row.rkey, None).await?;
            Ok(())
        }
        Prism::Unreachable | Prism::Timeout => {
            s.store.queue_audience_delete(owner, &row.rkey).await?;
            Ok(())
        }
        _ => Err(refusal(s, owner, &answer).await),
    }
}

/// Links open create attempts to the rows refreshes adopted: a row adopted
/// at or after the attempt was written, unclaimed, with the same
/// `createdAt` and `updatedAt` instants (locus stamped both) and the same
/// name, mode and member set. Exactly one candidate links; several link
/// none. Returns the (attempt, rkey) pairs linked.
pub async fn match_attempts(s: &AppState, owner: &str) -> XrpcResult<Vec<(String, String)>> {
    let timeout_ms = s.snapshot().int("kryphocron.prism_request_timeout_ms");
    let now = s.store.db_now().await?;
    let mut linked = Vec::new();
    for att in s.store.open_attempts(owner).await? {
        // A `sending` row older than the request timeout is a crash leftover.
        if att.outcome == "sending" && now - att.created_at <= timeout_ms {
            continue;
        }
        let Ok(sent) = serde_json::from_str::<Value>(&att.value_json) else {
            continue;
        };
        let Some(sent) = Audience::from_record(&sent) else {
            continue;
        };
        let instant = locus_core::time::parse_rfc3339_ms;
        let set = |m: &[String]| m.iter().cloned().collect::<BTreeSet<String>>();
        let candidates: Vec<_> = s
            .store
            .unlinked_adopted(owner, att.created_at)
            .await?
            .into_iter()
            .filter(|r| {
                let members: Vec<String> =
                    serde_json::from_str(&r.members_json).unwrap_or_default();
                instant(&r.created_at).is_some()
                    && instant(&r.created_at) == instant(&sent.created_at)
                    && instant(&r.updated_at) == instant(&sent.updated_at)
                    && r.name == sent.name
                    && r.mode == sent.mode
                    && set(&members) == set(&sent.members)
            })
            .collect();
        if let [only] = candidates.as_slice()
            && s.store.link_attempt(owner, &only.rkey, &att.id).await?
        {
            linked.push((att.id.clone(), only.rkey.clone()));
        }
    }
    Ok(linked)
}

// ---------------------------------------------------------------- endpoints

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;

type St = State<std::sync::Arc<AppState>>;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManageAudienceInput {
    repo: String,
    rkey: Option<String>,
    swap_record: Option<String>,
    record: Value,
}

/// `tools.kryphocron.policy.manageAudience`: without an rkey it creates
/// (prism mints the rkey); with one it updates, refused only when a
/// complete listing of the owner's audiences lacks it **[deviation: the
/// lexicon creates a record there; prism mints every rkey, so locus
/// can't]**. No commit and no firehose event: audiences live in the private
/// store.
pub async fn manage_audience(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<ManageAudienceInput>,
) -> XrpcResult<Json<Value>> {
    let action = if input.rkey.is_some() {
        "update"
    } else {
        "create"
    };
    let account = crate::repo::writer(&s, &headers, &input.repo, &|p| {
        p.assert_repo(AUDIENCE, action)
    })
    .await?;
    crate::ratelimit::limit_repo_write(&s, &account.did, if input.rkey.is_some() { 2 } else { 3 })
        .await?;
    let w = match &input.rkey {
        None => create(&s, &account.did, &input.record).await?,
        Some(rkey) => {
            update(
                &s,
                &account.did,
                rkey,
                &input.record,
                input.swap_record.as_deref(),
            )
            .await?
        }
    };
    Ok(Json(json!({"uri": w.uri, "cid": w.cid})))
}

#[derive(serde::Deserialize)]
pub struct DeleteAudienceInput {
    repo: String,
    rkey: String,
}

/// `tools.kryphocron.policy.deleteAudience`: never gated.
pub async fn delete_audience(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<DeleteAudienceInput>,
) -> XrpcResult<Json<Value>> {
    let account = crate::repo::writer(&s, &headers, &input.repo, &|p| {
        p.assert_repo(AUDIENCE, "delete")
    })
    .await?;
    crate::ratelimit::limit_repo_write(&s, &account.did, 1).await?;
    delete(&s, &account.did, &input.rkey).await?;
    Ok(Json(json!({})))
}

/// `createRecord` on the audience collection: no rkey creates; an rkey
/// is an update of that audience if it exists **[deviation: the reference
/// refuses a create on an existing key; here the rkey can only be
/// prism's]**, which needs `update` under OAuth.
pub async fn generic_create(
    s: &std::sync::Arc<AppState>,
    headers: &HeaderMap,
    owner: &str,
    rkey: Option<&str>,
    record: &Value,
) -> XrpcResult<Written> {
    let Some(rkey) = rkey else {
        return create(s, owner, record).await;
    };
    crate::repo::writer(s, headers, owner, &|p| p.assert_repo(AUDIENCE, "update")).await?;
    if find_for_write(s, owner, rkey).await?.is_none() {
        return Err(invalid(
            "An audience's rkey is minted by the AppView; create without one",
        ));
    }
    update(s, owner, rkey, record, None).await
}

/// Reads of the audience collection: the owner reads their own audiences
/// (a mapped local rkey reads under the prism URI); everyone else gets
/// `RecordNotFound`, as for a missing record.
pub async fn owner_record(
    s: &AppState,
    headers: &HeaderMap,
    repo_did: &str,
    rkey: &str,
) -> XrpcResult<Value> {
    let not_found = || {
        XrpcError::named(
            "RecordNotFound",
            format!("Could not locate record: {}", uri(repo_did, rkey)),
        )
    };
    let owner = crate::auth::authorization(
        s,
        headers,
        crate::auth::STANDARD,
        &[],
        crate::auth::any_scope,
    )
    .await
    .ok()
    .filter(|a| a.did == repo_did)
    .ok_or_else(not_found)?;
    let (u, row) = lookup(s, &owner.did, rkey).await?.ok_or_else(not_found)?;
    Ok(json!({"uri": u, "cid": row.local_cid, "value": Audience::of_row(&row).to_record()}))
}

/// `listRecords` of the audience collection: the owner's audiences, newest
/// first; nothing for anyone else.
pub async fn owner_list(s: &AppState, headers: &HeaderMap, repo_did: &str) -> XrpcResult<Value> {
    let owner = crate::auth::authorization(
        s,
        headers,
        crate::auth::STANDARD,
        &[],
        crate::auth::any_scope,
    )
    .await
    .ok()
    .filter(|a| a.did == repo_did);
    let Some(owner) = owner else {
        return Ok(json!({"records": []}));
    };
    let records: Vec<Value> = s
        .store
        .kr_audiences(&owner.did)
        .await?
        .iter()
        .map(|r| {
            json!({"uri": uri(&owner.did, &r.rkey), "cid": r.local_cid, "value": Audience::of_row(r).to_record()})
        })
        .collect();
    Ok(json!({"records": records}))
}

// ---------------------------------------------------------------- proxied prism calls

const PRISM_AUDIENCE_METHODS: [&str; 4] = [
    "app.nearhorizon.private.createAudience",
    "app.nearhorizon.private.updateAudience",
    "app.nearhorizon.private.deleteAudience",
    "app.nearhorizon.private.listAudiences",
];

/// Whether a proxied call is one of prism's audience methods, for the
/// configured prism.
#[must_use]
pub fn is_prism_audience_call(s: &AppState, headers: &HeaderMap, lxm: &str) -> bool {
    if !PRISM_AUDIENCE_METHODS.contains(&lxm) {
        return false;
    }
    let Some((_, prism_did)) = crate::pipethrough::appview(s) else {
        return false;
    };
    match headers.get("atproto-proxy").and_then(|v| v.to_str().ok()) {
        Some(v) => v.split('#').next() == Some(prism_did.as_str()),
        None => true,
    }
}

/// A row's version token as prism's methods expose it: prism's `cid` for a
/// prism row with no pending edit, otherwise the stored value's CID.
fn version_token(r: &AudienceRow) -> String {
    match (&r.prism_cid, r.origin.as_str(), r.pending.as_str()) {
        (Some(c), "prism", "none") => c.clone(),
        _ => r.local_cid.clone(),
    }
}

/// A proxied prism audience call. While prism is up it's proxied as usual
/// (a write refreshes the account's copy right after); while this node
/// sees prism down, locus answers it from the store, in prism's shapes, so
/// the account's client keeps working through the outage.
pub async fn proxied(
    s: &std::sync::Arc<AppState>,
    lxm: &str,
    method: axum::http::Method,
    uri_in: &axum::http::Uri,
    headers: &HeaderMap,
    body: axum::body::Body,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if !s.kr.prism.is_down() {
        let res = match crate::pipethrough::proxy_inner(s, lxm, method, uri_in, headers, body).await
        {
            Ok(r) | Err(r) => r,
        };
        if res.status().is_success()
            && lxm != "app.nearhorizon.private.listAudiences"
            && let Ok(a) = crate::auth::authorization(
                s,
                headers,
                crate::auth::STANDARD,
                &[],
                crate::auth::any_scope,
            )
            .await
        {
            let (st, did) = (std::sync::Arc::clone(s), a.did);
            tokio::spawn(async move {
                for _ in 0..3 {
                    let far = Instant::now() + Duration::from_secs(60);
                    match refresh(&st, &did, Trigger::Owner, None, far).await {
                        Ok(r) if r.error.as_deref() == Some("throttled") => {
                            tokio::time::sleep(Duration::from_secs(5)).await;
                        }
                        _ => break,
                    }
                }
            });
        }
        return res;
    }
    match local_prism_call(s, lxm, uri_in, headers, body).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn local_prism_call(
    s: &std::sync::Arc<AppState>,
    lxm: &str,
    uri_in: &axum::http::Uri,
    headers: &HeaderMap,
    body: axum::body::Body,
) -> XrpcResult<Value> {
    let aud = crate::pipethrough::proxy_to(s, headers, lxm).unwrap_or_default();
    let authed = crate::auth::authorization(s, headers, crate::auth::STANDARD, &[], |p| {
        Ok(p.assert_rpc(lxm, &aud)?)
    })
    .await?;
    let did = authed.did;
    let input: Value = if lxm == "app.nearhorizon.private.listAudiences" {
        Value::Null
    } else {
        let bytes = axum::body::to_bytes(body, MAX_BODY + 4096)
            .await
            .map_err(|_| invalid("Request body too large"))?;
        serde_json::from_slice(&bytes).map_err(|_| invalid("Invalid JSON body"))?
    };
    let target = |input: &Value| -> XrpcResult<String> {
        input["uri"]
            .as_str()
            .and_then(|u| rkey_of(u, &did))
            .map(str::to_owned)
            .ok_or_else(|| invalid("Invalid audience URI"))
    };
    match lxm {
        "app.nearhorizon.private.createAudience" => {
            let w = create(s, &did, &input["record"]).await?;
            let rkey = rkey_of(&w.uri, &did).unwrap_or_default().to_owned();
            let token = lookup(s, &did, &rkey)
                .await?
                .map_or(w.cid, |(_, r)| version_token(&r));
            Ok(json!({"uri": w.uri, "cid": token}))
        }
        "app.nearhorizon.private.updateAudience" => {
            let rkey = target(&input)?;
            let (_, row) = lookup(s, &did, &rkey)
                .await?
                .ok_or_else(|| invalid("No such audience"))?;
            if input["swapCid"].as_str() != Some(version_token(&row).as_str()) {
                return Err(XrpcError::named("InvalidSwap", "The audience has changed"));
            }
            let w = update(s, &did, &rkey, &input["record"], None).await?;
            let token = lookup(s, &did, &rkey)
                .await?
                .map_or(w.cid, |(_, r)| version_token(&r));
            Ok(json!({"uri": w.uri, "cid": token}))
        }
        "app.nearhorizon.private.deleteAudience" => {
            delete(s, &did, &target(&input)?).await?;
            Ok(json!({}))
        }
        _ => {
            let q: BTreeMap<String, String> = uri_in
                .query()
                .map(|q| {
                    url::form_urlencoded::parse(q.as_bytes())
                        .into_owned()
                        .collect()
                })
                .unwrap_or_default();
            let limit = q
                .get("limit")
                .and_then(|l| l.parse::<usize>().ok())
                .unwrap_or(50)
                .clamp(1, 100);
            // A mapped local rkey as the cursor resolves through the alias.
            let cursor = match q.get("cursor") {
                Some(c) => Some(
                    s.store
                        .audience_alias(&did, c)
                        .await?
                        .unwrap_or_else(|| c.clone()),
                ),
                None => None,
            };
            let rows = s.store.kr_audiences(&did).await?;
            let page: Vec<&AudienceRow> = rows
                .iter()
                .filter(|r| cursor.as_ref().is_none_or(|c| r.rkey < *c))
                .take(limit)
                .collect();
            let mut out = json!({"audiences": page.iter().map(|r| json!({
                "uri": uri(&did, &r.rkey),
                "cid": version_token(r),
                "record": Audience::of_row(r).to_record(),
            })).collect::<Vec<_>>()});
            if page.len() == limit
                && let Some(last) = page.last()
            {
                out["cursor"] = json!(last.rkey);
            }
            Ok(out)
        }
    }
}
