-- Admin & Moderation Schema
-- Minimal schema required for admin panel functionality

-- Admin roles and permissions
CREATE TABLE IF NOT EXISTS admin_roles (
    did TEXT PRIMARY KEY NOT NULL,
    role TEXT NOT NULL CHECK(role IN ('superadmin', 'admin', 'moderator')),
    granted_by TEXT,
    granted_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_admin_role_did ON admin_roles(did);

-- Moderation reports
CREATE TABLE IF NOT EXISTS moderation_reports (
    id TEXT PRIMARY KEY NOT NULL,
    reason_type TEXT NOT NULL,
    reported_by TEXT NOT NULL,
    subject_uri TEXT NOT NULL,
    reason_text TEXT,
    status TEXT NOT NULL DEFAULT 'open',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_moderation_reports_status ON moderation_reports(status);

-- Records table (if not exists - for posts counting)
CREATE TABLE IF NOT EXISTS records (
    did TEXT NOT NULL,
    collection TEXT NOT NULL,
    rkey TEXT NOT NULL,
    cid TEXT NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (did, collection, rkey)
);

CREATE INDEX IF NOT EXISTS idx_records_collection ON records(collection);

-- Blobs table (if not exists - for storage calculation)
CREATE TABLE IF NOT EXISTS blobs (
    cid TEXT PRIMARY KEY NOT NULL,
    size INTEGER NOT NULL,
    mime_type TEXT NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- Sessions table with last_active for active users tracking
CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY NOT NULL,
    did TEXT NOT NULL,
    access_token TEXT UNIQUE NOT NULL,
    refresh_token TEXT UNIQUE NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at DATETIME NOT NULL,
    last_active DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_sessions_did ON sessions(did);
CREATE INDEX IF NOT EXISTS idx_sessions_last_active ON sessions(last_active);

-- Accounts table (if not exists)
CREATE TABLE IF NOT EXISTS accounts (
    did TEXT PRIMARY KEY NOT NULL,
    handle TEXT UNIQUE NOT NULL,
    email TEXT UNIQUE,
    password_hash TEXT NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    status TEXT NOT NULL DEFAULT 'active'
);

CREATE INDEX IF NOT EXISTS idx_accounts_handle ON accounts(handle);
CREATE INDEX IF NOT EXISTS idx_accounts_status ON accounts(status);

-- Account moderation actions (for suspension tracking)
CREATE TABLE IF NOT EXISTS account_moderation (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    did TEXT NOT NULL,
    action TEXT NOT NULL CHECK(action IN ('suspend', 'unsuspend', 'delete', 'flag')),
    reason TEXT,
    moderated_by TEXT,
    moderated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at DATETIME,
    reversed INTEGER NOT NULL DEFAULT 0,
    reversed_at DATETIME,
    reversed_by TEXT,
    reversal_reason TEXT,
    report_id INTEGER,
    notes TEXT,
    FOREIGN KEY (did) REFERENCES account(did) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_account_moderation_did ON account_moderation(did);
CREATE INDEX IF NOT EXISTS idx_account_moderation_expires ON account_moderation(expires_at);
CREATE INDEX IF NOT EXISTS idx_account_moderation_reversed ON account_moderation(reversed);
