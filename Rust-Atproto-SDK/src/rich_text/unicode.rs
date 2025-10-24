//! Unicode string utilities for rich text handling
//!
//! JavaScript uses UTF-16 encoded strings while most environments and specs
//! have standardized around UTF-8 (including JSON).
//!
//! Rich text facets need to use UTF-8 indices. This module provides tools to
//! convert indices between UTF-8 and UTF-16, which is essential for proper
//! facet handling in rich text.

use std::fmt;
use unicode_segmentation::UnicodeSegmentation;

/// A string wrapper that provides both UTF-8 and UTF-16 representations
///
/// This is necessary because:
/// - JavaScript/TypeScript uses UTF-16 encoding
/// - ATProto specs use UTF-8 byte indices for facets
/// - We need to convert between the two
///
/// # Examples
///
/// ```
/// use atproto::rich_text::unicode::UnicodeString;
///
/// let mut s = UnicodeString::new("Hello 👋 World");
///
/// // UTF-8 byte length
/// assert_eq!(s.len(), 16); // "Hello 👋 World" is 16 bytes in UTF-8
///
/// // Grapheme count (visual characters)
/// assert_eq!(s.grapheme_len(), 13); // 13 visible characters
///
/// // Slicing by UTF-8 byte indices
/// assert_eq!(s.slice(0, Some(5)), "Hello");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnicodeString {
    /// UTF-16 encoded string (original)
    utf16: String,

    /// UTF-8 encoded bytes
    utf8: Vec<u8>,

    /// Cached grapheme length
    grapheme_len: Option<usize>,
}

impl UnicodeString {
    /// Creates a new UnicodeString from a UTF-8 Rust string
    ///
    /// # Arguments
    ///
    /// * `s` - A string slice to convert
    ///
    /// # Examples
    ///
    /// ```
    /// use atproto::rich_text::unicode::UnicodeString;
    ///
    /// let us = UnicodeString::new("Hello, world!");
    /// assert_eq!(us.as_str(), "Hello, world!");
    /// ```
    pub fn new(s: impl Into<String>) -> Self {
        let utf16 = s.into();
        let utf8 = utf16.as_bytes().to_vec();

        Self {
            utf16,
            utf8,
            grapheme_len: None,
        }
    }

    /// Returns the UTF-8 byte length of the string
    ///
    /// This is the length that should be used for facet indices in ATProto.
    ///
    /// # Examples
    ///
    /// ```
    /// use atproto::rich_text::unicode::UnicodeString;
    ///
    /// let s = UnicodeString::new("Hello");
    /// assert_eq!(s.len(), 5);
    ///
    /// let emoji = UnicodeString::new("👋");
    /// assert_eq!(emoji.len(), 4); // Wave emoji is 4 bytes in UTF-8
    /// ```
    pub fn len(&self) -> usize {
        self.utf8.len()
    }

    /// Returns true if the string is empty
    pub fn is_empty(&self) -> bool {
        self.utf8.is_empty()
    }

    /// Returns the grapheme length of the string
    ///
    /// Graphemes are user-perceived characters. This is useful for
    /// character limits and display purposes.
    ///
    /// # Examples
    ///
    /// ```
    /// use atproto::rich_text::unicode::UnicodeString;
    ///
    /// let mut s = UnicodeString::new("Hello");
    /// assert_eq!(s.grapheme_len(), 5);
    ///
    /// // Emoji with skin tone modifier counts as 1 grapheme
    /// let mut emoji = UnicodeString::new("👋🏻");
    /// assert_eq!(emoji.grapheme_len(), 1);
    ///
    /// // Multiple emojis
    /// let mut emojis = UnicodeString::new("👋🏻🎉");
    /// assert_eq!(emojis.grapheme_len(), 2);
    /// ```
    pub fn grapheme_len(&mut self) -> usize {
        if let Some(len) = self.grapheme_len {
            return len;
        }

        let len = self.utf16.graphemes(true).count();
        self.grapheme_len = Some(len);
        len
    }

    /// Slices the string by UTF-8 byte indices
    ///
    /// # Arguments
    ///
    /// * `start` - Starting byte index (inclusive)
    /// * `end` - Ending byte index (exclusive), or None for end of string
    ///
    /// # Panics
    ///
    /// Panics if indices are not on UTF-8 character boundaries.
    ///
    /// # Examples
    ///
    /// ```
    /// use atproto::rich_text::unicode::UnicodeString;
    ///
    /// let s = UnicodeString::new("Hello, world!");
    /// assert_eq!(s.slice(0, Some(5)), "Hello");
    /// assert_eq!(s.slice(7, Some(12)), "world");
    /// assert_eq!(s.slice(7, None), "world!");
    /// ```
    pub fn slice(&self, start: usize, end: Option<usize>) -> String {
        let end_idx = end.unwrap_or(self.utf8.len());
        let bytes = &self.utf8[start..end_idx];
        String::from_utf8_lossy(bytes).to_string()
    }

    /// Converts a UTF-16 code unit index to a UTF-8 byte index
    ///
    /// This is necessary when converting facet indices from JavaScript
    /// (which uses UTF-16) to ATProto format (which uses UTF-8).
    ///
    /// # Arguments
    ///
    /// * `utf16_index` - Index in UTF-16 code units
    ///
    /// # Returns
    ///
    /// The corresponding index in UTF-8 bytes
    ///
    /// # Examples
    ///
    /// ```
    /// use atproto::rich_text::unicode::UnicodeString;
    ///
    /// let s = UnicodeString::new("Hello 👋");
    /// // "Hello " is 6 chars in both UTF-16 and position
    /// // The emoji "👋" is at UTF-16 index 6
    /// assert_eq!(s.utf16_index_to_utf8_index(6), 6);
    /// ```
    pub fn utf16_index_to_utf8_index(&self, utf16_index: usize) -> usize {
        // In Rust, strings are already UTF-8
        // We need to count UTF-16 code units in the original string
        let mut utf16_count = 0;
        let mut utf8_index = 0;

        for ch in self.utf16.chars() {
            if utf16_count >= utf16_index {
                break;
            }

            // Each char in Rust is a Unicode scalar value
            // Count UTF-16 code units this character would use
            let utf16_len = ch.len_utf16();
            utf16_count += utf16_len;

            // Count UTF-8 bytes
            utf8_index += ch.len_utf8();
        }

        utf8_index
    }

    /// Returns the string as a string slice
    pub fn as_str(&self) -> &str {
        &self.utf16
    }

    /// Converts to a String, consuming self
    pub fn into_string(self) -> String {
        self.utf16
    }
}

impl fmt::Display for UnicodeString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.utf16)
    }
}

impl From<String> for UnicodeString {
    fn from(s: String) -> Self {
        UnicodeString::new(s)
    }
}

impl From<&str> for UnicodeString {
    fn from(s: &str) -> Self {
        UnicodeString::new(s)
    }
}

impl AsRef<str> for UnicodeString {
    fn as_ref(&self) -> &str {
        &self.utf16
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unicode_string_new() {
        let s = UnicodeString::new("Hello");
        assert_eq!(s.as_str(), "Hello");
    }

    #[test]
    fn test_len_ascii() {
        let s = UnicodeString::new("Hello");
        assert_eq!(s.len(), 5);
    }

    #[test]
    fn test_len_emoji() {
        let s = UnicodeString::new("👋");
        assert_eq!(s.len(), 4); // Wave emoji is 4 bytes in UTF-8
    }

    #[test]
    fn test_len_mixed() {
        let s = UnicodeString::new("Hello 👋 World");
        assert_eq!(s.len(), 16); // "Hello " (6) + "👋" (4) + " World" (6)
    }

    #[test]
    fn test_is_empty() {
        let s = UnicodeString::new("");
        assert!(s.is_empty());

        let s = UnicodeString::new("Hello");
        assert!(!s.is_empty());
    }

    #[test]
    fn test_grapheme_len_ascii() {
        let mut s = UnicodeString::new("Hello");
        assert_eq!(s.grapheme_len(), 5);
    }

    #[test]
    fn test_grapheme_len_emoji() {
        let mut s = UnicodeString::new("👋");
        assert_eq!(s.grapheme_len(), 1);
    }

    #[test]
    fn test_grapheme_len_emoji_with_modifier() {
        // Emoji with skin tone modifier should count as 1 grapheme
        let mut s = UnicodeString::new("👋🏻");
        assert_eq!(s.grapheme_len(), 1);
    }

    #[test]
    fn test_grapheme_len_mixed() {
        let mut s = UnicodeString::new("Hello 👋 World");
        assert_eq!(s.grapheme_len(), 13); // H-e-l-l-o-space-wave-space-W-o-r-l-d
    }

    #[test]
    fn test_grapheme_len_cached() {
        let mut s = UnicodeString::new("Hello");
        let len1 = s.grapheme_len();
        let len2 = s.grapheme_len(); // Should use cached value
        assert_eq!(len1, len2);
    }

    #[test]
    fn test_slice_ascii() {
        let s = UnicodeString::new("Hello, world!");
        assert_eq!(s.slice(0, Some(5)), "Hello");
        assert_eq!(s.slice(7, Some(12)), "world");
    }

    #[test]
    fn test_slice_to_end() {
        let s = UnicodeString::new("Hello, world!");
        assert_eq!(s.slice(7, None), "world!");
    }

    #[test]
    fn test_slice_emoji() {
        let s = UnicodeString::new("Hello 👋");
        assert_eq!(s.slice(0, Some(6)), "Hello ");
        assert_eq!(s.slice(6, None), "👋");
    }

    #[test]
    fn test_utf16_index_to_utf8_index_ascii() {
        let s = UnicodeString::new("Hello");
        assert_eq!(s.utf16_index_to_utf8_index(0), 0);
        assert_eq!(s.utf16_index_to_utf8_index(5), 5);
    }

    #[test]
    fn test_utf16_index_to_utf8_index_emoji() {
        // "👋" is 1 UTF-16 code point (well, 2 code units for surrogates)
        // but 4 UTF-8 bytes
        let s = UnicodeString::new("👋");
        assert_eq!(s.utf16_index_to_utf8_index(0), 0);
        assert_eq!(s.utf16_index_to_utf8_index(2), 4); // After the emoji (2 UTF-16 units)
    }

    #[test]
    fn test_utf16_index_to_utf8_index_mixed() {
        let s = UnicodeString::new("Hello 👋");
        assert_eq!(s.utf16_index_to_utf8_index(0), 0); // Start
        assert_eq!(s.utf16_index_to_utf8_index(6), 6); // After "Hello "
        assert_eq!(s.utf16_index_to_utf8_index(8), 10); // After emoji (6 bytes + 4 bytes)
    }

    #[test]
    fn test_display() {
        let s = UnicodeString::new("Hello 👋");
        assert_eq!(format!("{}", s), "Hello 👋");
    }

    #[test]
    fn test_from_string() {
        let s: UnicodeString = "Hello".to_string().into();
        assert_eq!(s.as_str(), "Hello");
    }

    #[test]
    fn test_from_str() {
        let s: UnicodeString = "Hello".into();
        assert_eq!(s.as_str(), "Hello");
    }

    #[test]
    fn test_as_ref() {
        let s = UnicodeString::new("Hello");
        let r: &str = s.as_ref();
        assert_eq!(r, "Hello");
    }

    #[test]
    fn test_into_string() {
        let s = UnicodeString::new("Hello");
        let string = s.into_string();
        assert_eq!(string, "Hello");
    }

    #[test]
    fn test_clone() {
        let s1 = UnicodeString::new("Hello");
        let s2 = s1.clone();
        assert_eq!(s1, s2);
    }

    #[test]
    fn test_equality() {
        let s1 = UnicodeString::new("Hello");
        let s2 = UnicodeString::new("Hello");
        let s3 = UnicodeString::new("World");

        assert_eq!(s1, s2);
        assert_ne!(s1, s3);
    }

    #[test]
    fn test_complex_unicode() {
        // Test with various complex Unicode scenarios
        let s = UnicodeString::new("Héllo 世界 🌍");

        // UTF-8 byte length
        assert!(s.len() > 10); // Accented chars and emoji take more bytes

        // Grapheme length: H-é-l-l-o-space-世-界-space-🌍
        let mut s_mut = s.clone();
        assert_eq!(s_mut.grapheme_len(), 10);
    }

    #[test]
    fn test_emoji_sequences() {
        // Test emoji sequences (like flags, family emojis, etc.)
        let mut s = UnicodeString::new("👨‍👩‍👧‍👦"); // Family emoji
        // This is multiple code points but displays as one grapheme
        assert_eq!(s.grapheme_len(), 1);
    }
}
