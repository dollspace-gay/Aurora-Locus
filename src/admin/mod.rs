// Allow dead_code - admin audit features defined for future use
#![allow(dead_code)]

//! Admin and Moderation System
//!
//! Handles administrative functions including role management,
//! account moderation, labeling, and invite codes.

pub mod appeals;
pub mod audit_chain;
pub mod defs;
pub mod events;
pub mod invites;
pub mod labels;
pub mod migration_check;
pub mod moderation;
pub mod operator_session;
pub mod reports;
pub mod roles;
pub mod security_config;
pub mod time_range;
pub mod totp;

pub use invites::{InviteCode, InviteCodeManager};
pub use labels::LabelManager;
pub use moderation::ModerationManager;
pub use operator_session::OperatorSessionStore;
pub use reports::ReportManager;
pub use roles::{AdminRoleManager, Role};
pub use time_range::TimeRange;
