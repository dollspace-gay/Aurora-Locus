/// Actor Store - Per-user repository management
///
/// Each user (actor) has their own SQLite database containing their repository data.
/// This module manages the lifecycle and operations on these per-user databases.

pub mod models;
pub mod repository;
pub mod store;

// Re-export commonly used types (allow unused for now as they're part of the public API)
#[allow(unused_imports)]
pub use models::*;
pub use repository::{RepositoryManager, WriteOp};
#[allow(unused_imports)]
pub use repository::WriteOpAction;
pub use store::{ActorStore, ActorStoreConfig};

use std::path::PathBuf;

/// Get the storage location for a user's actor store
pub fn get_actor_location(base_dir: &PathBuf, did: &str) -> ActorLocation {
    // Hash the DID to get directory sharding (first 2 chars of hash)
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    did.hash(&mut hasher);
    let hash = hasher.finish();
    let shard = format!("{:02x}", hash % 256);

    // Sanitize DID for use as directory name (replace colons with underscores)
    // Windows doesn't allow colons in filenames
    let safe_did = did.replace(':', "_");

    // Directory structure: {base_dir}/{shard}/{safe_did}/
    let directory = base_dir.join(&shard).join(&safe_did);
    let db_location = directory.join("store.sqlite");
    let key_location = directory.join("key");

    ActorLocation {
        directory,
        db_location,
        key_location,
    }
}

/// Location information for an actor's data
#[derive(Debug, Clone)]
pub struct ActorLocation {
    pub directory: PathBuf,
    pub db_location: PathBuf,
    pub key_location: PathBuf,
}
