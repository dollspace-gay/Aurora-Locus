//! The commit pipeline every record write goes through.
//!
//! One commit is one database transaction covering the MST blocks, the
//! repository root, the record index and the `#commit` outbox event. Writers
//! of one repository are serialized (an in-process lock per DID, plus the
//! database's repo lock). MST nodes are loaded lazily from the repository's
//! stored blocks inside the transaction, so a write reads only the paths it
//! changes.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use locus_core::CoreError;
use locus_store::repo::{BlockDelta, RecordChange, RepoRoot};
use locus_store::{Store, Tx};
use proto_blue_crypto::Signer;
use proto_blue_lex_data::{Cid, LexValue};
use proto_blue_repo::block_map::BlockMap;
use proto_blue_repo::commit::{UnsignedCommit, sign_commit};
use proto_blue_repo::error::RepoError;
use proto_blue_repo::mst::{Mst, MstOp};
use thiserror::Error;

use crate::event::{CommitEvent, EventOp, MAX_EVENT_BLOCKS_BYTES};
use crate::records::BlobUse;
use crate::tid::TidClock;
pub use locus_store::blob::PendingMove;

#[derive(Debug, Error)]
pub enum CommitError {
    /// `swapCommit` or `swapRecord` didn't match.
    #[error("{0}")]
    InvalidSwap(String),
    /// A write conflicts with the record index (create on an existing path,
    /// update/delete of a missing one) or the event would be too large.
    #[error("{0}")]
    InvalidRequest(String),
    #[error("repository not found: {0}")]
    RepoNotFound(String),
    /// Nothing to commit: every write was a no-op (a put of the record the
    /// path already holds, a delete of a missing record).
    #[error("no changes")]
    Unchanged,
    /// A bulk job (an import or deletion) holds the account's writes; retry.
    #[error("the repository's writes are blocked while another operation runs")]
    WriteBlocked,
    /// The tree didn't come out as the ops say it should. Nothing was written.
    #[error("invalid MST: {0}")]
    InvalidMst(String),
    /// A referenced blob is missing or doesn't match (`BlobNotFound`,
    /// `InvalidMimeType`, `InvalidSize`, the reference's names).
    #[error("{message}")]
    Blob { name: &'static str, message: String },
    #[error(transparent)]
    Core(#[from] CoreError),
}

impl From<RepoError> for CommitError {
    fn from(e: RepoError) -> Self {
        Self::InvalidMst(e.to_string())
    }
}

/// What a write does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Create,
    Update,
    Delete,
    /// Create, or update if the path holds a record (`putRecord`); a no-op
    /// if it holds this exact record.
    Put,
    /// Delete if the path holds a record (`deleteRecord`), else a no-op.
    DeleteIfPresent,
}

/// A validated, encoded write.
#[derive(Debug, Clone)]
pub struct PreparedWrite {
    pub action: Action,
    pub collection: String,
    pub rkey: String,
    /// The record's CID and DAG-CBOR bytes (create and update).
    pub record: Option<(Cid, Vec<u8>)>,
    /// `swapRecord`: None = no check; Some(None) = the path must be empty;
    /// Some(Some(cid)) = the path must hold `cid`.
    pub swap_record: Option<Option<Cid>>,
    /// Blobs the record references (create and update).
    pub blobs: Vec<BlobUse>,
    /// What the record points at, `(path, target)` (create and update).
    pub backlinks: Vec<(String, String)>,
}

impl PreparedWrite {
    fn path(&self) -> String {
        format!("{}/{}", self.collection, self.rkey)
    }
}

/// A commit request.
pub struct CommitRequest<'a> {
    pub did: &'a str,
    pub writes: Vec<PreparedWrite>,
    /// `swapCommit`: the commit the repository must be at.
    pub swap_commit: Option<Cid>,
    pub signer: &'a dyn Signer,
    /// The reference's `createRecord` cleanup: delete the account's other
    /// follows, blocks, likes and reposts of the same subject in the same
    /// commit.
    pub dedupe_backlinks: bool,
}

/// The collections whose duplicates `createRecord` removes.
const DEDUPED: &[&str] = &[
    "app.bsky.graph.follow",
    "app.bsky.graph.block",
    "app.bsky.feed.like",
    "app.bsky.feed.repost",
];

/// What a commit did.
#[derive(Debug, Clone)]
pub struct CommitOutcome {
    pub commit_cid: Cid,
    pub rev: String,
    pub outbox_id: i64,
    /// Blob copies promoted by this commit, to finish after it.
    pub moves: Vec<PendingMove>,
}

/// A newly created repository.
#[derive(Debug, Clone)]
pub struct CreatedRepo {
    pub commit_cid: Cid,
    pub commit_bytes: Vec<u8>,
    pub rev: String,
    /// The `#commit` outbox row, when events were emitted.
    pub commit_outbox_id: Option<i64>,
}

/// The repository engine shared by every write path.
pub struct RepoEngine {
    store: Store,
    tids: Arc<TidClock>,
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

fn cid_str(c: &Cid) -> String {
    c.to_string_base32()
}

fn parse_cid(s: &str) -> Result<Cid, CommitError> {
    Cid::from_str_multibase(s)
        .map_err(|e| CoreError::Storage(format!("stored CID {s:?} is invalid: {e}")).into())
}

/// Runs a pure MST step, fetching blocks it asks for from the repository's
/// stored blocks (inside the transaction) and retrying.
async fn with_blocks<T>(
    tx: &mut Tx,
    did: &str,
    cache: &mut BlockMap,
    mut step: impl FnMut(&BlockMap) -> Result<T, RepoError>,
) -> Result<T, CommitError> {
    loop {
        match step(cache) {
            Ok(v) => return Ok(v),
            Err(RepoError::MissingBlock(cid)) => {
                let bytes = tx.repo_block(did, &cid_str(&cid)).await?.ok_or_else(|| {
                    CommitError::InvalidMst(format!("block {cid} of {did} is not stored"))
                })?;
                cache.set(cid, bytes);
            }
            Err(e) => return Err(e.into()),
        }
    }
}

impl RepoEngine {
    pub fn new(store: Store, tids: Arc<TidClock>) -> Self {
        Self {
            store,
            tids,
            locks: Mutex::new(HashMap::new()),
        }
    }

    pub fn tids(&self) -> &TidClock {
        &self.tids
    }

    fn repo_lock(&self, did: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self
            .locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(locks.entry(did.to_owned()).or_default())
    }

    /// A transaction holding `did`'s repo lock (in this process and in the
    /// database), for writing events that don't touch the repo, such as
    /// `#identity` and `#account`, in order with the DID's commits. The lock
    /// is released when the guard drops; commit the transaction first.
    pub async fn locked_tx(
        &self,
        did: &str,
    ) -> Result<(tokio::sync::OwnedMutexGuard<()>, Tx), CommitError> {
        let guard = self.repo_lock(did).lock_owned().await;
        let mut tx = self.store.begin().await?;
        tx.lock_repo(did).await?;
        Ok((guard, tx))
    }

    /// Creates an account's empty repository in its own transaction, with
    /// its `#commit` event.
    pub async fn create_repo(
        &self,
        did: &str,
        signer: &dyn Signer,
    ) -> Result<CommitOutcome, CommitError> {
        let lock = self.repo_lock(did);
        let _held = lock.lock().await;
        let mut tx = self.store.begin().await?;
        let created = self.create_repo_in(&mut tx, did, signer, true).await?;
        tx.commit().await?;
        Ok(CommitOutcome {
            commit_cid: created.commit_cid,
            rev: created.rev,
            outbox_id: created.commit_outbox_id.unwrap_or_default(),
            moves: Vec::new(),
        })
    }

    /// Creates an account's empty repository (an empty MST and a first
    /// signed commit) inside the caller's transaction, so account creation
    /// writes everything at once. With `emit`, the `#commit` event is written
    /// too; an account migrating in emits nothing until it's activated.
    pub async fn create_repo_in(
        &self,
        tx: &mut Tx,
        did: &str,
        signer: &dyn Signer,
        emit: bool,
    ) -> Result<CreatedRepo, CommitError> {
        tx.lock_repo(did).await?;
        if tx.repo_root(did).await?.is_some() {
            return Err(CommitError::InvalidRequest(format!(
                "{did} already has a repository"
            )));
        }
        let tree = Mst::empty();
        let diff = Mst::diff(&Mst::empty(), &tree, &BlockMap::new())?;
        let data = tree.root_cid()?;
        let rev = self.tids.next();
        let commit = sign_commit(
            &UnsignedCommit::new(did.to_owned(), data.clone(), rev.clone(), None),
            signer,
        )?;
        let commit_bytes = commit.to_cbor()?;
        let commit_cid = commit.cid()?;

        // The empty tree's one node is new on a brand-new repository.
        let mut root_block = diff.added;
        if !root_block.has(&data) {
            let (cid, blocks) = proto_blue_repo::mst::MstNode::empty().get_all_blocks()?;
            root_block.add_map(&blocks);
            debug_assert_eq!(cid, data);
        }
        let mut deltas: Vec<BlockDelta> = root_block
            .iter()
            .map(|(cid, bytes)| BlockDelta {
                cid: cid_str(cid),
                bytes: Some(bytes.to_vec()),
                delta: 1,
            })
            .collect();
        deltas.push(BlockDelta {
            cid: cid_str(&commit_cid),
            bytes: Some(commit_bytes.clone()),
            delta: 1,
        });

        let mut event_blocks = root_block;
        event_blocks.set(commit_cid.clone(), commit_bytes.clone());
        let event = CommitEvent {
            repo: did.to_owned(),
            commit: commit_cid.clone(),
            rev: rev.clone(),
            since: None,
            prev_data: None,
            ops: Vec::new(),
            blocks: event_blocks,
        };
        let payload = event.encode()?;

        tx.apply_block_deltas(did, &rev, &deltas).await?;
        tx.set_repo_root(
            did,
            &RepoRoot {
                commit_cid: cid_str(&commit_cid),
                rev: rev.clone(),
                data_cid: cid_str(&data),
            },
        )
        .await?;
        let commit_outbox_id = if emit {
            Some(tx.insert_outbox(did, "commit", &payload).await?)
        } else {
            None
        };
        Ok(CreatedRepo {
            commit_cid,
            commit_bytes,
            rev,
            commit_outbox_id,
        })
    }

    /// Applies `req.writes` as one signed commit.
    ///
    /// Writes are checked in order against the record index with this
    /// batch's earlier writes applied, so a batch that names one path twice
    /// (create, then delete) behaves as the reference does. Swap checks fail
    /// with [`CommitError::InvalidSwap`]; a create of an existing path, or an
    /// update or delete of a missing one, fails with
    /// [`CommitError::InvalidRequest`]. As in the reference, a batch whose net
    /// effect on the tree is nothing still makes a commit with a new rev
    /// (handlers skip the no-op cases the reference skips, like a put of an
    /// identical record).
    pub async fn commit(&self, req: CommitRequest<'_>) -> Result<CommitOutcome, CommitError> {
        let did = req.did;
        let lock = self.repo_lock(did);
        let _held = lock.lock().await;
        let mut tx = self.store.begin().await?;
        tx.lock_repo(did).await?;
        if tx.write_blocked(did).await? {
            return Err(CommitError::WriteBlocked);
        }
        let root = tx
            .repo_root(did)
            .await?
            .ok_or_else(|| CommitError::RepoNotFound(did.to_owned()))?;
        let commit_cid_before = parse_cid(&root.commit_cid)?;

        if let Some(expected) = &req.swap_commit
            && *expected != commit_cid_before
        {
            return Err(CommitError::InvalidSwap(format!(
                "Commit was at {}",
                root.commit_cid
            )));
        }

        // The batch, with the duplicates createRecord removes put first.
        let mut writes: Vec<PreparedWrite> = Vec::with_capacity(req.writes.len());
        if req.dedupe_backlinks {
            for w in &req.writes {
                if w.action != Action::Create || !DEDUPED.contains(&w.collection.as_str()) {
                    continue;
                }
                for (path, target) in &w.backlinks {
                    for rkey in tx
                        .backlinked_records(did, &w.collection, path, target)
                        .await?
                    {
                        if rkey != w.rkey {
                            writes.push(PreparedWrite {
                                action: Action::DeleteIfPresent,
                                collection: w.collection.clone(),
                                rkey,
                                record: None,
                                swap_record: None,
                                blobs: Vec::new(),
                                backlinks: Vec::new(),
                            });
                        }
                    }
                }
            }
        }
        writes.extend(req.writes.iter().cloned());

        // Current CIDs of every touched path, before the batch.
        let mut paths: Vec<(String, String)> = writes
            .iter()
            .map(|w| (w.collection.clone(), w.rkey.clone()))
            .collect();
        paths.sort();
        paths.dedup();
        let initial: HashMap<(String, String), String> = tx.record_cids(did, &paths).await?;

        // Check each write in order against the overlay.
        let mut overlay: BTreeMap<(String, String), Option<Cid>> = BTreeMap::new();
        for p in &paths {
            let cid = initial.get(p).map(|s| parse_cid(s)).transpose()?;
            overlay.insert(p.clone(), cid);
        }
        let mut event_ops = Vec::with_capacity(writes.len());
        let mut record_deltas: Vec<BlockDelta> = Vec::new();
        for w in &writes {
            let key = (w.collection.clone(), w.rkey.clone());
            let current = overlay.get(&key).cloned().flatten();
            if let Some(swap) = &w.swap_record {
                let ok = match (swap, &current) {
                    (None, None) => true,
                    (Some(expected), Some(cur)) => expected == cur,
                    _ => false,
                };
                if !ok {
                    let at = current.as_ref().map_or_else(|| "null".to_owned(), cid_str);
                    return Err(CommitError::InvalidSwap(format!("Record was at {at}")));
                }
            }
            // Put and DeleteIfPresent resolve against the overlay.
            let action = match w.action {
                Action::Put if current.is_none() => Action::Create,
                Action::Put if current == w.record.as_ref().map(|(c, _)| c.clone()) => continue,
                Action::Put => Action::Update,
                Action::DeleteIfPresent if current.is_none() => continue,
                Action::DeleteIfPresent => Action::Delete,
                a => a,
            };
            let new = match action {
                Action::Put | Action::DeleteIfPresent => unreachable!("resolved above"),
                Action::Create => {
                    if current.is_some() {
                        return Err(CommitError::InvalidRequest(format!(
                            "Record already exists at {}",
                            w.path()
                        )));
                    }
                    let (cid, bytes) = w.record.clone().ok_or_else(|| {
                        CommitError::InvalidRequest(format!("create of {} has no record", w.path()))
                    })?;
                    record_deltas.push(BlockDelta {
                        cid: cid_str(&cid),
                        bytes: Some(bytes),
                        delta: 1,
                    });
                    Some(cid)
                }
                Action::Update => {
                    let prev = current.clone().ok_or_else(|| {
                        CommitError::InvalidRequest(format!(
                            "Could not find record to update at {}",
                            w.path()
                        ))
                    })?;
                    let (cid, bytes) = w.record.clone().ok_or_else(|| {
                        CommitError::InvalidRequest(format!("update of {} has no record", w.path()))
                    })?;
                    record_deltas.push(BlockDelta {
                        cid: cid_str(&cid),
                        bytes: Some(bytes),
                        delta: 1,
                    });
                    record_deltas.push(BlockDelta {
                        cid: cid_str(&prev),
                        bytes: None,
                        delta: -1,
                    });
                    Some(cid)
                }
                Action::Delete => {
                    let prev = current.clone().ok_or_else(|| {
                        CommitError::InvalidRequest(format!(
                            "Could not find record to delete at {}",
                            w.path()
                        ))
                    })?;
                    record_deltas.push(BlockDelta {
                        cid: cid_str(&prev),
                        bytes: None,
                        delta: -1,
                    });
                    None
                }
            };
            event_ops.push(EventOp {
                action: match action {
                    Action::Create => "create",
                    Action::Update => "update",
                    _ => "delete",
                },
                path: w.path(),
                cid: new.clone(),
                prev: current,
            });
            overlay.insert(key, new);
        }

        // Net MST ops: compare each path's final value with its initial one.
        let mut mst_ops = Vec::new();
        let mut record_changes = Vec::new();
        let mut expected: Vec<(String, Option<Cid>, Option<Cid>)> = Vec::new();
        for (key, fin) in &overlay {
            let before = initial.get(key).map(|s| parse_cid(s)).transpose()?;
            if before == *fin {
                continue;
            }
            let path = format!("{}/{}", key.0, key.1);
            match (&before, fin) {
                (None, Some(v)) => mst_ops.push(MstOp::Add {
                    key: path.clone(),
                    value: v.clone(),
                }),
                (Some(_), Some(v)) => mst_ops.push(MstOp::Update {
                    key: path.clone(),
                    value: v.clone(),
                }),
                (Some(_), None) => mst_ops.push(MstOp::Delete { key: path.clone() }),
                (None, None) => unreachable!(),
            }
            record_changes.push(match fin {
                Some(v) => RecordChange::Put {
                    collection: key.0.clone(),
                    rkey: key.1.clone(),
                    cid: cid_str(v),
                },
                None => RecordChange::Delete {
                    collection: key.0.clone(),
                    rkey: key.1.clone(),
                },
            });
            expected.push((path, before, fin.clone()));
        }
        if writes.is_empty() {
            return Err(CommitError::InvalidRequest("no writes".into()));
        }
        if event_ops.is_empty() {
            return Err(CommitError::Unchanged);
        }

        // MST update, diff and proofs, reading only the paths they need.
        let old_data = parse_cid(&root.data_cid)?;
        let old_tree = Mst::load(old_data.clone());
        let mut cache = BlockMap::new();
        let new_tree = with_blocks(&mut tx, did, &mut cache, |src| {
            old_tree.apply(&mst_ops, src)
        })
        .await?;
        let new_data = new_tree.root_cid()?;
        let diff = with_blocks(&mut tx, did, &mut cache, |src| {
            Mst::diff(&old_tree, &new_tree, src)
        })
        .await?;
        let mut proofs = BlockMap::new();
        for (collection, rkey) in &paths {
            // Proof blocks come from the new tree: in memory, or stored and
            // unchanged (fetched like any other block).
            let key = format!("{collection}/{rkey}");
            let p = with_blocks(&mut tx, did, &mut cache, |s| {
                new_tree.covering_proof(&key, s)
            })
            .await?;
            proofs.add_map(&p);
        }

        verify_diff(&expected, &diff.added, &diff.removed, &cache)?;

        // The commit.
        let rev = self.tids.next_after(Some(&root.rev));
        let commit = sign_commit(
            &UnsignedCommit::new(did.to_owned(), new_data.clone(), rev.clone(), None),
            req.signer,
        )?;
        let commit_bytes = commit.to_cbor()?;
        let commit_cid = commit.cid()?;

        // The event, checked for size before anything is written.
        let mut event_blocks = BlockMap::new();
        event_blocks.set(commit_cid.clone(), commit_bytes.clone());
        event_blocks.add_map(&diff.added);
        for w in &writes {
            if let Some((cid, bytes)) = &w.record {
                event_blocks.set(cid.clone(), bytes.clone());
            }
        }
        event_blocks.add_map(&proofs);
        if event_blocks.byte_size() > MAX_EVENT_BLOCKS_BYTES {
            return Err(CommitError::InvalidRequest(
                "Too many writes. Max event size: 2MB".into(),
            ));
        }
        let event = CommitEvent {
            repo: did.to_owned(),
            commit: commit_cid.clone(),
            rev: rev.clone(),
            since: Some(root.rev.clone()),
            prev_data: Some(old_data),
            ops: event_ops,
            blocks: event_blocks,
        };
        let payload = event.encode()?;

        // Reference counts: MST nodes from the diff, records per op, commits.
        let mut deltas: Vec<BlockDelta> = Vec::new();
        for (cid, bytes) in diff.added.iter() {
            deltas.push(BlockDelta {
                cid: cid_str(cid),
                bytes: Some(bytes.to_vec()),
                delta: 1,
            });
        }
        for cid in diff.removed.iter() {
            deltas.push(BlockDelta {
                cid: cid_str(cid),
                bytes: None,
                delta: -1,
            });
        }
        deltas.extend(record_deltas);
        deltas.push(BlockDelta {
            cid: cid_str(&commit_cid),
            bytes: Some(commit_bytes),
            delta: 1,
        });
        deltas.push(BlockDelta {
            cid: root.commit_cid.clone(),
            bytes: None,
            delta: -1,
        });

        tx.apply_block_deltas(did, &rev, &net_deltas(deltas))
            .await?;
        tx.apply_record_changes(did, &rev, &record_changes).await?;
        let moves = apply_references(&mut tx, did, &writes, &paths).await?;
        tx.set_repo_root(
            did,
            &RepoRoot {
                commit_cid: cid_str(&commit_cid),
                rev: rev.clone(),
                data_cid: cid_str(&new_data),
            },
        )
        .await?;
        let outbox_id = tx.insert_outbox(did, "commit", &payload).await?;
        tx.commit().await?;
        Ok(CommitOutcome {
            commit_cid,
            rev,
            outbox_id,
            moves,
        })
    }
}

fn blob_error(name: &'static str, message: String) -> CommitError {
    CommitError::Blob { name, message }
}

/// Blob references and backlinks for every path the batch touched, under
/// the repo lock (in the commit's transaction).
///
/// Every blob any write names is checked: the row must exist and be
/// referenceable, with the MIME type and size the record says. Blobs the
/// paths reference after the batch take a reference (a row update, so GC's
/// conditional delete can't win a race), and a temp blob is promoted with a
/// pending move to a permanent key. References net across the batch: a
/// blob referenced and dropped within it takes no reference. A blob whose
/// last reference went is marked unreferenced.
async fn apply_references(
    tx: &mut Tx,
    did: &str,
    writes: &[PreparedWrite],
    paths: &[(String, String)],
) -> Result<Vec<PendingMove>, CommitError> {
    // Each path's final write (None: deleted).
    let mut last: BTreeMap<(String, String), Option<&PreparedWrite>> = BTreeMap::new();
    for w in writes {
        last.insert(
            (w.collection.clone(), w.rkey.clone()),
            (w.action != Action::Delete).then_some(w),
        );
    }
    let old = tx.record_blob_cids(did, paths).await?;
    let named: BTreeMap<String, Vec<&BlobUse>> =
        writes
            .iter()
            .flat_map(|w| &w.blobs)
            .fold(BTreeMap::new(), |mut m, b| {
                m.entry(cid_str(&b.cid)).or_default().push(b);
                m
            });
    let mut moves = Vec::new();
    if !named.is_empty() || !old.is_empty() {
        let backend = tx.blob_backend().await?;
        let referenced: BTreeSet<String> = last
            .values()
            .flatten()
            .flat_map(|w| w.blobs.iter().map(|b| cid_str(&b.cid)))
            .collect();
        // In (did, cid) order: BTreeMap keys are sorted.
        for (cid, uses) in &named {
            let row = if referenced.contains(cid) {
                let key = locus_store::blob::permanent_key(did, cid);
                let row = tx.reference_blob(did, cid, &key, backend.epoch).await?;
                if let Some(r) = &row
                    && r.pending_move.as_deref() == Some(key.as_str())
                {
                    moves.push(PendingMove {
                        did: did.to_owned(),
                        cid: cid.clone(),
                        from: r.storage_key.clone(),
                        to: key,
                    });
                }
                row
            } else {
                tx.blob_for_update(did, cid).await?.filter(|r| {
                    r.takedown_ref.is_none()
                        && (r.state == "temp" || r.state == "permanent")
                        && !r.review_hold
                })
            };
            let row = row
                .ok_or_else(|| blob_error("BlobNotFound", format!("Could not find blob: {cid}")))?;
            for b in uses {
                if b.mime_type != row.mime_type {
                    return Err(blob_error(
                        "InvalidMimeType",
                        format!(
                            "Referenced Mimetype does not match stored blob. Expected: {}, Got: {}",
                            row.mime_type, b.mime_type
                        ),
                    ));
                }
                if b.size != row.size {
                    return Err(blob_error(
                        "InvalidSize",
                        format!(
                            "Referenced Size does not match stored blob. Expected: {}, Got: {}",
                            row.size, b.size
                        ),
                    ));
                }
            }
        }
        for (path, w) in &last {
            let cids: Vec<String> = w
                .map(|w| w.blobs.iter().map(|b| cid_str(&b.cid)).collect())
                .unwrap_or_default();
            let had = old.get(path).cloned().unwrap_or_default();
            if cids != had {
                tx.replace_record_blobs(did, &path.0, &path.1, &cids)
                    .await?;
            }
        }
        let dropped: BTreeSet<&String> = old
            .values()
            .flatten()
            .filter(|c| !referenced.contains(*c))
            .collect();
        for cid in dropped {
            tx.orphan_blob_if_unreferenced(did, cid).await?;
        }
    }
    for (path, w) in &last {
        let links = w.map(|w| w.backlinks.as_slice()).unwrap_or_default();
        tx.replace_backlinks(did, &path.0, &path.1, links).await?;
    }
    Ok(moves)
}

/// Sums deltas per block, keeping bytes from any increment, so a block that
/// gains and loses a reference in one batch nets out and its row isn't touched.
fn net_deltas(deltas: Vec<BlockDelta>) -> Vec<BlockDelta> {
    let mut by_cid: BTreeMap<String, BlockDelta> = BTreeMap::new();
    for d in deltas {
        let e = by_cid.entry(d.cid.clone()).or_insert(BlockDelta {
            cid: d.cid.clone(),
            bytes: None,
            delta: 0,
        });
        e.delta += d.delta;
        if e.bytes.is_none() {
            e.bytes = d.bytes;
        }
    }
    by_cid.into_values().filter(|d| d.delta != 0).collect()
}

/// Checks, from the changed nodes only, that the tree changed exactly as the
/// ops say: the leaves in added nodes minus the leaves in removed nodes are
/// the new values, and the reverse are the old values. A tree that silently
/// dropped or duplicated a record fails here, before anything is written.
fn verify_diff(
    expected: &[(String, Option<Cid>, Option<Cid>)],
    added: &BlockMap,
    removed: &proto_blue_repo::cid_set::CidSet,
    cache: &BlockMap,
) -> Result<(), CommitError> {
    let mut gained: BTreeMap<(String, Cid), i64> = BTreeMap::new();
    for (_, bytes) in added.iter() {
        for leaf in node_leaves(bytes)? {
            *gained.entry(leaf).or_default() += 1;
        }
    }
    for cid in removed.iter() {
        let bytes = cache
            .get(cid)
            .ok_or_else(|| CommitError::InvalidMst(format!("removed node {cid} wasn't read")))?;
        for leaf in node_leaves(bytes)? {
            *gained.entry(leaf).or_default() -= 1;
        }
    }
    gained.retain(|_, n| *n != 0);

    let mut want: BTreeMap<(String, Cid), i64> = BTreeMap::new();
    for (path, before, after) in expected {
        if let Some(b) = before {
            *want.entry((path.clone(), b.clone())).or_default() -= 1;
        }
        if let Some(a) = after {
            *want.entry((path.clone(), a.clone())).or_default() += 1;
        }
    }
    want.retain(|_, n| *n != 0);
    if gained != want {
        return Err(CommitError::InvalidMst(
            "the new tree's leaves don't match the writes".into(),
        ));
    }
    Ok(())
}

fn node_leaves(bytes: &[u8]) -> Result<Vec<(String, Cid)>, CommitError> {
    let value: LexValue = proto_blue_lex_cbor::decode(bytes).map_err(RepoError::from)?;
    let data = proto_blue_repo::mst::deserialize_node_data(&value)?;
    let keys = proto_blue_repo::mst::entries_to_keys(&data);
    Ok(keys
        .into_iter()
        .zip(data.entries.into_iter().map(|e| e.value))
        .collect())
}
