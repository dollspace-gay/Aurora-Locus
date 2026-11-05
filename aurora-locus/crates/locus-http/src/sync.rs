//! `com.atproto.sync.subscribeRepos`: the firehose.
//!
//! A subscriber with a cursor is backfilled from the log in pages, then
//! switched to the live stream (deduplicated by seq). A cursor ahead of the
//! log gets `FutureCursor`; one behind retention gets `#info
//! OutdatedCursor` and continues from the oldest retained event. A
//! subscriber that falls too far behind live gets `ConsumerTooSlow`.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::Response;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;

use crate::state::AppState;

type St = State<Arc<AppState>>;

const PAGE: i64 = 500;

#[derive(Deserialize)]
pub struct SubscribeParams {
    cursor: Option<i64>,
}

pub async fn subscribe_repos(
    State(s): St,
    Query(p): Query<SubscribeParams>,
    ws: WebSocketUpgrade,
) -> Response {
    let place = match crate::ratelimit::subscribe(&s) {
        Ok(g) => g,
        Err(e) => return axum::response::IntoResponse::into_response(e),
    };
    ws.on_upgrade(move |socket| serve(s, socket, p.cursor, place))
}

async fn serve(
    s: Arc<AppState>,
    socket: WebSocket,
    cursor: Option<i64>,
    _place: crate::ratelimit::SubscriberGuard,
) {
    let (mut tx, mut rx) = socket.split();
    let send = |bytes: Vec<u8>| Message::Binary(bytes.into());
    // Subscribe before reading the log, so nothing falls between.
    let mut live = s.firehose.subscribe();
    let store = s.firehose.store();
    let Ok(state) = store.seq_state().await else {
        return;
    };
    let mut pos = match cursor {
        None => state.last_seq,
        Some(c) if c > state.last_seq => {
            let _ = tx
                .send(send(locus_sync::error_frame(
                    "FutureCursor",
                    "Cursor in the future.",
                )))
                .await;
            let _ = tx.close().await;
            return;
        }
        Some(c) => c,
    };
    // Backfill, checking retention before each page.
    if cursor.is_some() {
        loop {
            let Ok(state) = store.seq_state().await else {
                return;
            };
            if pos < state.pruned_through {
                let msg = "Requested cursor exceeded limit. Possibly missing events";
                if tx
                    .send(send(locus_sync::info_frame("OutdatedCursor", msg)))
                    .await
                    .is_err()
                {
                    return;
                }
                pos = state.pruned_through;
            }
            let Ok(page) = store.seq_events_after(pos, PAGE).await else {
                return;
            };
            let full = page.len() == PAGE as usize;
            for ev in &page {
                let Ok(f) = locus_sync::frame(ev) else {
                    continue;
                };
                if tx.send(send(f.bytes)).await.is_err() {
                    return;
                }
                pos = ev.seq;
            }
            if !full {
                break;
            }
        }
    }
    let ping_every = Duration::from_secs(
        u64::try_from(s.snapshot().int("sync.ws_ping_interval_s")).unwrap_or(30),
    );
    let mut ping = tokio::time::interval(ping_every);
    ping.tick().await;
    loop {
        tokio::select! {
            ev = live.recv() => match ev {
                Ok(ev) => {
                    if ev.seq <= pos {
                        continue;
                    }
                    if tx.send(send(ev.bytes.clone())).await.is_err() {
                        return;
                    }
                    pos = ev.seq;
                }
                Err(RecvError::Lagged(_)) => {
                    let _ = tx.send(send(locus_sync::error_frame("ConsumerTooSlow", "Stream consumer too slow"))).await;
                    let _ = tx.close().await;
                    return;
                }
                Err(RecvError::Closed) => return,
            },
            _ = ping.tick() => {
                if tx.send(Message::Ping(Vec::new().into())).await.is_err() {
                    return;
                }
            }
            msg = rx.next() => match msg {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                Some(Ok(_)) => {}
            },
        }
    }
}

// ── Sync reads ────────────────────────────────────────────────────────

use axum::Json;
use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, header};
use locus_store::accounts::{Account, Status};
use proto_blue_lex_data::Cid;
use serde_json::{Value, json};

use crate::error::{XrpcError, XrpcResult};

const CAR: &str = "application/vnd.ipld.car";
const BLOCK_PAGE: i64 = 1000;

/// The reference's `assertRepoAvailability`: a missing or deleted account
/// is `RepoNotFound`; for anyone but its owner, a taken-down, suspended or
/// deactivated one is refused.
async fn available(s: &AppState, headers: &HeaderMap, did: &str) -> XrpcResult<Account> {
    let account = s
        .store
        .account_by_did(did)
        .await?
        .filter(|a| a.status != Status::Deleted)
        .ok_or_else(|| {
            XrpcError::named(
                "RepoNotFound",
                format!("Could not find repo for DID: {did}"),
            )
        })?;
    let owner = crate::auth::authorization(
        s,
        headers,
        crate::auth::STANDARD,
        &[locus_auth::session::SCOPE_TAKENDOWN],
        crate::auth::any_scope,
    )
    .await
    .is_ok_and(|a| a.did == account.did);
    if !owner {
        crate::repo::unavailable(&account)?;
    }
    Ok(account)
}

fn car_response(body: Body) -> Response {
    let mut res = Response::new(body);
    res.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(CAR));
    res
}

/// A repository as a CAR, streamed in pages: the commit as root, every
/// live block (or only those introduced after `since`).
fn repo_car(s: &AppState, did: String, root: Cid, since: Option<String>) -> Response {
    let store = s.store.clone();
    let head = futures::stream::once(async move {
        Ok::<_, std::io::Error>(axum::body::Bytes::from(locus_repo::car::header(Some(
            &root,
        ))))
    });
    let pages = futures::stream::unfold(Some(None::<String>), move |after| {
        let (store, did, since) = (store.clone(), did.clone(), since.clone());
        async move {
            let after = after?;
            match store
                .repo_blocks_page(&did, after.as_deref(), since.as_deref(), BLOCK_PAGE)
                .await
            {
                Ok(page) if page.is_empty() => None,
                Ok(page) => {
                    let mut chunk = Vec::new();
                    for (cid, bytes) in &page {
                        if let Ok(c) = Cid::from_str_multibase(cid) {
                            chunk.extend(locus_repo::car::block(&c, bytes));
                        }
                    }
                    let next = (page.len() as i64 == BLOCK_PAGE)
                        .then(|| page.last().map(|(c, _)| c.clone()));
                    Some((Ok(axum::body::Bytes::from(chunk)), next))
                }
                Err(e) => Some((Err(std::io::Error::other(e.to_string())), None)),
            }
        }
    });
    car_response(Body::from_stream(head.chain(pages)))
}

#[derive(Deserialize)]
pub struct GetRepoParams {
    did: String,
    since: Option<String>,
}

pub async fn get_repo(
    State(s): St,
    headers: HeaderMap,
    Query(p): Query<GetRepoParams>,
) -> XrpcResult<Response> {
    available(&s, &headers, &p.did).await?;
    let root = s.store.repo_root_of(&p.did).await?.ok_or_else(|| {
        XrpcError::named(
            "RepoNotFound",
            format!("Could not find repo for DID: {}", p.did),
        )
    })?;
    let cid = Cid::from_str_multibase(&root.commit_cid)
        .map_err(|e| XrpcError::internal(e.to_string()))?;
    Ok(repo_car(&s, p.did, cid, p.since))
}

#[derive(Deserialize)]
pub struct DidParam {
    did: String,
}

/// Deprecated: the whole repository, as `getRepo`.
pub async fn get_checkout(
    State(s): St,
    headers: HeaderMap,
    Query(p): Query<DidParam>,
) -> XrpcResult<Response> {
    get_repo(
        State(s),
        headers,
        Query(GetRepoParams {
            did: p.did,
            since: None,
        }),
    )
    .await
}

#[derive(Deserialize)]
pub struct SyncGetRecordParams {
    did: String,
    collection: String,
    rkey: String,
}

/// A record with its proof (or a proof of absence) as a CAR rooted at the
/// current commit.
pub async fn get_record(
    State(s): St,
    headers: HeaderMap,
    Query(p): Query<SyncGetRecordParams>,
) -> XrpcResult<Response> {
    available(&s, &headers, &p.did).await?;
    let (commit, blocks) = s
        .repo
        .record_proof(&p.did, &p.collection, &p.rkey)
        .await
        .map_err(|e| XrpcError::internal(e.to_string()))?
        .ok_or_else(|| {
            XrpcError::named(
                "RepoNotFound",
                format!("Could not find repo for DID: {}", p.did),
            )
        })?;
    let mut car = locus_repo::car::header(Some(&commit));
    for (cid, bytes) in blocks.iter() {
        car.extend(locus_repo::car::block(cid, bytes));
    }
    Ok(car_response(Body::from(car)))
}

#[derive(Deserialize)]
pub struct GetBlocksParams {
    did: String,
    #[serde(default)]
    cids: Vec<String>,
}

pub async fn get_blocks(
    State(s): St,
    headers: HeaderMap,
    axum::extract::RawQuery(q): axum::extract::RawQuery,
) -> XrpcResult<Response> {
    // `cids` repeats in the query string.
    let mut p = GetBlocksParams {
        did: String::new(),
        cids: Vec::new(),
    };
    for (k, v) in url::form_urlencoded::parse(q.unwrap_or_default().as_bytes()) {
        match k.as_ref() {
            "did" => p.did = v.into_owned(),
            "cids" => p.cids.push(v.into_owned()),
            _ => {}
        }
    }
    available(&s, &headers, &p.did).await?;
    let mut car = locus_repo::car::header(None);
    let mut missing = Vec::new();
    for c in &p.cids {
        let cid = Cid::from_str_multibase(c)
            .map_err(|_| XrpcError::invalid_request(format!("Invalid CID: {c}")))?;
        match s.store.block(&p.did, &cid.to_string_base32()).await? {
            Some(bytes) => car.extend(locus_repo::car::block(&cid, &bytes)),
            None => missing.push(c.clone()),
        }
    }
    if !missing.is_empty() {
        return Err(XrpcError::invalid_request(format!(
            "Could not find cids: {}",
            missing.join(",")
        )));
    }
    Ok(car_response(Body::from(car)))
}

pub async fn get_latest_commit(
    State(s): St,
    headers: HeaderMap,
    Query(p): Query<DidParam>,
) -> XrpcResult<Json<Value>> {
    available(&s, &headers, &p.did).await?;
    let root = s.store.repo_root_of(&p.did).await?.ok_or_else(|| {
        XrpcError::named(
            "RepoNotFound",
            format!("Could not find root for DID: {}", p.did),
        )
    })?;
    Ok(Json(json!({ "cid": root.commit_cid, "rev": root.rev })))
}

/// Deprecated: the current commit.
pub async fn get_head(
    State(s): St,
    headers: HeaderMap,
    Query(p): Query<DidParam>,
) -> XrpcResult<Json<Value>> {
    available(&s, &headers, &p.did).await?;
    let root = s.store.repo_root_of(&p.did).await?.ok_or_else(|| {
        XrpcError::named(
            "HeadNotFound",
            format!("Could not find root for DID: {}", p.did),
        )
    })?;
    Ok(Json(json!({ "root": root.commit_cid })))
}

fn status_fields(status: Status) -> (bool, Option<&'static str>) {
    match status {
        Status::Active => (true, None),
        other => (false, Some(other.as_str())),
    }
}

/// Status for everyone, as the reference (no refusals; `rev` only while active).
pub async fn get_repo_status(State(s): St, Query(p): Query<DidParam>) -> XrpcResult<Json<Value>> {
    let account = s
        .store
        .account_by_did(&p.did)
        .await?
        .filter(|a| a.status != Status::Deleted)
        .ok_or_else(|| {
            XrpcError::named(
                "RepoNotFound",
                format!("Could not find repo for DID: {}", p.did),
            )
        })?;
    let (active, status) = status_fields(account.status);
    let mut out = json!({ "did": account.did, "active": active });
    if let Some(st) = status {
        out["status"] = json!(st);
    }
    if active && let Some(root) = s.store.repo_root_of(&account.did).await? {
        out["rev"] = json!(root.rev);
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct ListReposParams {
    limit: Option<i64>,
    cursor: Option<String>,
}

pub async fn list_repos(State(s): St, Query(p): Query<ListReposParams>) -> XrpcResult<Json<Value>> {
    let limit = p.limit.unwrap_or(500);
    if !(1..=1000).contains(&limit) {
        return Err(XrpcError::invalid_request(
            "limit must be between 1 and 1000",
        ));
    }
    let cursor = match p.cursor.as_deref() {
        Some(c) => {
            let (at, did) = c
                .split_once("::")
                .ok_or_else(|| XrpcError::invalid_request("Malformed cursor"))?;
            Some((
                at.parse::<i64>()
                    .map_err(|_| XrpcError::invalid_request("Malformed cursor"))?,
                did.to_owned(),
            ))
        }
        None => None,
    };
    let rows = s
        .store
        .list_repos(limit, cursor.as_ref().map(|(a, d)| (*a, d.as_str())))
        .await?;
    let repos: Vec<Value> = rows
        .iter()
        .map(|(did, _, head, rev, status)| {
            let status = Status::parse(status).unwrap_or(Status::Active);
            let (active, st) = status_fields(status);
            let mut r = json!({ "did": did, "head": head, "rev": rev, "active": active });
            if let Some(st) = st {
                r["status"] = json!(st);
            }
            r
        })
        .collect();
    let mut out = json!({ "repos": repos });
    if rows.len() as i64 == limit
        && let Some((did, at, ..)) = rows.last()
    {
        out["cursor"] = json!(format!("{at}::{did}"));
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct ListBlobsParams {
    did: String,
    since: Option<String>,
    limit: Option<i64>,
    cursor: Option<String>,
}

pub async fn list_blobs(
    State(s): St,
    headers: HeaderMap,
    Query(p): Query<ListBlobsParams>,
) -> XrpcResult<Json<Value>> {
    available(&s, &headers, &p.did).await?;
    let limit = p.limit.unwrap_or(500);
    if !(1..=1000).contains(&limit) {
        return Err(XrpcError::invalid_request(
            "limit must be between 1 and 1000",
        ));
    }
    let mut cids = s
        .store
        .list_blob_cids(&p.did, p.since.as_deref(), p.cursor.as_deref(), limit)
        .await?;
    let cursor = cids.last().cloned();
    // While the media gate is on, private-only blobs are listed only to the
    // owner and trusted services.
    if crate::media_gate::gate_on(&s) {
        use crate::media_gate::Reader;
        let reader = crate::media_gate::reader(&s, &headers, "com.atproto.sync.listBlobs").await;
        let sees_all = matches!(&reader, Reader::Trusted(_))
            || matches!(&reader, Reader::Account(d) if *d == p.did);
        if !sees_all {
            let mut visible = Vec::with_capacity(cids.len());
            for c in cids {
                if !s.store.blob_private_only(&p.did, &c).await? {
                    visible.push(c);
                }
            }
            cids = visible;
        }
    }
    let mut out = json!({ "cids": cids });
    if let Some(last) = cursor {
        out["cursor"] = json!(last);
    }
    Ok(Json(out))
}
