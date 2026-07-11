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
    /// Arc 14 §7.3.6 (migration 0010): suspended-state timestamp.
    /// v0.5: populated only by test-affordance direct DB writes.
    pub suspended_at: Option<DateTime<Utc>>,
    /// Arc 14 §7.3.6 (migration 0010): desync-detected timestamp.
    /// v0.5: populated only by test-affordance direct DB writes.
    pub desynchronized_at: Option<DateTime<Utc>>,

    // Account fields (optional - may be None for federated actors)
    pub email: Option<String>,
    pub password_hash: Option<String>,
    pub email_confirmed_at: Option<DateTime<Utc>>,
    pub invites_disabled: Option<bool>,
}

/// v0.10 Arc 1 (#414) — a did:web account's public-key-only sovereign identity.
///
/// Mirrors the `did_web_account` table (migration 0027 sqlite / 0028 postgres).
/// `identity_public_key` is the holder's identity/login verification method
/// (multibase / did:key); this table deliberately has NO private-key field.
/// v0.10 (chainlink #448): the account's atproto SIGNING key is held on the PDS
/// in `plc_keys`, identical to did:plc (parity), so signing is in-process; v0.11
/// (Phase γ / did:web sovereignty) moves signing to a holder-held key over the
/// [`crate::holder_signing::HolderSigningChannel`]. `slug` is the stable minted DID segment (AD-3) and the
/// serve-route reverse-lookup key. `created_at` is RFC3339 text, matching the
/// column (not parsed to a `DateTime` — the serve path doesn't arithmetic on it).
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct DidWebAccount {
    /// The did:web DID — PK and FK to `actor(did)` (ON DELETE CASCADE).
    pub did: String,
    /// The did:web host.
    pub domain: String,
    /// Stable minted DID segment; UNIQUE; the `/user/{slug}/did.json` lookup key.
    pub slug: String,
    /// The holder's `#atproto` verification-method public key (multibase).
    pub identity_public_key: String,
    /// Creation time, RFC3339.
    pub created_at: String,
}

/// v0.10 Arc 1 Phase D (#414) — the actor-table fields the did:web serve route
/// needs, read without the `account` join. `handle` composes `alsoKnownAs`
/// (AD-2 β); `deactivated` / `taken_down` are the AD-1 serve-side gate inputs.
/// Actor-only by design: serving an identity must not depend on the presence of
/// an `account` row (which `get_account` requires for its `invites_disabled`
/// read).
#[derive(Debug, Clone)]
pub struct ActorServeState {
    pub handle: Option<String>,
    pub deactivated: bool,
    pub taken_down: bool,
}

/// PLC key storage per Arc 13 §6.3.2 key separation.
///
/// The PDS-wide rotation key (which signs PLC ops) lives in
/// `config.authentication.plc_rotation_key`, NOT in per-account
/// rows. What `plc_keys` carries per account is only the per-actor
/// atproto signing key (consumed by repo commit signing + Arc 12
/// `entryway_auth_headers`).
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct PlcKeys {
    /// DID this key belongs to.
    pub did: String,
    /// Per-actor atproto signing key (private key, hex-encoded,
    /// 32 bytes). Added by Arc 12 Step 1.5; sole crypto column
    /// per Arc 13 Step 0.7.1.
    pub atproto_signing_key: String,
    /// Last PLC operation CID (the `prev` value for the next
    /// update op).
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
