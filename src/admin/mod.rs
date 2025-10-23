/// Admin and Moderation System
///
/// Handles administrative functions including role management,
/// account moderation, labeling, and invite codes.

pub mod roles;
pub mod moderation;
pub mod labels;
pub mod invites;
pub mod reports;

pub use roles::{AdminRoleManager, Role};
pub use moderation::{ModerationAction, ModerationManager, ModerationRecord};
pub use labels::{Label, LabelManager};
pub use invites::{InviteCode, InviteCodeManager};
pub use reports::{Report, ReportManager, ReportReason, ReportStatus};

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
