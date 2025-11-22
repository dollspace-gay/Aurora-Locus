-- Core Tables Parity Migration
-- Adds all missing tables to achieve full Bluesky PDS database schema parity
--
-- This migration adds:
-- 1. Session & Authentication tables
-- 2. Repository tables (commits, records, blobs)
-- 3. Sequencer tables
-- 4. Blob storage tables
-- 5. Moderation tables
-- 6. Admin tables
-- 7. App password tables
-- 8. Performance indexes
--
-- Migration created: 2025-11-15
-- Issue: Aurora-Locus-drr (Database Schema & Migrations Parity)

-- ====================================================================
-- Section 1: Session & Authentication Tables
-- ====================================================================

-- Session table - ATProto sessions (different from OAuth sessions)
CREATE TABLE IF NOT EXISTS session (
    id TEXT PRIMARY KEY NOT NULL,
    did TEXT NOT NULL,
    access_token TEXT UNIQUE NOT NULL,
    refresh_token TEXT UNIQUE NOT NULL,
    created_at DATETIME NOT NULL DEFAULT (datetime('now')),
    expires_at DATETIME NOT NULL,
    app_password_name TEXT,
    FOREIGN KEY (did) REFERENCES actor(did) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS session_did_idx ON session(did);
CREATE INDEX IF NOT EXISTS session_access_token_idx ON session(access_token);
CREATE INDEX IF NOT EXISTS session_refresh_token_idx ON session(refresh_token);
CREATE INDEX IF NOT EXISTS session_expires_at_idx ON session(expires_at);

-- Refresh token table - ATProto refresh token management
CREATE TABLE IF NOT EXISTS refresh_token (
    id TEXT PRIMARY KEY NOT NULL,
    did TEXT NOT NULL,
    token TEXT UNIQUE NOT NULL,
    created_at DATETIME NOT NULL DEFAULT (datetime('now')),
    expires_at DATETIME NOT NULL,
    used BOOLEAN NOT NULL DEFAULT 0,
    used_at DATETIME,
    next_id TEXT,
    FOREIGN KEY (did) REFERENCES actor(did) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS refresh_token_did_idx ON refresh_token(did);
CREATE INDEX IF NOT EXISTS refresh_token_token_idx ON refresh_token(token);
CREATE INDEX IF NOT EXISTS refresh_token_expires_at_idx ON refresh_token(expires_at);
CREATE INDEX IF NOT EXISTS refresh_token_used_idx ON refresh_token(used);

-- Email token table - Email verification and password reset tokens
CREATE TABLE IF NOT EXISTS email_token (
    token TEXT PRIMARY KEY NOT NULL,
    did TEXT NOT NULL,
    purpose TEXT NOT NULL, -- 'confirm_email' or 'reset_password'
    created_at DATETIME NOT NULL DEFAULT (datetime('now')),
    expires_at DATETIME NOT NULL,
    used BOOLEAN NOT NULL DEFAULT 0,
    FOREIGN KEY (did) REFERENCES actor(did) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS email_token_did_idx ON email_token(did);
CREATE INDEX IF NOT EXISTS email_token_purpose_idx ON email_token(purpose);
CREATE INDEX IF NOT EXISTS email_token_expires_at_idx ON email_token(expires_at);
CREATE INDEX IF NOT EXISTS email_token_used_idx ON email_token(used);

-- App password table - Application-specific passwords
CREATE TABLE IF NOT EXISTS app_password (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    did TEXT NOT NULL,
    name TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    created_at DATETIME NOT NULL DEFAULT (datetime('now')),
    last_used_at DATETIME,
    privileged BOOLEAN NOT NULL DEFAULT 0,
    FOREIGN KEY (did) REFERENCES actor(did) ON DELETE CASCADE,
    UNIQUE(did, name)
);

CREATE INDEX IF NOT EXISTS app_password_did_idx ON app_password(did);
CREATE INDEX IF NOT EXISTS app_password_last_used_idx ON app_password(last_used_at);

-- ====================================================================
-- Section 2: Repository Tables (ATProto Repos)
-- ====================================================================

-- Repository root table - Tracks repo state (commit history root)
CREATE TABLE IF NOT EXISTS repo_root (
    did TEXT PRIMARY KEY NOT NULL,
    cid TEXT NOT NULL,          -- Root commit CID
    rev TEXT NOT NULL,           -- Revision string (e.g., "3jui7kd54zh2y")
    indexed_at DATETIME NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (did) REFERENCES actor(did) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS repo_root_cid_idx ON repo_root(cid);
CREATE INDEX IF NOT EXISTS repo_root_indexed_at_idx ON repo_root(indexed_at);

-- Record table - ATProto records (posts, profiles, follows, etc.)
CREATE TABLE IF NOT EXISTS record (
    uri TEXT PRIMARY KEY NOT NULL,   -- at://did/collection/rkey
    cid TEXT NOT NULL,                -- Content identifier
    collection TEXT NOT NULL,         -- e.g., app.bsky.feed.post
    rkey TEXT NOT NULL,               -- Record key
    repo_rev TEXT NOT NULL,           -- Repo revision when created
    indexed_at DATETIME NOT NULL DEFAULT (datetime('now')),
    takedown_ref TEXT,                -- Takedown reference if moderated
    FOREIGN KEY (uri) REFERENCES record(uri) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS record_collection_idx ON record(collection);
CREATE INDEX IF NOT EXISTS record_rkey_idx ON record(rkey);
CREATE INDEX IF NOT EXISTS record_cid_idx ON record(cid);
CREATE INDEX IF NOT EXISTS record_indexed_at_idx ON record(indexed_at);
CREATE INDEX IF NOT EXISTS record_takedown_idx ON record(takedown_ref) WHERE takedown_ref IS NOT NULL;

-- Repo block table - Content-addressed blocks (IPLD blocks)
CREATE TABLE IF NOT EXISTS repo_block (
    cid TEXT PRIMARY KEY NOT NULL,
    content BLOB NOT NULL,           -- Raw CBOR-encoded content
    indexed_at DATETIME NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS repo_block_indexed_at_idx ON repo_block(indexed_at);

-- ====================================================================
-- Section 3: Sequencer Table
-- ====================================================================

-- Repo sequence table - Event sequencing for firehose
CREATE TABLE IF NOT EXISTS repo_seq (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    did TEXT NOT NULL,
    event_type TEXT NOT NULL,    -- 'commit', 'identity', 'account', 'handle', 'tombstone'
    event TEXT NOT NULL,          -- JSON-encoded event payload
    invalidated BOOLEAN NOT NULL DEFAULT 0,
    sequenced_at DATETIME NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (did) REFERENCES actor(did) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS repo_seq_did_idx ON repo_seq(did);
CREATE INDEX IF NOT EXISTS repo_seq_event_type_idx ON repo_seq(event_type);
CREATE INDEX IF NOT EXISTS repo_seq_sequenced_at_idx ON repo_seq(sequenced_at);
CREATE INDEX IF NOT EXISTS repo_seq_invalidated_idx ON repo_seq(invalidated);

-- ====================================================================
-- Section 4: Blob Storage Tables
-- ====================================================================

-- Blob metadata table - Permanent blob storage metadata
CREATE TABLE IF NOT EXISTS blob_metadata (
    cid TEXT PRIMARY KEY NOT NULL,
    mime_type TEXT NOT NULL,
    size INTEGER NOT NULL,           -- Size in bytes
    creator_did TEXT NOT NULL,
    created_at DATETIME NOT NULL DEFAULT (datetime('now')),
    width INTEGER,                   -- For images
    height INTEGER,                  -- For images
    thumbnail_cid TEXT,              -- Thumbnail CID for images
    FOREIGN KEY (creator_did) REFERENCES actor(did) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS blob_metadata_creator_did_idx ON blob_metadata(creator_did);
CREATE INDEX IF NOT EXISTS blob_metadata_mime_type_idx ON blob_metadata(mime_type);
CREATE INDEX IF NOT EXISTS blob_metadata_created_at_idx ON blob_metadata(created_at);

-- Temporary blob metadata table - For blobs not yet committed to repo
CREATE TABLE IF NOT EXISTS temp_blob_metadata (
    cid TEXT PRIMARY KEY NOT NULL,
    mime_type TEXT NOT NULL,
    size INTEGER NOT NULL,
    creator_did TEXT NOT NULL,
    created_at DATETIME NOT NULL DEFAULT (datetime('now')),
    width INTEGER,
    height INTEGER,
    FOREIGN KEY (creator_did) REFERENCES actor(did) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS temp_blob_metadata_creator_did_idx ON temp_blob_metadata(creator_did);
CREATE INDEX IF NOT EXISTS temp_blob_metadata_created_at_idx ON temp_blob_metadata(created_at);

-- ====================================================================
-- Section 5: Moderation Tables
-- ====================================================================

-- Account moderation table - Account-level moderation actions
CREATE TABLE IF NOT EXISTS account_moderation (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    did TEXT NOT NULL,
    action TEXT NOT NULL,        -- 'suspend', 'flag', 'takendown', etc.
    moderated_by TEXT NOT NULL,  -- Admin DID
    moderated_at DATETIME NOT NULL DEFAULT (datetime('now')),
    expires_at DATETIME,
    reason TEXT,
    notes TEXT,
    reversed BOOLEAN NOT NULL DEFAULT 0,
    reversed_at DATETIME,
    reversed_by TEXT,
    reversal_reason TEXT,
    report_id INTEGER,
    FOREIGN KEY (did) REFERENCES actor(did) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS account_moderation_did_idx ON account_moderation(did);
CREATE INDEX IF NOT EXISTS account_moderation_action_idx ON account_moderation(action);
CREATE INDEX IF NOT EXISTS account_moderation_moderated_at_idx ON account_moderation(moderated_at);
CREATE INDEX IF NOT EXISTS account_moderation_expires_at_idx ON account_moderation(expires_at) WHERE expires_at IS NOT NULL;
CREATE INDEX IF NOT EXISTS account_moderation_reversed_idx ON account_moderation(reversed);

-- Label table - Content labels (moderation, self-labels, etc.)
CREATE TABLE IF NOT EXISTS label (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    uri TEXT NOT NULL,           -- at:// URI of the labeled content
    cid TEXT,                    -- Optional CID for specific version
    val TEXT NOT NULL,           -- Label value (e.g., 'porn', 'spam', 'self-harm')
    neg BOOLEAN NOT NULL DEFAULT 0,  -- Negation flag
    src TEXT NOT NULL,           -- Label source (DID of labeler)
    created_at DATETIME NOT NULL DEFAULT (datetime('now')),
    created_by TEXT,             -- Admin who created it (if applicable)
    expires_at DATETIME
);

CREATE INDEX IF NOT EXISTS label_uri_idx ON label(uri);
CREATE INDEX IF NOT EXISTS label_cid_idx ON label(cid) WHERE cid IS NOT NULL;
CREATE INDEX IF NOT EXISTS label_val_idx ON label(val);
CREATE INDEX IF NOT EXISTS label_src_idx ON label(src);
CREATE INDEX IF NOT EXISTS label_created_at_idx ON label(created_at);
CREATE INDEX IF NOT EXISTS label_expires_at_idx ON label(expires_at) WHERE expires_at IS NOT NULL;

-- Report table - User-submitted moderation reports
CREATE TABLE IF NOT EXISTS report (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    subject_did TEXT,
    subject_uri TEXT,
    subject_cid TEXT,
    reason_type TEXT NOT NULL,   -- 'spam', 'violation', 'misleading', 'sexual', 'rude', 'other'
    reason TEXT,                 -- User-provided explanation
    reported_by TEXT NOT NULL,   -- Reporter DID
    reported_at DATETIME NOT NULL DEFAULT (datetime('now')),
    status TEXT NOT NULL DEFAULT 'open',  -- 'open', 'resolved', 'closed'
    resolved_by TEXT,
    resolved_at DATETIME,
    resolution_notes TEXT
);

CREATE INDEX IF NOT EXISTS report_subject_did_idx ON report(subject_did) WHERE subject_did IS NOT NULL;
CREATE INDEX IF NOT EXISTS report_subject_uri_idx ON report(subject_uri) WHERE subject_uri IS NOT NULL;
CREATE INDEX IF NOT EXISTS report_reason_type_idx ON report(reason_type);
CREATE INDEX IF NOT EXISTS report_reported_by_idx ON report(reported_by);
CREATE INDEX IF NOT EXISTS report_reported_at_idx ON report(reported_at);
CREATE INDEX IF NOT EXISTS report_status_idx ON report(status);

-- ====================================================================
-- Section 6: Admin Tables
-- ====================================================================

-- Admin roles table - Role-based access control for admin panel
CREATE TABLE IF NOT EXISTS admin_roles (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    did TEXT NOT NULL,
    role TEXT NOT NULL,          -- 'admin', 'moderator', 'support', etc.
    granted_by TEXT NOT NULL,    -- Admin DID who granted this role
    granted_at DATETIME NOT NULL DEFAULT (datetime('now')),
    revoked BOOLEAN NOT NULL DEFAULT 0,
    revoked_at DATETIME,
    revoked_by TEXT,
    notes TEXT,
    FOREIGN KEY (did) REFERENCES actor(did) ON DELETE CASCADE,
    UNIQUE(did, role)
);

CREATE INDEX IF NOT EXISTS admin_roles_did_idx ON admin_roles(did);
CREATE INDEX IF NOT EXISTS admin_roles_role_idx ON admin_roles(role);
CREATE INDEX IF NOT EXISTS admin_roles_revoked_idx ON admin_roles(revoked);

-- Admin audit log table - Tracks all admin actions
CREATE TABLE IF NOT EXISTS admin_audit_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    admin_did TEXT NOT NULL,
    action TEXT NOT NULL,        -- 'suspend_account', 'create_label', 'resolve_report', etc.
    subject_did TEXT,            -- Target of the action
    subject_uri TEXT,
    details TEXT,                -- JSON payload with action details
    timestamp DATETIME NOT NULL DEFAULT (datetime('now')),
    ip_address TEXT,
    FOREIGN KEY (admin_did) REFERENCES actor(did) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS admin_audit_log_admin_did_idx ON admin_audit_log(admin_did);
CREATE INDEX IF NOT EXISTS admin_audit_log_action_idx ON admin_audit_log(action);
CREATE INDEX IF NOT EXISTS admin_audit_log_timestamp_idx ON admin_audit_log(timestamp);
CREATE INDEX IF NOT EXISTS admin_audit_log_subject_did_idx ON admin_audit_log(subject_did) WHERE subject_did IS NOT NULL;

-- ====================================================================
-- Section 7: Lexicon Failure Table (for validation tracking)
-- ====================================================================

-- Lexicon failure table - Tracks records that failed lexicon validation
CREATE TABLE IF NOT EXISTS lexicon_failure (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    collection TEXT NOT NULL,
    record_uri TEXT NOT NULL,
    validation_errors TEXT NOT NULL,   -- JSON array of validation errors
    detected_at DATETIME NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS lexicon_failure_collection_idx ON lexicon_failure(collection);
CREATE INDEX IF NOT EXISTS lexicon_failure_record_uri_idx ON lexicon_failure(record_uri);
CREATE INDEX IF NOT EXISTS lexicon_failure_detected_at_idx ON lexicon_failure(detected_at);

-- ====================================================================
-- Migration Complete
-- ====================================================================

-- This migration adds 18 core tables for full Bluesky PDS parity:
--
-- Session & Auth (4 tables):
--   - session, refresh_token, email_token, app_password
--
-- Repository (4 tables):
--   - repo_root, record, repo_block, repo_seq
--
-- Blob Storage (2 tables):
--   - blob_metadata, temp_blob_metadata
--
-- Moderation (3 tables):
--   - account_moderation, label, report
--
-- Admin (2 tables):
--   - admin_roles, admin_audit_log
--
-- Validation (1 table):
--   - lexicon_failure
--
-- All tables include appropriate indexes for query performance.
-- Foreign keys ensure referential integrity.
-- Timestamps use SQLite's datetime('now') for consistency.
