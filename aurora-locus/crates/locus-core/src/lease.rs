//! Lease tokens for singleton jobs. A token proves a node acquired a named lease
//! at a given epoch. Every transaction a leased job commits re-checks the
//! token inside that transaction (fencing), so a stale holder never commits.

use std::time::Duration;

/// Default lease TTL. Holders renew every third of it.
pub const DEFAULT_LEASE_TTL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseToken {
    /// Lease name, e.g. `sequencer`.
    pub name: String,
    /// The holding node's id.
    pub holder: String,
    /// The epoch this holder acquired. Every acquisition increments it.
    pub epoch: i64,
    /// TTL the lease was acquired and is renewed with.
    pub ttl: Duration,
}

impl LeaseToken {
    /// How often the holder renews.
    pub fn renew_interval(&self) -> Duration {
        self.ttl / 3
    }
}
