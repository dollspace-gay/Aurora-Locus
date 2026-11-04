//! Errors shared across crates. Each crate keeps its own `thiserror` enum and
//! converts into `CoreError` at boundaries; the HTTP layer maps `CoreError`
//! to atproto error names.

use thiserror::Error;

pub type CoreResult<T> = Result<T, CoreError>;

#[derive(Debug, Error)]
pub enum CoreError {
    /// The storage backend failed (connection, query, I/O).
    #[error("storage error: {0}")]
    Storage(String),

    /// A value failed validation. The message is safe to show to an operator.
    #[error("invalid value: {0}")]
    Invalid(String),

    /// The thing asked for doesn't exist.
    #[error("not found: {0}")]
    NotFound(String),

    /// A compare-and-set or uniqueness check failed.
    #[error("conflict: {0}")]
    Conflict(String),

    /// A leased job's lease is no longer held by this node. Work under a lost
    /// lease must never commit.
    #[error("lease lost: {0}")]
    LeaseLost(String),

    /// Configuration is unusable (bootstrap file, env, schema version).
    #[error("configuration error: {0}")]
    Config(String),
}
