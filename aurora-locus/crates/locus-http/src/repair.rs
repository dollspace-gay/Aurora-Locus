//! Operator repair tools: `repo rebuild` (the reference's `rebuild-repo`: a
//! new tree and commit from the record index), `repair repo` (the record
//! index from the tree) and `repair identity` (the PLC document back to
//! what this PDS holds, then `#identity`).

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
    let did = input.did.as_str();
    let account = s
        .store
        .account_by_did(did)
        .await?
        .ok_or_else(|| XrpcError::invalid_request("Account not found"))?;
    let root = s
        .store
        .repo_root_of(did)
        .await?
        .ok_or_else(|| XrpcError::invalid_request("The account has no repository"))?;
    let index = record_index(&s, did).await?;
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
    let _ = account;
    admin_auth::audit(
        &s,
        &who,
        "repo.rebuild",
        Some(did),
        Some(json!({"records": record_count, "missing": missing})),
    )
    .await;
    Ok(Json(json!({
        "did": did,
        "records": record_count,
        "treeNodes": nodes.iter().count(),
        "blocksRecounted": dropped,
        "missingRecords": missing,
    })))
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
    let did = input.did.as_str();
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
    let index = record_index(&s, did).await?;
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
    admin_auth::audit(
        &s,
        &who,
        "repo.repair",
        Some(did),
        Some(json!({"added": added, "updated": updated, "removed": removed})),
    )
    .await;
    Ok(Json(json!({
        "did": did,
        "records": tree.len(),
        "added": added,
        "updated": updated,
        "removed": removed,
    })))
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
}
