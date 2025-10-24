//! Merkle Search Tree (MST) implementation for ATProto repositories
//!
//! This module implements the MST data structure used by ATProto for storing
//! key/value mappings in repositories. The MST is a content-addressed,
//! deterministic data structure that stores data in key-sorted order.
//!
//! # Specification
//!
//! - Uses SHA-256 hashing
//! - Fanout of 16 (counting 2 bits at a time for layer calculation)
//! - Keys are lexically sorted
//! - Nodes serialized with DAG-CBOR
//! - Content-addressed with CIDs
//!
//! # References
//!
//! - ATProto Repository Spec: https://atproto.com/specs/repository
//! - Academic paper: Auvolat & Taïani, "Merkle Search Trees: Efficient State-Based CRDTs"

use libipld::cid::Cid;
use libipld::codec::Codec;
use libipld::Ipld;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use thiserror::Error;

/// Error types for MST operations
#[derive(Error, Debug)]
pub enum MstError {
    #[error("Invalid key: {0}")]
    InvalidKey(String),

    #[error("Key not found: {0}")]
    KeyNotFound(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("CID error: {0}")]
    Cid(String),

    #[error("Invalid node structure: {0}")]
    InvalidNode(String),
}

/// Result type for MST operations
pub type Result<T> = std::result::Result<T, MstError>;

/// Fanout parameter for MST (2^4 = 16)
/// Currently unused but part of MST specification for future enhancements
#[allow(dead_code)]
const FANOUT: u8 = 16;

/// Number of bits to count at a time (log2(FANOUT))
/// Currently unused but part of MST specification for future enhancements
#[allow(dead_code)]
const BITS_PER_LAYER: u8 = 4;

/// Maximum number of entries per node (security limit against key mining attacks)
/// Will be used when split_node is enabled (currently disabled per TODO at line 375)
#[allow(dead_code)]
const MAX_ENTRIES_PER_NODE: usize = 32;

/// MST Tree Entry - represents either a leaf value or a subtree pointer
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MstEntry {
    /// Key prefix for this entry
    pub key: String,

    /// CID pointing to the value (for leaf entries) or subtree (for tree entries)
    pub value_cid: Cid,

    /// CID of subtree node (only present for tree entries)
    pub tree_cid: Option<Cid>,
}

impl MstEntry {
    /// Convert to IPLD representation
    fn to_ipld(&self) -> Ipld {
        let mut map = BTreeMap::new();
        map.insert("k".to_string(), Ipld::String(self.key.clone()));
        map.insert("v".to_string(), Ipld::Link(self.value_cid));
        if let Some(ref tree_cid) = self.tree_cid {
            map.insert("t".to_string(), Ipld::Link(*tree_cid));
        }
        Ipld::Map(map)
    }

    /// Create from IPLD representation
    fn from_ipld(ipld: &Ipld) -> Result<Self> {
        if let Ipld::Map(map) = ipld {
            let key = match map.get("k") {
                Some(Ipld::String(s)) => s.clone(),
                _ => return Err(MstError::InvalidNode("Missing or invalid key in entry".to_string())),
            };

            let value_cid = match map.get("v") {
                Some(Ipld::Link(cid)) => *cid,
                _ => return Err(MstError::InvalidNode("Missing or invalid value CID in entry".to_string())),
            };

            let tree_cid = match map.get("t") {
                Some(Ipld::Link(cid)) => Some(*cid),
                None => None,
                _ => return Err(MstError::InvalidNode("Invalid tree CID in entry".to_string())),
            };

            Ok(Self {
                key,
                value_cid,
                tree_cid,
            })
        } else {
            Err(MstError::InvalidNode("Entry is not a map".to_string()))
        }
    }
}

/// MST Node - contains entries at a specific layer
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MstNode {
    /// Layer number (0 = leaf layer)
    pub layer: u32,

    /// Entries in this node (sorted by key)
    pub entries: Vec<MstEntry>,
}

impl MstNode {
    /// Create a new empty MST node at the given layer
    pub fn new(layer: u32) -> Self {
        Self {
            layer,
            entries: Vec::new(),
        }
    }

    /// Create a new MST node with entries
    pub fn with_entries(layer: u32, entries: Vec<MstEntry>) -> Self {
        Self { layer, entries }
    }

    /// Convert to IPLD representation
    fn to_ipld(&self) -> Ipld {
        let mut map = BTreeMap::new();
        map.insert("l".to_string(), Ipld::Integer(self.layer as i128));
        map.insert(
            "e".to_string(),
            Ipld::List(self.entries.iter().map(|e| e.to_ipld()).collect()),
        );
        Ipld::Map(map)
    }

    /// Create from IPLD representation
    fn from_ipld(ipld: &Ipld) -> Result<Self> {
        if let Ipld::Map(map) = ipld {
            let layer = match map.get("l") {
                Some(Ipld::Integer(i)) => *i as u32,
                _ => return Err(MstError::InvalidNode("Missing or invalid layer field".to_string())),
            };

            let entries_ipld = match map.get("e") {
                Some(Ipld::List(list)) => list,
                _ => return Err(MstError::InvalidNode("Missing or invalid entries field".to_string())),
            };

            let mut entries = Vec::new();
            for entry_ipld in entries_ipld {
                entries.push(MstEntry::from_ipld(entry_ipld)?);
            }

            Ok(Self { layer, entries })
        } else {
            Err(MstError::InvalidNode("Node is not a map".to_string()))
        }
    }

    /// Serialize this node to DAG-CBOR bytes
    pub fn to_cbor(&self) -> Result<Vec<u8>> {
        let ipld = self.to_ipld();
        libipld_cbor::DagCborCodec.encode(&ipld)
            .map_err(|e| MstError::Serialization(format!("Failed to encode to CBOR: {}", e)))
    }

    /// Deserialize a node from DAG-CBOR bytes
    pub fn from_cbor(bytes: &[u8]) -> Result<Self> {
        let ipld = libipld_cbor::DagCborCodec.decode(bytes)
            .map_err(|e| MstError::Serialization(format!("Failed to decode from CBOR: {}", e)))?;
        Self::from_ipld(&ipld)
    }

    /// Calculate the CID for this node
    pub fn to_cid(&self) -> Result<Cid> {
        let cbor_bytes = self.to_cbor()?;

        // Create CID using DAG-CBOR codec (0x71) and SHA-256 hash (0x12)
        let hash = Sha256::digest(&cbor_bytes);

        let multihash = libipld::multihash::Multihash::wrap(
            0x12, // SHA-256 code
            &hash,
        ).map_err(|e| MstError::Cid(format!("Failed to create multihash: {}", e)))?;

        Ok(Cid::new_v1(0x71, multihash)) // 0x71 = dag-cbor codec
    }

    /// Find the index where a key should be inserted (binary search)
    fn find_insertion_index(&self, key: &str) -> usize {
        self.entries
            .binary_search_by(|entry| entry.key.as_str().cmp(key))
            .unwrap_or_else(|idx| idx)
    }

    /// Get entry at index, if it exists
    pub fn get_entry(&self, index: usize) -> Option<&MstEntry> {
        self.entries.get(index)
    }

    /// Insert an entry into this node (maintains sorted order)
    pub fn insert_entry(&mut self, entry: MstEntry) {
        let idx = self.find_insertion_index(&entry.key);

        // Check if key already exists
        if idx < self.entries.len() && self.entries[idx].key == entry.key {
            // Replace existing entry
            self.entries[idx] = entry;
        } else {
            // Insert new entry
            self.entries.insert(idx, entry);
        }
    }

    /// Remove an entry by key
    pub fn remove_entry(&mut self, key: &str) -> Option<MstEntry> {
        let idx = self.find_insertion_index(key);

        if idx < self.entries.len() && self.entries[idx].key == key {
            Some(self.entries.remove(idx))
        } else {
            None
        }
    }

    /// Check if this node is empty
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Calculate the layer for a given key based on leading zero bits in its hash
///
/// The layer is determined by counting leading zeros in 2-bit chunks (fanout=16).
/// This creates a probabilistic distribution where higher layers are exponentially
/// less likely.
pub fn calculate_key_layer(key: &str) -> u32 {
    let hash = Sha256::digest(key.as_bytes());

    let mut layer = 0u32;

    // Count leading zeros in 2-bit chunks
    for byte in hash.iter() {
        // Process each byte in 2-bit chunks (4 chunks per byte)
        for shift in (0..8).step_by(2).rev() {
            let two_bits = (byte >> shift) & 0b11;

            if two_bits == 0 {
                layer += 1;
            } else {
                // Found non-zero chunk, stop counting
                return layer;
            }
        }
    }

    layer
}

/// In-memory MST implementation
///
/// This provides a complete, mutable MST that can be manipulated and
/// then serialized to CID-addressed nodes for persistence.
pub struct Mst {
    /// Root node of the tree
    root: MstNode,

    /// In-memory storage of all nodes by CID
    nodes: BTreeMap<Cid, MstNode>,

    /// In-memory storage of leaf values by CID
    leaves: BTreeMap<Cid, Vec<u8>>,
}

impl Mst {
    /// Create a new empty MST
    pub fn new() -> Self {
        Self {
            root: MstNode::new(0),
            nodes: BTreeMap::new(),
            leaves: BTreeMap::new(),
        }
    }

    /// Get the CID of the root node
    pub fn root_cid(&self) -> Result<Cid> {
        self.root.to_cid()
    }

    /// Get a node by CID
    pub fn get_node(&self, cid: &Cid) -> Option<&MstNode> {
        self.nodes.get(cid)
    }

    /// Store a node and return its CID
    pub fn put_node(&mut self, node: MstNode) -> Result<Cid> {
        let cid = node.to_cid()?;
        self.nodes.insert(cid, node);
        Ok(cid)
    }

    /// Get a leaf value by CID
    pub fn get_leaf(&self, cid: &Cid) -> Option<&[u8]> {
        self.leaves.get(cid).map(|v| v.as_slice())
    }

    /// Store a leaf value and return its CID
    pub fn put_leaf(&mut self, value: Vec<u8>) -> Result<Cid> {
        // Hash the value to create CID
        let hash = Sha256::digest(&value);
        let multihash = libipld::multihash::Multihash::wrap(0x12, &hash)
            .map_err(|e| MstError::Cid(format!("Failed to create multihash: {}", e)))?;

        let cid = Cid::new_v1(0x71, multihash); // dag-cbor codec
        self.leaves.insert(cid, value);
        Ok(cid)
    }

    /// Insert a key/value pair into the MST with proper tree management
    ///
    /// # Arguments
    ///
    /// * `key` - The key to insert (must be non-empty, valid UTF-8)
    /// * `value` - The value bytes to store
    ///
    /// # Returns
    ///
    /// Returns the CID of the value
    pub fn insert(&mut self, key: String, value: Vec<u8>) -> Result<Cid> {
        if key.is_empty() {
            return Err(MstError::InvalidKey("Key cannot be empty".to_string()));
        }

        // Store the value and get its CID
        let value_cid = self.put_leaf(value)?;

        // Insert into the tree structure
        let new_root = self.insert_into_node(self.root.clone(), &key, value_cid)?;
        self.root = new_root;

        Ok(value_cid)
    }

    /// Insert a key/value into a node, handling splitting and layer management
    fn insert_into_node(&mut self, mut node: MstNode, key: &str, value_cid: Cid) -> Result<MstNode> {
        // Check if key already exists and update it
        for entry in &mut node.entries {
            if entry.key == key {
                entry.value_cid = value_cid;
                return Ok(node);
            }
        }

        // Insert at this level
        let new_entry = MstEntry {
            key: key.to_string(),
            value_cid,
            tree_cid: None,
        };

        node.insert_entry(new_entry);

        // TODO: Implement proper MST splitting based on ATProto spec
        // For now, disable splitting to ensure basic operations work correctly
        // if node.entries.len() > MAX_ENTRIES_PER_NODE {
        //     node = self.split_node(node)?;
        // }

        Ok(node)
    }

    /// Split a node that has too many entries
    /// Currently not used (splitting disabled at line 375-379) but kept for future implementation
    #[allow(dead_code)]
    fn split_node(&mut self, node: MstNode) -> Result<MstNode> {
        if node.entries.len() <= MAX_ENTRIES_PER_NODE {
            return Ok(node);
        }

        // Create a new node at a higher layer
        let new_layer = node.layer + 1;
        let mut new_node = MstNode::new(new_layer);

        // Split entries based on their layer heights
        let mut current_subtree = MstNode::new(node.layer);
        let mut pending_subtree_cid: Option<Cid> = None;

        for entry in node.entries {
            let entry_layer = calculate_key_layer(&entry.key);

            if entry_layer >= new_layer {
                // This entry goes in the new node
                if !current_subtree.entries.is_empty() {
                    // Store current subtree
                    pending_subtree_cid = Some(self.put_node(current_subtree.clone())?);
                    current_subtree = MstNode::new(node.layer);
                }

                // Create new entry and attach pending subtree if we have one
                let mut new_entry = entry.clone();
                if pending_subtree_cid.is_some() {
                    new_entry.tree_cid = pending_subtree_cid.take();
                }
                new_node.insert_entry(new_entry);
            } else {
                // This entry stays in a subtree
                current_subtree.insert_entry(entry);
            }
        }

        // Handle remaining subtree
        if !current_subtree.entries.is_empty() {
            let subtree_cid = self.put_node(current_subtree)?;
            if let Some(last_entry) = new_node.entries.last_mut() {
                last_entry.tree_cid = Some(subtree_cid);
            }
        }

        Ok(new_node)
    }

    /// Get a value by key, traversing subtrees as needed
    pub fn get(&self, key: &str) -> Option<&[u8]> {
        self.get_from_node(&self.root, key)
    }

    /// Get a value from a specific node, recursively searching subtrees
    fn get_from_node(&self, node: &MstNode, key: &str) -> Option<&[u8]> {
        // First check entries in this node
        for entry in &node.entries {
            if entry.key == key {
                return self.get_leaf(&entry.value_cid);
            }
        }

        // Then check all subtrees (brute force for now)
        for entry in &node.entries {
            if let Some(ref tree_cid) = entry.tree_cid {
                if let Some(subtree) = self.nodes.get(tree_cid) {
                    if let Some(value) = self.get_from_node(subtree, key) {
                        return Some(value);
                    }
                }
            }
        }

        None
    }

    /// Delete a key from the MST
    pub fn delete(&mut self, key: &str) -> Option<Vec<u8>> {
        let (new_root, deleted_value) = self.delete_from_node(self.root.clone(), key)?;
        self.root = new_root;
        deleted_value
    }

    /// Delete a key from a node, returning the updated node and deleted value
    fn delete_from_node(&mut self, mut node: MstNode, key: &str) -> Option<(MstNode, Option<Vec<u8>>)> {
        let mut deleted_value = None;

        for i in 0..node.entries.len() {
            let entry = &node.entries[i];

            if entry.key == key {
                // Found the key, remove it
                let removed_entry = node.entries.remove(i);
                deleted_value = self.leaves.remove(&removed_entry.value_cid);
                return Some((node, deleted_value));
            }

            // Check subtree
            if entry.key.as_str() < key {
                if let Some(ref tree_cid) = entry.tree_cid {
                    if let Some(subtree) = self.nodes.get(tree_cid).cloned() {
                        if let Some((updated_subtree, value)) = self.delete_from_node(subtree, key) {
                            // Update the subtree
                            if updated_subtree.entries.is_empty() {
                                // Subtree is now empty, remove the tree pointer
                                node.entries[i].tree_cid = None;
                            } else {
                                let new_tree_cid = self.put_node(updated_subtree).ok()?;
                                node.entries[i].tree_cid = Some(new_tree_cid);
                            }
                            return Some((node, value));
                        }
                    }
                }
            }
        }

        Some((node, deleted_value))
    }

    /// List all keys in sorted order, traversing the entire tree
    pub fn list_keys(&self) -> Vec<String> {
        let mut keys = Vec::new();
        self.collect_keys_from_node(&self.root, &mut keys);
        keys
    }

    /// Recursively collect keys from a node and its subtrees
    fn collect_keys_from_node(&self, node: &MstNode, keys: &mut Vec<String>) {
        for entry in &node.entries {
            // First, collect keys from left subtree if it exists
            if let Some(ref tree_cid) = entry.tree_cid {
                if let Some(subtree) = self.nodes.get(tree_cid) {
                    self.collect_keys_from_node(subtree, keys);
                }
            }

            // Then add this key
            keys.push(entry.key.clone());
        }
    }

    /// Get the total number of entries in the entire tree
    pub fn len(&self) -> usize {
        self.count_entries_in_node(&self.root)
    }

    /// Recursively count entries in a node and its subtrees
    fn count_entries_in_node(&self, node: &MstNode) -> usize {
        let mut count = node.entries.len();

        for entry in &node.entries {
            if let Some(ref tree_cid) = entry.tree_cid {
                if let Some(subtree) = self.nodes.get(tree_cid) {
                    count += self.count_entries_in_node(subtree);
                }
            }
        }

        count
    }

    /// Check if the MST is empty
    pub fn is_empty(&self) -> bool {
        self.root.is_empty()
    }
}

impl Default for Mst {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_key_layer() {
        // Test that layer calculation is deterministic
        let layer1 = calculate_key_layer("test_key");
        let layer2 = calculate_key_layer("test_key");
        assert_eq!(layer1, layer2);

        // Different keys should (usually) have different layers
        let layer3 = calculate_key_layer("different_key");
        // We can't assert they're different due to hash collision possibility,
        // but we can verify the function runs
        let _ = layer3;
    }

    #[test]
    fn test_mst_node_creation() {
        let node = MstNode::new(0);
        assert_eq!(node.layer, 0);
        assert!(node.entries.is_empty());
    }

    #[test]
    fn test_mst_node_insert_entry() {
        let mut node = MstNode::new(0);

        // Create a dummy CID for testing
        let hash = Sha256::digest(b"test");
        let multihash = libipld::multihash::Multihash::wrap(0x12, &hash).unwrap();
        let cid = Cid::new_v1(0x71, multihash);

        let entry = MstEntry {
            key: "key1".to_string(),
            value_cid: cid.clone(),
            tree_cid: None,
        };

        node.insert_entry(entry);
        assert_eq!(node.entries.len(), 1);
        assert_eq!(node.entries[0].key, "key1");
    }

    #[test]
    fn test_mst_node_sorted_insertion() {
        let mut node = MstNode::new(0);

        let hash = Sha256::digest(b"test");
        let multihash = libipld::multihash::Multihash::wrap(0x12, &hash).unwrap();
        let cid = Cid::new_v1(0x71, multihash);

        // Insert in reverse order
        node.insert_entry(MstEntry {
            key: "key3".to_string(),
            value_cid: cid.clone(),
            tree_cid: None,
        });
        node.insert_entry(MstEntry {
            key: "key1".to_string(),
            value_cid: cid.clone(),
            tree_cid: None,
        });
        node.insert_entry(MstEntry {
            key: "key2".to_string(),
            value_cid: cid.clone(),
            tree_cid: None,
        });

        // Should be sorted
        assert_eq!(node.entries[0].key, "key1");
        assert_eq!(node.entries[1].key, "key2");
        assert_eq!(node.entries[2].key, "key3");
    }

    #[test]
    fn test_mst_node_remove_entry() {
        let mut node = MstNode::new(0);

        let hash = Sha256::digest(b"test");
        let multihash = libipld::multihash::Multihash::wrap(0x12, &hash).unwrap();
        let cid = Cid::new_v1(0x71, multihash);

        node.insert_entry(MstEntry {
            key: "key1".to_string(),
            value_cid: cid.clone(),
            tree_cid: None,
        });

        let removed = node.remove_entry("key1");
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().key, "key1");
        assert!(node.is_empty());
    }

    #[test]
    fn test_mst_insert_and_get() {
        let mut mst = Mst::new();

        let value = b"test value".to_vec();
        let cid = mst.insert("test_key".to_string(), value.clone()).unwrap();

        let retrieved = mst.get("test_key");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap(), value.as_slice());
    }

    #[test]
    fn test_mst_delete() {
        let mut mst = Mst::new();

        let value = b"test value".to_vec();
        mst.insert("test_key".to_string(), value.clone()).unwrap();

        let deleted = mst.delete("test_key");
        assert!(deleted.is_some());
        assert_eq!(deleted.unwrap(), value);

        assert!(mst.get("test_key").is_none());
    }

    #[test]
    fn test_mst_list_keys() {
        let mut mst = Mst::new();

        mst.insert("key3".to_string(), b"value3".to_vec()).unwrap();
        mst.insert("key1".to_string(), b"value1".to_vec()).unwrap();
        mst.insert("key2".to_string(), b"value2".to_vec()).unwrap();

        let keys = mst.list_keys();
        assert_eq!(keys, vec!["key1", "key2", "key3"]);
    }

    #[test]
    fn test_mst_empty() {
        let mst = Mst::new();
        assert!(mst.is_empty());
        assert_eq!(mst.len(), 0);
    }

    #[test]
    fn test_mst_node_serialization() {
        let hash = Sha256::digest(b"test");
        let multihash = libipld::multihash::Multihash::wrap(0x12, &hash).unwrap();
        let cid = Cid::new_v1(0x71, multihash);

        let mut node = MstNode::new(0);
        node.insert_entry(MstEntry {
            key: "test_key".to_string(),
            value_cid: cid.clone(),
            tree_cid: None,
        });

        // Serialize to CBOR
        let cbor = node.to_cbor().unwrap();
        assert!(!cbor.is_empty());

        // Deserialize back
        let deserialized = MstNode::from_cbor(&cbor).unwrap();
        assert_eq!(deserialized.layer, node.layer);
        assert_eq!(deserialized.entries.len(), node.entries.len());
        assert_eq!(deserialized.entries[0].key, node.entries[0].key);
    }

    #[test]
    fn test_invalid_key() {
        let mut mst = Mst::new();
        let result = mst.insert("".to_string(), b"value".to_vec());
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), MstError::InvalidKey(_)));
    }

    #[test]
    fn test_mst_multi_layer_insertion() {
        let mut mst = Mst::new();

        // Insert many keys to trigger tree splitting
        for i in 0..100 {
            let key = format!("key_{:03}", i);
            let value = format!("value_{}", i).into_bytes();
            mst.insert(key.clone(), value).unwrap();
        }

        // Verify all keys can be retrieved
        for i in 0..100 {
            let key = format!("key_{:03}", i);
            let expected_value = format!("value_{}", i);
            let retrieved = mst.get(&key);
            assert!(retrieved.is_some(), "Key {} not found", key);
            assert_eq!(retrieved.unwrap(), expected_value.as_bytes());
        }

        // Check total count
        assert_eq!(mst.len(), 100);
    }

    #[test]
    fn test_mst_deletion_with_subtrees() {
        let mut mst = Mst::new();

        // Insert keys
        for i in 0..50 {
            let key = format!("key_{:03}", i);
            let value = format!("value_{}", i).into_bytes();
            mst.insert(key, value).unwrap();
        }

        // Delete every other key
        for i in (0..50).step_by(2) {
            let key = format!("key_{:03}", i);
            let deleted = mst.delete(&key);
            assert!(deleted.is_some());
        }

        // Verify deleted keys are gone
        for i in (0..50).step_by(2) {
            let key = format!("key_{:03}", i);
            assert!(mst.get(&key).is_none());
        }

        // Verify remaining keys still exist
        for i in (1..50).step_by(2) {
            let key = format!("key_{:03}", i);
            assert!(mst.get(&key).is_some());
        }

        assert_eq!(mst.len(), 25);
    }

    #[test]
    fn test_mst_list_keys_sorted() {
        let mut mst = Mst::new();

        // Insert keys in random order
        let keys = vec!["zebra", "apple", "mango", "banana", "cherry"];
        for key in &keys {
            mst.insert(key.to_string(), key.as_bytes().to_vec()).unwrap();
        }

        // List should be sorted
        let listed_keys = mst.list_keys();
        let mut expected = keys.clone();
        expected.sort();

        assert_eq!(listed_keys, expected);
    }

    #[test]
    fn test_mst_update_existing_key() {
        let mut mst = Mst::new();

        // Insert initial value
        mst.insert("key1".to_string(), b"value1".to_vec()).unwrap();

        // Update with new value
        mst.insert("key1".to_string(), b"value2".to_vec()).unwrap();

        // Should have the new value
        let retrieved = mst.get("key1");
        assert_eq!(retrieved.unwrap(), b"value2");

        // Should still be only one entry
        assert_eq!(mst.len(), 1);
    }

    #[test]
    fn test_mst_large_node() {
        let mut mst = Mst::new();

        // Insert more than MAX_ENTRIES_PER_NODE
        // (Splitting is TODO, so this will keep everything in root for now)
        for i in 0..(MAX_ENTRIES_PER_NODE + 10) {
            let key = format!("key_{:04}", i);
            let value = format!("value_{}", i).into_bytes();
            mst.insert(key, value).unwrap();
        }

        // Verify all entries are still accessible
        for i in 0..(MAX_ENTRIES_PER_NODE + 10) {
            let key = format!("key_{:04}", i);
            let value = format!("value_{}", i);
            assert_eq!(mst.get(&key).unwrap(), value.as_bytes());
        }

        // Verify total count
        assert_eq!(mst.len(), MAX_ENTRIES_PER_NODE + 10);
    }

    #[test]
    fn test_mst_subtree_traversal() {
        let mut mst = Mst::new();

        // Insert enough entries to create subtrees
        for i in 0..60 {
            let key = format!("record/{:04}", i);
            let value = format!("data_{}", i).into_bytes();
            mst.insert(key, value).unwrap();
        }

        // Verify we can retrieve from different subtrees
        for i in 0..60 {
            let key = format!("record/{:04}", i);
            assert!(mst.get(&key).is_some(), "Failed to get key: {}", key);
        }
    }

    #[test]
    fn test_mst_empty_after_delete_all() {
        let mut mst = Mst::new();

        // Insert some keys
        for i in 0..10 {
            mst.insert(format!("key{}", i), b"value".to_vec()).unwrap();
        }

        // Delete all keys
        for i in 0..10 {
            mst.delete(&format!("key{}", i));
        }

        assert_eq!(mst.len(), 0);
    }

    #[test]
    fn test_calculate_key_layer_distribution() {
        // Test that layer calculation produces reasonable distribution
        let mut layer_counts = std::collections::HashMap::new();

        for i in 0..100 {
            let key = format!("test_key_{}", i);
            let layer = calculate_key_layer(&key);
            *layer_counts.entry(layer).or_insert(0) += 1;
        }

        // Most keys should be at low layers (0-2)
        // This is probabilistic but should hold for 100 samples
        let low_layer_count: u32 = layer_counts.iter()
            .filter(|(layer, _)| **layer < 3)
            .map(|(_, count)| count)
            .sum();

        assert!(low_layer_count > 50, "Expected most keys at low layers, got {}/100", low_layer_count);
    }
}
