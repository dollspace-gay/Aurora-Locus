// Allow dead_code - admin audit features defined for future use
#![allow(dead_code)]

//! Admin and Moderation System
//!
//! Handles administrative functions including role management,
//! account moderation, labeling, and invite codes.

pub mod appeals;
pub mod defs;
pub mod events;
pub mod invites;
pub mod labels;
pub mod moderation;
pub mod reports;
pub mod roles;

pub use invites::{InviteCode, InviteCodeManager};
pub use labels::LabelManager;
pub use moderation::ModerationManager;
pub use reports::ReportManager;
pub use roles::{AdminRoleManager, Role};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Admin action audit log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogEntry {
    pub id: i64,
    pub admin_did: String,
    pub action: String,
    pub subject_did: Option<String>,
    pub details: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub ip_address: Option<String>,
}
