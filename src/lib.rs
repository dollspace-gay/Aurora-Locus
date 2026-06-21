#![recursion_limit = "512"]

//! Aurora Locus Library
//!
//! This module re-exports public modules for integration testing.
//!
//! Note: This is only used for integration tests. The main binary is in main.rs.

// Core modules
pub mod account;
pub mod actor_store;
pub mod admin;
pub mod api;
pub mod auth;
pub mod blob_store;
pub mod cache;
pub mod cascade;
pub mod cli;
pub mod config;
pub mod context;
pub mod crypto;
pub mod db;
pub mod distributed;
pub mod error;
pub mod federation;
pub mod identity;
pub mod jobs;
pub mod kryphocron;
pub mod kryphocron_audit;
pub mod kryphocron_content;
pub mod kryphocron_oracle_activity;
pub mod kryphocron_override;
pub mod kryphocron_policy;
pub mod kryphocron_rewrite;
pub mod kryphocron_rotation;
pub mod mailer;
pub mod metrics;
pub mod oauth;
pub mod rate_limit;
pub mod read_after_write; // Must come before actor_store (actor_store uses its types)
pub mod rebuild;
pub mod repo_scan;
pub mod repository;
pub mod sequencer;
pub mod sequencer_recovery;
pub mod service_auth;
pub mod themes;
pub mod validation;

// Re-export commonly used types for easier testing
pub use context::AppContext;
pub use error::{PdsError, PdsResult};
