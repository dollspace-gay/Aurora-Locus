//! Test-clock primitive for deterministic time-dependent tests.
//!
//! Adopted by [`crate::identity::cache::DidCache`] per Arc 9
//! Step 2 / chainlink #55 (V04_DESIGN.md §8.4.1 Item 12) to make
//! `test_stale_handle_detection` deterministic — the previous
//! implementation relied on `tokio::time::sleep` against
//! real-wall-clock TTLs and flaked under suite-wide load.
//!
//! Scope discipline (Arc 9): only `identity::cache` adopts this
//! trait in v0.4. The other ~218 `Utc::now()` call sites across
//! `src/` are not flaky and stay on direct
//! `chrono::Utc::now()`. Broader adoption is a v0.6 candidate.

use chrono::{DateTime, Utc};

/// Source of the current time.
///
/// Production code wires [`SystemClock`] (a thin wrapper over
/// `chrono::Utc::now()`). Tests wire [`MockClock`] and call
/// [`MockClock::advance`] to move time forward by a precise
/// duration without sleeping.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

/// Wall-clock implementation. Use this in every non-test
/// construction path.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// Test-controlled clock. Stays under `#[cfg(test)]` so it
/// never leaks into production builds.
#[cfg(test)]
pub struct MockClock {
    current: std::sync::Mutex<DateTime<Utc>>,
}

#[cfg(test)]
impl MockClock {
    pub fn new(start: DateTime<Utc>) -> Self {
        Self {
            current: std::sync::Mutex::new(start),
        }
    }

    /// Move the clock forward. Subsequent `now()` calls return
    /// the new value.
    pub fn advance(&self, by: chrono::Duration) {
        let mut current = self.current.lock().unwrap();
        *current += by;
    }
}

#[cfg(test)]
impl Clock for MockClock {
    fn now(&self) -> DateTime<Utc> {
        *self.current.lock().unwrap()
    }
}
