//! Domain types, error types, the settings model and the storage traits
//! shared by every aurora-locus crate.

pub mod bootstrap;
pub mod error;
pub mod keys;
pub mod lease;
pub mod settings;

pub use error::{CoreError, CoreResult};
