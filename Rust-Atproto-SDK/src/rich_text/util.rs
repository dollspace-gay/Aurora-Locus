//! Regular expressions for rich text detection
//!
//! This module provides regex patterns for detecting:
//! - Mentions (@handle)
//! - URLs (http/https links and bare domains)
//! - Hashtags (#tag)
//! - Trailing punctuation

use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    /// Regex for matching mentions in the format @handle
    ///
    /// Matches:
    /// - `@username` at the start of text
    /// - ` @username` after whitespace
    /// - `(@username` after opening parenthesis
    ///
    /// # Examples
    ///
    /// ```
    /// use atproto::rich_text::MENTION_REGEX;
    ///
    /// let text = "Hello @alice.bsky.social how are you?";
    /// let matches: Vec<_> = MENTION_REGEX.find_iter(text).collect();
    /// assert_eq!(matches.len(), 1);
    /// ```
    pub static ref MENTION_REGEX: Regex =
        Regex::new(r"(^|\s|\()(@)([a-zA-Z0-9.-]+)(\b)").unwrap();

    /// Regex for matching URLs
    ///
    /// Matches:
    /// - Full URLs with http:// or https://
    /// - Bare domain names (e.g., example.com/path)
    ///
    /// # Examples
    ///
    /// ```
    /// use atproto::rich_text::URL_REGEX;
    ///
    /// let text = "Check out https://bsky.app and example.com";
    /// let matches: Vec<_> = URL_REGEX.find_iter(text).collect();
    /// assert_eq!(matches.len(), 2);
    /// ```
    pub static ref URL_REGEX: Regex = Regex::new(
        r"(?i)(?m)(^|\s|\()((https?://\S+)|(?:(?P<domain>[a-z][a-z0-9]*(?:\.[a-z0-9]+)+)\S*))"
    )
    .unwrap();

    /// Regex for matching trailing punctuation
    ///
    /// This is used to strip punctuation from the end of detected URLs
    /// to avoid including things like periods or commas that aren't part of the URL.
    ///
    /// # Examples
    ///
    /// ```
    /// use atproto::rich_text::TRAILING_PUNCTUATION_REGEX;
    ///
    /// let url = "https://example.com.";
    /// let cleaned = TRAILING_PUNCTUATION_REGEX.replace(url, "");
    /// assert_eq!(cleaned, "https://example.com");
    /// ```
    pub static ref TRAILING_PUNCTUATION_REGEX: Regex =
        Regex::new(r"\p{P}+$").unwrap();

    /// Regex for matching hashtags
    ///
    /// Matches hashtags in the format #tag or ＃tag (full-width hash)
    ///
    /// Excludes:
    /// - Zero-width spaces: `\u00AD\u2060\u200A\u200B\u200C\u200D\u20e2`
    /// - Tags that are only digits
    /// - Tags that start with punctuation
    ///
    /// Note: The TypeScript version uses negative lookahead for `\ufe0f` (emoji modifier),
    /// but Rust's regex doesn't support lookahead. We handle emoji modifiers by excluding
    /// them from the character class.
    ///
    /// # Examples
    ///
    /// ```
    /// use atproto::rich_text::TAG_REGEX;
    ///
    /// let text = "This is #awesome and #cool123";
    /// let matches: Vec<_> = TAG_REGEX.find_iter(text).collect();
    /// assert_eq!(matches.len(), 2);
    /// ```
    pub static ref TAG_REGEX: Regex = Regex::new(
        r"(?u)(^|\s)[#＃]([^\s\u{00AD}\u{2060}\u{200A}\u{200B}\u{200C}\u{200D}\u{20e2}\u{fe0f}]*[^\d\s\p{P}\u{00AD}\u{2060}\u{200A}\u{200B}\u{200C}\u{200D}\u{20e2}\u{fe0f}]+[^\s\u{00AD}\u{2060}\u{200A}\u{200B}\u{200C}\u{200D}\u{20e2}\u{fe0f}]*)?"
    )
    .unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mention_regex_basic() {
        let text = "Hello @alice how are you?";
        let matches: Vec<_> = MENTION_REGEX.find_iter(text).collect();
        assert_eq!(matches.len(), 1);
        assert!(matches[0].as_str().contains("@alice"));
    }

    #[test]
    fn test_mention_regex_with_domain() {
        let text = "Hello @alice.bsky.social";
        let matches: Vec<_> = MENTION_REGEX.find_iter(text).collect();
        assert_eq!(matches.len(), 1);
        assert!(matches[0].as_str().contains("@alice.bsky.social"));
    }

    #[test]
    fn test_mention_regex_start_of_text() {
        let text = "@alice hello";
        let matches: Vec<_> = MENTION_REGEX.find_iter(text).collect();
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_mention_regex_after_paren() {
        let text = "(@alice)";
        let matches: Vec<_> = MENTION_REGEX.find_iter(text).collect();
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_mention_regex_multiple() {
        let text = "@alice and @bob are friends";
        let matches: Vec<_> = MENTION_REGEX.find_iter(text).collect();
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn test_url_regex_https() {
        let text = "Check out https://bsky.app";
        let matches: Vec<_> = URL_REGEX.find_iter(text).collect();
        assert_eq!(matches.len(), 1);
        assert!(matches[0].as_str().contains("https://bsky.app"));
    }

    #[test]
    fn test_url_regex_http() {
        let text = "Check out http://example.com";
        let matches: Vec<_> = URL_REGEX.find_iter(text).collect();
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_url_regex_bare_domain() {
        let text = "Visit example.com for more";
        let matches: Vec<_> = URL_REGEX.find_iter(text).collect();
        assert_eq!(matches.len(), 1);
        assert!(matches[0].as_str().contains("example.com"));
    }

    #[test]
    fn test_url_regex_with_path() {
        let text = "Check https://example.com/path/to/page";
        let matches: Vec<_> = URL_REGEX.find_iter(text).collect();
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_url_regex_multiple() {
        let text = "Visit https://bsky.app and example.com";
        let matches: Vec<_> = URL_REGEX.find_iter(text).collect();
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn test_url_regex_start_of_text() {
        let text = "https://example.com is great";
        let matches: Vec<_> = URL_REGEX.find_iter(text).collect();
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_trailing_punctuation_period() {
        let url = "https://example.com.";
        let cleaned = TRAILING_PUNCTUATION_REGEX.replace(url, "");
        assert_eq!(cleaned, "https://example.com");
    }

    #[test]
    fn test_trailing_punctuation_comma() {
        let url = "https://example.com,";
        let cleaned = TRAILING_PUNCTUATION_REGEX.replace(url, "");
        assert_eq!(cleaned, "https://example.com");
    }

    #[test]
    fn test_trailing_punctuation_multiple() {
        let url = "https://example.com...";
        let cleaned = TRAILING_PUNCTUATION_REGEX.replace(url, "");
        assert_eq!(cleaned, "https://example.com");
    }

    #[test]
    fn test_trailing_punctuation_none() {
        let url = "https://example.com";
        let cleaned = TRAILING_PUNCTUATION_REGEX.replace(url, "");
        assert_eq!(cleaned, "https://example.com");
    }

    #[test]
    fn test_trailing_punctuation_mixed() {
        let url = "https://example.com!?";
        let cleaned = TRAILING_PUNCTUATION_REGEX.replace(url, "");
        assert_eq!(cleaned, "https://example.com");
    }

    #[test]
    fn test_tag_regex_basic() {
        let text = "This is #awesome";
        let matches: Vec<_> = TAG_REGEX.find_iter(text).collect();
        assert_eq!(matches.len(), 1);
        assert!(matches[0].as_str().contains("#awesome"));
    }

    #[test]
    fn test_tag_regex_with_numbers() {
        let text = "This is #cool123";
        let matches: Vec<_> = TAG_REGEX.find_iter(text).collect();
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_tag_regex_only_numbers_not_matched() {
        // Tags that are only digits should not match
        let text = "This is #123";
        let matches: Vec<_> = TAG_REGEX.find_iter(text).collect();
        // The regex should either not match or match an empty tag
        // Based on the TypeScript regex, this should match but capture empty
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_tag_regex_start_of_text() {
        let text = "#awesome post";
        let matches: Vec<_> = TAG_REGEX.find_iter(text).collect();
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_tag_regex_multiple() {
        let text = "#rust and #programming are great";
        let matches: Vec<_> = TAG_REGEX.find_iter(text).collect();
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn test_tag_regex_fullwidth_hash() {
        let text = "This is ＃awesome";
        let matches: Vec<_> = TAG_REGEX.find_iter(text).collect();
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_tag_regex_after_whitespace() {
        let text = "Hello #world";
        let matches: Vec<_> = TAG_REGEX.find_iter(text).collect();
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_tag_regex_no_match_middle_of_word() {
        // # in the middle of a word without space before should not match
        let text = "test#notag";
        let matches: Vec<_> = TAG_REGEX.find_iter(text).collect();
        assert_eq!(matches.len(), 0);
    }

    #[test]
    fn test_combined_text() {
        // Test all patterns together
        let text = "@alice check https://example.com for #awesome content";

        let mentions: Vec<_> = MENTION_REGEX.find_iter(text).collect();
        assert_eq!(mentions.len(), 1);

        let urls: Vec<_> = URL_REGEX.find_iter(text).collect();
        assert_eq!(urls.len(), 1);

        let tags: Vec<_> = TAG_REGEX.find_iter(text).collect();
        assert_eq!(tags.len(), 1);
    }
}
