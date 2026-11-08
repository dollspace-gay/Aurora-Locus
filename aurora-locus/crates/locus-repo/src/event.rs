//! Firehose event bodies written to the outbox.
//!
//! A `#commit` body is stored as DAG-CBOR without `seq` and `time`; the
//! sequencer adds those when it numbers the event. The body is built as a
//! `LexValue`, never from serde structs, so CIDs encode as links and bytes as
//! byte strings.

use std::collections::BTreeMap;

use proto_blue_lex_data::{Cid, LexValue};
use proto_blue_repo::block_map::BlockMap;
use proto_blue_repo::car::blocks_to_car;
use proto_blue_repo::error::RepoError;

/// An `#identity` body: the DID, and the handle when it is known (as the
/// reference, a PLC operation emits one without the handle).
pub fn identity_body(did: &str, handle: Option<&str>) -> Result<Vec<u8>, RepoError> {
    let mut m = BTreeMap::new();
    m.insert("did".to_owned(), LexValue::String(did.to_owned()));
    if let Some(h) = handle {
        m.insert("handle".to_owned(), LexValue::String(h.to_owned()));
    }
    Ok(proto_blue_lex_cbor::encode(&LexValue::Map(m))?)
}

/// An `#account` body. `status` is set when the account isn't active
/// (`takendown`, `suspended`, `deactivated`, `deleted`).
pub fn account_body(did: &str, active: bool, status: Option<&str>) -> Result<Vec<u8>, RepoError> {
    let mut m = BTreeMap::new();
    m.insert("did".to_owned(), LexValue::String(did.to_owned()));
    m.insert("active".to_owned(), LexValue::Bool(active));
    if let Some(s) = status {
        m.insert("status".to_owned(), LexValue::String(s.to_owned()));
    }
    Ok(proto_blue_lex_cbor::encode(&LexValue::Map(m))?)
}

/// A `#sync` body: the current commit, as a CAR holding only the commit block.
pub fn sync_body(
    did: &str,
    rev: &str,
    commit_cid: &Cid,
    commit_bytes: &[u8],
) -> Result<Vec<u8>, RepoError> {
    let mut blocks = BlockMap::new();
    blocks.set(commit_cid.clone(), commit_bytes.to_vec());
    let car = blocks_to_car(Some(commit_cid), &blocks)?;
    let mut m = BTreeMap::new();
    m.insert("did".to_owned(), LexValue::String(did.to_owned()));
    m.insert("rev".to_owned(), LexValue::String(rev.to_owned()));
    m.insert("blocks".to_owned(), LexValue::Bytes(car));
    Ok(proto_blue_lex_cbor::encode(&LexValue::Map(m))?)
}

/// The lexicon's limit on a `#commit` event's `blocks`.
pub const MAX_EVENT_BLOCKS_BYTES: usize = 2_000_000;

/// One op in a `#commit` event.
#[derive(Debug, Clone)]
pub struct EventOp {
    pub action: &'static str,
    pub path: String,
    /// The new record's CID; None for a delete.
    pub cid: Option<Cid>,
    /// The record's CID before this op, if it existed.
    pub prev: Option<Cid>,
}

#[derive(Debug, Clone)]
pub struct CommitEvent {
    pub repo: String,
    pub commit: Cid,
    pub rev: String,
    /// The previous commit's rev; None on a repository's first commit.
    pub since: Option<String>,
    /// The previous commit's data root; omitted on the first commit.
    pub prev_data: Option<Cid>,
    pub ops: Vec<EventOp>,
    /// The commit block, new MST nodes, written record blocks and covering
    /// proofs; sent as a CAR rooted at the commit.
    pub blocks: BlockMap,
}

impl CommitEvent {
    /// The event body as DAG-CBOR, without `seq` and `time`.
    pub fn encode(&self) -> Result<Vec<u8>, RepoError> {
        Ok(proto_blue_lex_cbor::encode(&self.to_lex()?)?)
    }

    pub fn to_lex(&self) -> Result<LexValue, RepoError> {
        let car = blocks_to_car(Some(&self.commit), &self.blocks)?;
        let ops = self
            .ops
            .iter()
            .map(|op| {
                let mut m = BTreeMap::new();
                m.insert("action".to_owned(), LexValue::String(op.action.to_owned()));
                m.insert("path".to_owned(), LexValue::String(op.path.clone()));
                m.insert(
                    "cid".to_owned(),
                    op.cid.clone().map_or(LexValue::Null, LexValue::Cid),
                );
                if let Some(prev) = &op.prev {
                    m.insert("prev".to_owned(), LexValue::Cid(prev.clone()));
                }
                LexValue::Map(m)
            })
            .collect();
        let mut m = BTreeMap::new();
        m.insert("repo".to_owned(), LexValue::String(self.repo.clone()));
        m.insert("commit".to_owned(), LexValue::Cid(self.commit.clone()));
        m.insert("rev".to_owned(), LexValue::String(self.rev.clone()));
        m.insert(
            "since".to_owned(),
            self.since.clone().map_or(LexValue::Null, LexValue::String),
        );
        m.insert("blocks".to_owned(), LexValue::Bytes(car));
        m.insert("ops".to_owned(), LexValue::Array(ops));
        m.insert("blobs".to_owned(), LexValue::Array(Vec::new()));
        m.insert("rebase".to_owned(), LexValue::Bool(false));
        m.insert("tooBig".to_owned(), LexValue::Bool(false));
        if let Some(prev) = &self.prev_data {
            m.insert("prevData".to_owned(), LexValue::Cid(prev.clone()));
        }
        Ok(LexValue::Map(m))
    }
}
