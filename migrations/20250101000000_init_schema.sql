-- Initial Schema Migration
-- Creates the base account table required for subsequent migrations
--
-- Migration created: 2025-01-01
-- This is the first migration that sets up the base account table

-- ====================================================================
-- Initial account table (will be migrated to actor + account later)
-- ====================================================================
CREATE TABLE IF NOT EXISTS account (
    did TEXT PRIMARY KEY NOT NULL,
    handle TEXT UNIQUE NOT NULL,
    email TEXT UNIQUE,
    password_hash TEXT NOT NULL,
    created_at DATETIME NOT NULL DEFAULT (datetime('now')),
    email_confirmed BOOLEAN NOT NULL DEFAULT 0,
    email_confirmed_at DATETIME,
    taken_down BOOLEAN NOT NULL DEFAULT 0,
    deactivated_at DATETIME,
    invites_disabled BOOLEAN NOT NULL DEFAULT 0,
    plc_rotation_key TEXT,
    plc_rotation_key_public TEXT,
    plc_last_operation_cid TEXT
);

CREATE INDEX IF NOT EXISTS account_handle_idx ON account(handle);
CREATE INDEX IF NOT EXISTS account_email_idx ON account(email) WHERE email IS NOT NULL;
CREATE INDEX IF NOT EXISTS account_created_at_idx ON account(created_at);

-- ====================================================================
-- Invite codes table
-- ====================================================================
CREATE TABLE IF NOT EXISTS invite_code (
    code TEXT PRIMARY KEY NOT NULL,
    created_by TEXT NOT NULL,
    uses_remaining INTEGER NOT NULL,
    created_at DATETIME NOT NULL DEFAULT (datetime('now')),
    expires_at DATETIME,
    disabled BOOLEAN NOT NULL DEFAULT 0,
    note TEXT,
    for_account TEXT,
    FOREIGN KEY (created_by) REFERENCES account(did) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS invite_code_created_by_idx ON invite_code(created_by);
CREATE INDEX IF NOT EXISTS invite_code_expires_at_idx ON invite_code(expires_at) WHERE expires_at IS NOT NULL;
CREATE INDEX IF NOT EXISTS invite_code_disabled_idx ON invite_code(disabled);

-- ====================================================================
-- Migration Complete
-- ====================================================================
