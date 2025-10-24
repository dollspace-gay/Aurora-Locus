//! Rich text processing for ATProto
//!
//! This module provides utilities for handling rich text in ATProto, including:
//! - Unicode string handling (UTF-8 ↔ UTF-16 conversion)
//! - Mention and link detection
//! - Text sanitization
//! - Facet management

pub mod detection;
pub mod text;
pub mod sanitization;
pub mod unicode;
pub mod util;

// Re-export commonly used types
pub use unicode::UnicodeString;

// Re-export regex patterns
pub use util::{MENTION_REGEX, URL_REGEX, TRAILING_PUNCTUATION_REGEX, TAG_REGEX};

// Re-export detection types and functions
pub use detection::{Facet, FacetFeature, ByteSlice, detect_facets};

// Re-export RichText types
pub use text::{RichText, RichTextSegment};

// Re-export sanitization
pub use sanitization::{sanitize_rich_text, SanitizeOptions};
