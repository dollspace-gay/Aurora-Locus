//! CAR (Content Addressable aRchive) Export Utilities
//!
//! Wraps `proto_blue::repo::car` with the (cid_string, bytes) shape this
//! crate uses for blocks coming out of the SQLite blockstore.

use crate::{
    actor_store::ActorStore,
    error::{PdsError, PdsResult},
};
use proto_blue::lex_data::Cid;
use proto_blue::repo::{block_map::BlockMap, car as pb_car, error::RepoError};
use std::str::FromStr;

/// Build a `BlockMap` from the (cid_string, bytes) pairs emitted by
/// `ActorStore::get_all_blocks` etc.
fn blocks_to_block_map(blocks: Vec<(String, Vec<u8>)>) -> PdsResult<BlockMap> {
    let mut map = BlockMap::new();
    for (cid_str, data) in blocks {
        let cid = Cid::from_str(&cid_str)
            .map_err(|e| PdsError::Internal(format!("Invalid block CID {}: {}", cid_str, e)))?;
        map.set(cid, data);
    }
    Ok(map)
}

/// Map a proto-blue `RepoError` from the CAR layer onto `PdsError`.
fn repo_err(err: RepoError) -> PdsError {
    PdsError::Internal(format!("CAR export error: {}", err))
}

/// Export every block in a repository as a single CAR file.
///
/// The CAR's root is the current commit CID for this DID.
pub async fn export_repo_to_car(
    store: &ActorStore,
    did: &str,
    since: Option<&str>,
) -> PdsResult<Vec<u8>> {
    let repo_root = store.get_repo_root(did).await?;
    let root_cid = Cid::from_str(&repo_root.cid)
        .map_err(|e| PdsError::Internal(format!("Invalid root CID: {}", e)))?;

    if since.is_some() {
        // Incremental export not yet implemented — fall through to a full export
        // and warn so operators can see when this path is hit.
        tracing::warn!(
            "Incremental CAR export (since={:?}) not yet implemented, exporting full repo",
            since
        );
    }

    let blocks = store.get_all_blocks(did).await?;
    let block_map = blocks_to_block_map(blocks)?;

    pb_car::blocks_to_car(Some(&root_cid), &block_map).map_err(repo_err)
}

/// Pack an arbitrary set of (cid, bytes) blocks as a CAR file with an
/// optional root CID.
pub async fn blocks_to_car(
    blocks: Vec<(String, Vec<u8>)>,
    root: Option<&str>,
) -> PdsResult<Vec<u8>> {
    let root_cid = match root {
        Some(r) => Some(
            Cid::from_str(r).map_err(|e| PdsError::Internal(format!("Invalid root CID: {}", e)))?,
        ),
        None => None,
    };

    let block_map = blocks_to_block_map(blocks)?;
    pb_car::blocks_to_car(root_cid.as_ref(), &block_map).map_err(repo_err)
}

/// Export a single record as a CAR file.
///
/// Currently emits the full repo's blocks alongside the record CID — the
/// tradeoff is that consumers can verify the proof chain back to the
/// signed commit, at the cost of redundant bytes when only one record was
/// requested. A trimmed proof-set export is a follow-up optimisation.
pub async fn export_record_to_car(
    store: &ActorStore,
    did: &str,
    collection: &str,
    rkey: &str,
) -> PdsResult<Vec<u8>> {
    let uri = format!("at://{}/{}/{}", did, collection, rkey);
    let record = store
        .get_record(did, &uri)
        .await?
        .ok_or_else(|| PdsError::NotFound(format!("Record not found: {}", uri)))?;

    // Validate that the recorded CID is well-formed even though we don't pass
    // it to the CAR encoder directly — corrupt metadata is worth surfacing
    // here rather than silently exporting a broken archive.
    Cid::from_str(&record.cid)
        .map_err(|e| PdsError::Internal(format!("Invalid record CID: {}", e)))?;

    let repo_root = store.get_repo_root(did).await?;
    let root_cid = Cid::from_str(&repo_root.cid)
        .map_err(|e| PdsError::Internal(format!("Invalid root CID: {}", e)))?;

    let blocks = store.get_all_blocks(did).await?;
    let block_map = blocks_to_block_map(blocks)?;

    pb_car::blocks_to_car(Some(&root_cid), &block_map).map_err(repo_err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor_store::ActorStoreConfig;
    use tempfile::tempdir;

    #[allow(dead_code)] // Helper function for future tests
    async fn create_test_store() -> ActorStore {
        let temp_dir = tempdir().unwrap();
        let config = ActorStoreConfig {
            base_directory: temp_dir.path().to_path_buf(),
            cache_size: 10,
        };
        ActorStore::new(config)
    }

    #[tokio::test]
    async fn test_blocks_to_car_round_trips_via_proto_blue() {
        // Build two blocks with hash-correct CIDs so read_car (which verifies)
        // is happy.
        let payload1 = b"test block 1".to_vec();
        let payload2 = b"test block 2".to_vec();
        let cid1 = Cid::for_raw(&payload1);
        let cid2 = Cid::for_raw(&payload2);

        let blocks = vec![
            (cid1.to_string(), payload1.clone()),
            (cid2.to_string(), payload2.clone()),
        ];

        let car_bytes = blocks_to_car(blocks, None).await.unwrap();
        assert!(car_bytes.len() > 16);

        let (roots, decoded) = pb_car::read_car(&car_bytes).unwrap();
        assert!(roots.is_empty());
        assert_eq!(decoded.get(&cid1).unwrap(), &payload1[..]);
        assert_eq!(decoded.get(&cid2).unwrap(), &payload2[..]);
    }

    #[tokio::test]
    async fn test_blocks_to_car_with_root() {
        let payload = b"root block".to_vec();
        let cid = Cid::for_raw(&payload);
        let cid_str = cid.to_string();

        let blocks = vec![(cid_str.clone(), payload.clone())];
        let car_bytes = blocks_to_car(blocks, Some(&cid_str)).await.unwrap();

        let (roots, decoded) = pb_car::read_car(&car_bytes).unwrap();
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].to_string(), cid_str);
        assert_eq!(decoded.get(&cid).unwrap(), &payload[..]);
    }

    #[tokio::test]
    async fn test_invalid_cid_error() {
        let invalid_block = ("invalid-cid".to_string(), b"data".to_vec());
        let result = blocks_to_car(vec![invalid_block], None).await;

        assert!(result.is_err());
        match result {
            Err(PdsError::Internal(msg)) => {
                assert!(msg.contains("Invalid block CID"));
            }
            _ => panic!("Expected Internal error"),
        }
    }
}
