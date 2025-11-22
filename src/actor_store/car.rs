//! CAR (Content Addressable aRchive) Export Utilities
//!
//! This module provides functions for exporting repository data to CAR format,
//! which is used by ATProto sync protocol endpoints.
//!
//! Uses the `atproto::car` module from the SDK for CAR file generation.

use crate::{
    actor_store::ActorStore,
    error::{PdsError, PdsResult},
};
use atproto::car::{CarWriter, CarError};
use libipld::cid::Cid;
use std::str::FromStr;

/// Export all repository blocks as CAR bytes
///
/// This creates a complete CAR file containing all blocks from a repository.
/// The root CID is set to the current repo root.
///
/// # Arguments
///
/// * `store` - The actor store
/// * `did` - The DID of the repository
/// * `since` - Optional revision to export incrementally (not yet implemented)
///
/// # Returns
///
/// CAR file as bytes
pub async fn export_repo_to_car(
    store: &ActorStore,
    did: &str,
    since: Option<&str>,
) -> PdsResult<Vec<u8>> {
    // Get repo root for CAR header
    let repo_root = store.get_repo_root(did).await?;

    // Parse root CID
    let root_cid = Cid::from_str(&repo_root.cid)
        .map_err(|e| PdsError::Internal(format!("Invalid root CID: {}", e)))?;

    // Get all blocks
    let blocks = if since.is_some() {
        // TODO: Implement incremental export
        // For now, just export everything
        tracing::warn!("Incremental CAR export (since={:?}) not yet implemented, exporting full repo", since);
        store.get_all_blocks(did).await?
    } else {
        store.get_all_blocks(did).await?
    };

    // Create CAR writer with root CID
    let mut car_writer = CarWriter::with_roots(Vec::new(), vec![root_cid]);

    // Write all blocks to CAR
    for (cid_str, block_data) in blocks {
        let cid = Cid::from_str(&cid_str)
            .map_err(|e| PdsError::Internal(format!("Invalid block CID {}: {}", cid_str, e)))?;

        car_writer
            .write_block(&cid, &block_data)
            .map_err(car_error_to_pds_error)?;
    }

    // Finish and get bytes
    let car_bytes = car_writer
        .finish()
        .map_err(car_error_to_pds_error)?;

    Ok(car_bytes)
}

/// Export specific blocks as CAR bytes
///
/// # Arguments
///
/// * `blocks` - List of (CID string, block data) tuples
/// * `root` - Optional root CID string (if None, no root is set)
///
/// # Returns
///
/// CAR file as bytes
pub async fn blocks_to_car(
    blocks: Vec<(String, Vec<u8>)>,
    root: Option<&str>,
) -> PdsResult<Vec<u8>> {
    // Parse root CID if provided
    let roots = if let Some(root_str) = root {
        let root_cid = Cid::from_str(root_str)
            .map_err(|e| PdsError::Internal(format!("Invalid root CID: {}", e)))?;
        vec![root_cid]
    } else {
        vec![]
    };

    // Create CAR writer
    let mut car_writer = CarWriter::with_roots(Vec::new(), roots);

    // Write all blocks to CAR
    for (cid_str, block_data) in blocks {
        let cid = Cid::from_str(&cid_str)
            .map_err(|e| PdsError::Internal(format!("Invalid block CID {}: {}", cid_str, e)))?;

        car_writer
            .write_block(&cid, &block_data)
            .map_err(car_error_to_pds_error)?;
    }

    // Finish and get bytes
    let car_bytes = car_writer
        .finish()
        .map_err(car_error_to_pds_error)?;

    Ok(car_bytes)
}

/// Export a single record as CAR bytes
///
/// This exports the blocks needed to reconstruct a specific record.
/// Currently exports all repo blocks (TODO: optimize to only export record-specific blocks).
///
/// # Arguments
///
/// * `store` - The actor store
/// * `did` - The DID of the repository
/// * `collection` - The record collection
/// * `rkey` - The record key
///
/// # Returns
///
/// CAR file as bytes containing the record
pub async fn export_record_to_car(
    store: &ActorStore,
    did: &str,
    collection: &str,
    rkey: &str,
) -> PdsResult<Vec<u8>> {
    // Get record to verify it exists
    let uri = format!("at://{}/{}/{}", did, collection, rkey);
    let record = store
        .get_record(did, &uri)
        .await?
        .ok_or_else(|| PdsError::NotFound(format!("Record not found: {}", uri)))?;

    // Parse record CID (validated but not used - record struct is sufficient)
    let _record_cid = Cid::from_str(&record.cid)
        .map_err(|e| PdsError::Internal(format!("Invalid record CID: {}", e)))?;

    // Get repo root
    let repo_root = store.get_repo_root(did).await?;
    let root_cid = Cid::from_str(&repo_root.cid)
        .map_err(|e| PdsError::Internal(format!("Invalid root CID: {}", e)))?;

    // Create CAR writer with root CID
    let mut car_writer = CarWriter::with_roots(Vec::new(), vec![root_cid]);

    // TODO: Optimize this to only export blocks needed for this specific record
    // For now, export all blocks (this is safe but inefficient)
    let blocks = store.get_all_blocks(did).await?;

    for (cid_str, block_data) in blocks {
        let cid = Cid::from_str(&cid_str)
            .map_err(|e| PdsError::Internal(format!("Invalid block CID {}: {}", cid_str, e)))?;

        car_writer
            .write_block(&cid, &block_data)
            .map_err(car_error_to_pds_error)?;
    }

    // Finish and get bytes
    let car_bytes = car_writer
        .finish()
        .map_err(car_error_to_pds_error)?;

    Ok(car_bytes)
}

/// Convert CarError to PdsError
fn car_error_to_pds_error(err: CarError) -> PdsError {
    PdsError::Internal(format!("CAR export error: {}", err))
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
    async fn test_blocks_to_car() {
        // Create test blocks
        let block1 = (
            "bafyreihk5ztsfapt6g2cnxbxgbxb7dltipq5pufb4jtwmqrxrxqaygceyq".to_string(),
            b"test block 1".to_vec(),
        );
        let block2 = (
            "bafyreia5vwvxmrwdjvg2otlawfdzqrkjq3gkcl7t56krvsvfundhwmhwru".to_string(),
            b"test block 2".to_vec(),
        );

        let blocks = vec![block1, block2];

        // Export to CAR
        let car_bytes = blocks_to_car(blocks, None).await.unwrap();

        // Verify CAR file is not empty
        assert!(!car_bytes.is_empty());
        assert!(car_bytes.len() > 50); // Should have header + 2 blocks
    }

    #[tokio::test]
    async fn test_blocks_to_car_with_root() {
        let root = "bafyreihk5ztsfapt6g2cnxbxgbxb7dltipq5pufb4jtwmqrxrxqaygceyq";
        let block1 = (
            root.to_string(),
            b"root block".to_vec(),
        );

        let blocks = vec![block1];

        // Export to CAR with root
        let car_bytes = blocks_to_car(blocks, Some(root)).await.unwrap();

        // Verify CAR file is not empty
        assert!(!car_bytes.is_empty());

        // Read back using SDK's CarReader to verify
        use atproto::car::CarReader;
        let reader = CarReader::new(&car_bytes[..]).unwrap();

        // Verify root is set
        assert_eq!(reader.roots().len(), 1);
        assert_eq!(reader.roots()[0].to_string(), root);
    }

    #[tokio::test]
    async fn test_invalid_cid_error() {
        let invalid_block = ("invalid-cid".to_string(), b"data".to_vec());
        let result = blocks_to_car(vec![invalid_block], None).await;

        // Should error with invalid CID
        assert!(result.is_err());
        match result {
            Err(PdsError::Internal(msg)) => {
                assert!(msg.contains("Invalid block CID"));
            }
            _ => panic!("Expected Internal error"),
        }
    }
}
