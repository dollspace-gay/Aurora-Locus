//! RichText type for managing text with facets
//!
//! This module provides the core RichText type which manages text content along with
//! facets (mentions, links, tags). It handles insertion and deletion of text while
//! maintaining correct facet byte indices.
//!
//! See the documentation at the top of the TypeScript source for detailed explanations
//! of the insert/delete scenarios.

use super::detection::{detect_facets, Facet};
use super::unicode::UnicodeString;

/// A segment of rich text, optionally with an associated facet
#[derive(Debug, Clone, PartialEq)]
pub struct RichTextSegment {
    /// The text content of this segment
    pub text: String,
    /// Optional facet metadata
    pub facet: Option<Facet>,
}

impl RichTextSegment {
    /// Create a new text segment
    pub fn new(text: String, facet: Option<Facet>) -> Self {
        Self { text, facet }
    }

    /// Check if this segment is a link
    pub fn is_link(&self) -> bool {
        self.facet.as_ref().is_some_and(|f| {
            f.features.iter().any(|feat| {
                matches!(
                    feat,
                    super::detection::FacetFeature::Link { .. }
                )
            })
        })
    }

    /// Check if this segment is a mention
    pub fn is_mention(&self) -> bool {
        self.facet.as_ref().is_some_and(|f| {
            f.features.iter().any(|feat| {
                matches!(
                    feat,
                    super::detection::FacetFeature::Mention { .. }
                )
            })
        })
    }

    /// Check if this segment is a tag
    pub fn is_tag(&self) -> bool {
        self.facet.as_ref().is_some_and(|f| {
            f.features.iter().any(|feat| {
                matches!(
                    feat,
                    super::detection::FacetFeature::Tag { .. }
                )
            })
        })
    }
}

/// Rich text with facets
///
/// This type manages text content along with facets (mentions, links, tags).
/// It provides methods for inserting and deleting text while maintaining
/// correct facet byte indices.
///
/// # Examples
///
/// ```
/// use atproto::rich_text::{RichText, detect_facets};
///
/// let mut rt = RichText::new("Hello @alice.com check https://example.com".to_string(), None);
/// rt.detect_facets_without_resolution();
///
/// // Iterate over segments
/// for segment in rt.segments() {
///     if segment.is_mention() {
///         println!("Found mention: {}", segment.text);
///     }
/// }
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct RichText {
    /// The text content with UTF-8/UTF-16 support
    pub unicode_text: UnicodeString,
    /// Facets (mentions, links, tags) with byte indices
    pub facets: Option<Vec<Facet>>,
}

impl RichText {
    /// Create a new RichText instance
    ///
    /// # Arguments
    ///
    /// * `text` - The text content
    /// * `facets` - Optional facets (will be sorted and filtered)
    pub fn new(text: String, facets: Option<Vec<Facet>>) -> Self {
        let unicode_text = UnicodeString::new(text);
        let facets = facets.map(|mut f| {
            f.retain(facet_filter);
            f.sort_by(facet_sort);
            f
        });

        Self {
            unicode_text,
            facets,
        }
    }

    /// Get the text as a string
    pub fn text(&self) -> &str {
        self.unicode_text.as_str()
    }

    /// Get the length in UTF-8 bytes
    pub fn len(&self) -> usize {
        self.unicode_text.len()
    }

    /// Check if the text is empty
    pub fn is_empty(&self) -> bool {
        self.unicode_text.is_empty()
    }

    /// Get the grapheme length (user-perceived characters)
    pub fn grapheme_len(&mut self) -> usize {
        self.unicode_text.grapheme_len()
    }

    /// Clone this RichText
    pub fn clone_deep(&self) -> Self {
        Self {
            unicode_text: self.unicode_text.clone(),
            facets: self.facets.clone(),
        }
    }

    /// Insert text at the specified byte index
    ///
    /// This updates facet indices according to the insertion rules:
    /// - Facets before the insertion point: both start and end are moved forward
    /// - Facets containing the insertion point: only end is moved forward
    /// - Facets after the insertion point: no change
    ///
    /// # Arguments
    ///
    /// * `insert_index` - Byte index where text should be inserted
    /// * `insert_text` - Text to insert
    pub fn insert(&mut self, insert_index: usize, insert_text: &str) -> &mut Self {
        // Update the text
        let before = self.unicode_text.slice(0, Some(insert_index));
        let after = self.unicode_text.slice(insert_index, None);
        let new_text = format!("{}{}{}", before, insert_text, after);
        self.unicode_text = UnicodeString::new(new_text);

        // Update facets if present
        if let Some(ref mut facets) = self.facets {
            let num_chars_added = insert_text.len();

            for facet in facets.iter_mut() {
                // Scenario A (before): insertion point is before or at facet start
                if insert_index <= facet.index.byte_start {
                    // Move both start and end by num added
                    facet.index.byte_start += num_chars_added;
                    facet.index.byte_end += num_chars_added;
                }
                // Scenario B (inner): insertion point is inside the facet
                else if insert_index > facet.index.byte_start && insert_index < facet.index.byte_end
                {
                    // Move only end by num added
                    facet.index.byte_end += num_chars_added;
                }
                // Scenario C (after): insertion point is after facet end
                // No change needed
            }
        }

        self
    }

    /// Delete text in the specified byte range
    ///
    /// This updates facet indices according to the deletion rules:
    /// - Facets entirely within deleted range: marked for removal (set to 0,0)
    /// - Facets entirely after deleted range: no change
    /// - Facets partially overlapping: adjusted appropriately
    /// - Facets entirely before deleted range: both start and end moved backward
    ///
    /// # Arguments
    ///
    /// * `remove_start_index` - Start of byte range to delete (inclusive)
    /// * `remove_end_index` - End of byte range to delete (exclusive)
    pub fn delete(&mut self, remove_start_index: usize, remove_end_index: usize) -> &mut Self {
        // Update the text
        let before = self.unicode_text.slice(0, Some(remove_start_index));
        let after = self.unicode_text.slice(remove_end_index, None);
        let new_text = format!("{}{}", before, after);
        self.unicode_text = UnicodeString::new(new_text);

        // Update facets if present
        if let Some(ref mut facets) = self.facets {
            let num_chars_removed = remove_end_index - remove_start_index;

            for facet in facets.iter_mut() {
                // Scenario A (entirely outer): deletion contains entire facet
                if remove_start_index <= facet.index.byte_start
                    && remove_end_index >= facet.index.byte_end
                {
                    // Mark for deletion
                    facet.index.byte_start = 0;
                    facet.index.byte_end = 0;
                }
                // Scenario B (entirely after): deletion is entirely after facet
                else if remove_start_index > facet.index.byte_end {
                    // No change
                }
                // Scenario C (partially after): deletion starts inside facet and extends beyond
                else if remove_start_index > facet.index.byte_start
                    && remove_start_index <= facet.index.byte_end
                    && remove_end_index > facet.index.byte_end
                {
                    // Move end to removal start
                    facet.index.byte_end = remove_start_index;
                }
                // Scenario D (entirely inner): deletion is entirely within facet
                else if remove_start_index >= facet.index.byte_start
                    && remove_end_index <= facet.index.byte_end
                {
                    // Move end by num removed
                    facet.index.byte_end -= num_chars_removed;
                }
                // Scenario E (partially before): deletion starts before facet and ends inside
                else if remove_start_index < facet.index.byte_start
                    && remove_end_index >= facet.index.byte_start
                    && remove_end_index <= facet.index.byte_end
                {
                    // Move start to removal start, move end by num removed
                    facet.index.byte_start = remove_start_index;
                    facet.index.byte_end -= num_chars_removed;
                }
                // Scenario F (entirely before): deletion is entirely before facet
                else if remove_end_index < facet.index.byte_start {
                    // Move both by num removed
                    facet.index.byte_start -= num_chars_removed;
                    facet.index.byte_end -= num_chars_removed;
                }
            }

            // Filter out facets that were marked for deletion
            facets.retain(|facet| facet.index.byte_start < facet.index.byte_end);
        }

        self
    }

    /// Get an iterator over text segments
    ///
    /// This splits the text into segments, where each segment either has no facet
    /// or is associated with exactly one facet.
    pub fn segments(&self) -> Vec<RichTextSegment> {
        let mut segments = Vec::new();

        let facets = match &self.facets {
            Some(f) if !f.is_empty() => f,
            _ => {
                // No facets, return the entire text as one segment
                segments.push(RichTextSegment::new(self.unicode_text.to_string(), None));
                return segments;
            }
        };

        let mut text_cursor = 0;
        let mut facet_cursor = 0;

        while facet_cursor < facets.len() {
            let curr_facet = &facets[facet_cursor];

            if text_cursor < curr_facet.index.byte_start {
                // Add text segment before this facet
                let text = self
                    .unicode_text
                    .slice(text_cursor, Some(curr_facet.index.byte_start));
                segments.push(RichTextSegment::new(text, None));
            } else if text_cursor > curr_facet.index.byte_start {
                // We've passed this facet, skip it
                facet_cursor += 1;
                continue;
            }

            // Add the facet segment
            if curr_facet.index.byte_start < curr_facet.index.byte_end {
                let subtext = self
                    .unicode_text
                    .slice(curr_facet.index.byte_start, Some(curr_facet.index.byte_end));

                if subtext.trim().is_empty() {
                    // Don't create facets for empty/whitespace-only text
                    segments.push(RichTextSegment::new(subtext, None));
                } else {
                    segments.push(RichTextSegment::new(subtext, Some(curr_facet.clone())));
                }
            }

            text_cursor = curr_facet.index.byte_end;
            facet_cursor += 1;
        }

        // Add any remaining text after the last facet
        if text_cursor < self.unicode_text.len() {
            let text = self.unicode_text.slice(text_cursor, None);
            segments.push(RichTextSegment::new(text, None));
        }

        segments
    }

    /// Detect facets (mentions, links, tags) without resolution
    ///
    /// This automatically detects facets in the text but does not resolve mentions
    /// to DIDs. The detected facets will overwrite any existing facets.
    pub fn detect_facets_without_resolution(&mut self) -> &mut Self {
        self.facets = detect_facets(&self.unicode_text);
        if let Some(ref mut facets) = self.facets {
            facets.sort_by(facet_sort);
        }
        self
    }
}

/// Sort facets by byte start position
fn facet_sort(a: &Facet, b: &Facet) -> std::cmp::Ordering {
    a.index.byte_start.cmp(&b.index.byte_start)
}

/// Filter facets to remove invalid ones (negative length)
fn facet_filter(facet: &Facet) -> bool {
    // Discard negative-length facets. Zero-length facets are valid.
    facet.index.byte_start <= facet.index.byte_end
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rich_text::detection::{ByteSlice, FacetFeature};

    #[test]
    fn test_rich_text_new() {
        let rt = RichText::new("Hello world".to_string(), None);
        assert_eq!(rt.text(), "Hello world");
        assert_eq!(rt.len(), 11);
        assert!(rt.facets.is_none());
    }

    #[test]
    fn test_rich_text_with_facets() {
        let facets = vec![Facet {
            index: ByteSlice {
                byte_start: 0,
                byte_end: 5,
            },
            features: vec![FacetFeature::Tag {
                tag: "test".to_string(),
            }],
        }];

        let rt = RichText::new("Hello world".to_string(), Some(facets));
        assert!(rt.facets.is_some());
        assert_eq!(rt.facets.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn test_insert_before_facet() {
        let mut rt = RichText::new(
            "hello world".to_string(),
            Some(vec![Facet {
                index: ByteSlice {
                    byte_start: 2,
                    byte_end: 7,
                },
                features: vec![],
            }]),
        );

        rt.insert(0, "test");
        assert_eq!(rt.text(), "testhello world");

        let facet = &rt.facets.as_ref().unwrap()[0];
        assert_eq!(facet.index.byte_start, 6); // 2 + 4
        assert_eq!(facet.index.byte_end, 11); // 7 + 4
    }

    #[test]
    fn test_insert_inside_facet() {
        let mut rt = RichText::new(
            "hello world".to_string(),
            Some(vec![Facet {
                index: ByteSlice {
                    byte_start: 2,
                    byte_end: 7,
                },
                features: vec![],
            }]),
        );

        rt.insert(4, "test");
        assert_eq!(rt.text(), "helltesto world");

        let facet = &rt.facets.as_ref().unwrap()[0];
        assert_eq!(facet.index.byte_start, 2); // unchanged
        assert_eq!(facet.index.byte_end, 11); // 7 + 4
    }

    #[test]
    fn test_insert_after_facet() {
        let mut rt = RichText::new(
            "hello world".to_string(),
            Some(vec![Facet {
                index: ByteSlice {
                    byte_start: 2,
                    byte_end: 7,
                },
                features: vec![],
            }]),
        );

        rt.insert(8, "test");
        assert_eq!(rt.text(), "hello wotestrld");

        let facet = &rt.facets.as_ref().unwrap()[0];
        assert_eq!(facet.index.byte_start, 2); // unchanged
        assert_eq!(facet.index.byte_end, 7); // unchanged
    }

    #[test]
    fn test_delete_entirely_outer() {
        let mut rt = RichText::new(
            "hello world".to_string(),
            Some(vec![Facet {
                index: ByteSlice {
                    byte_start: 2,
                    byte_end: 7,
                },
                features: vec![],
            }]),
        );

        rt.delete(0, 9);
        assert_eq!(rt.text(), "ld");
        assert!(rt.facets.as_ref().unwrap().is_empty()); // Facet was deleted
    }

    #[test]
    fn test_delete_entirely_after() {
        let mut rt = RichText::new(
            "hello world".to_string(),
            Some(vec![Facet {
                index: ByteSlice {
                    byte_start: 2,
                    byte_end: 7,
                },
                features: vec![],
            }]),
        );

        rt.delete(7, 11);
        assert_eq!(rt.text(), "hello w");

        let facet = &rt.facets.as_ref().unwrap()[0];
        assert_eq!(facet.index.byte_start, 2); // unchanged
        assert_eq!(facet.index.byte_end, 7); // unchanged
    }

    #[test]
    fn test_delete_partially_after() {
        let mut rt = RichText::new(
            "hello world".to_string(),
            Some(vec![Facet {
                index: ByteSlice {
                    byte_start: 2,
                    byte_end: 7,
                },
                features: vec![],
            }]),
        );

        rt.delete(4, 11);
        assert_eq!(rt.text(), "hell");

        let facet = &rt.facets.as_ref().unwrap()[0];
        assert_eq!(facet.index.byte_start, 2);
        assert_eq!(facet.index.byte_end, 4); // moved to removal start
    }

    #[test]
    fn test_delete_entirely_inner() {
        let mut rt = RichText::new(
            "hello world".to_string(),
            Some(vec![Facet {
                index: ByteSlice {
                    byte_start: 2,
                    byte_end: 7,
                },
                features: vec![],
            }]),
        );

        rt.delete(3, 5);
        assert_eq!(rt.text(), "hel world");

        let facet = &rt.facets.as_ref().unwrap()[0];
        assert_eq!(facet.index.byte_start, 2);
        assert_eq!(facet.index.byte_end, 5); // 7 - 2
    }

    #[test]
    fn test_delete_partially_before() {
        let mut rt = RichText::new(
            "hello world".to_string(),
            Some(vec![Facet {
                index: ByteSlice {
                    byte_start: 2,
                    byte_end: 7,
                },
                features: vec![],
            }]),
        );

        rt.delete(1, 5);
        assert_eq!(rt.text(), "h world");

        let facet = &rt.facets.as_ref().unwrap()[0];
        assert_eq!(facet.index.byte_start, 1); // moved to removal start
        assert_eq!(facet.index.byte_end, 3); // 7 - 4
    }

    #[test]
    fn test_delete_entirely_before() {
        let mut rt = RichText::new(
            "hello world".to_string(),
            Some(vec![Facet {
                index: ByteSlice {
                    byte_start: 2,
                    byte_end: 7,
                },
                features: vec![],
            }]),
        );

        rt.delete(0, 2);
        assert_eq!(rt.text(), "llo world");

        let facet = &rt.facets.as_ref().unwrap()[0];
        assert_eq!(facet.index.byte_start, 0); // 2 - 2
        assert_eq!(facet.index.byte_end, 5); // 7 - 2
    }

    #[test]
    fn test_segments_no_facets() {
        let rt = RichText::new("Hello world".to_string(), None);
        let segments = rt.segments();

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "Hello world");
        assert!(segments[0].facet.is_none());
    }

    #[test]
    fn test_segments_with_facets() {
        let rt = RichText::new(
            "Hello @alice world".to_string(),
            Some(vec![Facet {
                index: ByteSlice {
                    byte_start: 6,
                    byte_end: 12,
                },
                features: vec![FacetFeature::Mention {
                    did: "alice.com".to_string(),
                }],
            }]),
        );

        let segments = rt.segments();
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0].text, "Hello ");
        assert!(segments[0].facet.is_none());
        assert_eq!(segments[1].text, "@alice");
        assert!(segments[1].is_mention());
        assert_eq!(segments[2].text, " world");
        assert!(segments[2].facet.is_none());
    }

    #[test]
    fn test_detect_facets_without_resolution() {
        let mut rt = RichText::new("Hello @alice.com".to_string(), None);
        rt.detect_facets_without_resolution();

        assert!(rt.facets.is_some());
        let facets = rt.facets.as_ref().unwrap();
        assert_eq!(facets.len(), 1);
    }

    #[test]
    fn test_clone_deep() {
        let rt = RichText::new(
            "Hello".to_string(),
            Some(vec![Facet {
                index: ByteSlice {
                    byte_start: 0,
                    byte_end: 5,
                },
                features: vec![],
            }]),
        );

        let cloned = rt.clone_deep();
        assert_eq!(rt.text(), cloned.text());
        assert_eq!(rt.facets, cloned.facets);
    }
}
