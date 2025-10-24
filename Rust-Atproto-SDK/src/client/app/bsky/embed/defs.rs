//! Generated type definitions for app.bsky.embed.defs

use serde::{Deserialize, Serialize};

/// width:height represents an aspect ratio. It may be approximate, and may not correspond to absolute dimensions in any given unit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AspectRatio {
    /// width:height represents an aspect ratio. It may be approximate, and may not correspond to absolute dimensions in any given unit.
    pub height: i64,
    /// width:height represents an aspect ratio. It may be approximate, and may not correspond to absolute dimensions in any given unit.
    pub width: i64,
}


