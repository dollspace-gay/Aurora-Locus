//! Lease primitive for the distributed-state substrate.
//!
//! A `Lease` describes when a stored entry should be considered
//! expired. Backed by BIGINT epoch-millis for cross-backend
//! portability via `sqlx::Any` — the same convention the Arc 7
//! migration (`0007_distributed_state.sql`) uses for its time
//! columns. Consumers pass `Lease::from_now(...)` at insertion
//! time; the substrate persists `lease.expires_at_epoch_ms` into
//! the appropriate per-table column and the reaper job sweeps
//! rows where that column is less than the current epoch-ms.
//!
//! Epoch-millis (not RFC3339 TEXT) because §6.3.5's rate-limit
//! arithmetic UPDATE requires direct integer subtraction; the
//! lease primitive matches that convention so every time field
//! in the substrate stays portable.
//!
//! No serialization derives — the lease is an in-process primitive,
//! consumers convert to/from the wire (or DB) representation
//! explicitly via `expires_at_epoch_ms()`.
//!
//! `Lease::from_now` uses the system clock via `chrono::Utc::now()`,
//! matching the rest of the codebase's wall-clock pattern.

use chrono::Duration;

/// A lease describing when a stored entry should be considered
/// expired. Time is represented as milliseconds since the Unix
/// epoch (i64) so the entire substrate can use direct integer
/// arithmetic across both SQLite and Postgres backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lease {
    /// Absolute expiry instant, in milliseconds since the Unix
    /// epoch.
    pub expires_at_epoch_ms: i64,
}

impl Lease {
    /// Construct a lease that expires at the given absolute
    /// instant. The caller is responsible for choosing a sensible
    /// value; this constructor performs no validation (negative
    /// or near-now values are permitted — they simply produce
    /// already-expired leases).
    pub fn until(expires_at_epoch_ms: i64) -> Self {
        Self {
            expires_at_epoch_ms,
        }
    }

    /// Construct a lease that expires `duration` from now,
    /// computed against `chrono::Utc::now()`. A negative or zero
    /// duration produces an already-expired lease (no error).
    pub fn from_now(duration: Duration) -> Self {
        let now_ms = chrono::Utc::now().timestamp_millis();
        Self {
            expires_at_epoch_ms: now_ms.saturating_add(duration.num_milliseconds()),
        }
    }

    /// True iff this lease's expiry is strictly less than
    /// `now_epoch_ms`. The strict-less comparison matches the
    /// reaper sweep predicate (`WHERE exp_at_epoch_ms < $now`),
    /// so reader semantics line up with sweep semantics.
    pub fn is_expired_at(&self, now_epoch_ms: i64) -> bool {
        self.expires_at_epoch_ms < now_epoch_ms
    }

    /// Convenience accessor used by substrate implementations
    /// when persisting the lease into a backend column.
    pub fn expires_at_epoch_ms(&self) -> i64 {
        self.expires_at_epoch_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn until_round_trips() {
        let lease = Lease::until(1_700_000_000_000);
        assert_eq!(lease.expires_at_epoch_ms, 1_700_000_000_000);
        assert_eq!(lease.expires_at_epoch_ms(), 1_700_000_000_000);
    }

    #[test]
    fn from_now_is_in_the_future_for_positive_duration() {
        let before = chrono::Utc::now().timestamp_millis();
        let lease = Lease::from_now(Duration::seconds(60));
        let after = chrono::Utc::now().timestamp_millis();
        // Allow a small slack on either side for the time the
        // constructor took to run.
        assert!(lease.expires_at_epoch_ms >= before + 60_000);
        assert!(lease.expires_at_epoch_ms <= after + 60_000 + 5);
    }

    #[test]
    fn from_now_with_negative_duration_is_already_expired() {
        let lease = Lease::from_now(Duration::seconds(-1));
        let now = chrono::Utc::now().timestamp_millis();
        assert!(lease.is_expired_at(now));
    }

    #[test]
    fn is_expired_at_uses_strict_less_than() {
        // A lease whose expiry equals `now` is NOT yet expired —
        // strict-less semantics match the reaper sweep predicate.
        let lease = Lease::until(1_000);
        assert!(!lease.is_expired_at(1_000));
        assert!(lease.is_expired_at(1_001));
        assert!(!lease.is_expired_at(999));
    }

    #[test]
    fn from_now_does_not_overflow_on_extreme_duration() {
        // saturating_add prevents i64 overflow when the requested
        // duration is absurd. Guards against future operator
        // typos like `Duration::days(i64::MAX)`.
        let lease = Lease::from_now(Duration::milliseconds(i64::MAX));
        assert_eq!(lease.expires_at_epoch_ms, i64::MAX);
    }
}
