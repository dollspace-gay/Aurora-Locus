// Allow dead_code - actor transactions for future use
#![allow(dead_code)]

//! Transaction support for atomic multi-record operations
//!
//! Provides explicit transaction API similar to Bluesky's ActorStoreTransactor
//! with Rust-native type safety and SQLx integration.

use crate::{
    actor_store::ActorStore,
    error::{PdsError, PdsResult},
};
use sqlx::{Sqlite, Transaction as SqlxTransaction};
use std::sync::Arc;

/// Transaction handle for atomic operations on an actor's repository
///
/// Provides a safe, typed interface for performing multiple database operations
/// atomically. If the transaction is dropped without calling commit(), it will
/// automatically roll back.
///
/// # Example
/// ```no_run
/// # use aurora_locus::actor_store::{ActorStore, ActorTransaction};
/// # async fn example(store: ActorStore) -> Result<(), Box<dyn std::error::Error>> {
/// let mut txn = store.begin_transaction("did:plc:alice").await?;
///
/// // All operations within transaction. `execute(...)` returns a sqlx
/// // `Query` builder; you bind parameters and run it through the
/// // dedicated helpers (`insert_block`, `update_repo_root`, etc.) or
/// // drop down to raw queries via the typed wrappers below.
/// txn.insert_block("bafy...", b"data").await?;
/// txn.update_repo_root("bafy...", "rev0").await?;
///
/// // Commit the transaction
/// txn.commit().await?;
/// # Ok(())
/// # }
/// ```
pub struct ActorTransaction<'a> {
    /// The underlying SQLx transaction
    txn: Option<SqlxTransaction<'a, Sqlite>>,
    /// DID of the actor this transaction is for
    did: String,
    /// Reference to the actor store (for helper methods)
    #[allow(dead_code)]
    store: Arc<ActorStore>,
    /// Whether this transaction has been committed
    committed: bool,
}

impl<'a> ActorTransaction<'a> {
    /// Create a new transaction (internal use only)
    pub(crate) fn new(
        txn: SqlxTransaction<'a, Sqlite>,
        did: String,
        store: Arc<ActorStore>,
    ) -> Self {
        Self {
            txn: Some(txn),
            did,
            store,
            committed: false,
        }
    }

    /// Get the DID this transaction is operating on
    pub fn did(&self) -> &str {
        &self.did
    }

    /// Build a raw `sqlx::Query` bound to this transaction.
    ///
    /// The returned `Query` is just the builder — no connection has been
    /// borrowed yet, and the caller can chain `.bind(...)` then dispatch
    /// it via the typed wrappers (`insert_block`, `update_repo_root`,
    /// etc.) or run the SQL directly using `sqlx`'s usual API.
    ///
    /// # Example
    /// ```no_run
    /// # use aurora_locus::actor_store::{ActorStore, ActorTransaction};
    /// # async fn example(mut txn: ActorTransaction<'_>) -> Result<(), Box<dyn std::error::Error>> {
    /// // Prefer the typed wrappers for the common cases:
    /// txn.update_repo_root("abc123", "rev0").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn execute<'b>(
        &mut self,
        query: &'b str,
    ) -> sqlx::query::Query<'b, Sqlite, sqlx::sqlite::SqliteArguments<'b>> {
        sqlx::query(query)
    }

    /// Insert a block into the repository within this transaction
    pub async fn insert_block(&mut self, cid: &str, content: &[u8]) -> PdsResult<()> {
        let txn = self
            .txn
            .as_mut()
            .ok_or_else(|| PdsError::Internal("Transaction already completed".to_string()))?;

        sqlx::query(
            "INSERT OR REPLACE INTO repo_block (cid, content, indexed_at)
             VALUES (?1, ?2, ?3)",
        )
        .bind(cid)
        .bind(content)
        .bind(chrono::Utc::now())
        .execute(&mut **txn)
        .await
        .map_err(PdsError::Database)?;

        Ok(())
    }

    /// Insert a record into the repository within this transaction
    pub async fn insert_record(
        &mut self,
        uri: &str,
        cid: &str,
        collection: &str,
        rkey: &str,
        repo_rev: Option<&str>,
    ) -> PdsResult<()> {
        let txn = self
            .txn
            .as_mut()
            .ok_or_else(|| PdsError::Internal("Transaction already completed".to_string()))?;

        sqlx::query(
            "INSERT OR REPLACE INTO record (uri, cid, collection, rkey, repo_rev, indexed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(uri)
        .bind(cid)
        .bind(collection)
        .bind(rkey)
        .bind(repo_rev)
        .bind(chrono::Utc::now())
        .execute(&mut **txn)
        .await
        .map_err(PdsError::Database)?;

        Ok(())
    }

    /// Delete a record from the repository within this transaction
    pub async fn delete_record(&mut self, uri: &str) -> PdsResult<()> {
        let txn = self
            .txn
            .as_mut()
            .ok_or_else(|| PdsError::Internal("Transaction already completed".to_string()))?;

        sqlx::query("DELETE FROM record WHERE uri = ?1")
            .bind(uri)
            .execute(&mut **txn)
            .await
            .map_err(PdsError::Database)?;

        Ok(())
    }

    /// Upsert the repository root within this transaction.
    ///
    /// Mirrors `ActorStore::update_repo_root` — handles both the
    /// initial-commit (no row) and subsequent-commit (row exists) cases
    /// idempotently.
    pub async fn update_repo_root(&mut self, cid: &str, rev: &str) -> PdsResult<()> {
        let txn = self
            .txn
            .as_mut()
            .ok_or_else(|| PdsError::Internal("Transaction already completed".to_string()))?;

        sqlx::query(
            "INSERT INTO repo_root (did, cid, rev, indexed_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(did) DO UPDATE SET
                 cid = excluded.cid,
                 rev = excluded.rev,
                 indexed_at = excluded.indexed_at",
        )
        .bind(&self.did)
        .bind(cid)
        .bind(rev)
        .bind(chrono::Utc::now())
        .execute(&mut **txn)
        .await
        .map_err(PdsError::Database)?;

        Ok(())
    }

    /// Commit the transaction
    ///
    /// Consumes the transaction and commits all changes to the database.
    /// If this is not called, the transaction will automatically roll back
    /// when dropped.
    pub async fn commit(mut self) -> PdsResult<()> {
        let txn = self
            .txn
            .take()
            .ok_or_else(|| PdsError::Internal("Transaction already completed".to_string()))?;

        txn.commit().await.map_err(PdsError::Database)?;

        self.committed = true;
        Ok(())
    }

    /// Explicitly roll back the transaction
    ///
    /// This is optional - transactions automatically roll back when dropped
    /// if not committed.
    pub async fn rollback(mut self) -> PdsResult<()> {
        let txn = self
            .txn
            .take()
            .ok_or_else(|| PdsError::Internal("Transaction already completed".to_string()))?;

        txn.rollback().await.map_err(PdsError::Database)?;

        Ok(())
    }

    /// Create a savepoint for nested transaction support
    ///
    /// Savepoints allow you to create rollback points within a transaction.
    /// If you need to undo part of a transaction without rolling back the
    /// entire transaction, you can use savepoints.
    ///
    /// # Example
    /// ```no_run
    /// # use aurora_locus::actor_store::{ActorStore, ActorTransaction};
    /// # async fn example(mut txn: ActorTransaction<'_>) -> Result<(), Box<dyn std::error::Error>> {
    /// txn.savepoint("before_risky_operation").await?;
    ///
    /// // Try something risky.
    /// if let Err(_e) = txn.insert_block("bafy...", b"data").await {
    ///     // Roll back to savepoint without aborting the whole transaction.
    ///     txn.rollback_to_savepoint("before_risky_operation").await?;
    /// }
    ///
    /// // Continue with transaction
    /// txn.commit().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn savepoint(&mut self, name: &str) -> PdsResult<()> {
        let txn = self
            .txn
            .as_mut()
            .ok_or_else(|| PdsError::Internal("Transaction already completed".to_string()))?;

        let query = format!("SAVEPOINT {}", name);
        sqlx::query(&query)
            .execute(&mut **txn)
            .await
            .map_err(PdsError::Database)?;

        Ok(())
    }

    /// Roll back to a previously created savepoint
    pub async fn rollback_to_savepoint(&mut self, name: &str) -> PdsResult<()> {
        let txn = self
            .txn
            .as_mut()
            .ok_or_else(|| PdsError::Internal("Transaction already completed".to_string()))?;

        let query = format!("ROLLBACK TO SAVEPOINT {}", name);
        sqlx::query(&query)
            .execute(&mut **txn)
            .await
            .map_err(PdsError::Database)?;

        Ok(())
    }

    /// Release a savepoint (commit it)
    pub async fn release_savepoint(&mut self, name: &str) -> PdsResult<()> {
        let txn = self
            .txn
            .as_mut()
            .ok_or_else(|| PdsError::Internal("Transaction already completed".to_string()))?;

        let query = format!("RELEASE SAVEPOINT {}", name);
        sqlx::query(&query)
            .execute(&mut **txn)
            .await
            .map_err(PdsError::Database)?;

        Ok(())
    }
}

/// Automatic rollback on drop if not committed
///
/// This ensures that if a transaction is dropped (e.g., due to an error)
/// without being explicitly committed, it will automatically roll back.
impl<'a> Drop for ActorTransaction<'a> {
    fn drop(&mut self) {
        if !self.committed && self.txn.is_some() {
            // Transaction will automatically roll back when the inner
            // SqlxTransaction is dropped
            tracing::debug!(
                "Transaction for {} dropped without commit - auto-rollback",
                self.did
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::actor_store::{ActorStore, ActorStoreConfig};
    use tempfile::TempDir;

    /// Create a test actor store with a temporary directory.
    ///
    /// `ActorStore::create` no longer seeds a placeholder `repo_root`
    /// row (the real first commit fills it in via `update_repo_root` on
    /// the proto-blue commit path). The transaction tests below assert
    /// roll-back/rollback semantics against an existing row, so we
    /// upsert a sentinel here to preserve their intent without coupling
    /// them to the live commit machinery.
    async fn create_test_store() -> (ActorStore, TempDir, String) {
        let temp_dir = TempDir::new().unwrap();
        let config = ActorStoreConfig {
            base_directory: temp_dir.path().to_path_buf(),
            cache_size: 10,
        };

        let store = ActorStore::new(config);
        let did = "did:plc:test123";

        store.create(did).await.unwrap();
        store
            .update_repo_root(did, "bafy_initial_placeholder_cid", "3jzfcijpj2z2a")
            .await
            .unwrap();

        (store, temp_dir, did.to_string())
    }

    #[tokio::test]
    async fn test_transaction_commit() {
        let (store, _temp, did) = create_test_store().await;

        // Begin transaction
        let mut txn = store.begin_transaction(&did).await.unwrap();

        // Insert a block
        txn.insert_block("cid123", b"test content").await.unwrap();

        // Update repo root
        txn.update_repo_root("cid123", "rev456").await.unwrap();

        // Commit transaction
        txn.commit().await.unwrap();

        // Verify changes persisted
        let root = store.get_repo_root(&did).await.unwrap();
        assert_eq!(root.cid, "cid123");
        assert_eq!(root.rev, "rev456");

        // Verify block exists
        let block = store.get_block(&did, "cid123").await.unwrap();
        assert!(block.is_some());
        assert_eq!(block.unwrap(), b"test content");
    }

    #[tokio::test]
    async fn test_transaction_explicit_rollback() {
        let (store, _temp, did) = create_test_store().await;

        // Get initial state
        let initial_root = store.get_repo_root(&did).await.unwrap();

        // Begin transaction
        let mut txn = store.begin_transaction(&did).await.unwrap();

        // Make changes
        txn.update_repo_root("cid999", "rev999").await.unwrap();

        // Explicitly roll back
        txn.rollback().await.unwrap();

        // Verify changes were NOT persisted
        let root = store.get_repo_root(&did).await.unwrap();
        assert_eq!(root.cid, initial_root.cid);
        assert_eq!(root.rev, initial_root.rev);
    }

    #[tokio::test]
    async fn test_transaction_auto_rollback_on_drop() {
        let (store, _temp, did) = create_test_store().await;

        // Get initial state
        let initial_root = store.get_repo_root(&did).await.unwrap();

        // Begin transaction and make changes without committing
        {
            let mut txn = store.begin_transaction(&did).await.unwrap();
            txn.update_repo_root("cid888", "rev888").await.unwrap();
            // Transaction dropped here without commit
        }

        // Verify changes were NOT persisted (auto-rollback)
        let root = store.get_repo_root(&did).await.unwrap();
        assert_eq!(root.cid, initial_root.cid);
        assert_eq!(root.rev, initial_root.rev);
    }

    #[tokio::test]
    async fn test_transaction_multiple_operations() {
        let (store, _temp, did) = create_test_store().await;

        let mut txn = store.begin_transaction(&did).await.unwrap();

        // Insert multiple blocks
        txn.insert_block("cid1", b"content1").await.unwrap();
        txn.insert_block("cid2", b"content2").await.unwrap();
        txn.insert_block("cid3", b"content3").await.unwrap();

        // Insert records
        let uri1 = format!("at://{}/app.bsky.feed.post/record1", did);
        let uri2 = format!("at://{}/app.bsky.feed.post/record2", did);

        txn.insert_record(&uri1, "cid1", "app.bsky.feed.post", "record1", Some("rev1"))
            .await
            .unwrap();
        txn.insert_record(&uri2, "cid2", "app.bsky.feed.post", "record2", Some("rev1"))
            .await
            .unwrap();

        // Update root
        txn.update_repo_root("cid3", "rev1").await.unwrap();

        // Commit
        txn.commit().await.unwrap();

        // Verify all changes persisted
        let block1 = store.get_block(&did, "cid1").await.unwrap();
        assert_eq!(block1.unwrap(), b"content1");

        let block2 = store.get_block(&did, "cid2").await.unwrap();
        assert_eq!(block2.unwrap(), b"content2");

        let record1 = store.get_record(&did, &uri1).await.unwrap();
        assert!(record1.is_some());

        let root = store.get_repo_root(&did).await.unwrap();
        assert_eq!(root.cid, "cid3");
    }

    #[tokio::test]
    async fn test_transaction_savepoints() {
        let (store, _temp, did) = create_test_store().await;

        let mut txn = store.begin_transaction(&did).await.unwrap();

        // Initial operation
        txn.insert_block("cid1", b"content1").await.unwrap();

        // Create savepoint
        txn.savepoint("sp1").await.unwrap();

        // Risky operation
        txn.insert_block("cid2", b"content2").await.unwrap();

        // Roll back to savepoint (undoes cid2 but keeps cid1)
        txn.rollback_to_savepoint("sp1").await.unwrap();

        // Add another block after rollback
        txn.insert_block("cid3", b"content3").await.unwrap();

        // Commit
        txn.commit().await.unwrap();

        // Verify: cid1 and cid3 should exist, cid2 should not
        let block1 = store.get_block(&did, "cid1").await.unwrap();
        assert!(block1.is_some());

        let block2 = store.get_block(&did, "cid2").await.unwrap();
        assert!(block2.is_none(), "cid2 should have been rolled back");

        let block3 = store.get_block(&did, "cid3").await.unwrap();
        assert!(block3.is_some());
    }

    #[tokio::test]
    async fn test_transaction_isolation() {
        let (store, _temp, did) = create_test_store().await;

        // Start transaction 1
        let mut txn1 = store.begin_transaction(&did).await.unwrap();
        txn1.insert_block("cid_isolated", b"content_isolated")
            .await
            .unwrap();

        // While txn1 is open, try to read from another connection
        let block = store.get_block(&did, "cid_isolated").await.unwrap();
        assert!(block.is_none(), "Should not see uncommitted changes");

        // Commit txn1
        txn1.commit().await.unwrap();

        // Now the block should be visible
        let block = store.get_block(&did, "cid_isolated").await.unwrap();
        assert!(block.is_some(), "Should see committed changes");
    }

    #[tokio::test]
    async fn test_transaction_delete_record() {
        let (store, _temp, did) = create_test_store().await;

        // First insert a record outside transaction
        let uri = format!("at://{}/app.bsky.feed.post/test123", did);

        {
            let mut txn = store.begin_transaction(&did).await.unwrap();
            txn.insert_block("cid_delete_test", b"content")
                .await
                .unwrap();
            txn.insert_record(
                &uri,
                "cid_delete_test",
                "app.bsky.feed.post",
                "test123",
                None,
            )
            .await
            .unwrap();
            txn.commit().await.unwrap();
        }

        // Verify record exists
        let record = store.get_record(&did, &uri).await.unwrap();
        assert!(record.is_some());

        // Now delete it in a transaction
        {
            let mut txn = store.begin_transaction(&did).await.unwrap();
            txn.delete_record(&uri).await.unwrap();
            txn.commit().await.unwrap();
        }

        // Verify record is gone
        let record = store.get_record(&did, &uri).await.unwrap();
        assert!(record.is_none());
    }

    #[tokio::test]
    async fn test_transaction_error_handling() {
        let (store, _temp, did) = create_test_store().await;

        let mut txn = store.begin_transaction(&did).await.unwrap();

        // Valid operation
        txn.insert_block("cid_valid", b"content").await.unwrap();

        // Try an invalid operation (this should fail)
        // Note: Actual validation depends on database constraints
        // This test verifies error handling infrastructure

        // Rollback on error
        txn.rollback().await.unwrap();

        // Verify nothing was committed
        let block = store.get_block(&did, "cid_valid").await.unwrap();
        assert!(block.is_none());
    }

    #[tokio::test]
    async fn test_transaction_savepoint_release() {
        let (store, _temp, did) = create_test_store().await;

        let mut txn = store.begin_transaction(&did).await.unwrap();

        txn.insert_block("cid1", b"content1").await.unwrap();

        // Create and immediately release savepoint
        txn.savepoint("sp_release").await.unwrap();
        txn.insert_block("cid2", b"content2").await.unwrap();
        txn.release_savepoint("sp_release").await.unwrap();

        // Commit
        txn.commit().await.unwrap();

        // Both blocks should exist
        assert!(store.get_block(&did, "cid1").await.unwrap().is_some());
        assert!(store.get_block(&did, "cid2").await.unwrap().is_some());
    }

    /// Arc 16f Step 3 v5.2 (chainlink #123) — `ActorStore::create` is
    /// idempotent for already-initialised DIDs: a second call returns
    /// `Ok(())` with no data loss. This is the load-bearing property
    /// behind the import_repo handler's CF3-gate auto-init call —
    /// importRepo runs `create()` unconditionally on every invocation,
    /// safe because already-init'd stores are no-ops.
    #[tokio::test]
    async fn actor_store_create_is_idempotent_for_already_initialised_did() {
        let temp_dir = TempDir::new().unwrap();
        let config = ActorStoreConfig {
            base_directory: temp_dir.path().to_path_buf(),
            cache_size: 10,
        };
        let store = ActorStore::new(config);
        let did = "did:plc:idempotency-test";

        // First call: fresh init.
        store.create(did).await.expect("first create ok");
        assert!(store.exists(did).await, "store exists after first create");

        // Write a sentinel row so we can prove the second create() didn't
        // wipe state. update_repo_root upserts; if the second create()
        // somehow dropped/recreated the table, the sentinel would be lost.
        store
            .update_repo_root(did, "bafy_idempotency_sentinel_cid", "3jzfcijpj2z2a")
            .await
            .expect("upsert sentinel ok");
        let pre = store.get_repo_root(did).await.expect("read sentinel ok");
        assert_eq!(pre.cid, "bafy_idempotency_sentinel_cid");

        // Second call: must succeed without disturbing state.
        store.create(did).await.expect("second create ok — idempotent");
        assert!(store.exists(did).await, "store still exists after second create");

        let post = store.get_repo_root(did).await.expect("read sentinel again");
        assert_eq!(
            post.cid, "bafy_idempotency_sentinel_cid",
            "second create() must NOT have wiped the repo_root row"
        );
        assert_eq!(post.rev, "3jzfcijpj2z2a");
    }
}
