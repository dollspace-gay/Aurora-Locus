//! Distributed-state substrate (Arc 7, V04_DESIGN.md §6.3.2).
//!
//! Aurora-Locus's multi-instance correctness story rests on three
//! pieces of cross-instance state: OAuth flow state (the existing
//! `authorization_request` table; trait-adopted in Step 2), DPoP
//! JTI replay tracking (`dpop_jti_replay` table; Step 3), and
//! rate-limit buckets (`rate_limit_buckets` table; Step 3). All
//! three live behind the [`DistributedStore`] trait so consumers
//! can swap backing stores without rewriting per-surface logic.
//!
//! The trait is intentionally narrow: insert / get / delete /
//! CAS plus per-table reaper sweeping. Per-surface semantics
//! (DPoP single-use replay rejection, rate-limit token-bucket
//! math) live in the consumer, not the substrate.
//!
//! Backends:
//! - `postgres_cas::PostgresCasStore` — the v0.4 default; uses
//!   `sqlx::Any` so a single implementation serves both SQLite
//!   and Postgres deployments. Per V04_DESIGN.md §6.3.1,
//!   Postgres-CAS is the default because Aurora-Locus already
//!   requires Postgres for multi-instance — no new operational
//!   dependency.
//! - Redis is a forward-compat slot in `DistributedStateMode`
//!   (`src/config.rs`) but not implemented in v0.4. Operators
//!   selecting `redis` get a clear startup error.
//!
//! Operators may opt out of the distributed substrate entirely
//! by setting `PDS_DISTRIBUTED_STATE_MODE=single_instance_inmemory`;
//! in that mode, no maintenance pool is constructed and consumers
//! fall back to per-instance in-memory state (auth state is lost
//! on restart — operator-accepted trade-off per §6.3.6).

pub mod cache;
pub mod lease;
pub mod postgres_cas;

pub use lease::Lease;
pub use postgres_cas::PostgresCasStore;

use async_trait::async_trait;
use thiserror::Error;

/// Substrate-level operations every backing store must provide.
///
/// The `table` parameter is a free-form string at the trait
/// surface; implementations dispatch internally to the correct
/// per-table SQL. Unknown tables produce
/// [`DistributedError::UnsupportedTable`] rather than silently
/// no-op'ing, so consumer-side typos surface loudly.
///
/// The `value` parameter is opaque bytes. Consumers serialize
/// their domain types (JSON for OAuth state, raw nonces for
/// DPoP) before calling. The substrate doesn't interpret the
/// payload.
#[async_trait]
pub trait DistributedStore: Send + Sync {
    /// Atomic insert with single-use semantics.
    ///
    /// Returns `Err(DistributedError::KeyExists)` if a row with
    /// the given primary key already exists. This is the
    /// guard that backs DPoP JTI replay rejection: the first
    /// `insert` of a given `jti` succeeds; subsequent ones fail
    /// with `KeyExists`, regardless of which Aurora-Locus
    /// instance issued the call.
    ///
    /// `lease` is optional because not every table has a
    /// lease-expires column. For tables that do, implementations
    /// persist `lease.expires_at_epoch_ms()` into the table's
    /// expiry column; for tables that don't, implementations
    /// ignore the argument.
    async fn insert(
        &self,
        table: &str,
        key: &str,
        value: &[u8],
        lease: Option<Lease>,
    ) -> Result<(), DistributedError>;

    /// Read by key. Returns `None` if absent.
    ///
    /// Lease-expired rows behave as `None` from the consumer's
    /// perspective; the reaper job is responsible for actually
    /// deleting them, but `get` filters them out so consumers
    /// never observe stale entries between reaper sweeps.
    async fn get(
        &self,
        table: &str,
        key: &str,
    ) -> Result<Option<Vec<u8>>, DistributedError>;

    /// Atomic delete. Returns whether a row was actually
    /// removed (true) or no matching row existed (false).
    ///
    /// Used both for consume-on-redemption semantics (OAuth
    /// state completion path; Step 2) and as an idempotent
    /// no-op-on-miss primitive (test cleanup, manual purge).
    async fn delete(
        &self,
        table: &str,
        key: &str,
    ) -> Result<bool, DistributedError>;

    /// Compare-and-swap on a row's `version` column.
    ///
    /// Returns [`CasResult::Success`] with the post-update
    /// version on success, or [`CasResult::Conflict`] with the
    /// actual current version on failure. Implementations
    /// increment `version` by exactly 1 on success.
    ///
    /// Tables without a `version` column return
    /// [`DistributedError::UnsupportedTable`] for this method;
    /// this is intentional, not silent — consumers shouldn't
    /// call `cas` against a table that doesn't support it.
    async fn cas(
        &self,
        table: &str,
        key: &str,
        expected_version: i64,
        new_value: &[u8],
    ) -> Result<CasResult, DistributedError>;

    /// Sweep expired rows from the given table.
    ///
    /// `now_epoch_ms` is supplied by the caller (the reaper
    /// job) rather than read off the database clock so the
    /// sweep is testable with controlled wall-clocks and
    /// uniform across instances that may have small clock
    /// drift. Implementations dispatch per table to the
    /// appropriate `<expires>_at_epoch_ms` column.
    ///
    /// Returns the count of rows deleted. Idempotent across
    /// instances: per V04_DESIGN.md §6.3.7, all instances run
    /// the reaper and concurrent sweeps from siblings are
    /// fine (`DELETE WHERE …` is naturally racing-safe).
    async fn reap_expired(
        &self,
        table: &str,
        now_epoch_ms: i64,
    ) -> Result<usize, DistributedError>;
}

/// Outcome of a [`DistributedStore::cas`] call. Distinguishes
/// success (with the new version) from optimistic-concurrency
/// conflict (with the version the caller is racing against).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CasResult {
    /// CAS succeeded; the row's new version is `new_version`.
    Success { new_version: i64 },
    /// CAS failed because the row's current version doesn't
    /// match the caller's `expected_version`. `current_version`
    /// is the version the caller is actually racing against;
    /// retry loops can use it to refetch + recompute.
    Conflict { current_version: i64 },
}

/// Errors a [`DistributedStore`] implementation can return.
///
/// `Database` wraps `sqlx::Error` for unexpected backend
/// failures (connection drops, transient timeouts); business-
/// semantic errors (key already exists, table unsupported)
/// get their own variants so call sites can pattern-match
/// without inspecting error strings.
#[derive(Debug, Error)]
pub enum DistributedError {
    /// Insert collided with an existing primary key. The
    /// table-level guard that backs DPoP JTI single-use
    /// semantics: a replay attempt always lands here.
    #[error("key already exists in table '{table}': {key}")]
    KeyExists { table: String, key: String },

    /// The caller named a table the implementation doesn't
    /// know how to operate on, or called a method that the
    /// named table doesn't support (e.g., `cas` on a table
    /// without a `version` column).
    #[error("table '{0}' is not supported by this substrate")]
    UnsupportedTable(String),

    /// Unexpected backend error. Includes connection drops,
    /// timeouts, schema-drift surprises, etc.
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cas_result_pattern_matches() {
        // Compile-time check that the variants are usable in
        // exhaustive pattern matches without falling through.
        fn classify(result: CasResult) -> &'static str {
            match result {
                CasResult::Success { new_version: _ } => "ok",
                CasResult::Conflict { current_version: _ } => "racing",
            }
        }
        assert_eq!(classify(CasResult::Success { new_version: 7 }), "ok");
        assert_eq!(
            classify(CasResult::Conflict { current_version: 5 }),
            "racing"
        );
    }

    #[test]
    fn key_exists_error_carries_table_and_key() {
        let err = DistributedError::KeyExists {
            table: "dpop_jti_replay".to_string(),
            key: "abc-jti".to_string(),
        };
        let msg = format!("{}", err);
        assert!(msg.contains("dpop_jti_replay"), "got: {}", msg);
        assert!(msg.contains("abc-jti"), "got: {}", msg);
    }

    #[test]
    fn unsupported_table_error_names_the_table() {
        let err = DistributedError::UnsupportedTable("zorblax".to_string());
        let msg = format!("{}", err);
        assert!(msg.contains("zorblax"), "got: {}", msg);
    }
}
