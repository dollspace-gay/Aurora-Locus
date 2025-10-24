//! TID (Timestamp Identifier) implementation
//!
//! TIDs are 13-character base32-encoded identifiers used in ATProto for:
//! - Record keys in collections
//! - Repository commit revision numbers
//! - Sortable, URL-safe identifiers with timestamp ordering
//!
//! ## Format
//!
//! - 13 ASCII characters using base32 alphabet: "234567abcdefghijklmnopqrstuvwxyz"
//! - Encodes a 64-bit integer: timestamp (microseconds since Unix epoch) + clock ID
//! - Monotonically increasing, never repeats
//! - Zero value: "2222222222222"
//!
//! ## References
//!
//! - Spec: https://atproto.com/specs/tid
//! - TypeScript: https://github.com/bluesky-social/atproto/blob/main/packages/common-web/src/tid.ts

use std::fmt;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

/// Special base32 alphabet for TIDs (lexicographically sortable)
const TID_ALPHABET: &[u8; 32] = b"234567abcdefghijklmnopqrstuvwxyz";

/// TID length in characters
const TID_LEN: usize = 13;

/// Number of bits used for the clock identifier
const CLOCK_ID_BITS: u64 = 10;

/// Maximum clock ID value (1024 values: 0-1023)
const MAX_CLOCK_ID: u64 = (1 << CLOCK_ID_BITS) - 1;

/// Error types for TID operations
#[derive(Debug, Error, PartialEq)]
pub enum TidError {
    #[error("Invalid TID length: expected {}, got {0}", TID_LEN)]
    InvalidLength(usize),

    #[error("Invalid character in TID: '{0}'")]
    InvalidCharacter(char),

    #[error("Clock ID exceeds maximum value of {}", MAX_CLOCK_ID)]
    ClockIdTooLarge,

    #[error("System time error: {0}")]
    SystemTimeError(String),
}

/// Global state for TID generation to ensure monotonic increasing values
static TID_STATE: Mutex<Option<TidState>> = Mutex::new(None);

#[derive(Debug, Clone)]
struct TidState {
    last_timestamp: u64,
    clock_id: u64,
}

/// A Timestamp Identifier (TID)
///
/// TIDs are compact, sortable identifiers based on timestamps.
/// They are URL-safe and suitable for use as record keys in ATProto.
///
/// # Examples
///
/// ```
/// use atproto::tid::Tid;
///
/// // Generate a new TID
/// let tid = Tid::next().unwrap();
/// println!("Generated TID: {}", tid);
///
/// // Parse a TID string
/// let tid = Tid::from_str("3jui7kd54zh2y").unwrap();
///
/// // Check if a string is a valid TID
/// assert!(Tid::is_valid("3jui7kd54zh2y"));
/// assert!(!Tid::is_valid("invalid"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tid(String);

impl Tid {
    /// Create a TID from a string
    ///
    /// # Errors
    ///
    /// Returns an error if the string is not a valid TID.
    ///
    /// Note: Use `s.parse::<Tid>()` or the `FromStr` trait for idiomatic parsing.
    fn parse_tid(s: &str) -> Result<Self, TidError> {
        if s.len() != TID_LEN {
            return Err(TidError::InvalidLength(s.len()));
        }

        // Validate all characters are in the alphabet
        for ch in s.chars() {
            if !TID_ALPHABET.contains(&(ch as u8)) {
                return Err(TidError::InvalidCharacter(ch));
            }
        }

        Ok(Tid(s.to_string()))
    }

    /// Check if a string is a valid TID
    pub fn is_valid(s: &str) -> bool {
        s.parse::<Tid>().is_ok()
    }

    /// Generate the next TID
    ///
    /// This ensures TIDs are monotonically increasing and never repeat,
    /// even if multiple TIDs are generated in the same microsecond.
    ///
    /// # Errors
    ///
    /// Returns an error if the system time is unavailable.
    pub fn next() -> Result<Self, TidError> {
        let now_micros = Self::current_timestamp_micros()?;

        let mut state = TID_STATE.lock().unwrap();

        // Initialize state on first use with a random clock ID
        if state.is_none() {
            *state = Some(TidState {
                last_timestamp: 0,
                clock_id: Self::random_clock_id(),
            });
        }

        let tid_state = state.as_mut().unwrap();

        let timestamp = if now_micros <= tid_state.last_timestamp {
            // Time hasn't advanced or went backward - increment from last
            tid_state.last_timestamp + 1
        } else {
            // Time has advanced
            now_micros
        };

        tid_state.last_timestamp = timestamp;
        let clock_id = tid_state.clock_id;

        drop(state); // Release lock

        Self::from_timestamp(timestamp, clock_id)
    }

    /// Create a TID from a timestamp and clock ID
    ///
    /// # Arguments
    ///
    /// * `timestamp` - Microseconds since Unix epoch
    /// * `clock_id` - Clock identifier (0-1023)
    ///
    /// # Errors
    ///
    /// Returns an error if the clock ID exceeds the maximum value.
    pub fn from_timestamp(timestamp: u64, clock_id: u64) -> Result<Self, TidError> {
        if clock_id > MAX_CLOCK_ID {
            return Err(TidError::ClockIdTooLarge);
        }

        // Combine timestamp and clock_id into a 64-bit value
        // Top 54 bits: timestamp, bottom 10 bits: clock_id
        let value = (timestamp << CLOCK_ID_BITS) | clock_id;

        // Encode to base32
        let encoded = Self::encode_base32(value);

        Ok(Tid(encoded))
    }

    /// Get the timestamp component (microseconds since Unix epoch)
    pub fn timestamp(&self) -> u64 {
        let value = self.decode_base32();
        value >> CLOCK_ID_BITS
    }

    /// Get the clock ID component
    pub fn clock_id(&self) -> u64 {
        let value = self.decode_base32();
        value & MAX_CLOCK_ID
    }

    /// Get the TID as a string
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Encode a 64-bit value to base32 (13 characters)
    fn encode_base32(mut value: u64) -> String {
        let mut result = vec![b'2'; TID_LEN]; // Initialize with '2' (zero in our alphabet)

        for i in (0..TID_LEN).rev() {
            result[i] = TID_ALPHABET[(value & 0x1F) as usize];
            value >>= 5;
        }

        String::from_utf8(result).unwrap()
    }

    /// Decode a base32 string to a 64-bit value
    fn decode_base32(&self) -> u64 {
        let mut value = 0u64;

        for ch in self.0.bytes() {
            value <<= 5;
            // Find position in alphabet
            let pos = TID_ALPHABET.iter().position(|&c| c == ch).unwrap();
            value |= pos as u64;
        }

        value
    }

    /// Get current timestamp in microseconds since Unix epoch
    fn current_timestamp_micros() -> Result<u64, TidError> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_micros() as u64)
            .map_err(|e| TidError::SystemTimeError(e.to_string()))
    }

    /// Generate a random clock ID (0-1023)
    fn random_clock_id() -> u64 {
        use std::collections::hash_map::RandomState;
        use std::hash::{BuildHasher, Hasher};

        let hasher = RandomState::new().build_hasher();
        hasher.finish() & MAX_CLOCK_ID
    }
}

impl fmt::Display for Tid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for Tid {
    type Err = TidError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Tid::parse_tid(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tid_zero_value() {
        let tid = Tid::from_timestamp(0, 0).unwrap();
        assert_eq!(tid.as_str(), "2222222222222");
    }

    #[test]
    fn test_tid_encoding_decoding() {
        let timestamp = 1234567890123456u64;
        let clock_id = 42u64;

        let tid = Tid::from_timestamp(timestamp, clock_id).unwrap();
        assert_eq!(tid.timestamp(), timestamp);
        assert_eq!(tid.clock_id(), clock_id);
    }

    #[test]
    fn test_tid_length() {
        let tid = Tid::next().unwrap();
        assert_eq!(tid.as_str().len(), TID_LEN);
    }

    #[test]
    fn test_tid_valid_characters() {
        let tid = Tid::next().unwrap();
        for ch in tid.as_str().chars() {
            assert!(TID_ALPHABET.contains(&(ch as u8)));
        }
    }

    #[test]
    fn test_tid_from_str_valid() {
        let tid = Tid::from_str("3jui7kd54zh2y").unwrap();
        assert_eq!(tid.as_str(), "3jui7kd54zh2y");
    }

    #[test]
    fn test_tid_from_str_invalid_length() {
        let result = Tid::from_str("tooshort");
        assert!(matches!(result, Err(TidError::InvalidLength(_))));
    }

    #[test]
    fn test_tid_from_str_invalid_character() {
        let result = Tid::from_str("1234567890123"); // '1' not in alphabet
        assert!(matches!(result, Err(TidError::InvalidCharacter(_))));
    }

    #[test]
    fn test_tid_is_valid() {
        assert!(Tid::is_valid("3jui7kd54zh2y"));
        assert!(Tid::is_valid("2222222222222"));
        assert!(!Tid::is_valid("invalid"));
        assert!(!Tid::is_valid(""));
        assert!(!Tid::is_valid("1234567890123"));
    }

    #[test]
    fn test_tid_monotonic_increasing() {
        let tid1 = Tid::next().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1));
        let tid2 = Tid::next().unwrap();

        // TID strings should be lexicographically sortable
        assert!(tid2.as_str() > tid1.as_str());
        assert!(tid2 > tid1);
    }

    #[test]
    fn test_tid_multiple_in_same_microsecond() {
        // Generate multiple TIDs rapidly
        let mut tids = vec![];
        for _ in 0..10 {
            tids.push(Tid::next().unwrap());
        }

        // All should be unique and increasing
        for i in 1..tids.len() {
            assert!(tids[i] > tids[i - 1]);
            assert_ne!(tids[i], tids[i - 1]);
        }
    }

    #[test]
    fn test_tid_clock_id_bounds() {
        // Valid clock IDs
        assert!(Tid::from_timestamp(1000, 0).is_ok());
        assert!(Tid::from_timestamp(1000, MAX_CLOCK_ID).is_ok());

        // Invalid clock ID
        assert!(matches!(
            Tid::from_timestamp(1000, MAX_CLOCK_ID + 1),
            Err(TidError::ClockIdTooLarge)
        ));
    }

    #[test]
    fn test_tid_ordering() {
        let tid1 = Tid::from_timestamp(1000, 0).unwrap();
        let tid2 = Tid::from_timestamp(2000, 0).unwrap();
        let tid3 = Tid::from_timestamp(2000, 1).unwrap();

        assert!(tid1 < tid2);
        assert!(tid2 < tid3);
    }

    #[test]
    fn test_tid_display() {
        let tid = Tid::from_str("3jui7kd54zh2y").unwrap();
        assert_eq!(format!("{}", tid), "3jui7kd54zh2y");
    }

    #[test]
    fn test_tid_known_values() {
        // Test with some known timestamp values
        let tid = Tid::from_timestamp(1, 0).unwrap();
        assert_eq!(tid.timestamp(), 1);
        assert_eq!(tid.clock_id(), 0);

        let tid = Tid::from_timestamp(1000000, 100).unwrap();
        assert_eq!(tid.timestamp(), 1000000);
        assert_eq!(tid.clock_id(), 100);
    }

    #[test]
    fn test_tid_roundtrip() {
        let original = Tid::next().unwrap();
        let parsed = Tid::from_str(original.as_str()).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn test_tid_clone_eq() {
        let tid1 = Tid::next().unwrap();
        let tid2 = tid1.clone();
        assert_eq!(tid1, tid2);
    }
}
