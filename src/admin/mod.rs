//! Admin and Moderation System
//!
//! Handles administrative functions including role management,
//! account moderation, labeling, and invite codes.

pub mod appeals;
pub mod events;
pub mod invites;
pub mod labels;
pub mod moderation;
pub mod reports;
pub mod roles;

pub use appeals::{Appeal, AppealManager, AppealStatus};
pub use events::{ModerationEvent, ModerationEventLogger, ModerationEventType};
pub use invites::{InviteCode, InviteCodeManager};
pub use labels::{Label, LabelManager};
pub use moderation::{ModerationAction, ModerationManager, ModerationRecord};
pub use reports::{Report, ReportManager, ReportReason, ReportStatus};
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
