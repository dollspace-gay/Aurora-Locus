//! Lock-holder fixture for offline-check integration tests.
//!
//! Acquires the same `LivenessLock` the PDS's `serve` entry point
//! would, returning the guard back to the test. While the test holds
//! the guard, `LivenessLock::acquire` from a second caller (e.g.
//! the `grant-admin` CLI under test) fast-fails with the operator-
//! actionable "PDS instance appears to be running" message.
//!
//! Drop the guard at end-of-test to release the lock — no
//! subprocess management, no race conditions.

use aurora_locus::config::ServerConfig;
use aurora_locus::db::liveness_lock::LivenessLock;
use aurora_locus::error::PdsResult;

/// Acquire the PDS-liveness lock for the given config and return
/// the guard. Drop the guard to release.
///
/// The fixture is a thin wrapper around `LivenessLock::acquire` so
/// any future change to the lock mechanism (key, backend dispatch,
/// keepalive cadence) updates the test surface in one place.
pub async fn hold(config: &ServerConfig) -> PdsResult<LivenessLock> {
    LivenessLock::acquire(config).await
}
