//! TID generation for repo revisions and record keys.
//!
//! A TID is a 64-bit value (top bit 0): 53 bits of microseconds since the
//! epoch, then a 10-bit clock id, written as 13 characters of sortable
//! base32. Each node holds a leased clock id, so two live nodes never mint
//! the same TID. Revisions must increase per repository, so a new rev is
//! `max(now, stored rev + 1, last minted + 1)`: a clock step backwards never
//! produces a smaller rev.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const ALPHABET: &[u8; 32] = b"234567abcdefghijklmnopqrstuvwxyz";

/// Encodes a TID value as its 13-character string.
#[must_use]
pub fn encode(value: u64) -> String {
    let mut out = [0u8; 13];
    for (i, slot) in out.iter_mut().rev().enumerate() {
        *slot = ALPHABET[((value >> (5 * i)) & 31) as usize];
    }
    String::from_utf8(out.to_vec()).expect("alphabet is ASCII")
}

/// Decodes a TID string. Returns None for anything that isn't a valid TID.
#[must_use]
pub fn decode(s: &str) -> Option<u64> {
    if s.len() != 13 {
        return None;
    }
    let mut value: u64 = 0;
    for (i, b) in s.bytes().enumerate() {
        let digit = ALPHABET.iter().position(|&a| a == b)? as u64;
        // The first character carries only the top 4 bits; its high bit must be 0.
        if i == 0 && digit >= 16 {
            return None;
        }
        value = (value << 5) | digit;
    }
    Some(value)
}

/// Mints TIDs for one node.
#[derive(Debug)]
pub struct TidClock {
    clock_id: u16,
    last: Mutex<u64>,
}

impl TidClock {
    /// `clock_id` must be the node's leased slot, 0..1024.
    #[must_use]
    pub fn new(clock_id: u16) -> Self {
        assert!(clock_id < 1024, "clock id must fit 10 bits");
        Self {
            clock_id,
            last: Mutex::new(0),
        }
    }

    fn now_value(&self) -> u64 {
        let micros = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_micros())
            .unwrap_or(0);
        let micros = u64::try_from(micros).unwrap_or(u64::MAX) & ((1u64 << 53) - 1);
        (micros << 10) | u64::from(self.clock_id)
    }

    /// A TID greater than every TID this clock minted before, and greater
    /// than `after` when given (a repo's stored rev).
    pub fn next_after(&self, after: Option<&str>) -> String {
        let floor = after.and_then(decode).map_or(0, |v| v.saturating_add(1));
        let mut last = self
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let value = self.now_value().max(last.saturating_add(1)).max(floor);
        *last = value;
        encode(value)
    }

    /// A fresh TID (record keys).
    pub fn next(&self) -> String {
        self.next_after(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proto_blue_syntax::Tid;

    #[test]
    fn matches_proto_blue_encoding() {
        for (micros, clock) in [
            (0u64, 0u16),
            (1_700_000_000_000_000, 7),
            ((1 << 53) - 1, 1023),
        ] {
            let ours = encode((micros << 10) | u64::from(clock));
            assert_eq!(ours, Tid::from_timestamp(micros, clock).as_str());
            assert_eq!(decode(&ours), Some((micros << 10) | u64::from(clock)));
            assert!(Tid::is_valid(&ours));
        }
    }

    #[test]
    fn revisions_always_increase() {
        let clock = TidClock::new(3);
        let mut prev = clock.next();
        for _ in 0..1000 {
            let next = clock.next_after(Some(&prev));
            assert!(next > prev);
            prev = next;
        }
        // A stored rev from the future (another node's clock ahead) still wins.
        let future = encode(((1u64 << 52) << 10) | 9);
        let after = clock.next_after(Some(&future));
        assert!(after > future);
    }

    #[test]
    fn interop_tid_vectors() {
        let read = |f: &str| {
            let path = format!(
                "{}/../../interop-test-files/syntax/{f}",
                env!("CARGO_MANIFEST_DIR")
            );
            std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{path}: {e}"))
                .split('\n')
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        let valid = read("tid_syntax_valid.txt");
        assert!(valid.len() >= 6);
        for t in &valid {
            let v = decode(t).unwrap_or_else(|| panic!("{t:?} refused"));
            assert_eq!(encode(v), *t, "round trip");
        }
        let invalid = read("tid_syntax_invalid.txt");
        assert!(invalid.len() >= 6);
        for t in &invalid {
            assert_eq!(decode(t), None, "{t:?} accepted");
        }
    }

    #[test]
    fn rejects_invalid() {
        assert_eq!(decode("short"), None);
        assert_eq!(decode("zzzzzzzzzzzzz"), None); // high bit set
        assert_eq!(decode("2222222222221"), None); // '1' not in the alphabet
    }
}
