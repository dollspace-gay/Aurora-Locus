//! Rotation: generation marks and the cluster-wide rotation oracle.
//!
//! Marks use laquna's lex-sortable format, `laquna/{unix seconds, 20
//! digits}/{64 hex}`, so "older than current" is a string comparison that
//! holds for imported and client-written marks too.

use std::sync::RwLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use kryphocron::{RotationContext, RotationGenerationMark, RotationOracle};
use rand::RngCore;

/// How long a node's copy of the current generation stays usable after
/// its last successful refresh.
pub const FRESHNESS_BOUND: Duration = Duration::from_secs(300);

/// How often every node refreshes it from the database.
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(30);

/// A mark for `unix_secs` with a fresh random batch id.
#[must_use]
pub fn new_mark(unix_secs: u64) -> String {
    let mut batch = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut batch);
    format!("laquna/{unix_secs:020}/{}", hex::encode(batch))
}

/// The time a well-formed mark carries.
#[must_use]
pub fn mark_secs(mark: &str) -> Option<u64> {
    let rest = mark.strip_prefix("laquna/")?;
    let (secs, batch) = rest.split_once('/')?;
    if secs.len() != 20 || batch.len() != 64 || !batch.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    secs.parse().ok()
}

/// The lowest mark for `unix_secs`: every mark dated after it compares
/// greater.
#[must_use]
pub fn mark_floor(unix_secs: u64) -> String {
    format!("laquna/{unix_secs:020}/{}", "f".repeat(64))
}

/// The current generation as one node knows it, refreshed from the database
/// by a background task. Encodes fail closed (`RotationStateUnavailable`)
/// once the copy is older than [`FRESHNESS_BOUND`].
#[derive(Debug)]
pub struct SharedOracle {
    state: RwLock<Option<(String, SystemTime)>>,
    bound: Duration,
}

impl Default for SharedOracle {
    fn default() -> Self {
        Self::new(FRESHNESS_BOUND)
    }
}

impl SharedOracle {
    #[must_use]
    pub fn new(bound: Duration) -> Self {
        Self {
            state: RwLock::new(None),
            bound,
        }
    }

    /// Records a successful refresh: the current mark, read now.
    pub fn set(&self, mark: &str) {
        *self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some((mark.to_owned(), SystemTime::now()));
    }

    /// The current mark, if the copy is fresh.
    #[must_use]
    pub fn current(&self) -> Option<String> {
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (mark, at) = state.as_ref()?;
        let age = SystemTime::now().duration_since(*at).ok()?;
        (age <= self.bound).then(|| mark.clone())
    }
}

impl RotationOracle for SharedOracle {
    fn current_generation(&self, _ctx: &RotationContext) -> Option<RotationGenerationMark> {
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state
            .as_ref()
            .and_then(|(m, _)| RotationGenerationMark::new(m.clone()).ok())
    }

    fn last_synced_at(&self) -> SystemTime {
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map_or(UNIX_EPOCH, |(_, at)| *at)
    }

    fn data_freshness_bound(&self) -> Duration {
        self.bound
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_sort_by_time() {
        let a = new_mark(1_700_000_000);
        let b = new_mark(1_700_000_001);
        assert!(a < b);
        assert_eq!(mark_secs(&a), Some(1_700_000_000));
        assert_eq!(mark_secs("laquna/123/abc"), None);
        assert!(new_mark(1_700_000_000) < mark_floor(1_700_000_000));
        assert!(mark_floor(1_700_000_000) < new_mark(1_700_000_001));
    }

    #[test]
    fn a_stale_copy_fails_closed() {
        let o = SharedOracle::new(Duration::from_millis(20));
        assert_eq!(o.current(), None);
        o.set("laquna/00000000001700000000/aa");
        assert!(o.current().is_some());
        std::thread::sleep(Duration::from_millis(40));
        assert_eq!(o.current(), None);
        let now = SystemTime::now();
        assert!(
            kryphocron::resolve_rotation_generation(
                Some(&o),
                &kryphocron::CodecId::new("laquna/0.2").unwrap(),
                &RotationContext::for_install_probe(),
                now
            )
            .is_err()
        );
    }
}

/// A new mark that sorts after `current`: dated now, or a second after
/// `current` when that's not later (two generations in one second would
/// otherwise compare by their random batch ids).
#[must_use]
pub fn next_mark(current: Option<&str>, now_secs: u64) -> String {
    let after = current.and_then(mark_secs).map_or(0, |s| s + 1);
    new_mark(now_secs.max(after))
}

#[cfg(test)]
mod next_tests {
    use super::*;

    #[test]
    fn a_new_mark_always_sorts_after_the_current_one() {
        let cur = new_mark(1_700_000_000);
        for _ in 0..50 {
            assert!(next_mark(Some(&cur), 1_700_000_000) > cur);
        }
        assert_eq!(mark_secs(&next_mark(None, 5)), Some(5));
    }
}
