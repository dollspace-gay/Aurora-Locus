//! Hostname migration (the standalone role): prepare a candidate (reserved,
//! approved by `/tls-check`, served only health), pre-check it, confirm it
//! (it becomes the hostname; every node reloads), then a job moves every
//! account's PDS endpoint and emits `#identity`. The old host keeps full
//! service, and its service DID stays an accepted audience, while any DID
//! document still names it; then it becomes a former hostname: its service
//! `did.json` is served forever and everything else redirects.

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use locus_core::{CoreResult, locus_nsid};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::Digest as _;

use crate::admin_auth::{self, Reauth, Role, Surface};
use crate::error::{XrpcError, XrpcResult};
use crate::state::AppState;

/// What the other hostnames are to this node, loaded at boot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hosts {
    /// Old hosts that keep full service (a DID document may still name
    /// them).
    pub old_full: Vec<String>,
    /// A prepared candidate: `/tls-check` and health only.
    pub candidate: Option<String>,
    /// Old hosts no DID document names any more: `did.json`, then redirects.
    pub former: Vec<String>,
}

impl Hosts {
    pub async fn load(store: &locus_store::Store, current: &str) -> CoreResult<Self> {
        let mut h = Self::default();
        for m in store.hostname_migrations().await? {
            match m.state.as_str() {
                "prepared" if m.new_host != current => h.candidate = Some(m.new_host),
                "running" if m.old_host != current => h.old_full.push(m.old_host),
                "done" if m.old_host != current => {
                    if m.former_at.is_some() {
                        h.former.push(m.old_host);
                    } else {
                        h.old_full.push(m.old_host);
                    }
                }
                _ => {}
            }
        }
        h.former.retain(|f| !h.old_full.contains(f));
        h.old_full.dedup();
        h.former.dedup();
        Ok(h)
    }

    /// Whether `host` is an old host this PDS still serves in full.
    #[must_use]
    pub fn serves_in_full(&self, host: &str) -> bool {
        self.old_full.iter().any(|h| h == host)
    }
}

/// The value `/xrpc/_health` carries in `x-locus-instance`, so a pre-check
/// can tell a candidate reaches this install and not another server.
#[must_use]
pub fn instance_tag(s: &AppState) -> String {
    hex::encode(&sha2::Sha256::digest(format!("locus-instance:{}", s.master_key.key_id()))[..8])
}

/// The pre-checks for a candidate: still not a handle or did:web host, and
/// `https://<host>/xrpc/_health` answers over TLS from this install.
pub async fn prechecks(s: &AppState, host: &str) -> Vec<(String, bool, String)> {
    let mut out = Vec::new();
    let reserved = s
        .store
        .account_by_handle(host)
        .await
        .ok()
        .flatten()
        .is_some()
        || s.store
            .hosted_did_web_by_host(host)
            .await
            .ok()
            .flatten()
            .is_some();
    out.push((
        "not a handle or did:web host".into(),
        !reserved,
        if reserved {
            "an account uses it".into()
        } else {
            String::new()
        },
    ));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build();
    let health = match client {
        Ok(c) => c
            .get(format!("https://{host}/xrpc/_health"))
            .send()
            .await
            .map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    let (ok, why) = match health {
        Ok(r) if r.status().is_success() => {
            let tag = r
                .headers()
                .get("x-locus-instance")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            if tag.as_deref() == Some(instance_tag(s).as_str()) {
                (true, String::new())
            } else {
                (false, "it answers, but not from this install".into())
            }
        }
        Ok(r) => (false, format!("it answered {}", r.status())),
        Err(e) => (
            false,
            format!("it doesn't answer over HTTPS with a valid certificate ({e})"),
        ),
    };
    out.push((
        "resolves here and serves HTTPS with a valid certificate".into(),
        ok,
        why,
    ));
    let approved = crate::health::approved(s, host).await;
    out.push((
        "/tls-check approves it".into(),
        approved,
        if approved {
            String::new()
        } else {
            "this node hasn't loaded the candidate yet; check again in a few seconds".into()
        },
    ));
    out
}

async fn owner(s: &AppState, headers: &HeaderMap, lxm: &str) -> XrpcResult<admin_auth::Caller> {
    admin_auth::require(s, headers, lxm, Surface::Locus, Role::Owner).await
}

fn refuse_roles(s: &AppState) -> XrpcResult<()> {
    match s.role {
        locus_core::bootstrap::Role::Pds => Ok(()),
        _ => Err(XrpcError::invalid_request(
            "migrate hostname is for the standalone role in this version",
        )),
    }
}

#[derive(Deserialize)]
pub struct PrepareInput {
    host: String,
}

/// `owner.hostname.prepare`: records a candidate and runs the pre-checks.
pub async fn prepare(
    State(s): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<PrepareInput>,
) -> XrpcResult<Json<Value>> {
    let who = owner(&s, &headers, locus_nsid!("owner.hostname.prepare")).await?;
    refuse_roles(&s)?;
    let host = locus_core::setup::normalize_host(&input.host);
    if host.is_empty() || !host.contains('.') || crate::routing::is_internal_name(&host) {
        return Err(XrpcError::invalid_request("Give a public DNS name"));
    }
    if host == s.hostname {
        return Err(XrpcError::invalid_request("That is the hostname already"));
    }
    if s.store.account_by_handle(&host).await?.is_some()
        || s.store.hosted_did_web_by_host(&host).await?.is_some()
    {
        return Err(XrpcError::invalid_request(
            "That name is an account's handle or did:web host",
        ));
    }
    let id = format!(
        "{}-{}",
        s.store.db_now_ms().await?,
        hex::encode(rand::random::<[u8; 3]>())
    );
    s.store
        .prepare_hostname_migration(&id, &s.hostname, &host, &who.actor)
        .await?;
    admin_auth::audit(&s, &who, "hostname.prepare", Some(&host), None).await;
    crate::keys::reload_soon(&s);
    Ok(Json(json!({
        "id": id,
        "host": host,
        "next": "Point the name's DNS at this server, then check it (aurora-locus migrate hostname --status) and confirm (--confirm).",
    })))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmInput {
    /// Dev mode only: confirm without the network pre-checks.
    #[serde(default)]
    skip_checks: bool,
    #[serde(flatten)]
    reauth: Reauth,
}

/// `owner.hostname.confirm`: starts the prepared migration once its
/// pre-checks pass. Every OAuth session signs in again.
pub async fn confirm(
    State(s): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<ConfirmInput>,
) -> XrpcResult<Json<Value>> {
    let who = owner(&s, &headers, locus_nsid!("owner.hostname.confirm")).await?;
    admin_auth::reauth(&s, &who, &input.reauth).await?;
    refuse_roles(&s)?;
    let m = s
        .store
        .open_hostname_migration()
        .await?
        .filter(|m| m.state == "prepared")
        .ok_or_else(|| XrpcError::invalid_request("No hostname is prepared"))?;
    if !(input.skip_checks && s.dev_mode) {
        let checks = prechecks(&s, &m.new_host).await;
        let json = serde_json::to_string(&checks).unwrap_or_default();
        s.store.set_hostname_prechecks(&m.id, &json).await?;
        if let Some((name, _, why)) = checks.iter().find(|(_, ok, _)| !ok) {
            return Err(XrpcError::invalid_request(format!(
                "The pre-check \"{name}\" fails: {why}"
            )));
        }
    }
    let window = s.snapshot().int("identity.hostname_transition_days") * 86_400_000;
    if !s.store.confirm_hostname_migration(&m.id, window).await? {
        return Err(XrpcError::invalid_request("No hostname is prepared"));
    }
    admin_auth::audit(
        &s,
        &who,
        "hostname.confirm",
        Some(&m.new_host),
        Some(json!({"from": m.old_host})),
    )
    .await;
    crate::keys::reload_soon(&s);
    Ok(Json(json!({
        "id": m.id,
        "host": m.new_host,
        "note": "The new hostname is current. Every OAuth session signs in again; the old one keeps full service while any DID document names it.",
    })))
}

/// `owner.hostname.cancel`: clears a prepared candidate.
pub async fn cancel(
    State(s): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(_): Json<Value>,
) -> XrpcResult<Json<Value>> {
    let who = owner(&s, &headers, locus_nsid!("owner.hostname.cancel")).await?;
    let id = s
        .store
        .cancel_hostname_migration()
        .await?
        .ok_or_else(|| XrpcError::invalid_request("No hostname is prepared"))?;
    admin_auth::audit(&s, &who, "hostname.cancel", Some(&id), None).await;
    crate::keys::reload_soon(&s);
    Ok(Json(json!({"id": id})))
}

/// `owner.hostname.close`: ends a running migration after its window,
/// with its stragglers still listed (and still served).
pub async fn close(
    State(s): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(_): Json<Value>,
) -> XrpcResult<Json<Value>> {
    let who = owner(&s, &headers, locus_nsid!("owner.hostname.close")).await?;
    let m = s
        .store
        .open_hostname_migration()
        .await?
        .filter(|m| m.state == "running")
        .ok_or_else(|| XrpcError::invalid_request("No hostname migration is running"))?;
    let now = s.store.db_now_ms().await?;
    if m.job_done_at.is_none() || m.window_ends_at.is_some_and(|w| now < w) {
        return Err(XrpcError::invalid_request(
            "A migration closes after its job is done and its window has ended",
        ));
    }
    let left = s.store.hostname_straggler_count(&m.id).await?;
    s.store.finish_hostname_migration(&m.id, left == 0).await?;
    admin_auth::audit(
        &s,
        &who,
        "hostname.close",
        Some(&m.id),
        Some(json!({"stragglers": left})),
    )
    .await;
    crate::keys::reload_soon(&s);
    Ok(Json(json!({"id": m.id, "stragglers": left})))
}

#[derive(Deserialize, Default)]
pub struct StatusParams {
    /// False: a prepared candidate's last pre-check results, not fresh ones.
    #[serde(default)]
    checks: Option<bool>,
}

/// `viewer.hostname.status`.
pub async fn status(
    State(s): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(p): Query<StatusParams>,
) -> XrpcResult<Json<Value>> {
    admin_auth::require(
        &s,
        &headers,
        locus_nsid!("viewer.hostname.status"),
        Surface::Locus,
        Role::Viewer,
    )
    .await?;
    let mut migrations = Vec::new();
    for m in s.store.hostname_migrations().await? {
        let stragglers = s.store.hostname_stragglers(&m.id, 100).await?;
        let progress: serde_json::Map<String, Value> = s
            .store
            .hostname_move_counts(&m.id)
            .await?
            .into_iter()
            .map(|(state, n)| (state, json!(n)))
            .collect();
        let prechecks: Value = m
            .prechecks
            .as_deref()
            .and_then(|p| serde_json::from_str(p).ok())
            .unwrap_or(Value::Null);
        migrations.push(json!({
            "id": m.id, "from": m.old_host, "to": m.new_host, "state": m.state,
            "createdAt": m.created_at, "createdBy": m.created_by,
            "confirmedAt": m.confirmed_at, "jobDoneAt": m.job_done_at,
            "windowEndsAt": m.window_ends_at, "closedAt": m.closed_at, "formerAt": m.former_at,
            "prechecks": prechecks, "progress": progress,
            "stragglerCount": s.store.hostname_straggler_count(&m.id).await?,
            "stragglers": stragglers.iter().map(|x| json!({"did": x.did, "state": x.state, "reason": x.reason})).collect::<Vec<_>>(),
        }));
    }
    // A prepared candidate's checks are run fresh, so status shows them.
    let open = s.store.open_hostname_migration().await?;
    let checks = match open.filter(|m| m.state == "prepared") {
        Some(m) if p.checks == Some(false) => m
            .prechecks
            .as_deref()
            .and_then(|c| serde_json::from_str::<Vec<(String, bool, String)>>(c).ok()),
        Some(m) => {
            let c = prechecks(&s, &m.new_host).await;
            let json = serde_json::to_string(&c).unwrap_or_default();
            s.store.set_hostname_prechecks(&m.id, &json).await?;
            Some(c)
        }
        None => None,
    };
    Ok(Json(json!({
        "hostname": s.hostname,
        "oldHostsServed": s.hosts().old_full.clone(),
        "candidate": s.hosts().candidate.clone(),
        "formerHosts": s.hosts().former.clone(),
        "prechecks": checks.map(|c| c.iter().map(|(n, ok, why)| json!({"check": n, "ok": ok, "why": why})).collect::<Vec<_>>()),
        "migrations": migrations,
    })))
}

/// What the job did for one account.
async fn move_account(
    s: &AppState,
    a: &locus_store::backups::AccountBrief,
) -> (&'static str, Option<String>) {
    let endpoint = s.public_url.clone();
    if a.did_method == "plc" {
        match crate::identity::plc_held_signer(s, &a.did).await {
            Ok(false) => {
                return (
                    "cannot",
                    Some("its DID document lists none of this PDS's rotation keys".into()),
                );
            }
            Err(e) => return ("failed", Some(e.message())),
            Ok(true) => {}
        }
        match crate::identity::plc_set_endpoint(s, &a.did, &endpoint).await {
            Ok(changed) => {
                if changed && let Err(e) = crate::identity::emit_identity(s, &a.did).await {
                    tracing::warn!(did = a.did, "#identity after the move: {}", e.message);
                }
                ("moved", None)
            }
            Err(e) => ("failed", Some(e.message())),
        }
    } else {
        match s.store.hosted_did_web(&a.did).await {
            Ok(Some(web)) => {
                let Some(mut doc) = web.doc.and_then(|d| serde_json::from_str::<Value>(&d).ok())
                else {
                    return ("moved", None);
                };
                set_doc_endpoint(&mut doc, &endpoint);
                let mut tx = match s.store.begin().await {
                    Ok(t) => t,
                    Err(e) => return ("failed", Some(e.to_string())),
                };
                let r = async {
                    tx.set_hosted_did_web_doc(&a.did, Some(&doc.to_string()))
                        .await?;
                    tx.commit().await
                }
                .await;
                if let Err(e) = r {
                    return ("failed", Some(e.to_string()));
                }
                let _ = crate::identity::emit_identity(s, &a.did).await;
                ("moved", None)
            }
            Ok(None) => (
                "cannot",
                Some("a did:web document its owner hosts: they update its PDS endpoint".into()),
            ),
            Err(e) => ("failed", Some(e.to_string())),
        }
    }
}

/// A DID document with its `#atproto_pds` endpoint set.
pub(crate) fn set_doc_endpoint(doc: &mut Value, endpoint: &str) {
    if let Some(services) = doc.get_mut("service").and_then(Value::as_array_mut) {
        for svc in services {
            let id = svc["id"].as_str().unwrap_or("");
            if id.ends_with("#atproto_pds") {
                svc["serviceEndpoint"] = json!(endpoint);
            }
        }
    }
}

/// Whether an account's live document names `host` as its PDS.
async fn still_names(s: &AppState, did: &str, host: &str) -> Option<bool> {
    if did.starts_with("did:plc:") {
        let (op, _) = crate::identity::plc_state(s, did).await.ok()?;
        let op = op?;
        Some(
            op.services
                .get("atproto_pds")
                .is_some_and(|svc| svc.endpoint.contains(&format!("//{host}"))),
        )
    } else {
        let web_host = did.strip_prefix("did:web:")?.replace("%3A", ":");
        let text = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .ok()?
            .get(format!("https://{web_host}/.well-known/did.json"))
            .send()
            .await
            .ok()?
            .text()
            .await
            .ok()?;
        Some(text.contains(&format!("//{host}")))
    }
}

/// One step of the hostname migration job.
pub async fn step(s: &AppState) -> CoreResult<bool> {
    for m in s.store.hostname_migrations().await? {
        let finished_with_stragglers = m.state == "done" && m.former_at.is_none();
        if m.state != "running" && !finished_with_stragglers {
            continue;
        }
        // Only a node already serving the new hostname moves accounts to it.
        if s.hostname != m.new_host {
            continue;
        }
        if m.state == "running" && m.job_done_at.is_none() {
            if m.cursor.is_none() {
                // The first step: relays learn the new host.
                let http = reqwest::Client::builder()
                    .timeout(Duration::from_secs(10))
                    .build()
                    .map_err(|e| locus_core::CoreError::Storage(e.to_string()))?;
                for c in s.snapshot().list("sync.crawlers") {
                    let _ = locus_sync::request_crawl(&http, c, &s.hostname).await;
                }
            }
            let after = m.cursor.clone().unwrap_or_default();
            let page = s.store.accounts_after(&after, 100).await?;
            for a in page.iter().filter(|a| a.status != "deleted") {
                let (state, reason) = move_account(s, a).await;
                s.store
                    .record_hostname_move(&m.id, &a.did, state, reason.as_deref())
                    .await?;
                // Within the PLC directory's rate limits.
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            match page.last() {
                Some(last) if page.len() == 100 => {
                    s.store.set_hostname_cursor(&m.id, Some(&last.did)).await?;
                }
                _ => s.store.set_hostname_cursor(&m.id, None).await?,
            }
            return Ok(true);
        }
        // The job is done: recheck the stragglers daily; with none left
        // after the window, the old host becomes former.
        let now = s.store.db_now_ms().await?;
        if m.rechecked_at.is_none_or(|r| now - r > 86_400_000) {
            for x in s.store.hostname_stragglers(&m.id, 1000).await? {
                if still_names(s, &x.did, &m.old_host).await == Some(false) {
                    s.store
                        .record_hostname_move(&m.id, &x.did, "moved", None)
                        .await?;
                }
            }
            s.store.set_hostname_rechecked(&m.id).await?;
        }
        let left = s.store.hostname_straggler_count(&m.id).await?;
        let window_over = m.window_ends_at.is_some_and(|w| now >= w);
        if left == 0 && (window_over || finished_with_stragglers) {
            s.store.finish_hostname_migration(&m.id, true).await?;
            tracing::info!(id = m.id, old = m.old_host, "hostname migration done");
            crate::keys::reload_soon(s);
            return Ok(true);
        }
    }
    Ok(false)
}
