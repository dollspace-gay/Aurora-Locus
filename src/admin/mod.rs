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
pub mod moderation;
pub mod reports;
pub mod roles;
pub mod time_range;

pub use invites::{InviteCode, InviteCodeManager};
pub use labels::LabelManager;
pub use moderation::ModerationManager;
pub use reports::ReportManager;
pub use roles::{AdminRoleManager, Role};
pub use time_range::TimeRange;
