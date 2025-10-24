//! Generated code for com.atproto.lexicon.schema
//!
//! Representation of Lexicon schemas themselves, when published as atproto records. Note that the schema language is not defined in Lexicon; this meta schema currently only includes a single version field ('lexicon'). See the atproto specifications for description of the other expected top-level fields ('id', 'defs', etc).

use serde::{Deserialize, Serialize};

/// Representation of Lexicon schemas themselves, when published as atproto records. Note that the schema language is not defined in Lexicon; this meta schema currently only includes a single version field ('lexicon'). See the atproto specifications for description of the other expected top-level fields ('id', 'defs', etc).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schema {
    /// Indicates the 'version' of the Lexicon language. Must be '1' for the current atproto/Lexicon schema system.
    pub lexicon: i64,
}

