//! Advisory-lock key registry for Aurora-Locus.
//!
//! Postgres `pg_advisory_lock` / `pg_try_advisory_lock` take a single
//! `bigint` (i64) key. Multiple subsystems in this codebase use
//! advisory locks for different purposes — sequencer leader election,
//! PDS-liveness gating, etc. Two locks colliding on the same key would
//! silently break one of the subsystems on Postgres (the symptom would
//! not appear until both subsystems were under contention), so all
//! keys are registered here in one place. New advisory-lock callers
//! MUST consult this file before picking a key.
//!
//! Keys are derived as `SHA-256(human-readable-identifier)` first 8
//! bytes (big-endian) interpreted as `i64`. Pre-computed const values
//! are paired with runtime tests that verify the byte sequences match
//! the documented hashes — a regression of a const away from the hash
//! is caught at test time.
//!
//! Operators sharing one Postgres between multiple deployments should
//! separate them by database/schema, not by lock key.

/// Sequencer leader-election lock — held by the leader process for
/// the lifetime of the leader role on a dedicated session-scoped
/// Postgres connection (Phase 4.2 / chainlink #89; see
/// [`crate::sequencer::leader_election`] for the election state
/// machine). SQLite deployments are inherently single-instance and
/// don't run leader election.
///
/// Derivation: SHA-256("aurora-locus.sequencer.leader") first 8
/// bytes (big-endian).
pub const SEQUENCER_LEADER_LOCK_KEY: i64 =
    i64::from_be_bytes([0x27, 0x21, 0x0a, 0x65, 0x7a, 0x4a, 0x3d, 0x34]);

/// PDS-liveness lock — held by the `serve` subcommand for the
/// lifetime of the running PDS process. The forthcoming `grant-admin`
/// CLI subcommand (Arc 1 Step 4) probes this lock with
/// `pg_try_advisory_lock` (Postgres) or `try_lock_exclusive` (SQLite)
/// to fast-fail if a PDS is already running against the same database.
/// See [`crate::db::liveness_lock`] for the acquisition impl.
///
/// Derivation: SHA-256("aurora-locus.pds.liveness") first 8 bytes
/// (big-endian).
pub const PDS_LIVENESS_LOCK_KEY: i64 =
    i64::from_be_bytes([0xee, 0xf5, 0x31, 0xce, 0x67, 0x69, 0x50, 0x97]);

// Audit-chain append serialization is the third active advisory-lock
// site — a transaction-scoped `pg_advisory_xact_lock` taken inside
// `audit_chain::insert_chain_entry` (v0.7 arc 1 step 2 rename of the
// former `append_entry_in_tx`). The key constant lives at
// [`crate::admin::audit_chain::AUDIT_CHAIN_LOCK_KEY`] and is derived
// from SHA-256("aurora.audit_chain"). A future relocation of that
// constant into this registry (matching the [`SEQUENCER_LEADER_LOCK_KEY`]
// pattern) would consolidate all three keys in one place; the spec
// for Arc 1 Step 0.11 didn't include it in the file list, so it
// remains in-place with this cross-reference for now. New advisory-
// lock callers MUST consult both this file AND the audit-chain
// constant before picking a key.

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn hash_first_8_bytes_be(input: &[u8]) -> i64 {
        let mut h = Sha256::new();
        h.update(input);
        let digest = h.finalize();
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&digest[..8]);
        i64::from_be_bytes(bytes)
    }

    #[test]
    fn sequencer_leader_key_matches_documented_hash() {
        assert_eq!(
            SEQUENCER_LEADER_LOCK_KEY,
            hash_first_8_bytes_be(b"aurora-locus.sequencer.leader"),
        );
    }

    #[test]
    fn pds_liveness_key_matches_documented_hash() {
        assert_eq!(
            PDS_LIVENESS_LOCK_KEY,
            hash_first_8_bytes_be(b"aurora-locus.pds.liveness"),
        );
    }

    #[test]
    fn registry_keys_are_distinct() {
        // Two locks colliding on the same key would silently break
        // one subsystem on Postgres. Distinctness is a load-bearing
        // invariant of this registry.
        assert_ne!(SEQUENCER_LEADER_LOCK_KEY, PDS_LIVENESS_LOCK_KEY);
    }
}
