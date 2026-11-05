//! Domain types, error types, the settings model and the storage traits
//! shared by every aurora-locus crate.

pub mod bootstrap;
pub mod error;
pub mod keys;
pub mod lease;
pub mod logs;
pub mod settings;
pub mod setup;
pub mod time;

pub use error::{CoreError, CoreResult};

/// The namespace of every locus-specific XRPC method. One constant: every
/// locus NSID is built from it (see [`locus_nsid!`]), so the namespace can
/// be swapped in one place.
pub const LOCUS_NS: &str = "app.nearhorizon.locus";

/// A locus NSID, `app.nearhorizon.locus.<name>`, as a `&'static str`.
#[macro_export]
macro_rules! locus_nsid {
    ($name:literal) => {
        concat!("app.nearhorizon.locus.", $name)
    };
}
