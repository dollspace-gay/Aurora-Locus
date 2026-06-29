//! History-aware repository verification (key-rotation arc #366, Phase A2 / #368).
//!
//! Where proto-blue's [`verify_repo`] checks only the head commit's signature
//! against a single key, [`verify_repo_with_history`] verifies EVERY commit in
//! the chain against the signing key that was valid at that commit's `rev`,
//! resolved from the account's PLC signing-key history (the
//! [`PlcOpHistoryEntry`] list from
//! [`PlcClient::get_op_history`](crate::crypto::plc_client::PlcClient::get_op_history)).
//!
//! This is the verification model real key rotation requires: once an account
//! has rotated, its commit chain spans multiple keys (commits before the
//! rotation signed with K1, after with K2), and a single-key check can only
//! ever validate the head.
//!
//! ## Validity windows (design §3.4)
//!
//! Each PLC op establishes a half-open validity window for the key it publishes:
//! `[entry.accepted_at, next_entry.accepted_at)`, open-ended for the most recent
//! entry. A commit at TID `rev = T` verifies against the key whose window
//! contains `T`. The comparison is in microseconds: a TID decodes to its
//! microsecond timestamp via [`proto_blue::syntax::Tid::timestamp_micros`], and a
//! PLC `accepted_at` is a `DateTime<Utc>` (`timestamp_micros()`). A commit at
//! exactly a window boundary falls into the NEW (post-rotation) window — the new
//! key signed it.
//!
//! ## Chain walk (design §3.5 / §3.6)
//!
//! Single-history accounts (never rotated → one PLC op) take a fast path:
//! delegate straight to [`verify_repo`]. Multi-history accounts walk head →
//! genesis via [`SignedCommit::prev`], verifying each commit against its window
//! key, terminating at genesis (`prev == None`) OR at the first `prev` whose
//! block is absent from `blocks` (a defensive guard — a partial block set is
//! verified as far as it reaches, not treated as an error).

use proto_blue::lex_data::Cid;
use proto_blue::repo::commit::{verify_commit_sig, SignedCommit};
use proto_blue::repo::{verify_repo, BlockMap, RepoError, VerifiedRepo};
use proto_blue::syntax::Tid;

use crate::crypto::plc_client::PlcOpHistoryEntry;

/// Failure modes of history-aware verification.
#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    /// A commit's `rev` predates the genesis PLC op — it cannot have been signed
    /// by any published key. Should not occur for genuine history.
    #[error("commit {commit_cid} at rev {rev} predates the genesis PLC op for {did}")]
    CommitBeforeGenesis {
        did: String,
        commit_cid: String,
        rev: String,
    },

    /// A commit's signature did not verify against the key valid at its `rev`.
    #[error("commit {commit_cid} (rev {rev}) failed signature verification with key {key_used}: {reason}")]
    CommitVerifyFailed {
        commit_cid: String,
        rev: String,
        key_used: String,
        reason: String,
    },

    /// The account has no PLC signing-key history (defensive — the fast path
    /// short-circuits single-history accounts before this is reachable).
    #[error("no PLC signing-key history for {did}")]
    EmptyHistory { did: String },

    /// proto-blue rejected the repo structurally (a missing reachable block, MST
    /// inconsistency, malformed commit) — surfaced from [`verify_repo`] and the
    /// per-commit decode path. Absorbs `RepoError` (incl. MST load errors).
    #[error("proto-blue verify / decode failed: {0}")]
    ProtoBlueRepo(#[from] RepoError),
}

/// A successful history-aware verification, plus the operator-facing stats the
/// preflight surface reports.
///
/// (Design §3.5 pinned `-> VerifiedRepo`; widened here to also carry
/// `commits_verified` and `keys_used`, which the A2 `preRebuildCheck` consumer
/// reports. A3's rebuild gate uses `.repo` and ignores the stats.)
#[derive(Debug)]
pub struct HistoryVerifyOutcome {
    /// The structurally-verified repo (head commit + MST + blocks).
    pub repo: VerifiedRepo,
    /// Number of commit signatures checked. `1` on the single-history fast path
    /// (head only); the full walked-chain length on the multi-history path.
    pub commits_verified: usize,
    /// Number of distinct signing keys the verified commits resolved to. `1` for
    /// a never-rotated account; `2+` once it has rotated.
    pub keys_used: usize,
}

/// The signing key valid at `rev_micros` per `history`, or `None` if `rev_micros`
/// predates the genesis op (or `history` is empty).
///
/// `history` MUST be ascending by `accepted_at` (as `get_op_history` returns it).
/// Windows are half-open: the chosen key is the latest entry whose `accepted_at`
/// is `<= rev_micros`, so a commit exactly at a rotation boundary takes the new
/// key.
fn resolve_key_at_rev(history: &[PlcOpHistoryEntry], rev_micros: i64) -> Option<&str> {
    let mut chosen = None;
    for entry in history {
        if entry.accepted_at.timestamp_micros() <= rev_micros {
            chosen = Some(entry.signing_did_key.as_str());
        } else {
            // Ascending order: every later entry is also `> rev_micros`.
            break;
        }
    }
    chosen
}

/// Decode the commit stored at `cid` in `blocks`.
fn load_commit(blocks: &BlockMap, cid: &Cid) -> Result<SignedCommit, RepoError> {
    let bytes = blocks
        .get(cid)
        .ok_or_else(|| RepoError::MissingBlock(cid.clone()))?;
    let value = proto_blue::lex_cbor::decode(bytes)?;
    SignedCommit::from_lex_value(&value)
}

/// Verify a repository against its PLC signing-key history (design §3.5/§3.6).
///
/// `plc_history` is the account's ascending PLC op history. On success returns a
/// [`HistoryVerifyOutcome`] (the [`VerifiedRepo`] plus verification stats); on
/// any per-commit failure returns the [`VerifyError`] carrying the offending
/// commit's identity.
pub fn verify_repo_with_history(
    blocks: BlockMap,
    root: &Cid,
    expected_did: Option<&str>,
    plc_history: &[PlcOpHistoryEntry],
) -> Result<HistoryVerifyOutcome, VerifyError> {
    let did_for_err = expected_did.unwrap_or("<unknown>").to_string();

    if plc_history.is_empty() {
        return Err(VerifyError::EmptyHistory { did: did_for_err });
    }

    // Fast path: a never-rotated account has one key — delegate to proto-blue's
    // single-key verify_repo (head-only sig check), preserving today's semantics.
    if plc_history.len() == 1 {
        let repo = verify_repo(blocks, root, expected_did, Some(&plc_history[0].signing_did_key))?;
        return Ok(HistoryVerifyOutcome {
            repo,
            commits_verified: 1,
            keys_used: 1,
        });
    }

    // Multi-history: walk head → genesis, verifying each commit against the key
    // valid at its rev.
    use std::collections::BTreeSet;
    let mut keys_used: BTreeSet<&str> = BTreeSet::new();
    let mut commits_verified = 0usize;
    let mut cursor = root.clone();

    loop {
        let commit = load_commit(&blocks, &cursor)?;

        let rev_micros = i64::try_from(
            Tid::new(&commit.rev)
                .map_err(|e| VerifyError::CommitVerifyFailed {
                    commit_cid: cursor.to_string(),
                    rev: commit.rev.clone(),
                    key_used: "<none>".to_string(),
                    reason: format!("unparseable rev TID: {e}"),
                })?
                .timestamp_micros(),
        )
        .unwrap_or(i64::MAX);

        let key = resolve_key_at_rev(plc_history, rev_micros).ok_or_else(|| {
            VerifyError::CommitBeforeGenesis {
                did: did_for_err.clone(),
                commit_cid: cursor.to_string(),
                rev: commit.rev.clone(),
            }
        })?;

        match verify_commit_sig(&commit, key) {
            Ok(true) => {}
            Ok(false) => {
                return Err(VerifyError::CommitVerifyFailed {
                    commit_cid: cursor.to_string(),
                    rev: commit.rev.clone(),
                    key_used: key.to_string(),
                    reason: "signature does not verify against the key valid at this rev".to_string(),
                })
            }
            Err(e) => {
                return Err(VerifyError::CommitVerifyFailed {
                    commit_cid: cursor.to_string(),
                    rev: commit.rev.clone(),
                    key_used: key.to_string(),
                    reason: e.to_string(),
                })
            }
        }
        keys_used.insert(key);
        commits_verified += 1;

        match &commit.prev {
            // Reached genesis.
            None => break,
            // Defensive (design §3.4 edge 5): the prev block isn't in this set —
            // verify what's present, stop. (Unreachable for the rebuild consumer,
            // whose block set is the full accumulated history.)
            Some(prev) if blocks.get(prev).is_none() => break,
            Some(prev) => cursor = prev.clone(),
        }
    }

    // Per-commit signatures are verified; reuse verify_repo for the structural
    // VerifiedRepo (DID check + MST load) WITHOUT re-checking the head sig
    // (signing_did_key = None skips it — we already checked it against the
    // history key above).
    let repo = verify_repo(blocks, root, expected_did, None)?;

    Ok(HistoryVerifyOutcome {
        repo,
        commits_verified,
        keys_used: keys_used.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use proto_blue::crypto::{K256Keypair, Keypair as _};
    use proto_blue::lex_cbor::cid_for_lex;
    use proto_blue::lex_data::LexValue;
    use proto_blue::repo::commit::{sign_commit, UnsignedCommit};
    use proto_blue::repo::MstNode;

    fn entry(signing_did_key: &str, accepted_at_micros: i64) -> PlcOpHistoryEntry {
        PlcOpHistoryEntry {
            op_cid: format!("cid-{accepted_at_micros}"),
            accepted_at: DateTime::<Utc>::from_timestamp_micros(accepted_at_micros).unwrap(),
            signing_did_key: signing_did_key.to_string(),
        }
    }

    // ---- resolve_key_at_rev (pure, design §3.4) ----

    #[test]
    fn resolve_key_single_entry_any_rev_after_start() {
        let h = [entry("did:key:zK1", 1000)];
        assert_eq!(resolve_key_at_rev(&h, 5000), Some("did:key:zK1"));
        assert_eq!(resolve_key_at_rev(&h, 1000), Some("did:key:zK1")); // at start
    }

    #[test]
    fn resolve_key_multi_entry_picks_containing_window() {
        let h = [entry("did:key:zK1", 1000), entry("did:key:zK2", 2000), entry("did:key:zK3", 3000)];
        assert_eq!(resolve_key_at_rev(&h, 1500), Some("did:key:zK1"));
        assert_eq!(resolve_key_at_rev(&h, 2500), Some("did:key:zK2"));
        assert_eq!(resolve_key_at_rev(&h, 9999), Some("did:key:zK3")); // open-ended last
    }

    #[test]
    fn resolve_key_exact_boundary_takes_new_window() {
        // Half-open: a rev exactly at K2's accepted_at falls into K2 (edge 2).
        let h = [entry("did:key:zK1", 1000), entry("did:key:zK2", 2000)];
        assert_eq!(resolve_key_at_rev(&h, 2000), Some("did:key:zK2"));
    }

    #[test]
    fn resolve_key_before_genesis_is_none() {
        let h = [entry("did:key:zK1", 1000), entry("did:key:zK2", 2000)];
        assert_eq!(resolve_key_at_rev(&h, 500), None); // edge 1
    }

    #[test]
    fn resolve_key_empty_history_is_none() {
        assert_eq!(resolve_key_at_rev(&[], 5000), None);
    }

    // ---- verify_repo_with_history (chain walk) ----

    /// Build a commit signed by `kp` at TID timestamp `rev_micros`, with `prev`,
    /// add it (and its empty MST root) to `blocks`, return its CID.
    fn add_commit(
        blocks: &mut BlockMap,
        did: &str,
        kp: &K256Keypair,
        rev_micros: u64,
        prev: Option<Cid>,
    ) -> Cid {
        // Distinct empty-MST root per commit so head's `data` resolves.
        let marker = LexValue::String(format!("mst-{rev_micros}"));
        let rec_cid = cid_for_lex(&marker).unwrap();
        let mst = MstNode::empty().add("app.test.rec/k", rec_cid).unwrap();
        let (mst_root, mst_blocks) = mst.get_all_blocks().unwrap();
        blocks.add_map(&mst_blocks);
        let rev = Tid::from_timestamp(rev_micros, 0).as_str().to_string();
        let unsigned = UnsignedCommit::new(did.to_string(), mst_root, rev, prev);
        let signed = sign_commit(&unsigned, kp).unwrap();
        blocks.add_value(&signed.to_lex_value()).unwrap()
    }

    #[test]
    fn verify_history_empty_history_errors() {
        let blocks = BlockMap::new();
        let root = cid_for_lex(&LexValue::String("x".into())).unwrap();
        let err = verify_repo_with_history(blocks, &root, Some("did:plc:a"), &[]).unwrap_err();
        assert!(matches!(err, VerifyError::EmptyHistory { .. }));
    }

    #[test]
    fn verify_history_multi_key_chain_verifies() {
        let did = "did:plc:multi";
        let k1 = K256Keypair::generate();
        let k2 = K256Keypair::generate();
        let mut blocks = BlockMap::new();
        // K1 window [1_000_000, 2_000_000); K2 window [2_000_000, ∞).
        let c1 = add_commit(&mut blocks, did, &k1, 1_500_000, None);
        let c2 = add_commit(&mut blocks, did, &k2, 2_500_000, Some(c1.clone()));
        let history = [entry(&k1.did(), 1_000_000), entry(&k2.did(), 2_000_000)];

        let out = verify_repo_with_history(blocks, &c2, Some(did), &history).expect("verifies");
        assert_eq!(out.commits_verified, 2);
        assert_eq!(out.keys_used, 2);
    }

    #[test]
    fn verify_history_wrong_key_window_fails() {
        // Sign commit2 with K1 but place it in K2's window → must fail.
        let did = "did:plc:bad";
        let k1 = K256Keypair::generate();
        let k2 = K256Keypair::generate();
        let mut blocks = BlockMap::new();
        let c1 = add_commit(&mut blocks, did, &k1, 1_500_000, None);
        let c2 = add_commit(&mut blocks, did, &k1, 2_500_000, Some(c1.clone())); // wrong: K1 in K2 window
        let history = [entry(&k1.did(), 1_000_000), entry(&k2.did(), 2_000_000)];

        let err = verify_repo_with_history(blocks, &c2, Some(did), &history).unwrap_err();
        match err {
            VerifyError::CommitVerifyFailed { rev, .. } => {
                assert_eq!(rev, Tid::from_timestamp(2_500_000, 0).as_str());
            }
            other => panic!("expected CommitVerifyFailed, got {other:?}"),
        }
    }

    #[test]
    fn verify_history_absent_prev_terminates_cleanly() {
        // Head's prev points at a CID not in `blocks`: walk verifies the head and
        // stops (design §3.4 edge 5), not an error.
        let did = "did:plc:partial";
        let k2 = K256Keypair::generate();
        let mut blocks = BlockMap::new();
        let dangling = cid_for_lex(&LexValue::String("absent-prev".into())).unwrap();
        let head = add_commit(&mut blocks, did, &k2, 2_500_000, Some(dangling));
        let history = [entry("did:key:zK1old", 1_000_000), entry(&k2.did(), 2_000_000)];

        let out = verify_repo_with_history(blocks, &head, Some(did), &history)
            .expect("partial-history head verifies and terminates");
        assert_eq!(out.commits_verified, 1, "only the present head was walked");
    }

    #[test]
    fn verify_history_commit_before_genesis() {
        let did = "did:plc:early";
        let k1 = K256Keypair::generate();
        let k2 = K256Keypair::generate();
        let mut blocks = BlockMap::new();
        // Head rev BEFORE the genesis op accepted_at.
        let head = add_commit(&mut blocks, did, &k1, 500_000, None);
        let history = [entry(&k1.did(), 1_000_000), entry(&k2.did(), 2_000_000)];

        let err = verify_repo_with_history(blocks, &head, Some(did), &history).unwrap_err();
        assert!(matches!(err, VerifyError::CommitBeforeGenesis { .. }));
    }
}
