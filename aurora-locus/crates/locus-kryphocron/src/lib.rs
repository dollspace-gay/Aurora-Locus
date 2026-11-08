//! Kryphocron integration: registry routing, the private-post write pipeline,
//! the laquna codec wrapper (friction, not encryption), rotation and audit.
//!
//! Locus uses kryphocron as a codec, registry, validation and audit library.
//! It authenticates and authorizes in its own types, never through
//! kryphocron's `AuthContext` layer, and never decodes for readers.

pub mod audit;
pub mod content;
pub mod nsid;
pub mod rotation;
pub mod rules;

pub use content::{Encoder, PipelineError};
pub use kryphocron;
pub use nsid::{Kind, classify};
pub use rotation::SharedOracle;
