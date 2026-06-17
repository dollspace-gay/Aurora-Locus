//! Repository rebuild — reconstruction + verification (Arc H §7.4.1 / #289).
//!
//! Reconstructs an account's canonical repo state by replaying its sequencer
//! history: walk the commit events ascending, fold each `#commit` event's CAR
//! block slice into one accumulating [`BlockMap`], then verify that the head
//! commit and its MST resolve via proto-blue's [`verify_repo`] — the substrate
//! primitive purpose-built for this ("used when the blocks came from a stream
//! of firehose commit events"). `verify_repo` loads the head commit, checks the
//! DID (and, when a key is supplied, the signature), then loads the MST from the
//! block map, tolerating extra/dead blocks (only the reachable closure must be
//! present) — so accumulating every delta and rooting at the head yields the
//! canonical current state.
//!
//! This module is **non-destructive**: it only reads the sequencer and
//! reconstructs in memory. The atomic in-memory→swap that replaces the live
//! repo with the reconstructed state is #290.

use crate::error::{PdsError, PdsResult};
use crate::sequencer::Sequencer;
use proto_blue::lex_data::Cid;
use proto_blue::repo::car as pb_car;
use proto_blue::repo::{verify_repo, BlockMap, VerifiedRepo};
use std::str::FromStr;

/// Reconstruct `did`'s canonical repo from its full sequencer history and
/// verify it resolves. Returns the [`VerifiedRepo`] (canonical reconstructed
/// state: head commit + MST + the accumulated block set), or `None` when the
/// account has no commit history (nothing to rebuild).
///
/// `signing_did_key`: `None` runs structural verification only (DID match + MST
/// resolution) — what the non-destructive preflight needs; #290's actual
/// rebuild passes the account's `did:key` for full signature verification.
///
/// Errors when a CAR slice fails to decode or `verify_repo` rejects the
/// assembled state (a missing reachable block, MST inconsistency, DID/signature
/// mismatch) — i.e. replay would NOT produce a coherent repo. The caller
/// surfaces that as the preflight's diagnostic; nothing is mutated either way.
pub async fn reconstruct_and_verify(
    sequencer: &Sequencer,
    did: &str,
    signing_did_key: Option<&str>,
) -> PdsResult<Option<VerifiedRepo>> {
    let mut blocks = BlockMap::new();
    let mut head_commit_cid: Option<String> = None;
    let mut cursor = 0i64;

    loop {
        let (events, last_seq) = sequencer.commit_events_after(did, cursor, None).await?;
        match last_seq {
            None => break, // end of history
            Some(s) => cursor = s,
        }
        for (seq, evt) in events {
            head_commit_cid = Some(evt.commit.clone());
            let (_roots, delta) = pb_car::read_car(&evt.blocks).map_err(|e| {
                PdsError::InvalidCar(format!("rebuild: CAR decode failed at seq {}: {}", seq, e))
            })?;
            blocks.add_map(&delta);
        }
    }

    let Some(head) = head_commit_cid else {
        return Ok(None); // no commit events → nothing to rebuild
    };
    let root = Cid::from_str(&head).map_err(|e| {
        PdsError::Internal(format!("rebuild: malformed head commit CID '{}': {}", head, e))
    })?;
    let verified = verify_repo(blocks, &root, Some(did), signing_did_key).map_err(|e| {
        PdsError::Internal(format!(
            "rebuild: reconstructed repo for {} failed verification: {}",
            did, e
        ))
    })?;
    Ok(Some(verified))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proto_blue::crypto::{Keypair, P256Keypair};
    use proto_blue::lex_cbor::cid_for_lex;
    use proto_blue::lex_data::LexValue;
    use proto_blue::repo::commit::{sign_commit, UnsignedCommit};
    use proto_blue::repo::{blocks_to_car, BlockMap, MstNode};
    use crate::sequencer::{events::CommitEvent, Sequencer, SequencerConfig};

    async fn test_sequencer() -> Sequencer {
        use std::sync::Once;
        static INSTALL: Once = Once::new();
        INSTALL.call_once(sqlx::any::install_default_drivers);
        let db = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE repo_seq (seq INTEGER PRIMARY KEY AUTOINCREMENT, did TEXT NOT NULL, \
             event_type TEXT NOT NULL, event BLOB NOT NULL, invalidated INTEGER NOT NULL DEFAULT 0, \
             sequenced_at TEXT NOT NULL)",
        )
        .execute(&db)
        .await
        .unwrap();
        Sequencer::new(db, SequencerConfig::default())
    }

    /// Build a real one-commit repo: an MST with one record, a signed commit,
    /// serialized to a CAR. Returns (did, did:key, head_commit_cid, car_bytes).
    fn build_repo_car(rev: &str) -> (String, String, String, Vec<u8>) {
        let kp = P256Keypair::generate();
        let did = kp.did().replace("did:key:", "did:plc:");
        let mut blocks = BlockMap::new();
        let value = LexValue::String("hello".to_string());
        let rec_cid = cid_for_lex(&value).unwrap();
        blocks.add_value(&value).unwrap();
        let mst = MstNode::empty().add("app.bsky.feed.post/abc", rec_cid).unwrap();
        let (mst_root, mst_blocks) = mst.get_all_blocks().unwrap();
        blocks.add_map(&mst_blocks);
        let unsigned = UnsignedCommit::new(did.clone(), mst_root, rev.to_string(), None);
        let signed = sign_commit(&unsigned, &kp).unwrap();
        let commit_cid = blocks.add_value(&signed.to_lex_value()).unwrap();
        let car = blocks_to_car(Some(&commit_cid), &blocks).unwrap();
        (did, kp.did(), commit_cid.to_string(), car)
    }

    #[tokio::test]
    async fn reconstruct_verifies_a_real_repo() {
        let seq = test_sequencer().await;
        let (did, did_key, head_cid, car) = build_repo_car("3jzfcijpj2z2a");
        seq.sequence_commit(CommitEvent::new(
            did.clone(),
            head_cid.clone(),
            "3jzfcijpj2z2a".to_string(),
            None,
            None,
            car,
            vec![],
        ))
        .await
        .unwrap();

        // Full verification (with the signing key) reconstructs the canonical repo.
        let verified = reconstruct_and_verify(&seq, &did, Some(&did_key))
            .await
            .unwrap()
            .expect("history present");
        assert_eq!(
            verified.commit_cid.to_string(),
            head_cid,
            "reconstructed head matches the sequenced commit"
        );
        assert_eq!(verified.commit.did, did);
    }

    #[tokio::test]
    async fn reconstruct_none_for_unknown_account() {
        let seq = test_sequencer().await;
        assert!(reconstruct_and_verify(&seq, "did:plc:nobody", None)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn reconstruct_errors_on_incomplete_history() {
        // A commit event whose CAR omits the MST/record blocks (head commit CID
        // points at blocks not present) → verify_repo can't resolve → error.
        // Models a corrupt/incomplete history the preflight must flag.
        let seq = test_sequencer().await;
        let did = "did:plc:broken";
        // A real CID (so it parses) whose block is NOT in the CAR → MissingBlock.
        let absent = cid_for_lex(&LexValue::String("absent-head".to_string())).unwrap();
        seq.sequence_commit(CommitEvent::new(
            did.to_string(),
            absent.to_string(),
            "3jzfcijpj2z2a".to_string(),
            None,
            None,
            // empty CAR — no blocks
            blocks_to_car(None, &BlockMap::new()).unwrap(),
            vec![],
        ))
        .await
        .unwrap();
        assert!(
            reconstruct_and_verify(&seq, did, None).await.is_err(),
            "incomplete history (head block absent) must fail verification"
        );
    }
}
