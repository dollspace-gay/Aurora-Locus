//! Audience membership: whether an account may read (or reply under) an
//! audience, for `participatePrivate` and the private media gate. Prism is
//! the source of truth; locus answers from its copy where the copy can and
//! asks prism (as the subject, never about anyone else) where only prism
//! can. Every refusal is the same `Forbidden`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use locus_store::kryphocron::{PermissionRow, SOURCE_MEMBERSHIP, SOURCE_PROBE};
use serde_json::{Value, json};

use crate::audience::{self, Prism, Trigger};
use crate::error::{XrpcError, XrpcResult};
use crate::state::AppState;

pub const ASSERTION_TYP: &str = "kryphocron-audience+jwt";
pub const ASSERTION_LXM: &str = locus_core::locus_nsid!("audience.getMembershipAssertion");
const ASSERTION_TTL_S: i64 = 300;

/// The one refusal every audience check returns.
#[must_use]
pub fn forbidden() -> XrpcError {
    XrpcError::new(StatusCode::FORBIDDEN, "Forbidden", "Not permitted")
}

// ---------------------------------------------------------------- caches

/// Short-lived answers, per node.
#[derive(Debug, Default)]
pub struct Caches {
    /// (subject, post) → served, until.
    probes: Mutex<HashMap<(String, String), (bool, Instant)>>,
    /// Probes per subject in the current second.
    probe_rate: Mutex<HashMap<String, (Instant, u32)>>,
    /// Audience URIs a third-party refresh didn't find, until.
    misses: Mutex<HashMap<String, Instant>>,
    /// Remote block answers: (writer, author) → blocked, until.
    blocks: Mutex<HashMap<(String, String), (bool, Instant)>>,
    /// Membership assertion requests per subject and per owner this minute.
    assertion_rate: Mutex<HashMap<String, (Instant, u32)>>,
    /// Issuer `#kryphocron_audience` keys by host, until.
    issuer_keys: Mutex<HashMap<String, (String, Instant)>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A fixed one-window counter: true while under `max` per `window`.
fn take(
    map: &Mutex<HashMap<String, (Instant, u32)>>,
    key: &str,
    max: u32,
    window: Duration,
) -> bool {
    let mut m = lock(map);
    let now = Instant::now();
    let e = m.entry(key.to_owned()).or_insert((now, 0));
    if now.duration_since(e.0) >= window {
        *e = (now, 0);
    }
    e.1 += 1;
    e.1 <= max
}

// ---------------------------------------------------------------- readiness

/// Whether a local account is a ready member of prism (`member` with
/// `privateRead`): from the newer fresh row of the two; during an outage,
/// the newer row, expired or not. `None`: no fresh row while prism is up
/// (the probe decides).
async fn ready(s: &AppState, did: &str) -> XrpcResult<Option<bool>> {
    let Some((_, prism_did)) = crate::pipethrough::appview(s) else {
        return Ok(Some(false));
    };
    let now = locus_core::time::now_ms();
    let mut rows = Vec::new();
    for source in [SOURCE_MEMBERSHIP, SOURCE_PROBE] {
        if let Some(r) = s.store.kr_permission(did, source).await?
            && r.prism_did == prism_did
        {
            rows.push(r);
        }
    }
    if rows
        .iter()
        .any(|r| r.status == "takedown" && r.expires_at > now)
    {
        return Ok(Some(false));
    }
    let pick = |rows: Vec<PermissionRow>| rows.into_iter().max_by_key(|r| r.checked_at);
    let fresh: Vec<PermissionRow> = rows
        .iter()
        .filter(|r| r.expires_at > now)
        .cloned()
        .collect();
    let row = if !fresh.is_empty() {
        pick(fresh)
    } else if s.kr.prism.is_down() {
        pick(rows)
    } else {
        return Ok(None);
    };
    Ok(Some(
        row.is_some_and(|r| r.status == "member" && r.private_read),
    ))
}

// ---------------------------------------------------------------- the probe

/// What the read probe found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Probe {
    /// Prism answered 200: the subject is a ready member. `served`: one of
    /// the posts came back (a reliable yes).
    Answered { served: bool },
    /// Anything else: proves nothing (fail closed).
    Nothing,
}

/// `app.nearhorizon.private.getPosts` as the subject, for up to 25
/// referencing posts. Locus reads only the status and whether a post was
/// served; the decoded content in the body is dropped at once, never kept,
/// logged or returned. At most 3 probes per second per subject; hits are
/// cached 60 s per (subject, post), misses 10 s.
async fn probe(s: &Arc<AppState>, subject: &str, posts: &[String]) -> XrpcResult<Probe> {
    if posts.is_empty() || s.kr.prism.is_down() {
        return Ok(Probe::Nothing);
    }
    let posts = &posts[..posts.len().min(25)];
    let key = (subject.to_owned(), posts.join(" "));
    if let Some((served, until)) = lock(&s.kr.membership.probes).get(&key).copied()
        && until > Instant::now()
    {
        return Ok(Probe::Answered { served });
    }
    if !take(
        &s.kr.membership.probe_rate,
        subject,
        3,
        Duration::from_secs(1),
    ) {
        return Ok(Probe::Nothing);
    }
    let query: Vec<(&str, &str)> = posts.iter().map(|p| ("uris", p.as_str())).collect();
    let answer = audience::call(s, subject, "app.nearhorizon.private.getPosts", &query, None).await;
    let Some((_, prism_did)) = crate::pipethrough::appview(s) else {
        return Ok(Probe::Nothing);
    };
    let now = locus_core::time::now_ms();
    match &answer {
        Prism::Answer {
            status: 200, body, ..
        } => {
            let served = body["posts"].as_array().is_some_and(|ps| {
                ps.iter().any(|p| {
                    p["uri"]
                        .as_str()
                        .is_some_and(|u| posts.iter().any(|w| w == u))
                })
            });
            // Any 200 proves readiness (prism checks it before reading).
            s.store
                .put_kr_permission(&PermissionRow {
                    did: subject.to_owned(),
                    source: SOURCE_PROBE.to_owned(),
                    prism_did,
                    status: "member".into(),
                    private_indexed: true,
                    private_read: true,
                    expires_at: now + 3_600_000,
                    checked_at: now,
                })
                .await?;
            let ttl = Duration::from_secs(if served { 60 } else { 10 });
            lock(&s.kr.membership.probes).insert(key, (served, Instant::now() + ttl));
            Ok(Probe::Answered { served })
        }
        a => {
            if a.error_name() == Some("NotEnrolled") {
                s.store
                    .put_kr_permission(&PermissionRow {
                        did: subject.to_owned(),
                        source: SOURCE_PROBE.to_owned(),
                        prism_did,
                        status: "notEnrolled".into(),
                        private_indexed: false,
                        private_read: false,
                        expires_at: now + 5_000,
                        checked_at: now,
                    })
                    .await?;
            } else {
                audience::apply_error_answer(s, subject, a).await?;
            }
            Ok(Probe::Nothing)
        }
    }
}

// ---------------------------------------------------------------- the algorithm

/// Whether `subject` is in the audience at `audience_uri` (owner O), with
/// `posts` the referencing posts a probe may ask prism about.
pub async fn is_member(
    s: &Arc<AppState>,
    subject: &str,
    audience_uri: &str,
    posts: &[String],
) -> XrpcResult<bool> {
    let Some((owner, collection, rkey)) = locus_kryphocron::rules::split_uri(audience_uri) else {
        return Ok(false);
    };
    if collection != audience::AUDIENCE || rkey.is_empty() {
        return Ok(false);
    }
    // 1. The owner.
    if subject == owner {
        return Ok(true);
    }
    let subject_local = s.store.account_by_did(subject).await?.is_some();
    if s.store.account_by_did(owner).await?.is_some() {
        return local_owner(s, subject, subject_local, owner, rkey, posts).await;
    }
    // 3. Owned on another host: a membership assertion from it, fetched as
    // the subject.
    if subject_local {
        if fetch_assertion(s, subject, audience_uri).await {
            return Ok(true);
        }
        // 4. Only a served post counts (the mode is hidden there).
        return Ok(matches!(
            probe(s, subject, posts).await?,
            Probe::Answered { served: true }
        ));
    }
    Ok(false)
}

/// Step 2: the owner's copy is here.
async fn local_owner(
    s: &Arc<AppState>,
    subject: &str,
    subject_local: bool,
    owner: &str,
    rkey: &str,
    posts: &[String],
) -> XrpcResult<bool> {
    if audience::owner_taken_down(s, owner).await? {
        return Ok(false);
    }
    let ttl_ms = s.snapshot().int("kryphocron.audience_check_ttl_s") * 1000;
    let mut found = audience::lookup(s, owner, rkey).await?;
    let uri_key = audience::uri(owner, rkey);
    if found.is_none() {
        let missed = lock(&s.kr.membership.misses)
            .get(&uri_key)
            .is_some_and(|until| *until > Instant::now());
        if !missed {
            let far = Instant::now() + Duration::from_secs(10);
            let r = audience::refresh(s, owner, Trigger::ThirdParty, Some(rkey), far).await?;
            if r.found {
                found = audience::lookup(s, owner, rkey).await?;
            }
            if found.is_none() {
                lock(&s.kr.membership.misses)
                    .insert(uri_key, Instant::now() + Duration::from_secs(10));
            }
        }
    }
    let Some((prism_uri, row)) = found else {
        // A missing or deleted audience means owner only.
        return Ok(false);
    };
    match row.mode.as_str() {
        "nobody" => Ok(false),
        "list" => {
            // A removal in prism applies within the check TTL.
            let sync = s.store.audience_sync(owner).await?;
            let db_now = s.store.db_now().await?;
            let stale = sync
                .last_listing_started_at
                .is_none_or(|t| db_now - t > ttl_ms);
            let row = if stale && !s.kr.prism.is_down() {
                let far = Instant::now() + Duration::from_secs(10);
                let _ = audience::refresh(s, owner, Trigger::ThirdParty, None, far).await;
                audience::lookup(s, owner, rkey)
                    .await?
                    .map_or(row, |(_, r)| r)
            } else {
                row
            };
            let members: Vec<String> = serde_json::from_str(&row.members_json).unwrap_or_default();
            if !members.iter().any(|m| m == subject) {
                return Ok(false);
            }
            if !subject_local {
                // Locus can't learn a remote subject's readiness.
                return Ok(true);
            }
            match ready(s, subject).await? {
                Some(r) => Ok(r),
                // Any 200 proves readiness; the list decides.
                None => Ok(matches!(
                    probe(s, subject, posts).await?,
                    Probe::Answered { .. }
                )),
            }
        }
        // everyone, followers, following: only prism knows.
        _ if subject_local => {
            let _ = prism_uri;
            Ok(matches!(
                probe(s, subject, posts).await?,
                Probe::Answered { served: true }
            ))
        }
        _ => Ok(false),
    }
}

// ---------------------------------------------------------------- assertions

/// The `did:web` of the host an account's DID document names as its PDS.
async fn pds_identity(s: &AppState, did: &str, fresh: bool) -> Option<(String, String)> {
    let doc = s.resolve_did_doc(did, fresh).await.ok().flatten()?;
    let endpoint = proto_blue_common::did_doc::get_service_endpoint(&doc, "#atproto_pds", None)?;
    let host = url::Url::parse(&endpoint).ok()?.host_str()?.to_owned();
    Some((format!("did:web:{host}"), endpoint))
}

/// A signed, short-lived assertion that `sub` is in `audience_uri`, for the
/// consuming host `aud`.
pub fn mint_assertion(
    s: &AppState,
    sub: &str,
    audience_uri: &str,
    aud: &str,
) -> XrpcResult<String> {
    use proto_blue_crypto::Signer;
    let now = locus_core::time::now_ms() / 1000;
    let header = json!({"typ": ASSERTION_TYP, "alg": s.secrets.audience_key.jwt_alg()});
    let claims = json!({
        "iss": format!("did:web:{}", s.hostname),
        "sub": sub,
        "audienceUri": audience_uri,
        "aud": aud,
        "iat": now,
        "exp": now + ASSERTION_TTL_S,
        "jti": hex::encode(rand::random::<[u8; 16]>()),
    });
    let input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(header.to_string()),
        URL_SAFE_NO_PAD.encode(claims.to_string())
    );
    let sig = s
        .secrets
        .audience_key
        .sign(input.as_bytes())
        .map_err(|e| XrpcError::internal(e.to_string()))?;
    Ok(format!("{input}.{}", URL_SAFE_NO_PAD.encode(sig)))
}

/// An issuer host's `#kryphocron_audience` key (from its `did.json`,
/// cached an hour).
async fn issuer_key(s: &AppState, host: &str, fresh: bool) -> Option<String> {
    if !fresh
        && let Some((k, until)) = lock(&s.kr.membership.issuer_keys).get(host).cloned()
        && until > Instant::now()
    {
        return Some(k);
    }
    let client = crate::pipethrough::client(s, true).ok()?;
    let url = format!("https://{host}/.well-known/did.json");
    let url = if s.public_url.starts_with("http://") && host.starts_with("localhost") {
        format!("http://{host}/.well-known/did.json")
    } else {
        url
    };
    let doc: Value = client
        .get(&url)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    let key = doc["verificationMethod"].as_array()?.iter().find(|m| {
        m["id"]
            .as_str()
            .is_some_and(|id| id.ends_with("#kryphocron_audience"))
    })?["publicKeyMultibase"]
        .as_str()
        .map(|mb| format!("did:key:{mb}"))?;
    lock(&s.kr.membership.issuer_keys).insert(
        host.to_owned(),
        (key.clone(), Instant::now() + Duration::from_secs(3600)),
    );
    Some(key)
}

/// Verifies an assertion for local subject `sub` about `audience_uri`.
pub async fn verify_assertion(s: &AppState, token: &str, sub: &str, audience_uri: &str) -> bool {
    let parts: Vec<&str> = token.split('.').collect();
    let [h, p, sig] = parts[..] else {
        return false;
    };
    let decode = |x: &str| -> Option<Value> {
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(x).ok()?).ok()
    };
    let (Some(header), Some(claims), Ok(sig)) = (decode(h), decode(p), URL_SAFE_NO_PAD.decode(sig))
    else {
        return false;
    };
    if header["typ"].as_str() != Some(ASSERTION_TYP) {
        return false;
    }
    let Some((owner, _, _)) = locus_kryphocron::rules::split_uri(audience_uri) else {
        return false;
    };
    // The issuer is derived from the owner's document, never the token.
    let mut issuer = pds_identity(s, owner, false).await;
    if issuer.as_ref().map(|(i, _)| i.as_str()) != claims["iss"].as_str() {
        issuer = pds_identity(s, owner, true).await;
    }
    let Some((iss, _)) = issuer else {
        return false;
    };
    let now = locus_core::time::now_ms() / 1000;
    let (iat, exp) = (
        claims["iat"].as_i64().unwrap_or(0),
        claims["exp"].as_i64().unwrap_or(0),
    );
    let claims_ok = claims["iss"].as_str() == Some(iss.as_str())
        && claims["aud"].as_str().is_some_and(|a| s.accepts_aud(a))
        && claims["sub"].as_str() == Some(sub)
        && claims["audienceUri"].as_str() == Some(audience_uri)
        && exp + 30 >= now
        && exp - iat <= ASSERTION_TTL_S;
    if !claims_ok {
        return false;
    }
    let host = iss.trim_start_matches("did:web:");
    let input = format!("{h}.{p}");
    let check = |k: &str| {
        proto_blue_crypto::verify_signature(k, input.as_bytes(), &sig, false).unwrap_or(false)
    };
    match issuer_key(s, host, false).await {
        Some(k) if check(&k) => true,
        // Refreshed once on a signature failure (a rotated key).
        _ => issuer_key(s, host, true).await.is_some_and(|k| check(&k)),
    }
}

/// Step 3: asks the owner's PDS, as the subject, for an assertion.
async fn fetch_assertion(s: &AppState, subject: &str, audience_uri: &str) -> bool {
    let Some((owner, _, _)) = locus_kryphocron::rules::split_uri(audience_uri) else {
        return false;
    };
    let Some((owner_host_did, endpoint)) = pds_identity(s, owner, false).await else {
        return false;
    };
    let Ok(token) =
        crate::pipethrough::service_token(s, subject, &owner_host_did, ASSERTION_LXM).await
    else {
        return false;
    };
    let Ok(client) = crate::pipethrough::client(s, true) else {
        return false;
    };
    let res = client
        .get(format!(
            "{}/xrpc/{ASSERTION_LXM}",
            endpoint.trim_end_matches('/')
        ))
        .query(&[
            ("audienceUri", audience_uri),
            ("aud", s.service_did.as_str()),
        ])
        .bearer_auth(token)
        .timeout(Duration::from_secs(10))
        .send()
        .await;
    let Ok(res) = res else {
        return false;
    };
    if !res.status().is_success() {
        return false;
    }
    let Ok(body) = res.json::<Value>().await else {
        return false;
    };
    let Some(t) = body["token"].as_str() else {
        return false;
    };
    verify_assertion(s, t, subject, audience_uri).await
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssertionParams {
    audience_uri: String,
    aud: String,
}

/// `app.nearhorizon.locus.audience.getMembershipAssertion`: a DID asks,
/// with service auth from itself (`aud` = this host's `did:web`), whether it
/// is in an audience owned here. Only a positive answer is issued; a
/// negative one is the same `Forbidden` as any other refusal.
pub async fn get_membership_assertion(
    State(s): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(p): Query<AssertionParams>,
) -> XrpcResult<Json<Value>> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| XrpcError::auth_required("Authentication Required"))?;
    let now = locus_core::time::now_ms() / 1000;
    let jwt = locus_auth::service_jwt::parse(token, Some(ASSERTION_LXM), now)
        .map_err(|e| XrpcError::named(e.name(), e.to_string()))?;
    if !s.accepts_aud(&jwt.aud) {
        return Err(XrpcError::named(
            "BadJwtAudience",
            "jwt audience does not match service did",
        ));
    }
    let (sub, _) = jwt.issuer_key_ref();
    let sub = sub.to_owned();
    let doc = s
        .resolve_did_doc(&sub, false)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| XrpcError::named("BadJwtIss", "could not resolve the issuer"))?;
    let key = proto_blue_common::did_doc::get_signing_did_key(&doc)
        .ok_or_else(|| XrpcError::named("BadJwtIss", "the issuer has no signing key"))?;
    if !jwt.verifies_with(&key) {
        return Err(XrpcError::named(
            "BadJwtSignature",
            "jwt signature does not match jwt issuer",
        ));
    }
    let Some((owner, _, _)) = locus_kryphocron::rules::split_uri(&p.audience_uri) else {
        return Err(forbidden());
    };
    let owner = owner.to_owned();
    if !take(
        &s.kr.membership.assertion_rate,
        &format!("s:{sub}"),
        30,
        Duration::from_secs(60),
    ) || !take(
        &s.kr.membership.assertion_rate,
        &format!("o:{owner}"),
        300,
        Duration::from_secs(60),
    ) {
        return Err(XrpcError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "RateLimitExceeded",
            "Too many requests",
        ));
    }
    // Only audiences owned here: the issuer answers from what it holds.
    if s.store.account_by_did(&owner).await?.is_none()
        || !is_member(&s, &sub, &p.audience_uri, &[]).await?
    {
        return Err(forbidden());
    }
    Ok(Json(
        json!({"token": mint_assertion(&s, &sub, &p.audience_uri, &p.aud)?}),
    ))
}

// ---------------------------------------------------------------- blocks and threadgates

/// Whether `a` blocks `b` in a repo hosted here (either block vocabulary).
async fn local_blocks(s: &AppState, a: &str, b: &str) -> XrpcResult<bool> {
    for collection in ["app.bsky.graph.block", "tools.kryphocron.graph.block"] {
        if s.store.has_backlink(a, collection, "subject", b).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Blocks in either direction between the writer (local) and `author`.
/// For a remote author: the AppView's relationships (as the writer) and the
/// author's own kryphocron blocks; any failure fails closed.
async fn blocked(s: &Arc<AppState>, writer: &str, author: &str) -> XrpcResult<bool> {
    if writer == author {
        return Ok(false);
    }
    if local_blocks(s, writer, author).await? {
        return Ok(true);
    }
    if s.store.account_by_did(author).await?.is_some() {
        return local_blocks(s, author, writer).await;
    }
    let key = (writer.to_owned(), author.to_owned());
    if let Some((b, until)) = lock(&s.kr.membership.blocks).get(&key).copied()
        && until > Instant::now()
    {
        return Ok(b);
    }
    let query = [("actor", writer), ("others", author)];
    let lxm = "app.bsky.graph.getRelationships";
    // The block-source AppView if one is set, else the configured one.
    let rel = match crate::pipethrough::configured(
        s,
        "kryphocron.block_appview_url",
        "kryphocron.block_appview_did",
    ) {
        Some((url, aud)) => audience::call_service(s, &url, &aud, writer, lxm, &query).await,
        None => audience::call(s, writer, lxm, &query, None).await,
    };
    let Prism::Answer {
        status: 200, body, ..
    } = rel
    else {
        return Ok(true);
    };
    let r = body["relationships"]
        .as_array()
        .and_then(|a| a.iter().find(|r| r["did"].as_str() == Some(author)));
    let app_bsky = r.is_some_and(|r| r.get("blocking").is_some() || r.get("blockedBy").is_some());
    let kryphocron = match remote_kryphocron_block(s, author, writer).await {
        Some(b) => b,
        None => return Ok(true),
    };
    let b = app_bsky || kryphocron;
    lock(&s.kr.membership.blocks).insert(key, (b, Instant::now() + Duration::from_secs(600)));
    Ok(b)
}

/// The author's `tools.kryphocron.graph.block` records naming `subject`,
/// listed from its PDS. `None`: the scan failed or ran too long.
async fn remote_kryphocron_block(s: &AppState, author: &str, subject: &str) -> Option<bool> {
    let (_, endpoint) = pds_identity(s, author, false).await?;
    let client = crate::pipethrough::client(s, true).ok()?;
    let max = s.snapshot().int("kryphocron.remote_block_scan_max");
    let mut cursor: Option<String> = None;
    let mut scanned = 0i64;
    loop {
        let mut q = vec![
            ("repo", author.to_owned()),
            ("collection", "tools.kryphocron.graph.block".to_owned()),
            ("limit", "100".to_owned()),
        ];
        if let Some(c) = &cursor {
            q.push(("cursor", c.clone()));
        }
        let body: Value = client
            .get(format!(
                "{}/xrpc/com.atproto.repo.listRecords",
                endpoint.trim_end_matches('/')
            ))
            .query(&q)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .ok()?
            .json()
            .await
            .ok()?;
        let records = body["records"].as_array()?;
        if records
            .iter()
            .any(|r| r["value"]["subject"].as_str() == Some(subject))
        {
            return Some(true);
        }
        scanned += records.len() as i64;
        if scanned > max {
            return None;
        }
        match body["cursor"].as_str() {
            Some(c) if !records.is_empty() => cursor = Some(c.to_owned()),
            _ => return Some(false),
        }
    }
}

/// A record by URI: from the index here, or `getRecord` on its PDS.
async fn fetch_record(s: &AppState, uri: &str) -> Option<Value> {
    let (did, collection, rkey) = locus_kryphocron::rules::split_uri(uri)?;
    if s.store.account_by_did(did).await.ok()?.is_some() {
        let (cid, takedown) = s.store.record_entry(did, collection, rkey).await.ok()??;
        if takedown.is_some() {
            return None;
        }
        let bytes = s.store.block(did, &cid).await.ok()??;
        return Some(proto_blue_lex_json::lex_to_json(
            &proto_blue_lex_cbor::decode(&bytes).ok()?,
        ));
    }
    let (_, endpoint) = pds_identity(s, did, false).await?;
    let client = crate::pipethrough::client(s, true).ok()?;
    let body: Value = client
        .get(format!(
            "{}/xrpc/com.atproto.repo.getRecord",
            endpoint.trim_end_matches('/')
        ))
        .query(&[("repo", did), ("collection", collection), ("rkey", rkey)])
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    body.get("value").cloned()
}

/// The root's threadgate, if any: whether `writer` may reply.
async fn threadgate_allows(s: &Arc<AppState>, writer: &str, root_uri: &str) -> XrpcResult<bool> {
    let Some((root_author, _, root_rkey)) = locus_kryphocron::rules::split_uri(root_uri) else {
        return Ok(false);
    };
    if writer == root_author {
        return Ok(true);
    }
    let gate_uri = format!("at://{root_author}/tools.kryphocron.feed.threadgate/{root_rkey}");
    let Some(gate) = fetch_record(s, &gate_uri).await else {
        return Ok(true);
    };
    let Some(rules) = gate["allow"].as_array() else {
        return Ok(true);
    };
    for rule in rules {
        let t = rule["$type"].as_str().unwrap_or_default();
        if t.ends_with("#mentionRule") {
            // A private root's mentions are its clear `mentionDids`.
            if let Some(root) = fetch_record(s, root_uri).await
                && root["mentionDids"]
                    .as_array()
                    .is_some_and(|m| m.iter().any(|d| d.as_str() == Some(writer)))
            {
                return Ok(true);
            }
        } else if t.ends_with("#followingRule") {
            let follows = if s.store.account_by_did(root_author).await?.is_some() {
                s.store
                    .has_backlink(root_author, "app.bsky.graph.follow", "subject", writer)
                    .await?
            } else {
                let rel = audience::call(
                    s,
                    writer,
                    "app.bsky.graph.getRelationships",
                    &[("actor", writer), ("others", root_author)],
                    None,
                )
                .await;
                matches!(&rel, Prism::Answer { status: 200, body, .. }
                    if body["relationships"].as_array().is_some_and(|a| a.iter().any(|r| r["did"].as_str() == Some(root_author) && r.get("followedBy").is_some())))
            };
            if follows {
                return Ok(true);
            }
        }
        // listRule: list membership isn't indexed here; it fails closed.
    }
    Ok(false)
}

// ---------------------------------------------------------------- participatePrivate

/// The checks for a private reply: the parent resolves and is a private
/// post; the reply's audience is the parent's (after alias resolution); no
/// block either way between writer and parent author; the root's
/// threadgate allows it; and the writer is in the parent's audience. Every
/// refusal is the same `Forbidden`. Returns the audience URI to store (the
/// prism URI for a mapped local one).
pub async fn check_reply(
    s: &Arc<AppState>,
    writer: &str,
    record: &std::collections::BTreeMap<String, proto_blue_lex_data::LexValue>,
) -> XrpcResult<String> {
    use proto_blue_lex_data::LexValue;
    let strong_uri = |k: &str| -> Option<String> {
        record
            .get("reply")?
            .as_map()?
            .get(k)?
            .as_map()?
            .get("uri")?
            .as_str()
            .map(str::to_owned)
    };
    let parent_uri = strong_uri("parent")
        .ok_or_else(|| XrpcError::invalid_request("reply.parent is required"))?;
    let root_uri = strong_uri("root").unwrap_or_else(|| parent_uri.clone());
    let named = record
        .get("audienceList")
        .and_then(LexValue::as_str)
        .ok_or_else(|| XrpcError::invalid_request("audienceList is required"))?
        .to_owned();
    let Some(parent) = fetch_record(s, &parent_uri).await else {
        return Err(forbidden());
    };
    let Some((parent_author, collection, _)) = locus_kryphocron::rules::split_uri(&parent_uri)
    else {
        return Err(forbidden());
    };
    if collection != locus_kryphocron::nsid::POST_PRIVATE {
        return Err(forbidden());
    }
    let Some(parent_audience) = parent["audienceList"].as_str() else {
        return Err(forbidden());
    };
    // A mapped local URI and its prism URI are the same audience.
    let canonical = |u: String| async move {
        let Some((owner, _, rkey)) = locus_kryphocron::rules::split_uri(&u) else {
            return u;
        };
        match s.store.audience_alias(owner, rkey).await {
            Ok(Some(prism)) => audience::uri(owner, &prism),
            _ => u,
        }
    };
    let theirs = canonical(parent_audience.to_owned()).await;
    if canonical(named).await != theirs {
        return Err(forbidden());
    }
    if blocked(s, writer, parent_author).await? {
        return Err(forbidden());
    }
    if !threadgate_allows(s, writer, &root_uri).await? {
        return Err(forbidden());
    }
    if !is_member(s, writer, &theirs, std::slice::from_ref(&parent_uri)).await? {
        return Err(forbidden());
    }
    Ok(theirs)
}

impl Caches {
    /// Seeds an issuer host's assertion key (tests; the harness can't fetch
    /// its own `did.json` over HTTPS).
    #[cfg(test)]
    pub(crate) fn seed_issuer_key(&self, host: &str, did_key: &str) {
        lock(&self.issuer_keys).insert(
            host.to_owned(),
            (
                did_key.to_owned(),
                Instant::now() + Duration::from_secs(3600),
            ),
        );
    }
}

impl Caches {
    /// Forgets cached probe answers (tests).
    #[cfg(test)]
    pub(crate) fn clear_probes(&self) {
        lock(&self.probes).clear();
    }
}
