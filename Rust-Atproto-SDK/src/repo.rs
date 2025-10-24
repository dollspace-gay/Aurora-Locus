//! Repository Manager for ATProto
//!
//! This module implements repository management for ATProto, including:
//! - Commit creation and signing
//! - Repository versioning
//! - MST integration
//! - CAR export
//!
//! # Repository Structure
//!
//! An ATProto repository consists of:
//! - **Signed Commit**: Top-level object containing metadata and signature
//! - **MST Root**: Merkle Search Tree containing all records
//! - **Records**: Individual data objects (posts, profiles, etc.)
//!
//! # Commit Object Format
//!
//! ```json
//! {
//!   "did": "did:plc:...",
//!   "version": 3,
//!   "data": <CID>,      // MST root
//!   "rev": "...",       // TID revision
//!   "prev": <CID|null>, // Previous commit
//!   "sig": <bytes>      // Signature
//! }
//! ```
//!
//! # Example
//!
//! ```no_run
//! use atproto::repo::Repository;
//! use atproto::types::Did;
//!
//! // Create a new repository
//! // let did = Did::new("did:plc:example").unwrap();
//! // let mut repo = Repository::create(did);
//!
//! // Add a record
//! // repo.put_record("app.bsky.feed.post", "key1", record_data)?;
//!
//! // Create a commit
//! // let commit_cid = repo.commit(signing_key)?;
//! ```

use crate::car::CarWriter;
use crate::mst::{Mst, MstError};
use crate::tid::Tid;
use crate::types::Did;
use libipld::cid::Cid;
use libipld::codec::Codec;
use libipld::Ipld;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use thiserror::Error;

/// Error types for repository operations
#[derive(Error, Debug)]
pub enum RepoError {
    #[error("MST error: {0}")]
    Mst(#[from] MstError),

    #[error("Invalid DID: {0}")]
    InvalidDid(String),

    #[error("Invalid collection: {0}")]
    InvalidCollection(String),

    #[error("Record not found: {0}")]
    RecordNotFound(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Signing error: {0}")]
    Signing(String),

    #[error("CID error: {0}")]
    Cid(String),

    #[error("CAR error: {0}")]
    Car(String),

    #[error("Invalid commit: {0}")]
    InvalidCommit(String),
}

/// Result type for repository operations
pub type Result<T> = std::result::Result<T, RepoError>;

/// Repository version (always 3 for current spec)
const REPO_VERSION: u32 = 3;

/// Unsigned commit object (for signing)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnsignedCommit {
    /// DID of the repository owner
    pub did: String,

    /// Repository format version (always 3)
    pub version: u32,

    /// CID of the MST root
    pub data: Cid,

    /// Revision identifier (TID)
    pub rev: String,

    /// Previous commit CID (nullable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prev: Option<Cid>,
}

impl UnsignedCommit {
    /// Convert to IPLD representation
    fn to_ipld(&self) -> Ipld {
        let mut map = BTreeMap::new();
        map.insert("did".to_string(), Ipld::String(self.did.clone()));
        map.insert("version".to_string(), Ipld::Integer(self.version as i128));
        map.insert("data".to_string(), Ipld::Link(self.data));
        map.insert("rev".to_string(), Ipld::String(self.rev.clone()));
        if let Some(ref prev) = self.prev {
            map.insert("prev".to_string(), Ipld::Link(*prev));
        } else {
            map.insert("prev".to_string(), Ipld::Null);
        }
        Ipld::Map(map)
    }

    /// Serialize to DAG-CBOR bytes
    pub fn to_cbor(&self) -> Result<Vec<u8>> {
        let ipld = self.to_ipld();
        libipld_cbor::DagCborCodec.encode(&ipld)
            .map_err(|e| RepoError::Serialization(format!("Failed to encode commit: {}", e)))
    }

    /// Calculate the signing hash (SHA-256 of DAG-CBOR bytes)
    pub fn signing_hash(&self) -> Result<[u8; 32]> {
        let cbor_bytes = self.to_cbor()?;
        let hash = Sha256::digest(&cbor_bytes);
        Ok(hash.into())
    }
}

/// Signed commit object
#[derive(Debug, Clone)]
pub struct SignedCommit {
    /// The unsigned commit data
    pub commit: UnsignedCommit,

    /// Signature bytes
    pub sig: Vec<u8>,
}

impl SignedCommit {
    /// Convert to IPLD representation
    fn to_ipld(&self) -> Ipld {
        let mut map = BTreeMap::new();
        map.insert("did".to_string(), Ipld::String(self.commit.did.clone()));
        map.insert("version".to_string(), Ipld::Integer(self.commit.version as i128));
        map.insert("data".to_string(), Ipld::Link(self.commit.data));
        map.insert("rev".to_string(), Ipld::String(self.commit.rev.clone()));
        if let Some(ref prev) = self.commit.prev {
            map.insert("prev".to_string(), Ipld::Link(*prev));
        } else {
            map.insert("prev".to_string(), Ipld::Null);
        }
        map.insert("sig".to_string(), Ipld::Bytes(self.sig.clone()));
        Ipld::Map(map)
    }

    /// Serialize to DAG-CBOR bytes
    pub fn to_cbor(&self) -> Result<Vec<u8>> {
        let ipld = self.to_ipld();
        libipld_cbor::DagCborCodec.encode(&ipld)
            .map_err(|e| RepoError::Serialization(format!("Failed to encode signed commit: {}", e)))
    }

    /// Calculate the CID of this commit
    pub fn to_cid(&self) -> Result<Cid> {
        let cbor_bytes = self.to_cbor()?;
        let hash = Sha256::digest(&cbor_bytes);

        let multihash = libipld::multihash::Multihash::wrap(0x12, &hash)
            .map_err(|e| RepoError::Cid(format!("Failed to create multihash: {}", e)))?;

        Ok(Cid::new_v1(0x71, multihash)) // dag-cbor codec
    }
}

/// Repository manager
///
/// Manages a user's ATProto repository including records, commits, and versioning.
pub struct Repository {
    /// DID of the repository owner
    did: Did,

    /// Current MST state
    mst: Mst,

    /// Commit history (CID -> SignedCommit)
    commits: HashMap<Cid, SignedCommit>,

    /// Current commit CID (head)
    head: Option<Cid>,

    /// Current revision (TID)
    current_rev: Option<Tid>,
}

impl Repository {
    /// Create a new repository for a DID
    ///
    /// # Arguments
    ///
    /// * `did` - The DID of the repository owner
    ///
    /// # Example
    ///
    /// ```
    /// use atproto::repo::Repository;
    /// use atproto::types::Did;
    ///
    /// let did = Did::new("did:plc:example123").unwrap();
    /// let repo = Repository::create(did);
    /// ```
    pub fn create(did: Did) -> Self {
        Self {
            did,
            mst: Mst::new(),
            commits: HashMap::new(),
            head: None,
            current_rev: None,
        }
    }

    /// Get the repository DID
    pub fn did(&self) -> &Did {
        &self.did
    }

    /// Get the current head commit CID
    pub fn head(&self) -> Option<&Cid> {
        self.head.as_ref()
    }

    /// Get the current revision
    pub fn rev(&self) -> Option<&Tid> {
        self.current_rev.as_ref()
    }

    /// Put a record into the repository
    ///
    /// # Arguments
    ///
    /// * `collection` - Collection NSID (e.g., "app.bsky.feed.post")
    /// * `rkey` - Record key
    /// * `record` - Record data as DAG-CBOR bytes
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use atproto::repo::Repository;
    /// # use atproto::types::Did;
    /// # let mut repo = Repository::create(Did::new("did:plc:test").unwrap());
    /// repo.put_record(
    ///     "app.bsky.feed.post",
    ///     "3jxyz",
    ///     br#"{"text":"Hello world"}"#.to_vec()
    /// ).unwrap();
    /// ```
    pub fn put_record(&mut self, collection: &str, rkey: &str, record: Vec<u8>) -> Result<Cid> {
        // Validate collection NSID
        if collection.is_empty() || !collection.contains('.') {
            return Err(RepoError::InvalidCollection(collection.to_string()));
        }

        // Create the record path: collection/rkey
        let path = format!("{}/{}", collection, rkey);

        // Insert into MST
        let cid = self.mst.insert(path, record)?;

        Ok(cid)
    }

    /// Get a record from the repository
    ///
    /// # Arguments
    ///
    /// * `collection` - Collection NSID
    /// * `rkey` - Record key
    pub fn get_record(&self, collection: &str, rkey: &str) -> Option<&[u8]> {
        let path = format!("{}/{}", collection, rkey);
        self.mst.get(&path)
    }

    /// Delete a record from the repository
    ///
    /// # Arguments
    ///
    /// * `collection` - Collection NSID
    /// * `rkey` - Record key
    pub fn delete_record(&mut self, collection: &str, rkey: &str) -> Option<Vec<u8>> {
        let path = format!("{}/{}", collection, rkey);
        self.mst.delete(&path)
    }

    /// List all record keys in a collection
    ///
    /// # Arguments
    ///
    /// * `collection` - Collection NSID
    pub fn list_records(&self, collection: &str) -> Vec<String> {
        let prefix = format!("{}/", collection);
        self.mst
            .list_keys()
            .into_iter()
            .filter(|k| k.starts_with(&prefix))
            .map(|k| k.strip_prefix(&prefix).unwrap().to_string())
            .collect()
    }

    /// Create a signed commit
    ///
    /// # Arguments
    ///
    /// * `sign_fn` - Function to sign the commit hash
    ///
    /// # Returns
    ///
    /// Returns the CID of the new commit
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use atproto::repo::Repository;
    /// # use atproto::types::Did;
    /// # let mut repo = Repository::create(Did::new("did:plc:test").unwrap());
    /// let commit_cid = repo.commit(|hash| {
    ///     // Sign the hash with your signing key
    ///     Ok(vec![0u8; 64]) // Dummy signature
    /// }).unwrap();
    /// ```
    pub fn commit<F>(&mut self, sign_fn: F) -> Result<Cid>
    where
        F: FnOnce(&[u8; 32]) -> Result<Vec<u8>>,
    {
        // Get MST root CID
        let data_cid = self.mst.root_cid()?;

        // Generate new revision (TID)
        let new_rev = Tid::next()
            .map_err(|e| RepoError::Signing(format!("Failed to generate TID: {}", e)))?;

        // Create unsigned commit
        let unsigned_commit = UnsignedCommit {
            did: self.did.to_string(),
            version: REPO_VERSION,
            data: data_cid,
            rev: new_rev.to_string(),
            prev: self.head,
        };

        // Calculate signing hash
        let signing_hash = unsigned_commit.signing_hash()?;

        // Sign the commit
        let sig = sign_fn(&signing_hash)?;

        // Create signed commit
        let signed_commit = SignedCommit {
            commit: unsigned_commit,
            sig,
        };

        // Calculate commit CID
        let commit_cid = signed_commit.to_cid()?;

        // Store commit
        self.commits.insert(commit_cid, signed_commit);
        self.head = Some(commit_cid);
        self.current_rev = Some(new_rev);

        Ok(commit_cid)
    }

    /// Get a commit by CID
    pub fn get_commit(&self, cid: &Cid) -> Option<&SignedCommit> {
        self.commits.get(cid)
    }

    /// Export repository to CAR file
    ///
    /// # Returns
    ///
    /// Returns CAR file bytes containing the repository
    pub fn export_car(&self) -> Result<Vec<u8>> {
        let mut writer = CarWriter::new(Vec::new());

        // Add head commit as root
        if let Some(head_cid) = &self.head {
            writer.add_root(*head_cid)
                .map_err(|e| RepoError::Car(e.to_string()))?;

            // Write head commit
            if let Some(commit) = self.commits.get(head_cid) {
                let commit_bytes = commit.to_cbor()?;
                writer.write_block(head_cid, &commit_bytes)
                    .map_err(|e| RepoError::Car(e.to_string()))?;

                // Write MST root
                let mst_root_cid = commit.commit.data;
                let mst_root_bytes = self.mst.root_cid()?.to_string().into_bytes(); // Placeholder
                writer.write_block(&mst_root_cid, &mst_root_bytes)
                    .map_err(|e| RepoError::Car(e.to_string()))?;
            }
        }

        writer.finish()
            .map_err(|e| RepoError::Car(e.to_string()))
    }

    /// Get total number of records in the repository
    pub fn len(&self) -> usize {
        self.mst.len()
    }

    /// Check if repository is empty
    pub fn is_empty(&self) -> bool {
        self.mst.is_empty()
    }

    /// Get the MST for direct access (for testing/debugging)
    pub fn mst(&self) -> &Mst {
        &self.mst
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_repo() -> Repository {
        let did = Did::new("did:plc:test123").unwrap();
        Repository::create(did)
    }

    fn dummy_signer(hash: &[u8; 32]) -> Result<Vec<u8>> {
        // Return a dummy 64-byte signature
        Ok(vec![0u8; 64])
    }

    #[test]
    fn test_repo_creation() {
        let repo = create_test_repo();
        assert_eq!(repo.did().to_string(), "did:plc:test123");
        assert!(repo.head().is_none());
        assert!(repo.is_empty());
    }

    #[test]
    fn test_put_and_get_record() {
        let mut repo = create_test_repo();

        let record_data = br#"{"text":"Hello world"}"#.to_vec();
        let cid = repo.put_record("app.bsky.feed.post", "key1", record_data.clone()).unwrap();

        let retrieved = repo.get_record("app.bsky.feed.post", "key1");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap(), record_data.as_slice());
    }

    #[test]
    fn test_delete_record() {
        let mut repo = create_test_repo();

        let record_data = br#"{"text":"Test"}"#.to_vec();
        repo.put_record("app.bsky.feed.post", "key1", record_data.clone()).unwrap();

        let deleted = repo.delete_record("app.bsky.feed.post", "key1");
        assert!(deleted.is_some());
        assert_eq!(deleted.unwrap(), record_data);

        assert!(repo.get_record("app.bsky.feed.post", "key1").is_none());
    }

    #[test]
    fn test_list_records() {
        let mut repo = create_test_repo();

        repo.put_record("app.bsky.feed.post", "key1", b"data1".to_vec()).unwrap();
        repo.put_record("app.bsky.feed.post", "key2", b"data2".to_vec()).unwrap();
        repo.put_record("app.bsky.feed.post", "key3", b"data3".to_vec()).unwrap();
        repo.put_record("app.bsky.actor.profile", "self", b"profile".to_vec()).unwrap();

        let posts = repo.list_records("app.bsky.feed.post");
        assert_eq!(posts.len(), 3);
        assert!(posts.contains(&"key1".to_string()));
        assert!(posts.contains(&"key2".to_string()));
        assert!(posts.contains(&"key3".to_string()));

        let profiles = repo.list_records("app.bsky.actor.profile");
        assert_eq!(profiles.len(), 1);
        assert!(profiles.contains(&"self".to_string()));
    }

    #[test]
    fn test_commit_creation() {
        let mut repo = create_test_repo();

        // Add some records
        repo.put_record("app.bsky.feed.post", "key1", b"data1".to_vec()).unwrap();
        repo.put_record("app.bsky.feed.post", "key2", b"data2".to_vec()).unwrap();

        // Create a commit
        let commit_cid = repo.commit(dummy_signer).unwrap();

        assert!(repo.head().is_some());
        assert_eq!(repo.head().unwrap(), &commit_cid);
        assert!(repo.rev().is_some());

        // Verify commit exists
        let commit = repo.get_commit(&commit_cid);
        assert!(commit.is_some());

        let commit = commit.unwrap();
        assert_eq!(commit.commit.did, "did:plc:test123");
        assert_eq!(commit.commit.version, 3);
        assert!(commit.commit.prev.is_none()); // First commit
    }

    #[test]
    fn test_multiple_commits() {
        let mut repo = create_test_repo();

        // First commit
        repo.put_record("app.bsky.feed.post", "key1", b"data1".to_vec()).unwrap();
        let commit1 = repo.commit(dummy_signer).unwrap();

        // Second commit
        repo.put_record("app.bsky.feed.post", "key2", b"data2".to_vec()).unwrap();
        let commit2 = repo.commit(dummy_signer).unwrap();

        // Verify commits are linked
        let commit2_obj = repo.get_commit(&commit2).unwrap();
        assert_eq!(commit2_obj.commit.prev, Some(commit1));
    }

    #[test]
    fn test_unsigned_commit_serialization() {
        let hash = Sha256::digest(b"test");
        let multihash = libipld::multihash::Multihash::wrap(0x12, &hash).unwrap();
        let test_cid = Cid::new_v1(0x71, multihash);

        let commit = UnsignedCommit {
            did: "did:plc:test".to_string(),
            version: 3,
            data: test_cid,
            rev: Tid::next().unwrap().to_string(),
            prev: None,
        };

        let cbor = commit.to_cbor().unwrap();
        assert!(!cbor.is_empty());

        let hash = commit.signing_hash().unwrap();
        assert_eq!(hash.len(), 32);
    }

    #[test]
    fn test_invalid_collection() {
        let mut repo = create_test_repo();

        let result = repo.put_record("", "key1", b"data".to_vec());
        assert!(result.is_err());

        let result = repo.put_record("invalid", "key1", b"data".to_vec());
        assert!(result.is_err());
    }

    #[test]
    fn test_repo_len() {
        let mut repo = create_test_repo();

        assert_eq!(repo.len(), 0);

        repo.put_record("app.bsky.feed.post", "key1", b"data1".to_vec()).unwrap();
        assert_eq!(repo.len(), 1);

        repo.put_record("app.bsky.feed.post", "key2", b"data2".to_vec()).unwrap();
        assert_eq!(repo.len(), 2);

        repo.delete_record("app.bsky.feed.post", "key1");
        assert_eq!(repo.len(), 1);
    }

    #[test]
    fn test_export_car() {
        let mut repo = create_test_repo();

        repo.put_record("app.bsky.feed.post", "key1", b"data1".to_vec()).unwrap();
        repo.commit(dummy_signer).unwrap();

        let car_bytes = repo.export_car().unwrap();
        assert!(!car_bytes.is_empty());

        // Verify it's a valid CAR file
        let reader = CarReader::new(&car_bytes[..]);
        assert!(reader.is_ok());
    }
}
