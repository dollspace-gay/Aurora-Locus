//! Repository import (`com.atproto.repo.importRepo`) as a bulk job.
//!
//! The upload is applied as a diff onto the account's repository, in
//! phases that each resume from staged rows:
//!
//! 1. `staging`: the CAR is streamed into `import_block`, the new tree is
//!    walked with a bounded walker (visited set, depth, block count, key
//!    order and layers), the old tree's changed part is walked too, and the
//!    record diff, per-block count deltas and blob CIDs are staged.
//! 2. `increments`: positive deltas and blob promotions only, so the old
//!    repository stays fully readable.
//! 3. swap: one transaction re-signs a commit over the imported data root,
//!    moves `repo_root`, writes `#sync` and moves to `reindex`. Before it,
//!    any failure rolls back; after it, the job only goes forward.
//! 4. `reindex`: the record index follows the new root.
//! 5. `decrements`: negative deltas and blobs left unreferenced.
//!
//! Subtrees the new tree shares with the current one (same node CID, so
//! the same contents) are skipped on both walks: they change no counts
//! and no records.

use std::collections::{HashSet, VecDeque};
use std::path::Path;

use locus_core::CoreError;
use locus_store::Tx;
use locus_store::blob::PendingMove;
use locus_store::bulk::BulkJob;
use locus_store::import::{BlockBatch, StagedBlock};
use locus_store::repo::{BlockDelta, RecordChange, RepoRoot};
use proto_blue_crypto::Signer;
use proto_blue_lex_data::{Cid, LexValue};
use proto_blue_repo::commit::{SignedCommit, UnsignedCommit, sign_commit, verify_commit_sig};
use proto_blue_repo::mst::{deserialize_node_data, is_valid_mst_key, leading_zeros_on_hash};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, BufReader};

use crate::engine::RepoEngine;
use crate::event::sync_body;
use crate::records::backlinks;

/// Largest block an import accepts.
pub const MAX_BLOCK_BYTES: u64 = 2_000_000;
/// Most blocks an import accepts (CAR blocks, and nodes walked).
pub const MAX_BLOCKS: u64 = 5_000_000;
/// Deepest tree an import accepts (a SHA-256 key hash has at most 128 layers).
const MAX_LAYER: usize = 128;
const HEADER_MAX: u64 = 1_000_000;
/// Staged rows written per transaction.
const FLUSH: usize = 500;
/// Rows applied per batch in the later phases.
const BATCH: i64 = 500;

#[derive(Debug, Error)]
pub enum ImportError {
    /// The upload is refused (400).
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Core(#[from] CoreError),
}

impl From<crate::CommitError> for ImportError {
    fn from(e: crate::CommitError) -> Self {
        match e {
            crate::CommitError::Core(c) => Self::Core(c),
            other => Self::Core(CoreError::Storage(other.to_string())),
        }
    }
}

fn invalid(msg: impl Into<String>) -> ImportError {
    ImportError::Invalid(msg.into())
}

fn cid_str(c: &Cid) -> String {
    c.to_string_base32()
}

fn parse_cid(s: &str) -> Result<Cid, ImportError> {
    Cid::from_str_multibase(s)
        .map_err(|e| CoreError::Storage(format!("stored CID {s:?} is invalid: {e}")).into())
}

// ---------------------------------------------------------------- CAR

/// A streaming CAR v1 reader with per-block and block-count limits. Every
/// block's CID is checked against its bytes.
pub struct CarReader<R> {
    r: BufReader<R>,
    blocks: u64,
}

impl<R: AsyncRead + Unpin> CarReader<R> {
    /// Reads the header. Returns the reader and the roots.
    pub async fn open(r: R) -> Result<(Self, Vec<Cid>), ImportError> {
        let mut me = Self {
            r: BufReader::with_capacity(64 * 1024, r),
            blocks: 0,
        };
        let len = me
            .varint()
            .await?
            .ok_or_else(|| invalid("Could not read CAR header"))?;
        if len == 0 || len > HEADER_MAX {
            return Err(invalid("Invalid CAR header"));
        }
        let bytes = me.exact(len).await?;
        let header: LexValue =
            proto_blue_lex_cbor::decode(&bytes).map_err(|_| invalid("Invalid CAR header"))?;
        let map = header
            .as_map()
            .ok_or_else(|| invalid("Invalid CAR header"))?;
        if map.get("version").and_then(LexValue::as_integer) != Some(1) {
            return Err(invalid("Unsupported CAR version"));
        }
        let roots = map
            .get("roots")
            .and_then(LexValue::as_array)
            .ok_or_else(|| invalid("Invalid CAR header"))?
            .iter()
            .map(|v| {
                v.as_cid()
                    .cloned()
                    .ok_or_else(|| invalid("Invalid CAR root"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok((me, roots))
    }

    async fn byte(&mut self) -> Result<Option<u8>, ImportError> {
        match self.r.read_u8().await {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(None),
            Err(e) => Err(CoreError::Storage(format!("read the upload: {e}")).into()),
        }
    }

    /// A varint; None at a clean end of input.
    async fn varint(&mut self) -> Result<Option<u64>, ImportError> {
        let mut value: u64 = 0;
        for i in 0..10 {
            let Some(b) = self.byte().await? else {
                return if i == 0 {
                    Ok(None)
                } else {
                    Err(invalid("Truncated CAR"))
                };
            };
            value |= u64::from(b & 0x7f) << (7 * i);
            if b & 0x80 == 0 {
                return Ok(Some(value));
            }
        }
        Err(invalid("Invalid varint in CAR"))
    }

    async fn exact(&mut self, len: u64) -> Result<Vec<u8>, ImportError> {
        let mut buf = vec![0u8; usize::try_from(len).map_err(|_| invalid("Block too large"))?];
        self.r.read_exact(&mut buf).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                invalid("Truncated CAR")
            } else {
                CoreError::Storage(format!("read the upload: {e}")).into()
            }
        })?;
        Ok(buf)
    }

    /// The next block, or None at the end.
    pub async fn next_block(&mut self) -> Result<Option<(Cid, Vec<u8>)>, ImportError> {
        let Some(len) = self.varint().await? else {
            return Ok(None);
        };
        if len > MAX_BLOCK_BYTES + 64 {
            return Err(invalid(format!(
                "CAR block too large (max {MAX_BLOCK_BYTES} bytes)"
            )));
        }
        self.blocks += 1;
        if self.blocks > MAX_BLOCKS {
            return Err(invalid(format!(
                "Too many blocks in CAR (max {MAX_BLOCKS})"
            )));
        }
        let mut section = self.exact(len).await?;
        let cid =
            Cid::from_bytes(&section).map_err(|e| invalid(format!("Invalid CID in CAR: {e}")))?;
        let cid_len = cid.to_bytes().len();
        if cid_len > section.len() || !cid.is_dasl_compliant() {
            return Err(invalid("Invalid CID in CAR"));
        }
        let data = section.split_off(cid_len);
        if !cid.verify(&data).unwrap_or(false) {
            return Err(invalid(format!("Not a valid CID for bytes: {cid}")));
        }
        Ok(Some((cid, data)))
    }
}

// ---------------------------------------------------------------- tree checks

/// A checked MST node: its leaves and children with their key bounds.
#[derive(Debug)]
struct Node {
    leaves: Vec<(String, Cid)>,
    /// (child, lower bound, upper bound), bounds exclusive.
    children: Vec<(Cid, Option<String>, Option<String>)>,
    layer: Option<usize>,
}

/// Decodes a node and checks it: keys valid, in order, within the bounds
/// its parent gives, all on one layer (the parent's minus one).
fn check_node(
    cid: &Cid,
    bytes: &[u8],
    lo: Option<&str>,
    hi: Option<&str>,
    expected_layer: Option<usize>,
    is_root: bool,
) -> Result<Node, ImportError> {
    let bad = |why: &str| invalid(format!("invalid MST node {cid}: {why}"));
    let value: LexValue = proto_blue_lex_cbor::decode(bytes).map_err(|_| bad("not DAG-CBOR"))?;
    let data = deserialize_node_data(&value).map_err(|e| bad(&e.to_string()))?;
    if !is_root && data.entries.is_empty() && data.left.is_none() {
        return Err(bad("empty node below the root"));
    }
    let mut keys: Vec<String> = Vec::with_capacity(data.entries.len());
    let mut last: Vec<u8> = Vec::new();
    for e in &data.entries {
        if e.prefix_len > last.len() {
            return Err(bad("prefix longer than the previous key"));
        }
        let mut k = last[..e.prefix_len].to_vec();
        k.extend_from_slice(&e.key_suffix);
        let key = String::from_utf8(k.clone()).map_err(|_| bad("key is not UTF-8"))?;
        if !is_valid_mst_key(&key) {
            return Err(bad(&format!("invalid key {key:?}")));
        }
        last = k;
        keys.push(key);
    }
    let mut layer = None;
    for k in &keys {
        let l = leading_zeros_on_hash(k);
        match layer {
            None => layer = Some(l),
            Some(x) if x != l => return Err(bad("keys on different layers")),
            Some(_) => {}
        }
    }
    let layer = layer.or(expected_layer);
    if let (Some(l), Some(e)) = (layer, expected_layer)
        && l != e
    {
        return Err(bad("node on the wrong layer"));
    }
    if layer.is_some_and(|l| l > MAX_LAYER) {
        return Err(bad("tree too deep"));
    }
    let has_children = data.left.is_some() || data.entries.iter().any(|e| e.tree.is_some());
    if has_children && layer == Some(0) {
        return Err(bad("subtree below layer 0"));
    }
    // Order and bounds.
    let mut prev: Option<&str> = lo;
    for k in &keys {
        if prev.is_some_and(|p| k.as_str() <= p) {
            return Err(bad("keys out of order"));
        }
        prev = Some(k);
    }
    if let (Some(h), Some(k)) = (hi, keys.last())
        && k.as_str() >= h
    {
        return Err(bad("keys out of order"));
    }
    let mut children = Vec::new();
    if let Some(l) = &data.left {
        children.push((
            l.clone(),
            lo.map(str::to_owned),
            keys.first().cloned().or_else(|| hi.map(str::to_owned)),
        ));
    }
    for (i, e) in data.entries.iter().enumerate() {
        if let Some(t) = &e.tree {
            let upper = keys.get(i + 1).cloned().or_else(|| hi.map(str::to_owned));
            children.push((t.clone(), Some(keys[i].clone()), upper));
        }
    }
    Ok(Node {
        leaves: keys
            .into_iter()
            .zip(data.entries.iter().map(|e| e.value.clone()))
            .collect(),
        children,
        layer,
    })
}

/// Splits `collection/rkey`.
fn split_key(key: &str) -> Result<(String, String), ImportError> {
    let (c, r) = key
        .split_once('/')
        .ok_or_else(|| invalid(format!("invalid record key {key:?}")))?;
    Ok((c.to_owned(), r.to_owned()))
}

/// Every blob CID a record references, legacy `{cid, mimeType}` refs
/// included (as the reference's import: `allowLegacy`, not strict).
#[must_use]
pub fn import_blob_cids(value: &LexValue) -> Vec<String> {
    fn walk(v: &LexValue, out: &mut Vec<String>) {
        match v {
            LexValue::Map(m) => {
                if m.get("$type").and_then(LexValue::as_str) == Some("blob")
                    && let Some(c) = m.get("ref").and_then(LexValue::as_cid)
                {
                    out.push(cid_str(c));
                    return;
                }
                if m.len() == 2
                    && m.get("mimeType").and_then(LexValue::as_str).is_some()
                    && let Some(c) = m.get("cid").and_then(LexValue::as_str)
                    && let Ok(c) = Cid::from_str_multibase(c)
                {
                    out.push(cid_str(&c));
                    return;
                }
                for x in m.values() {
                    walk(x, out);
                }
            }
            LexValue::Array(a) => {
                for x in a {
                    walk(x, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(value, &mut out);
    out.sort();
    out.dedup();
    out
}

fn parse_record(collection: &str, rkey: &str, bytes: &[u8]) -> Result<LexValue, ImportError> {
    let v: LexValue = proto_blue_lex_cbor::decode(bytes)
        .map_err(|_| invalid(format!("Could not parse record at '{collection}/{rkey}'")))?;
    if v.as_map().is_none() {
        return Err(invalid(format!(
            "Could not parse record at '{collection}/{rkey}'"
        )));
    }
    Ok(v)
}

// ---------------------------------------------------------------- staging

/// A node to visit: (cid, lower bound, upper bound, expected layer, is root).
type Visit = (Cid, Option<String>, Option<String>, Option<usize>, bool);

/// Staged rows waiting to be written.
#[derive(Default)]
struct Pending {
    deltas: Vec<StagedBlock>,
    new_leaves: Vec<(String, String, String)>,
    old_leaves: Vec<(String, String, String)>,
    blobs: Vec<String>,
}

impl Pending {
    fn len(&self) -> usize {
        self.deltas.len() + self.new_leaves.len() + self.old_leaves.len() + self.blobs.len()
    }
}

/// What staging found.
#[derive(Debug, Clone)]
pub struct Staged {
    pub data_cid: Cid,
    pub new_rev: String,
    /// Set when the uploaded commit isn't signed by the account's key.
    pub signature_note: Option<String>,
}

impl RepoEngine {
    /// A fenced transaction for one batch of `job`, after renewing it.
    async fn job_tx(&self, job: &BulkJob) -> Result<Tx, ImportError> {
        if !self.store().renew_bulk_job(job).await? {
            return Err(CoreError::LeaseLost(format!("bulk job {}", job.id)).into());
        }
        let mut tx = self.store().begin().await?;
        tx.fence_bulk_job(job).await?;
        Ok(tx)
    }

    /// A node of the account's current tree.
    async fn current_node(&self, did: &str, cid: &Cid) -> Result<Node, ImportError> {
        let bytes = self
            .store()
            .block(did, &cid_str(cid))
            .await?
            .ok_or_else(|| {
                CoreError::Storage(format!("block {cid} of {did}'s current tree is missing"))
            })?;
        check_node(cid, &bytes, None, None, None, true)
            .map_err(|e| CoreError::Storage(format!("current tree: {e}")).into())
    }

    async fn flush(&self, job: &BulkJob, p: &mut Pending) -> Result<(), ImportError> {
        if p.len() == 0 {
            return Ok(());
        }
        let mut tx = self.job_tx(job).await?;
        tx.add_import_deltas(&job.id, &p.deltas).await?;
        tx.stage_new_leaves(&job.id, &p.new_leaves).await?;
        tx.stage_old_leaves(&job.id, &p.old_leaves).await?;
        tx.stage_new_blobs(&job.id, &p.blobs).await?;
        tx.commit().await?;
        *p = Pending::default();
        Ok(())
    }

    /// Starts an import job for `did` (its writes blocked), under the repo
    /// lock so it orders with commits. Refused with `RepoWriteBlocked` if
    /// another job holds the account.
    pub async fn start_import(&self, did: &str, holder: &str) -> Result<BulkJob, ImportError> {
        let (_guard, mut tx) = self.locked_tx(did).await?;
        // A TID first, so a node's jobs sort in the order they started.
        let id = format!("import-{}-{}", self.tids().next(), hex_id());
        let job = tx
            .start_bulk_job(&id, did, "import", "staging", holder)
            .await?;
        tx.commit().await?;
        Ok(job)
    }

    /// Phase 1: reads the CAR at `car`, checks it and stages the diff.
    /// `signing_key` is the account's current `did:key`, for the signature
    /// check (a mismatch is noted, not refused).
    pub async fn stage_import(
        &self,
        job: &BulkJob,
        car: &Path,
        signing_key: Option<&str>,
    ) -> Result<Staged, ImportError> {
        let did = job.did.as_str();
        let store = self.store();
        let file = tokio::fs::File::open(car)
            .await
            .map_err(|e| CoreError::Storage(format!("open the upload: {e}")))?;
        let (mut reader, roots) = CarReader::open(file).await?;
        if roots.len() != 1 {
            return Err(invalid("expected one root"));
        }
        let root = roots[0].clone();
        let mut batch: Vec<(String, Vec<u8>)> = Vec::new();
        let mut batch_bytes = 0usize;
        while let Some((cid, bytes)) = reader.next_block().await? {
            batch_bytes += bytes.len();
            batch.push((cid_str(&cid), bytes));
            if batch.len() >= FLUSH || batch_bytes >= 8_000_000 {
                let mut tx = self.job_tx(job).await?;
                tx.stage_car_blocks(&job.id, &batch).await?;
                tx.commit().await?;
                batch.clear();
                batch_bytes = 0;
            }
        }
        if !batch.is_empty() {
            let mut tx = self.job_tx(job).await?;
            tx.stage_car_blocks(&job.id, &batch).await?;
            tx.commit().await?;
        }

        // The commit.
        let commit_bytes = store
            .staged_block(&job.id, &cid_str(&root))
            .await?
            .ok_or_else(|| invalid(format!("missing commit block: {root}")))?;
        let commit_value: LexValue = proto_blue_lex_cbor::decode(&commit_bytes)
            .map_err(|_| invalid("Could not parse commit"))?;
        let commit = SignedCommit::from_lex_value(&commit_value)
            .map_err(|e| invalid(format!("Could not parse commit: {e}")))?;
        if commit.did != did {
            return Err(invalid(format!("Commit is for {}, not {did}", commit.did)));
        }
        let signature_note = match signing_key {
            Some(key) if verify_commit_sig(&commit, key).unwrap_or(false) => None,
            Some(_) => Some(
                "The imported commit isn't signed by the account's current signing key; it was re-signed."
                    .to_owned(),
            ),
            None => None,
        };
        if let Some(n) = &signature_note {
            tracing::warn!(did, "import: {n}");
        }

        let old = store
            .repo_root_of(did)
            .await?
            .ok_or_else(|| invalid(format!("Could not find repo for DID: {did}")))?;

        // The current tree's nodes, so a shared subtree is one the current
        // tree really has as a subtree (not any stored block with node-like
        // bytes, such as a record).
        let mut old_nodes: HashSet<Cid> = HashSet::new();
        let mut queue: VecDeque<Cid> = VecDeque::new();
        queue.push_back(parse_cid(&old.data_cid)?);
        while let Some(cid) = queue.pop_front() {
            if !old_nodes.insert(cid.clone()) {
                continue;
            }
            let node = self.current_node(did, &cid).await?;
            queue.extend(node.children.into_iter().map(|(c, _, _)| c));
        }

        // The new tree, skipping subtrees the current tree has.
        let mut pending = Pending::default();
        let mut visited: HashSet<Cid> = HashSet::new();
        let mut shared: HashSet<Cid> = HashSet::new();
        let mut queue: VecDeque<Visit> = VecDeque::new();
        queue.push_back((commit.data.clone(), None, None, None, true));
        while let Some((cid, lo, hi, layer, is_root)) = queue.pop_front() {
            if !visited.insert(cid.clone()) {
                return Err(invalid(format!(
                    "invalid MST: node referenced more than once: {cid}"
                )));
            }
            if visited.len() as u64 > MAX_BLOCKS {
                return Err(invalid("invalid MST: too many nodes"));
            }
            let key = cid_str(&cid);
            if old_nodes.contains(&cid) {
                shared.insert(cid);
                continue;
            }
            let bytes = store
                .staged_block(&job.id, &key)
                .await?
                .ok_or_else(|| invalid(format!("missing block: {cid}")))?;
            let node = check_node(&cid, &bytes, lo.as_deref(), hi.as_deref(), layer, is_root)?;
            pending.deltas.push(StagedBlock {
                cid: key,
                content: None,
                delta: 1,
            });
            for (k, v) in &node.leaves {
                let (collection, rkey) = split_key(k)?;
                let vkey = cid_str(v);
                let (bytes, copied) = match store.staged_block(&job.id, &vkey).await? {
                    Some(b) => (b, false),
                    None => match store.block(did, &vkey).await? {
                        Some(b) if store.is_live_block(did, &vkey).await? => (b, true),
                        _ => {
                            return Err(invalid(format!(
                                "Could not parse record at '{collection}/{rkey}'"
                            )));
                        }
                    },
                };
                let value = parse_record(&collection, &rkey, &bytes)?;
                pending.blobs.extend(import_blob_cids(&value));
                pending.deltas.push(StagedBlock {
                    cid: vkey.clone(),
                    content: copied.then_some(bytes),
                    delta: 1,
                });
                pending.new_leaves.push((collection, rkey, vkey));
            }
            let child_layer = node.layer.and_then(|l| l.checked_sub(1));
            for (c, clo, chi) in node.children {
                queue.push_back((c, clo, chi, child_layer, false));
            }
            if pending.len() >= FLUSH {
                self.flush(job, &mut pending).await?;
            }
        }
        self.flush(job, &mut pending).await?;

        // The current tree's part that isn't shared.
        let mut seen: HashSet<Cid> = HashSet::new();
        let mut queue: VecDeque<Cid> = VecDeque::new();
        queue.push_back(parse_cid(&old.data_cid)?);
        while let Some(cid) = queue.pop_front() {
            if shared.contains(&cid) || !seen.insert(cid.clone()) {
                continue;
            }
            let key = cid_str(&cid);
            let node = self.current_node(did, &cid).await?;
            pending.deltas.push(StagedBlock {
                cid: key,
                content: None,
                delta: -1,
            });
            for (k, v) in &node.leaves {
                let (collection, rkey) = split_key(k)?;
                let vkey = cid_str(v);
                pending.deltas.push(StagedBlock {
                    cid: vkey.clone(),
                    content: None,
                    delta: -1,
                });
                pending.old_leaves.push((collection, rkey, vkey));
            }
            for (c, _, _) in node.children {
                queue.push_back(c);
            }
            if pending.len() >= FLUSH {
                self.flush(job, &mut pending).await?;
            }
        }
        // The previous commit loses its reference at the end.
        pending.deltas.push(StagedBlock {
            cid: old.commit_cid.clone(),
            content: None,
            delta: -1,
        });
        self.flush(job, &mut pending).await?;

        // rev = max(now, imported rev + 1, stored rev + 1).
        let floor = if commit.rev > old.rev {
            commit.rev.clone()
        } else {
            old.rev.clone()
        };
        let new_rev = self.tids().next_after(Some(&floor));
        let total = store.import_record_count(&job.id).await?;
        let mut tx = self.job_tx(job).await?;
        tx.stage_old_blobs(&job.id, did).await?;
        tx.set_import_rev(&job.id, &new_rev).await?;
        tx.set_bulk_counts(&job.id, 0, total).await?;
        if let Some(n) = &signature_note {
            tx.set_bulk_notes(&job.id, n).await?;
        }
        tx.set_bulk_progress(&job.id, "increments", Some(&cid_str(&commit.data)))
            .await?;
        tx.commit().await?;
        Ok(Staged {
            data_cid: commit.data,
            new_rev,
            signature_note,
        })
    }

    /// Phase 2: positive deltas and blob promotions. Returns blob moves to
    /// finish.
    pub async fn import_increments(
        &self,
        job: &BulkJob,
        new_rev: &str,
    ) -> Result<Vec<PendingMove>, ImportError> {
        let did = job.did.as_str();
        loop {
            let mut tx = self.job_tx(job).await?;
            let rows = tx
                .import_blocks_batch(&job.id, BlockBatch::Positive, BATCH)
                .await?;
            if rows.is_empty() {
                tx.rollback().await?;
                break;
            }
            let deltas: Vec<BlockDelta> = rows
                .iter()
                .map(|r| BlockDelta {
                    cid: r.cid.clone(),
                    bytes: r.content.clone(),
                    delta: r.delta,
                })
                .collect();
            tx.apply_block_deltas(did, new_rev, &deltas).await?;
            let cids: Vec<String> = rows.into_iter().map(|r| r.cid).collect();
            tx.mark_import_blocks(&job.id, &cids, true).await?;
            tx.commit().await?;
        }
        let mut moves = Vec::new();
        let mut after = String::new();
        loop {
            let mut tx = self.job_tx(job).await?;
            let page = tx.import_blobs_page(&job.id, &after, BATCH).await?;
            let Some(last) = page.last() else {
                tx.rollback().await?;
                break;
            };
            after = last.cid.clone();
            let backend = tx.blob_backend().await?;
            for b in page.iter().filter(|b| b.in_new_tree && !b.applied) {
                let key = locus_store::blob::permanent_key(did, &b.cid);
                if let Some(row) = tx.reference_blob(did, &b.cid, &key, backend.epoch).await? {
                    if row.pending_move.as_deref() == Some(key.as_str()) {
                        moves.push(PendingMove {
                            did: did.to_owned(),
                            cid: b.cid.clone(),
                            from: row.storage_key.clone(),
                            to: key,
                        });
                    }
                    tx.mark_import_blob(&job.id, &b.cid, true).await?;
                }
            }
            tx.commit().await?;
        }
        Ok(moves)
    }

    /// Phase 3: the point of no return. Re-signs a commit over the imported
    /// data root with `new_rev`, moves `repo_root`, writes `#sync` when
    /// `emit_sync`, and moves the job to `reindex`.
    pub async fn import_swap(
        &self,
        job: &BulkJob,
        data_cid: &Cid,
        new_rev: &str,
        signer: &dyn Signer,
        emit_sync: bool,
    ) -> Result<Cid, ImportError> {
        let did = job.did.as_str();
        if !self.store().renew_bulk_job(job).await? {
            return Err(CoreError::LeaseLost(format!("bulk job {}", job.id)).into());
        }
        let (_guard, mut tx) = self.locked_tx(did).await?;
        tx.fence_bulk_job(job).await?;
        let commit = sign_commit(
            &UnsignedCommit::new(did.to_owned(), data_cid.clone(), new_rev.to_owned(), None),
            signer,
        )
        .map_err(|e| CoreError::Storage(format!("sign the import's commit: {e}")))?;
        let bytes = commit
            .to_cbor()
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        let cid = commit
            .cid()
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        tx.apply_block_deltas(
            did,
            new_rev,
            &[BlockDelta {
                cid: cid_str(&cid),
                bytes: Some(bytes.clone()),
                delta: 1,
            }],
        )
        .await?;
        tx.set_repo_root(
            did,
            &RepoRoot {
                commit_cid: cid_str(&cid),
                rev: new_rev.to_owned(),
                data_cid: cid_str(data_cid),
            },
        )
        .await?;
        if emit_sync {
            let body = sync_body(did, new_rev, &cid, &bytes)
                .map_err(|e| CoreError::Storage(e.to_string()))?;
            tx.insert_outbox(did, "sync", &body).await?;
        }
        tx.set_bulk_progress(&job.id, "reindex", None).await?;
        tx.commit().await?;
        Ok(cid)
    }

    /// Phase 4: the record index follows the new root.
    pub async fn import_reindex(&self, job: &BulkJob, new_rev: &str) -> Result<(), ImportError> {
        let did = job.did.as_str();
        let total = self.store().import_record_count(&job.id).await?;
        let mut done = 0i64;
        loop {
            let mut tx = self.job_tx(job).await?;
            let rows = tx.pending_import_records(&job.id, BATCH).await?;
            if rows.is_empty() {
                tx.set_bulk_progress(&job.id, "decrements", None).await?;
                tx.set_bulk_counts(&job.id, total, total).await?;
                tx.commit().await?;
                return Ok(());
            }
            let wanted: Vec<String> = rows.iter().filter_map(|r| r.new_cid.clone()).collect();
            let contents = tx.staged_contents(&job.id, &wanted).await?;
            for r in &rows {
                match &r.new_cid {
                    Some(cid) if r.change != "delete" => {
                        let bytes = contents.get(cid).ok_or_else(|| {
                            CoreError::Storage(format!("staged record block {cid} is missing"))
                        })?;
                        let value = parse_record(&r.collection, &r.rkey, bytes)
                            .map_err(|e| CoreError::Storage(e.to_string()))?;
                        tx.apply_record_changes(
                            did,
                            new_rev,
                            &[RecordChange::Put {
                                collection: r.collection.clone(),
                                rkey: r.rkey.clone(),
                                cid: cid.clone(),
                            }],
                        )
                        .await?;
                        tx.replace_record_blobs(
                            did,
                            &r.collection,
                            &r.rkey,
                            &import_blob_cids(&value),
                        )
                        .await?;
                        tx.replace_backlinks(
                            did,
                            &r.collection,
                            &r.rkey,
                            &backlinks(&r.collection, &value),
                        )
                        .await?;
                        let private =
                            crate::records::private_entry(did, &r.collection, &r.rkey, &value);
                        tx.replace_private_record(did, &r.collection, &r.rkey, private.as_ref())
                            .await?;
                    }
                    _ => {
                        tx.apply_record_changes(
                            did,
                            new_rev,
                            &[RecordChange::Delete {
                                collection: r.collection.clone(),
                                rkey: r.rkey.clone(),
                            }],
                        )
                        .await?;
                        tx.replace_record_blobs(did, &r.collection, &r.rkey, &[])
                            .await?;
                        tx.replace_backlinks(did, &r.collection, &r.rkey, &[])
                            .await?;
                        tx.replace_private_record(did, &r.collection, &r.rkey, None)
                            .await?;
                    }
                }
                tx.mark_import_record(&job.id, &r.collection, &r.rkey)
                    .await?;
            }
            done += rows.len() as i64;
            tx.set_bulk_counts(&job.id, done.min(total), total).await?;
            tx.commit().await?;
        }
    }

    /// Phase 5 and the end: negative deltas, blobs left unreferenced (and
    /// temp rows of the new tree's CIDs uploaded meanwhile promoted), then
    /// staging dropped and the write block cleared.
    pub async fn import_decrements(
        &self,
        job: &BulkJob,
        new_rev: &str,
    ) -> Result<Vec<PendingMove>, ImportError> {
        let did = job.did.as_str();
        loop {
            let mut tx = self.job_tx(job).await?;
            let rows = tx
                .import_blocks_batch(&job.id, BlockBatch::Negative, BATCH)
                .await?;
            if rows.is_empty() {
                tx.rollback().await?;
                break;
            }
            let deltas: Vec<BlockDelta> = rows
                .iter()
                .map(|r| BlockDelta {
                    cid: r.cid.clone(),
                    bytes: None,
                    delta: r.delta,
                })
                .collect();
            tx.apply_block_deltas(did, new_rev, &deltas).await?;
            let cids: Vec<String> = rows.into_iter().map(|r| r.cid).collect();
            tx.mark_import_blocks(&job.id, &cids, true).await?;
            tx.commit().await?;
        }
        let mut moves = Vec::new();
        let gate_on = self.media_gate_on();
        let mut after = String::new();
        loop {
            let mut tx = self.job_tx(job).await?;
            let page = tx.import_blobs_page(&job.id, &after, BATCH).await?;
            let Some(last) = page.last() else {
                tx.rollback().await?;
                break;
            };
            after = last.cid.clone();
            let backend = tx.blob_backend().await?;
            for b in &page {
                let Some(row) = tx.blob_for_update(did, &b.cid).await? else {
                    continue;
                };
                if b.in_new_tree && row.state == "temp" && tx.blob_has_records(did, &b.cid).await? {
                    let key = locus_store::blob::permanent_key(did, &b.cid);
                    if let Some(r) = tx.reference_blob(did, &b.cid, &key, backend.epoch).await?
                        && r.pending_move.as_deref() == Some(key.as_str())
                    {
                        moves.push(PendingMove {
                            did: did.to_owned(),
                            cid: b.cid.clone(),
                            from: r.storage_key.clone(),
                            to: key,
                        });
                    }
                } else {
                    tx.orphan_blob_if_unreferenced(did, &b.cid).await?;
                }
                // Whether it's private-only now the records are indexed, and
                // the store that calls for.
                tx.recompute_private_only(did, &b.cid).await?;
                if let Some(m) = tx.place_blob(did, &b.cid, gate_on, backend.epoch).await? {
                    moves.retain(|p| p.cid != m.cid);
                    moves.push(m);
                }
            }
            tx.commit().await?;
        }
        self.drop_staging(job).await?;
        let mut tx = self.job_tx(job).await?;
        tx.finish_bulk_job(&job.id, "done", None).await?;
        tx.commit().await?;
        Ok(moves)
    }

    async fn drop_staging(&self, job: &BulkJob) -> Result<(), ImportError> {
        loop {
            let mut tx = self.job_tx(job).await?;
            let n = tx.drop_import_staging(&job.id, 5_000).await?;
            tx.commit().await?;
            if n == 0 {
                return Ok(());
            }
        }
    }

    /// Undoes an import that hadn't swapped: `rolling_back` (under the repo
    /// lock, so uploads stop counting the import's CIDs), applied positive
    /// deltas undone, promoted blobs no record references marked
    /// unreferenced, staging dropped, and the job ends `failed`.
    pub async fn import_rollback(&self, job: &BulkJob, error: &str) -> Result<(), ImportError> {
        let did = job.did.as_str();
        {
            if !self.store().renew_bulk_job(job).await? {
                return Err(CoreError::LeaseLost(format!("bulk job {}", job.id)).into());
            }
            let (_guard, mut tx) = self.locked_tx(did).await?;
            tx.fence_bulk_job(job).await?;
            tx.set_bulk_progress(&job.id, "rolling_back", None).await?;
            tx.set_bulk_error(&job.id, error).await?;
            tx.commit().await?;
        }
        loop {
            let mut tx = self.job_tx(job).await?;
            let rows = tx
                .import_blocks_batch(&job.id, BlockBatch::AppliedPositive, BATCH)
                .await?;
            if rows.is_empty() {
                tx.rollback().await?;
                break;
            }
            let deltas: Vec<BlockDelta> = rows
                .iter()
                .map(|r| BlockDelta {
                    cid: r.cid.clone(),
                    bytes: None,
                    delta: -r.delta,
                })
                .collect();
            // The rev only matters for increments; these are all decrements.
            tx.apply_block_deltas(did, "", &deltas).await?;
            let cids: Vec<String> = rows.into_iter().map(|r| r.cid).collect();
            tx.mark_import_blocks(&job.id, &cids, false).await?;
            tx.commit().await?;
        }
        loop {
            let mut tx = self.job_tx(job).await?;
            let cids = tx.applied_import_blobs(&job.id, BATCH).await?;
            if cids.is_empty() {
                tx.rollback().await?;
                break;
            }
            tx.blob_backend().await?;
            for cid in &cids {
                tx.orphan_blob_if_unreferenced(did, cid).await?;
                tx.mark_import_blob(&job.id, cid, false).await?;
            }
            tx.commit().await?;
        }
        self.drop_staging(job).await?;
        let mut tx = self.job_tx(job).await?;
        let error = self
            .store()
            .bulk_job_error(&job.id)
            .await?
            .unwrap_or_else(|| error.to_owned());
        tx.finish_bulk_job(&job.id, "failed", Some(&error)).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Runs an import from the start: staging through reindex, rolling back
    /// on any failure before the swap. Returns once the index follows the
    /// new root (decrements are left to [`RepoEngine::finish_import`]).
    pub async fn run_import(
        &self,
        job: &BulkJob,
        car: &Path,
        signing_key: Option<&str>,
        signer: &dyn Signer,
        emit_sync: bool,
    ) -> Result<Vec<PendingMove>, ImportError> {
        let before_swap: Result<(Staged, Vec<PendingMove>), ImportError> = async {
            let staged = self.stage_import(job, car, signing_key).await?;
            let moves = self.import_increments(job, &staged.new_rev).await?;
            Ok((staged, moves))
        }
        .await;
        let (staged, moves) = match before_swap {
            Ok(v) => v,
            Err(e) => {
                self.roll_back_after(job, &e).await;
                return Err(e);
            }
        };
        if let Err(e) = self
            .import_swap(job, &staged.data_cid, &staged.new_rev, signer, emit_sync)
            .await
        {
            // The swap is one transaction: if it failed, nothing moved.
            if self
                .store()
                .bulk_job(&job.id)
                .await
                .ok()
                .flatten()
                .map(|j| j.phase)
                != Some("reindex".to_owned())
            {
                self.roll_back_after(job, &e).await;
            }
            return Err(e);
        }
        self.import_reindex(job, &staged.new_rev).await?;
        Ok(moves)
    }

    async fn roll_back_after(&self, job: &BulkJob, e: &ImportError) {
        if matches!(e, ImportError::Core(CoreError::LeaseLost(_))) {
            return; // Another holder has the job now.
        }
        if let Err(re) = self.import_rollback(job, &e.to_string()).await {
            tracing::warn!(job = %job.id, "import rollback failed (the resumer will retry): {re}");
        }
    }

    /// Carries an import forward from `reindex` or `decrements` to the end.
    pub async fn finish_import(&self, job: &BulkJob) -> Result<Vec<PendingMove>, ImportError> {
        let rev = self
            .store()
            .bulk_job_rev(&job.id)
            .await?
            .ok_or_else(|| CoreError::Storage(format!("import {} has no rev", job.id)))?;
        let current = self
            .store()
            .bulk_job(&job.id)
            .await?
            .map(|j| j.phase)
            .unwrap_or_default();
        if current == "reindex" {
            self.import_reindex(job, &rev).await?;
        }
        self.import_decrements(job, &rev).await
    }

    /// Resumes an import a resumer took over: forward after the swap,
    /// otherwise a rollback.
    pub async fn resume_import(&self, job: &BulkJob) -> Result<Vec<PendingMove>, ImportError> {
        match job.phase.as_str() {
            "reindex" | "decrements" => self.finish_import(job).await,
            _ => {
                self.import_rollback(job, "The import was interrupted; try it again")
                    .await?;
                Ok(Vec::new())
            }
        }
    }
}

fn hex_id() -> String {
    hex::encode(rand::random::<[u8; 12]>())
}
