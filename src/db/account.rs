// Allow dead_code - database models for future features
#![allow(dead_code)]

/// Account database models and operations
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// Actor record - public identity information
///
/// Represents the public-facing identity of a user in the ATProto network.
/// This table can contain entries for both local accounts and federated actors
/// from other PDS instances.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Actor {
    /// Decentralized Identifier (DID) - primary key
    pub did: String,
    /// Handle (username/domain) - can be null for not-yet-claimed actors
    pub handle: Option<String>,
    /// When the actor was created
    pub created_at: DateTime<Utc>,
    /// Reference to takedown/moderation action (if any)
    pub takedown_ref: Option<String>,
    /// When the actor was deactivated (soft delete)
    pub deactivated_at: Option<DateTime<Utc>>,
    /// When the actor should be permanently deleted (after deactivation grace period)
    pub delete_after: Option<DateTime<Utc>>,
}

/// Account record - private authentication information
///
/// Represents the private authentication credentials for local accounts.
/// This table only contains entries for accounts hosted on this PDS instance.
/// Foreign key relationship: Account.did -> Actor.did
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Account {
    /// Decentralized Identifier (DID) - foreign key to actor.did
    pub did: String,
    /// Email address (optional, but required for password auth)
    pub email: Option<String>,
    /// Password hash (Argon2id)
    pub password_hash: String,
    /// When email was confirmed
    pub email_confirmed_at: Option<DateTime<Utc>>,
    /// Whether invite code generation is disabled for this account
    pub invites_disabled: bool,
}

/// Combined Actor + Account data for convenience
///
/// This struct is used when querying both tables together for operations
/// that need both public and private account information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActorAccount {
    // Actor fields
    pub did: String,
    pub handle: Option<String>,
    pub created_at: DateTime<Utc>,
    pub takedown_ref: Option<String>,
    pub deactivated_at: Option<DateTime<Utc>>,
    pub delete_after: Option<DateTime<Utc>>,

    // Account fields (optional - may be None for federated actors)
    pub email: Option<String>,
    pub password_hash: Option<String>,
    pub email_confirmed_at: Option<DateTime<Utc>>,
    pub invites_disabled: Option<bool>,
}

/// PLC (Public Ledger of Credentials) key storage
///
/// Stores the rotation keys for DID:PLC management.
/// Separate from Actor/Account to keep cryptographic material isolated.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct PlcKeys {
    /// DID this key belongs to
    pub did: String,
    /// PLC rotation key (private key, hex-encoded, 32 bytes)
    pub rotation_key: String,
    /// PLC rotation key public (compressed public key, hex-encoded, 33 bytes)
    pub rotation_key_public: String,
    /// Last PLC operation CID
    pub last_operation_cid: Option<String>,
}

/// Session record in the database
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub did: String,
    pub access_token: String,
    pub refresh_token: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub app_password_name: Option<String>,
}

/// Refresh token record
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct RefreshToken {
    pub id: String,
    pub did: String,
    pub token: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub used: bool,
    pub used_at: Option<DateTime<Utc>>,
    /// Next token ID in chain (for grace period support)
    pub next_id: Option<String>,
}

/// Email confirmation token
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct EmailToken {
    pub token: String,
    pub did: String,
    pub purpose: String, // "confirm_email" or "reset_password"
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub used: bool,
}

/// Invite code record
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct InviteCode {
    pub code: String,
    pub available_uses: i32,
    pub disabled: bool,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub created_for: Option<String>,
}

/// Invite code usage record
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct InviteCodeUse {
    pub code: String,
    pub used_by: String,
    pub used_at: DateTime<Utc>,
}

/// App password record (for OAuth/third-party apps)
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct AppPassword {
    pub did: String,
    pub name: String,
    pub password_hash: String,
    pub created_at: DateTime<Utc>,
    pub privileged: bool,
}
