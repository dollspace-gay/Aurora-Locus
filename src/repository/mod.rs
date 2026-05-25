//! Repository-layer building blocks shared across handlers and managers.
//!
//! Houses primitives that operate on repository record bodies and blob
//! references independent of the HTTP handler or the `RepositoryManager`
//! orchestration layer. Established by Arc 16e §9.5.4 Step 1 (#105) to
//! give Step 2's `apply_writes` refactor a stable home for the
//! validate-phase walker.

pub mod blob_refs;
