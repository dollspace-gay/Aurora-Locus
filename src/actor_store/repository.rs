/// Repository manager - integrates SDK MST with persistent storage
///
/// This module bridges the SDK's in-memory MST implementation with
/// our SQLite-based persistent storage system.

use crate::{
    actor_store::ActorStore,
    error::{PdsError, PdsResult},
};
use atproto::{
    repo::Repository as SdkRepo,
    tid::Tid,
    types::Did,
};
use serde::{Deserialize, Serialize};

/// Write operation action
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WriteOpAction {
    Create,
    Update,
    Delete,
}

/// Write operation for applyWrites
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteOp {
    pub action: WriteOpAction,
    pub collection: String,
    pub rkey: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
}

/// Repository manager for a single actor
///
/// Manages the integration between SDK's Repository/MST and persistent storage
pub struct RepositoryManager {
    did: String,
    store: ActorStore,
}

impl RepositoryManager {
    /// Create a new repository manager for a DID
    pub fn new(did: String, store: ActorStore) -> Self {
        Self { did, store }
    }

    /// Initialize a new repository for an actor
    pub async fn initialize(&self) -> PdsResult<()> {
        // Create the actor's database and directory structure
        self.store.create(&self.did).await?;
        Ok(())
    }

    /// Load the current repository state from storage
    ///
    /// This reconstructs the in-memory MST from stored blocks
    pub async fn load_repo(&self) -> PdsResult<SdkRepo> {
        let did = Did::new(&self.did)
            .map_err(|e| PdsError::Validation(format!("Invalid DID: {}", e)))?;

        // Check if repository exists
        if !self.store.exists(&self.did).await {
            return Err(PdsError::NotFound(format!("Repository not found for {}", self.did)));
        }

        // Get current repo root
        let repo_root = self.store.get_repo_root(&self.did).await?;

        // For now, create an empty SDK repo
        // TODO: Load MST blocks from repo_block table and reconstruct tree
        let repo = SdkRepo::create(did);

        Ok(repo)
    }

    /// Apply write operations and create a new commit
    ///
    /// # Arguments
    ///
    /// * `writes` - List of write operations to apply
    /// * `sign_fn` - Function to sign the commit
    ///
    /// # Returns
    ///
    /// Returns the new commit CID and revision TID
    pub async fn apply_writes<F>(
        &self,
        writes: Vec<WriteOp>,
        sign_fn: F,
    ) -> PdsResult<(String, String)>
    where
        F: FnOnce(&[u8; 32]) -> Result<Vec<u8>, atproto::repo::RepoError>,
    {
        // Load current repository state
        let mut repo = self.load_repo().await?;

        // Apply each write operation to the MST
        for write in writes {
            let collection = &write.collection;
            let rkey = &write.rkey;

            match write.action {
                WriteOpAction::Create | WriteOpAction::Update => {
                    let value = write.value
                        .ok_or_else(|| PdsError::Validation("Create/Update requires value".to_string()))?;

                    // Serialize record to DAG-CBOR bytes
                    let record_bytes = serde_json::to_vec(&value)
                        .map_err(|e| PdsError::Internal(format!("Failed to serialize record: {}", e)))?;

                    // Insert into MST
                    let record_cid = repo.put_record(collection, rkey, record_bytes)
                        .map_err(|e| PdsError::Internal(format!("MST insert failed: {}", e)))?;

                    // Store record metadata in database
                    let uri = format!("at://{}/{}/{}", self.did, collection, rkey);
                    let new_rev = Tid::next()
                        .map_err(|e| PdsError::Internal(format!("Failed to generate TID: {}", e)))?;

                    self.store.put_record(
                        &self.did,
                        &uri,
                        &record_cid.to_string(),
                        collection,
                        rkey,
                        &new_rev.to_string(),
                    ).await?;
                }
                WriteOpAction::Delete => {
                    // Delete from MST
                    repo.delete_record(collection, rkey);

                    // Delete from database
                    let uri = format!("at://{}/{}/{}", self.did, collection, rkey);
                    self.store.delete_record(&self.did, &uri).await?;
                }
            }
        }

        // Create signed commit
        let commit_cid = repo.commit(sign_fn)
            .map_err(|e| PdsError::Internal(format!("Commit creation failed: {}", e)))?;

        let rev = repo.rev()
            .ok_or_else(|| PdsError::Internal("No revision after commit".to_string()))?;

        // Store commit blocks to database
        // TODO: Extract blocks from MST and store in repo_block table
        // For now, just update the repo_root
        self.store.update_repo_root(
            &self.did,
            &commit_cid.to_string(),
            &rev.to_string(),
        ).await?;

        Ok((commit_cid.to_string(), rev.to_string()))
    }

    /// Create a single record
    pub async fn create_record<F>(
        &self,
        collection: &str,
        rkey: Option<&str>,
        value: serde_json::Value,
        sign_fn: F,
    ) -> PdsResult<(String, String, String)> // (uri, cid, rev)
    where
        F: FnOnce(&[u8; 32]) -> Result<Vec<u8>, atproto::repo::RepoError>,
    {
        // Generate rkey if not provided
        let rkey = match rkey {
            Some(k) => k.to_string(),
            None => {
                let tid = Tid::next()
                    .map_err(|e| PdsError::Internal(format!("Failed to generate TID: {}", e)))?;
                tid.to_string()
            }
        };

        // Apply as a single write operation
        let writes = vec![WriteOp {
            action: WriteOpAction::Create,
            collection: collection.to_string(),
            rkey: rkey.clone(),
            value: Some(value),
        }];

        let (commit_cid, rev) = self.apply_writes(writes, sign_fn).await?;

        let uri = format!("at://{}/{}/{}", self.did, collection, rkey);
        Ok((uri, commit_cid, rev))
    }

    /// Update a record
    pub async fn update_record<F>(
        &self,
        collection: &str,
        rkey: &str,
        value: serde_json::Value,
        sign_fn: F,
    ) -> PdsResult<(String, String)> // (cid, rev)
    where
        F: FnOnce(&[u8; 32]) -> Result<Vec<u8>, atproto::repo::RepoError>,
    {
        let writes = vec![WriteOp {
            action: WriteOpAction::Update,
            collection: collection.to_string(),
            rkey: rkey.to_string(),
            value: Some(value),
        }];

        self.apply_writes(writes, sign_fn).await
    }

    /// Delete a record
    pub async fn delete_record<F>(
        &self,
        collection: &str,
        rkey: &str,
        sign_fn: F,
    ) -> PdsResult<(String, String)> // (cid, rev)
    where
        F: FnOnce(&[u8; 32]) -> Result<Vec<u8>, atproto::repo::RepoError>,
    {
        let writes = vec![WriteOp {
            action: WriteOpAction::Delete,
            collection: collection.to_string(),
            rkey: rkey.to_string(),
            value: None,
        }];

        self.apply_writes(writes, sign_fn).await
    }

    /// Get a record by AT-URI
    pub async fn get_record(&self, uri: &str) -> PdsResult<Option<serde_json::Value>> {
        // Get record metadata from database
        let record = self.store.get_record(&self.did, uri).await?;

        if let Some(rec) = record {
            // TODO: Load actual record content from MST blocks
            // For now, return a placeholder indicating the record exists
            Ok(Some(serde_json::json!({
                "uri": rec.uri,
                "cid": rec.cid,
                "value": null // TODO: deserialize from blocks
            })))
        } else {
            Ok(None)
        }
    }

    /// List records in a collection
    pub async fn list_records(
        &self,
        collection: &str,
        limit: i64,
        cursor: Option<&str>,
    ) -> PdsResult<Vec<serde_json::Value>> {
        let records = self.store.list_records(&self.did, collection, limit, cursor).await?;

        // Convert to JSON array
        let results = records
            .into_iter()
            .map(|rec| {
                serde_json::json!({
                    "uri": rec.uri,
                    "cid": rec.cid,
                    "value": null // TODO: load from blocks
                })
            })
            .collect();

        Ok(results)
    }

    /// Get repository description
    pub async fn describe_repo(&self) -> PdsResult<serde_json::Value> {
        let repo_root = self.store.get_repo_root(&self.did).await?;

        Ok(serde_json::json!({
            "did": self.did,
            "handle": "unknown", // TODO: resolve from DID
            "didDoc": null, // TODO: fetch DID document
            "collections": [], // TODO: enumerate collections
            "handleIsCorrect": true,
        }))
    }

    /// Export repository to CAR file
    pub async fn export_car(&self, since: Option<&str>) -> PdsResult<Vec<u8>> {
        let repo = self.load_repo().await?;

        // Use SDK's CAR export
        repo.export_car()
            .map_err(|e| PdsError::Internal(format!("CAR export failed: {}", e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor_store::ActorStoreConfig;
    use std::path::PathBuf;

    fn test_store() -> ActorStore {
        let config = ActorStoreConfig {
            base_directory: PathBuf::from("./test_data/repos"),
            cache_size: 10,
        };
        ActorStore::new(config)
    }

    fn test_dummy_signer(_hash: &[u8; 32]) -> Result<Vec<u8>, atproto::repo::RepoError> {
        // Return a dummy 64-byte signature for testing
        Ok(vec![0u8; 64])
    }

    #[tokio::test]
    async fn test_repository_initialization() {
        let store = test_store();
        let repo_mgr = RepositoryManager::new("did:plc:test123".to_string(), store);

        let result = repo_mgr.initialize().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_create_record() {
        let store = test_store();
        let repo_mgr = RepositoryManager::new("did:plc:test456".to_string(), store);

        repo_mgr.initialize().await.unwrap();

        let value = serde_json::json!({
            "text": "Hello, ATProto!",
            "createdAt": "2025-01-01T00:00:00Z"
        });

        let result = repo_mgr.create_record(
            "app.bsky.feed.post",
            None,
            value,
            test_dummy_signer,
        ).await;

        assert!(result.is_ok());
        let (uri, cid, rev) = result.unwrap();
        assert!(uri.starts_with("at://did:plc:test456/app.bsky.feed.post/"));
        assert!(!cid.is_empty());
        assert!(!rev.is_empty());
    }

    #[tokio::test]
    async fn test_apply_writes() {
        let store = test_store();
        let repo_mgr = RepositoryManager::new("did:plc:test789".to_string(), store);

        repo_mgr.initialize().await.unwrap();

        let writes = vec![
            WriteOp {
                action: WriteOpAction::Create,
                collection: "app.bsky.feed.post".to_string(),
                rkey: "post1".to_string(),
                value: Some(serde_json::json!({"text": "Post 1"})),
            },
            WriteOp {
                action: WriteOpAction::Create,
                collection: "app.bsky.feed.post".to_string(),
                rkey: "post2".to_string(),
                value: Some(serde_json::json!({"text": "Post 2"})),
            },
        ];

        let result = repo_mgr.apply_writes(writes, test_dummy_signer).await;
        assert!(result.is_ok());
    }
}
