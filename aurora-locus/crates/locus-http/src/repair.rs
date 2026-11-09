//! Operator repair tools: `repo rebuild` (the reference's `rebuild-repo`: a
//! new tree and commit from the record index), `repair repo` (the record
//! index from the tree), `repair identity` (the PLC document back to what
//! this PDS holds, then `#identity`) and `repair tombstone` (a gone
//! account's did:plc tombstoned).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use locus_core::locus_nsid;
use locus_repo::import::{TreeWalk, decode_record, import_blob_cids, split_tree_key};
use locus_store::repo::{BlockDelta, RecordChange, RepoRoot};
use proto_blue_crypto::Keypair;
use proto_blue_lex_data::Cid;
use proto_blue_repo::commit::{UnsignedCommit, sign_commit};
use proto_blue_repo::mst::MstNode;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::admin_auth::{self, Role, Surface};
use crate::error::{XrpcError, XrpcResult};
use crate::state::AppState;

type St = State<Arc<AppState>>;

fn internal(e: impl std::fmt::Display) -> XrpcError {
    XrpcError::internal(e.to_string())
}

#[derive(Deserialize)]
pub struct DidInput {
    did: String,
}

async fn admin(s: &AppState, headers: &HeaderMap, lxm: &str) -> XrpcResult<admin_auth::Caller> {
    admin_auth::require(s, headers, lxm, Surface::Locus, Role::Admin).await
}

/// Every record index row of an account: `collection/rkey` → CID.
async fn record_index(s: &AppState, did: &str) -> XrpcResult<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    let mut after: Option<(String, String)> = None;
    loop {
        let page = s
            .store
            .record_index_page(
                did,
                after.as_ref().map(|(c, r)| (c.as_str(), r.as_str())),
                1000,
            )
            .await?;
        let Some((c, r, _)) = page.last() else { break };
        after = Some((c.clone(), r.clone()));
        for (c, r, cid) in page {
            out.insert(format!("{c}/{r}"), cid);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- rebuild

/// `admin.repo.rebuild`: a new tree built from the record index, and a new
/// commit over it signed with the account's key, in one transaction under
/// the repo lock: every block of the account loses its references and the
/// new tree's gain theirs (the rest go to GC), the root moves, `#sync`
/// goes out. Records whose blocks are gone are left out and listed.
pub async fn rebuild(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<DidInput>,
) -> XrpcResult<Json<Value>> {
    let who = admin(&s, &headers, locus_nsid!("admin.repo.rebuild")).await?;
    let out = rebuild_repo(&s, &input.did).await?;
    admin_auth::audit(
        &s,
        &who,
        "repo.rebuild",
        Some(&input.did),
        Some(out.clone()),
    )
    .await;
    Ok(Json(out))
}

/// The rebuild itself (also the first step of `repair repos
/// --from-sequencer`).
pub(crate) async fn rebuild_repo(s: &AppState, did: &str) -> XrpcResult<Value> {
    s.store
        .account_by_did(did)
        .await?
        .ok_or_else(|| XrpcError::invalid_request("Account not found"))?;
    let root = s
        .store
        .repo_root_of(did)
        .await?
        .ok_or_else(|| XrpcError::invalid_request("The account has no repository"))?;
    let index = record_index(s, did).await?;
    let mut tree = MstNode::empty();
    let mut records: Vec<(Cid, Vec<u8>)> = Vec::new();
    let mut missing = Vec::new();
    for (path, cid) in &index {
        match s.store.block(did, cid).await? {
            Some(bytes) => {
                let c = Cid::from_str_multibase(cid).map_err(internal)?;
                tree = tree.add(path, c.clone()).map_err(internal)?;
                records.push((c, bytes));
            }
            None => missing.push(path.clone()),
        }
    }
    let (data, nodes) = tree.get_all_blocks().map_err(internal)?;
    let key = s.signing_key(did).await?;
    let rev = s.repo.tids().next_after(Some(&root.rev));
    let commit = sign_commit(
        &UnsignedCommit::new(did.to_owned(), data.clone(), rev.clone(), None),
        &key,
    )
    .map_err(internal)?;
    let commit_bytes = commit.to_cbor().map_err(internal)?;
    let commit_cid = commit.cid().map_err(internal)?;

    // One transaction under the repo lock: every block loses its
    // references, the new tree's gain theirs, the root moves, #sync.
    let mut deltas: Vec<BlockDelta> = vec![BlockDelta {
        cid: commit_cid.to_string_base32(),
        bytes: Some(commit_bytes.clone()),
        delta: 1,
    }];
    for (cid, bytes) in nodes.iter() {
        deltas.push(BlockDelta {
            cid: cid.to_string_base32(),
            bytes: Some(bytes.to_vec()),
            delta: 1,
        });
    }
    let mut counts: HashMap<String, (i64, Vec<u8>)> = HashMap::new();
    for (cid, bytes) in records {
        counts
            .entry(cid.to_string_base32())
            .or_insert_with(|| (0, bytes))
            .0 += 1;
    }
    let record_count: i64 = counts.values().map(|(n, _)| *n).sum();
    for (cid, (n, bytes)) in counts {
        deltas.push(BlockDelta {
            cid,
            bytes: Some(bytes),
            delta: n,
        });
    }
    let (_lock, mut tx) = s.repo.locked_tx(did).await.map_err(internal)?;
    let dropped = tx.zero_block_counts(did).await?;
    tx.apply_block_deltas(did, &rev, &deltas).await?;
    tx.set_repo_root(
        did,
        &RepoRoot {
            commit_cid: commit_cid.to_string_base32(),
            rev: rev.clone(),
            data_cid: data.to_string_base32(),
        },
    )
    .await?;
    let body =
        locus_repo::event::sync_body(did, &rev, &commit_cid, &commit_bytes).map_err(internal)?;
    tx.insert_outbox(did, "sync", &body).await?;
    tx.commit().await?;
    Ok(json!({
        "did": did,
        "records": record_count,
        "treeNodes": nodes.iter().count(),
        "blocksRecounted": dropped,
        "missingRecords": missing,
    }))
}

// ---------------------------------------------------------------- record index

/// `admin.repo.repair`: the record index made to match the repository's
/// tree (missing rows added, stale ones fixed, extra ones removed, with
/// their blob references and backlinks). The tree must be whole; a tree
/// missing blocks needs `repo rebuild`.
pub async fn repair_repo(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<DidInput>,
) -> XrpcResult<Json<Value>> {
    let who = admin(&s, &headers, locus_nsid!("admin.repo.repair")).await?;
    let out = reindex(&s, &input.did).await?;
    admin_auth::audit(&s, &who, "repo.repair", Some(&input.did), Some(out.clone())).await;
    Ok(Json(out))
}

/// The record index from the tree (also the end of a recovery).
pub(crate) async fn reindex(s: &AppState, did: &str) -> XrpcResult<Value> {
    let root = s
        .store
        .repo_root_of(did)
        .await?
        .ok_or_else(|| XrpcError::invalid_request("The account has no repository"))?;
    let mut walk = TreeWalk::new(Cid::from_str_multibase(&root.data_cid).map_err(internal)?);
    let mut tree: HashMap<String, String> = HashMap::new();
    while let Some(cid) = walk
        .next_node()
        .map_err(|e| XrpcError::invalid_request(e.to_string()))?
    {
        let k = cid.to_string_base32();
        let bytes = s.store.block(did, &k).await?.ok_or_else(|| {
            XrpcError::invalid_request(format!(
                "The tree is missing block {k}; run `repo rebuild` instead"
            ))
        })?;
        for (path, value) in walk
            .visit(&bytes)
            .map_err(|e| XrpcError::invalid_request(e.to_string()))?
        {
            tree.insert(path, value.to_string_base32());
        }
    }
    let index = record_index(s, did).await?;
    let (_lock, mut tx) = s.repo.locked_tx(did).await.map_err(internal)?;
    let (mut added, mut updated, mut removed) = (0u64, 0u64, 0u64);
    for (path, cid) in &tree {
        let was = index.get(path);
        if was == Some(cid) {
            continue;
        }
        let (collection, rkey) = split_tree_key(path).map_err(internal)?;
        let bytes = tx
            .repo_block(did, cid)
            .await?
            .ok_or_else(|| XrpcError::invalid_request(format!("record block {cid} is missing")))?;
        let value = decode_record(&collection, &rkey, &bytes)
            .map_err(|e| XrpcError::invalid_request(e.to_string()))?;
        tx.apply_record_changes(
            did,
            &root.rev,
            &[RecordChange::Put {
                collection: collection.clone(),
                rkey: rkey.clone(),
                cid: cid.clone(),
            }],
        )
        .await?;
        tx.replace_record_blobs(did, &collection, &rkey, &import_blob_cids(&value))
            .await?;
        tx.replace_backlinks(
            did,
            &collection,
            &rkey,
            &locus_repo::records::backlinks(&collection, &value),
        )
        .await?;
        if was.is_some() {
            updated += 1;
        } else {
            added += 1;
        }
    }
    for path in index.keys().filter(|p| !tree.contains_key(*p)) {
        let (collection, rkey) = split_tree_key(path).map_err(internal)?;
        tx.apply_record_changes(
            did,
            &root.rev,
            &[RecordChange::Delete {
                collection: collection.clone(),
                rkey: rkey.clone(),
            }],
        )
        .await?;
        tx.replace_record_blobs(did, &collection, &rkey, &[])
            .await?;
        tx.replace_backlinks(did, &collection, &rkey, &[]).await?;
        removed += 1;
    }
    tx.commit().await?;
    Ok(json!({
        "did": did,
        "records": tree.len(),
        "added": added,
        "updated": updated,
        "removed": removed,
    }))
}

// ---------------------------------------------------------------- identity

/// `admin.identity.repair`: a did:plc account's document made to name its
/// handle, this PDS and the signing key it holds (one operation, signed
/// with a held rotation key the document lists), then `#identity`.
pub async fn repair_identity(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<DidInput>,
) -> XrpcResult<Json<Value>> {
    let who = admin(&s, &headers, locus_nsid!("admin.identity.repair")).await?;
    let did = input.did.as_str();
    let account = s
        .store
        .account_by_did(did)
        .await?
        .ok_or_else(|| XrpcError::invalid_request("Account not found"))?;
    let mut changed = false;
    if did.starts_with("did:plc:") {
        let key = s.signing_key(did).await?.did();
        let handle = account.handle.clone();
        let endpoint = s.public_url.clone();
        let aka = format!("at://{handle}");
        changed = crate::identity::plc_update(
            &s,
            did,
            |op| {
                op.also_known_as.first() == Some(&aka)
                    && op.verification_methods.get("atproto") == Some(&key)
                    && op
                        .services
                        .get("atproto_pds")
                        .is_some_and(|p| p.endpoint == endpoint)
            },
            |op| {
                op.also_known_as.retain(|a| !a.starts_with("at://"));
                op.also_known_as.insert(0, aka.clone());
                op.verification_methods
                    .insert("atproto".to_owned(), key.clone());
                op.services.insert(
                    "atproto_pds".to_owned(),
                    locus_identity::plc::Service {
                        kind: "AtprotoPersonalDataServer".to_owned(),
                        endpoint: endpoint.clone(),
                    },
                );
            },
        )
        .await
        .map_err(|e| match e {
            crate::identity::PlcStep::Refused(x) | crate::identity::PlcStep::Ambiguous(x) => x,
        })?;
    }
    let _ = s.store.clear_did_doc(did).await;
    crate::identity::emit_identity(&s, did).await?;
    admin_auth::audit(
        &s,
        &who,
        "identity.repair",
        Some(did),
        Some(json!({"plcUpdated": changed})),
    )
    .await;
    Ok(Json(
        json!({"did": did, "plcUpdated": changed, "identityEmitted": true}),
    ))
}

/// `admin.identity.tombstone`: tombstones the did:plc of an account that's
/// gone (deleted, or never finished), as `accounts.tombstone_plc_on_delete`
/// would have at deletion. Same limits as the repair job: only while the
/// document names this PDS and lists a rotation key it holds. When PLC's
/// answer is unclear it's queued for the repair job instead.
pub async fn tombstone_identity(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<DidInput>,
) -> XrpcResult<Json<Value>> {
    let who = admin(&s, &headers, locus_nsid!("admin.identity.tombstone")).await?;
    let did = input.did.as_str();
    if !did.starts_with("did:plc:") {
        return Err(XrpcError::invalid_request(
            "Only a did:plc can be tombstoned",
        ));
    }
    if s.role == locus_core::bootstrap::Role::Shard {
        return Err(XrpcError::invalid_request(
            "A shard holds no PLC rotation key; run this on the entryway",
        ));
    }
    if s.store
        .account_by_did(did)
        .await?
        .is_some_and(|a| a.status != locus_store::accounts::Status::Deleted)
    {
        return Err(XrpcError::invalid_request(
            "An account still holds this DID; delete the account first",
        ));
    }
    let outcome = match crate::identity::plc_tombstone(&s, did).await {
        Ok(()) => "tombstoned",
        Err(crate::identity::PlcStep::Refused(e)) => return Err(e),
        Err(crate::identity::PlcStep::Ambiguous(_)) => {
            s.store
                .queue_identity_repair(did, locus_store::identity::RepairKind::Tombstone, did)
                .await?;
            "queued"
        }
    };
    admin_auth::audit(
        &s,
        &who,
        "identity.tombstone",
        Some(did),
        Some(json!({"outcome": outcome})),
    )
    .await;
    Ok(Json(json!({"did": did, "outcome": outcome})))
}

// ---------------------------------------------------------------- the event log

/// A `#commit` from the event log.
struct CommitEvt {
    rev: String,
    commit: Cid,
    blocks: Vec<(Cid, Vec<u8>)>,
    /// (action, `collection/rkey`, record CID).
    ops: Vec<(String, String, Option<String>)>,
}

fn parse_commit(payload: &[u8]) -> Option<(String, CommitEvt)> {
    use proto_blue_lex_data::LexValue;
    let v: LexValue = proto_blue_lex_cbor::decode(payload).ok()?;
    let m = v.as_map()?;
    let did = m.get("repo")?.as_str()?.to_owned();
    let rev = m.get("rev")?.as_str()?.to_owned();
    let commit = m.get("commit")?.as_cid()?.clone();
    let car = match m.get("blocks")? {
        LexValue::Bytes(b) => b.clone(),
        _ => return None,
    };
    let (_, map) = proto_blue_repo::car::read_car(&car).ok()?;
    let blocks = map.iter().map(|(c, b)| (c.clone(), b.to_vec())).collect();
    let ops = m
        .get("ops")
        .and_then(LexValue::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|op| {
                    let o = op.as_map()?;
                    Some((
                        o.get("action")?.as_str()?.to_owned(),
                        o.get("path")?.as_str()?.to_owned(),
                        o.get("cid")
                            .and_then(LexValue::as_cid)
                            .map(Cid::to_string_base32),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    Some((
        did,
        CommitEvt {
            rev,
            commit,
            blocks,
            ops,
        },
    ))
}

/// The retained `#commit` events after `since`, per account, in order.
async fn commit_events(
    s: &AppState,
    only: Option<&str>,
    since: i64,
) -> XrpcResult<BTreeMap<String, Vec<CommitEvt>>> {
    let mut out: BTreeMap<String, Vec<CommitEvt>> = BTreeMap::new();
    let mut cursor = since;
    loop {
        let page = s.store.seq_events_after(cursor, 1000).await?;
        let Some(last) = page.last() else { break };
        cursor = last.seq;
        for ev in page {
            if ev.kind != "commit" || only.is_some_and(|d| d != ev.did) {
                continue;
            }
            if let Some((did, c)) = parse_commit(&ev.payload) {
                out.entry(did).or_default().push(c);
            }
        }
    }
    Ok(out)
}

/// Walks the tree under `data` from the account's stored blocks: its nodes
/// (with bytes) and leaf values. Err names a missing block.
async fn walk_tree(
    tx: &mut locus_store::Tx,
    did: &str,
    data: &Cid,
) -> Result<(Vec<(String, Vec<u8>)>, Vec<String>), String> {
    let mut walk = TreeWalk::new(data.clone());
    let (mut nodes, mut leaves) = (Vec::new(), Vec::new());
    while let Some(cid) = walk.next_node().map_err(|e| e.to_string())? {
        let k = cid.to_string_base32();
        let bytes = tx
            .any_repo_block(did, &k)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("tree node {k} is missing"))?;
        for (_, value) in walk.visit(&bytes).map_err(|e| e.to_string())? {
            leaves.push(value.to_string_base32());
        }
        nodes.push((k, bytes));
    }
    Ok((nodes, leaves))
}

/// One account: the event log's blocks stored, then the newest commit
/// after the current root whose whole tree is there adopted (blocks
/// re-counted, root moved, `#sync`). Ok(false): already up to date.
async fn recover_one(s: &AppState, did: &str, events: &[CommitEvt]) -> Result<bool, String> {
    let e = |x: locus_core::CoreError| x.to_string();
    let current = s
        .store
        .repo_root_of(did)
        .await
        .map_err(e)?
        .map(|r| r.rev)
        .unwrap_or_default();
    let (_lock, mut tx) = s.repo.locked_tx(did).await.map_err(|x| x.to_string())?;
    for ev in events {
        for (cid, bytes) in &ev.blocks {
            tx.put_unreferenced_block(did, &cid.to_string_base32(), &ev.rev, bytes)
                .await
                .map_err(e)?;
        }
    }
    let mut newer: Vec<&CommitEvt> = events.iter().filter(|ev| ev.rev > current).collect();
    newer.sort_by(|a, b| b.rev.cmp(&a.rev));
    if newer.is_empty() {
        tx.commit().await.map_err(e)?;
        return Ok(false);
    }
    let mut why: Option<String> = None;
    for ev in newer {
        let ck = ev.commit.to_string_base32();
        let Some(commit_bytes) = tx.any_repo_block(did, &ck).await.map_err(e)? else {
            why.get_or_insert_with(|| format!("commit {ck} is missing"));
            continue;
        };
        let Some(commit) = proto_blue_lex_cbor::decode(&commit_bytes)
            .ok()
            .and_then(|v| proto_blue_repo::commit::SignedCommit::from_lex_value(&v).ok())
        else {
            continue;
        };
        let (nodes, leaves) = match walk_tree(&mut tx, did, &commit.data).await {
            Ok(t) => t,
            Err(w) => {
                why.get_or_insert(w);
                continue;
            }
        };
        let mut deltas = vec![BlockDelta {
            cid: ck.clone(),
            bytes: Some(commit_bytes.clone()),
            delta: 1,
        }];
        for (cid, bytes) in nodes {
            deltas.push(BlockDelta {
                cid,
                bytes: Some(bytes),
                delta: 1,
            });
        }
        let mut counts: HashMap<String, i64> = HashMap::new();
        for l in leaves {
            *counts.entry(l).or_default() += 1;
        }
        for (cid, n) in counts {
            let Some(bytes) = tx.any_repo_block(did, &cid).await.map_err(e)? else {
                return Err(format!("record block {cid} is missing"));
            };
            deltas.push(BlockDelta {
                cid,
                bytes: Some(bytes),
                delta: n,
            });
        }
        tx.zero_block_counts(did).await.map_err(e)?;
        tx.apply_block_deltas(did, &ev.rev, &deltas)
            .await
            .map_err(e)?;
        tx.set_repo_root(
            did,
            &RepoRoot {
                commit_cid: ck,
                rev: ev.rev.clone(),
                data_cid: commit.data.to_string_base32(),
            },
        )
        .await
        .map_err(e)?;
        let body = locus_repo::event::sync_body(did, &ev.rev, &ev.commit, &commit_bytes)
            .map_err(|x| x.to_string())?;
        tx.insert_outbox(did, "sync", &body).await.map_err(e)?;
        tx.commit().await.map_err(e)?;
        return Ok(true);
    }
    tx.commit().await.map_err(e)?;
    Err(format!(
        "no commit in the event log after the current one has its whole tree (newest: {})",
        why.unwrap_or_default()
    ))
}

#[derive(Deserialize, Default)]
pub struct RecoverInput {
    did: Option<String>,
    #[serde(default)]
    since: i64,
}

/// `admin.repo.recoverFromSequencer` (`sequencer recover`): replays the
/// retained event log onto repositories, as the reference's
/// `sequencer-recovery`: each account's newest commit after its current
/// root whose tree the log (with what's stored) holds whole is adopted and
/// the record index follows; accounts it can't recover are recorded for
/// `repair repos --from-sequencer`.
pub async fn recover_from_sequencer(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<RecoverInput>,
) -> XrpcResult<Json<Value>> {
    let who = admin(&s, &headers, locus_nsid!("admin.repo.recoverFromSequencer")).await?;
    let events = commit_events(&s, input.did.as_deref(), input.since).await?;
    let (mut recovered, mut current, mut failed) = (Vec::new(), 0u64, Vec::new());
    for (did, evs) in &events {
        if s.store.account_by_did(did).await?.is_none() {
            s.store
                .record_recovery(did, Some("no account here"))
                .await?;
            failed.push(json!({"did": did, "error": "no account here"}));
            continue;
        }
        match recover_one(&s, did, evs).await {
            Ok(true) => {
                reindex(&s, did).await?;
                s.store.record_recovery(did, None).await?;
                recovered.push(did.clone());
            }
            Ok(false) => current += 1,
            Err(e) => {
                s.store.record_recovery(did, Some(&e)).await?;
                failed.push(json!({"did": did, "error": e}));
            }
        }
    }
    let out = json!({
        "accounts": events.len(),
        "recovered": recovered,
        "upToDate": current,
        "failed": failed,
    });
    admin_auth::audit(
        &s,
        &who,
        "repo.recoverFromSequencer",
        None,
        Some(out.clone()),
    )
    .await;
    Ok(Json(out))
}

/// `admin.repo.repairFromSequencer` (`repair repos --from-sequencer`), as
/// the reference's `recovery-repair-repos`: for each account the last
/// recovery couldn't recover, the event log's record changes are applied
/// to the record index, then the repository is rebuilt from it.
pub async fn repair_from_sequencer(State(s): St, headers: HeaderMap) -> XrpcResult<Json<Value>> {
    let who = admin(&s, &headers, locus_nsid!("admin.repo.repairFromSequencer")).await?;
    let mut fixed = Vec::new();
    let mut failed = Vec::new();
    for r in s.store.recoveries().await?.into_iter().filter(|r| !r.fixed) {
        let did = r.did.clone();
        if s.store.account_by_did(&did).await?.is_none() {
            failed.push(json!({"did": did, "error": "no account here"}));
            continue;
        }
        let result: XrpcResult<()> = async {
            let events = commit_events(&s, Some(&did), 0).await?;
            let evs = events.get(&did).map(Vec::as_slice).unwrap_or_default();
            let (lock, mut tx) = s.repo.locked_tx(&did).await.map_err(internal)?;
            for ev in evs {
                for (cid, bytes) in &ev.blocks {
                    tx.put_unreferenced_block(&did, &cid.to_string_base32(), &ev.rev, bytes)
                        .await?;
                }
                for (action, path, cid) in &ev.ops {
                    let (collection, rkey) = split_tree_key(path).map_err(internal)?;
                    match (action.as_str(), cid) {
                        ("create" | "update", Some(cid)) => {
                            let Some(bytes) = tx.any_repo_block(&did, cid).await? else {
                                continue;
                            };
                            let Ok(value) = decode_record(&collection, &rkey, &bytes) else {
                                continue;
                            };
                            tx.apply_record_changes(
                                &did,
                                &ev.rev,
                                &[RecordChange::Put {
                                    collection: collection.clone(),
                                    rkey: rkey.clone(),
                                    cid: cid.clone(),
                                }],
                            )
                            .await?;
                            tx.replace_record_blobs(
                                &did,
                                &collection,
                                &rkey,
                                &import_blob_cids(&value),
                            )
                            .await?;
                            tx.replace_backlinks(
                                &did,
                                &collection,
                                &rkey,
                                &locus_repo::records::backlinks(&collection, &value),
                            )
                            .await?;
                        }
                        ("delete", _) => {
                            tx.apply_record_changes(
                                &did,
                                &ev.rev,
                                &[RecordChange::Delete {
                                    collection: collection.clone(),
                                    rkey: rkey.clone(),
                                }],
                            )
                            .await?;
                            tx.replace_record_blobs(&did, &collection, &rkey, &[])
                                .await?;
                            tx.replace_backlinks(&did, &collection, &rkey, &[]).await?;
                        }
                        _ => {}
                    }
                }
            }
            tx.commit().await?;
            // The rebuild takes the account's lock itself.
            drop(lock);
            rebuild_repo(&s, &did).await?;
            Ok(())
        }
        .await;
        match result {
            Ok(()) => {
                s.store.record_recovery(&did, None).await?;
                fixed.push(did);
            }
            Err(e) => failed.push(json!({"did": did, "error": e.message})),
        }
    }
    let out = json!({"fixed": fixed, "failed": failed});
    admin_auth::audit(
        &s,
        &who,
        "repo.repairFromSequencer",
        None,
        Some(out.clone()),
    )
    .await;
    Ok(Json(out))
}

/// The accounts the recoveries touched (for `keys rotate-account
/// --recovery`).
pub async fn recovered_dids(s: &AppState) -> XrpcResult<Vec<String>> {
    Ok(s.store
        .recoveries()
        .await?
        .into_iter()
        .filter(|r| r.fixed)
        .map(|r| r.did)
        .collect())
}

/// The repair routes.
pub fn routes() -> axum::Router<Arc<AppState>> {
    use axum::routing::post;
    macro_rules! at {
        ($name:literal) => {
            concat!("/xrpc/", locus_nsid!($name))
        };
    }
    axum::Router::new()
        .route(at!("admin.repo.rebuild"), post(rebuild))
        .route(at!("admin.repo.repair"), post(repair_repo))
        .route(at!("admin.identity.repair"), post(repair_identity))
        .route(at!("admin.identity.tombstone"), post(tombstone_identity))
        .route(
            at!("admin.repo.recoverFromSequencer"),
            post(recover_from_sequencer),
        )
        .route(
            at!("admin.repo.repairFromSequencer"),
            post(repair_from_sequencer),
        )
}
