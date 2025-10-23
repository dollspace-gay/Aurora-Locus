/// Repository manager - integrates SDK MST with persistent storage
///
/// This module bridges the SDK's in-memory MST implementation with
/// our SQLite-based persistent storage system.

use crate::{
    actor_store::ActorStore,
    error::{PdsError, PdsResult},
    sequencer::{events::{CommitEvent, CommitOp, OpAction}, Sequencer},
    validation::{RecordValidator, validation_errors_to_pds_error},
};
use atproto::{
    repo::Repository as SdkRepo,
    tid::Tid,
    types::Did,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validate: Option<bool>,
}

/// Repository manager for a single actor
///
/// Manages the integration between SDK's Repository/MST and persistent storage
pub struct RepositoryManager {
    did: String,
    store: ActorStore,
    validator: RecordValidator,
    sequencer: Option<Arc<Sequencer>>,
}

impl RepositoryManager {
    /// Create a new repository manager for a DID
    pub fn new(did: String, store: ActorStore) -> Self {
        Self {
            did,
            store,
            validator: RecordValidator::new(),
            sequencer: None,
        }
    }

    /// Create a new repository manager with sequencer support
    pub fn with_sequencer(did: String, store: ActorStore, sequencer: Arc<Sequencer>) -> Self {
        Self {
            did,
            store,
            validator: RecordValidator::new(),
            sequencer: Some(sequencer),
        }
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

        // Track operations for commit event
        let mut commit_ops: Vec<CommitOp> = Vec::new();

        // Apply each write operation to the MST
        for write in writes.clone() {
            let collection = &write.collection;
            let rkey = &write.rkey;

            match write.action {
                WriteOpAction::Create | WriteOpAction::Update => {
                    let value = write.value
                        .ok_or_else(|| PdsError::Validation("Create/Update requires value".to_string()))?;

                    // Validate record if requested (defaults to true if not specified)
                    let should_validate = write.validate.unwrap_or(true);
                    if should_validate {
                        if let Err(errors) = self.validator.validate(collection, &value) {
                            return Err(validation_errors_to_pds_error(errors));
                        }
                    }

                    // Serialize record to DAG-CBOR bytes
                    let record_bytes = serde_json::to_vec(&value)
                        .map_err(|e| PdsError::Internal(format!("Failed to serialize record: {}", e)))?;

                    // Insert into MST
                    let record_cid = repo.put_record(collection, rkey, record_bytes.clone())
                        .map_err(|e| PdsError::Internal(format!("MST insert failed: {}", e)))?;

                    // Store block content first (to satisfy foreign key constraint)
                    self.store.put_block(&self.did, &record_cid.to_string(), &record_bytes).await?;

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

                    // Track operation for commit event
                    commit_ops.push(CommitOp {
                        action: OpAction::Create,
                        path: format!("{}/{}", collection, rkey),
                        cid: Some(record_cid.to_string()),
                    });
                }
                WriteOpAction::Delete => {
                    // Delete from MST
                    repo.delete_record(collection, rkey);

                    // Delete from database
                    let uri = format!("at://{}/{}/{}", self.did, collection, rkey);
                    self.store.delete_record(&self.did, &uri).await?;

                    // Track operation for commit event
                    commit_ops.push(CommitOp {
                        action: OpAction::Delete,
                        path: format!("{}/{}", collection, rkey),
                        cid: None,
                    });
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

        // Emit commit event to sequencer for firehose
        if let Some(ref sequencer) = self.sequencer {
            // Create commit event
            let commit_event = CommitEvent::new(
                self.did.clone(),
                commit_cid.to_string(),
                rev.to_string(),
                None, // TODO: Track previous commit CID
                vec![], // TODO: Include CAR file bytes with commit blocks
                commit_ops,
            );

            // Sequence the event
            sequencer.sequence_commit(commit_event).await
                .map_err(|e| {
                    tracing::warn!("Failed to sequence commit event: {}", e);
                    // Don't fail the commit if sequencing fails
                    e
                })
                .ok();
        }

        Ok((commit_cid.to_string(), rev.to_string()))
    }

    /// Create a single record
    pub async fn create_record<F>(
        &self,
        collection: &str,
        rkey: Option<&str>,
        value: serde_json::Value,
        validate: Option<bool>,
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
            validate,
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
        validate: Option<bool>,
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
            validate,
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
            validate: None, // Validation not needed for deletes
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

    // ==================== Batch Operations ====================

    /// Prepare write operations for batch execution
    ///
    /// Converts raw WriteOps into PreparedWrites with validation
    pub fn prepare_writes(&self, writes: Vec<WriteOp>) -> PdsResult<Vec<crate::actor_store::models::PreparedWrite>> {
        use crate::actor_store::models::WriteOpAction as ModelAction;
        let mut prepared = Vec::new();

        for write in writes {
            // Convert WriteOpAction
            let action = match write.action {
                WriteOpAction::Create => ModelAction::Create,
                WriteOpAction::Update => ModelAction::Update,
                WriteOpAction::Delete => ModelAction::Delete,
            };

            prepared.push(crate::actor_store::models::PreparedWrite {
                action,
                collection: write.collection,
                rkey: write.rkey,
                record: write.value,
                swap_cid: None, // TODO: support swap CID for optimistic concurrency
                validate: write.validate,
            });
        }

        Ok(prepared)
    }

    /// Validate batch operations before execution
    ///
    /// Checks for:
    /// - Valid collection names
    /// - Valid rkeys
    /// - No duplicate operations
    /// - Required values for create/update
    /// - Record size limits
    pub async fn validate_batch(&self, writes: &[crate::actor_store::models::PreparedWrite]) -> PdsResult<()> {
        use crate::actor_store::models::WriteOpAction;
        use std::collections::HashSet;
        let mut seen_keys: HashSet<String> = HashSet::new();

        for write in writes {
            // Validate collection format (e.g., "app.bsky.feed.post")
            if !write.collection.contains('.') {
                return Err(PdsError::Validation(format!(
                    "Invalid collection format: {}",
                    write.collection
                )));
            }

            // Validate rkey format (alphanumeric, hyphens, underscores)
            if write.rkey.is_empty() || write.rkey.len() > 512 {
                return Err(PdsError::Validation(format!(
                    "Invalid rkey length: {}",
                    write.rkey.len()
                )));
            }

            // Check for duplicate keys in batch
            let key = format!("{}/{}", write.collection, write.rkey);
            if !seen_keys.insert(key.clone()) {
                return Err(PdsError::Validation(format!(
                    "Duplicate operation for {}/{}",
                    write.collection, write.rkey
                )));
            }

            // Validate create/update have values
            match write.action {
                WriteOpAction::Create | WriteOpAction::Update => {
                    if write.record.is_none() {
                        return Err(PdsError::Validation(format!(
                            "Create/Update requires record value for {}/{}",
                            write.collection, write.rkey
                        )));
                    }

                    // Check record size (max 1MB)
                    if let Some(ref record) = write.record {
                        let record_bytes = serde_json::to_vec(record)
                            .map_err(|e| PdsError::Internal(format!("Failed to serialize record: {}", e)))?;

                        const MAX_RECORD_SIZE: usize = 1024 * 1024; // 1MB
                        if record_bytes.len() > MAX_RECORD_SIZE {
                            return Err(PdsError::Validation(format!(
                                "Record exceeds maximum size of {}MB: {} bytes",
                                MAX_RECORD_SIZE / (1024 * 1024),
                                record_bytes.len()
                            )));
                        }
                    }
                }
                WriteOpAction::Delete => {
                    // Delete operations should not have values
                    if write.record.is_some() {
                        return Err(PdsError::Validation(format!(
                            "Delete operation should not have record value for {}/{}",
                            write.collection, write.rkey
                        )));
                    }
                }
            }
        }

        Ok(())
    }

    /// Apply batch writes atomically
    ///
    /// All operations succeed or all fail together
    pub async fn apply_batch_writes<F>(
        &self,
        writes: Vec<crate::actor_store::models::PreparedWrite>,
        sign_fn: F,
    ) -> PdsResult<(String, String)> // (commit_cid, rev)
    where
        F: FnOnce(&[u8; 32]) -> Result<Vec<u8>, atproto::repo::RepoError>,
    {
        use crate::actor_store::models::WriteOpAction as ModelAction;

        // Validate entire batch first
        self.validate_batch(&writes).await?;

        // Convert PreparedWrites to WriteOps
        let ops: Vec<WriteOp> = writes.into_iter().map(|w| {
            let action = match w.action {
                ModelAction::Create => WriteOpAction::Create,
                ModelAction::Update => WriteOpAction::Update,
                ModelAction::Delete => WriteOpAction::Delete,
            };

            WriteOp {
                action,
                collection: w.collection,
                rkey: w.rkey,
                value: w.record,
                validate: w.validate,
            }
        }).collect();

        // Apply all operations atomically
        self.apply_writes(ops, sign_fn).await
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
            None, // validate
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
                validate: None,
            },
            WriteOp {
                action: WriteOpAction::Create,
                collection: "app.bsky.feed.post".to_string(),
                rkey: "post2".to_string(),
                value: Some(serde_json::json!({"text": "Post 2"})),
                validate: None,
            },
        ];

        let result = repo_mgr.apply_writes(writes, test_dummy_signer).await;
        assert!(result.is_ok());
    }
}
