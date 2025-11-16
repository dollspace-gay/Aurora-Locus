#![recursion_limit = "512"]

/// Aurora Locus Library
///
/// This module re-exports public modules for integration testing.
///
/// Note: This is only used for integration tests. The main binary is in main.rs.

// Core modules
pub mod error;
pub mod oauth;
pub mod admin;
pub mod account;
pub mod auth;
pub mod cache;
pub mod config;
pub mod db;
pub mod identity;
pub mod crypto;
pub mod context;
pub mod read_after_write;  // Must come before actor_store (actor_store uses its types)
pub mod actor_store;
pub mod api;
pub mod blob_store;
pub mod car;
pub mod federation;
pub mod jobs;
pub mod mailer;
pub mod metrics;
pub mod rate_limit;
pub mod rate_limit_new;  // Distributed Redis-backed rate limiting
pub mod sequencer;
pub mod validation;
pub mod service_auth;

// Re-export commonly used types for easier testing
pub use error::{PdsError, PdsResult};
pub use context::AppContext;
