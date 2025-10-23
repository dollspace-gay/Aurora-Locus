-- Admin & Moderation Schema
-- Implements admin roles, account moderation, labels, and invite codes

-- Admin roles and permissions
CREATE TABLE IF NOT EXISTS admin_role (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    did TEXT NOT NULL UNIQUE,
    role TEXT NOT NULL CHECK(role IN ('superadmin', 'admin', 'moderator')),
    granted_by TEXT,  -- DID of granter
    granted_at TEXT NOT NULL,  -- ISO 8601 timestamp
    revoked INTEGER NOT NULL DEFAULT 0,
    revoked_at TEXT,
    revoked_by TEXT,  -- DID of revoker
    notes TEXT,
    FOREIGN KEY (granted_by) REFERENCES admin_role(did)
);

CREATE INDEX idx_admin_role_did ON admin_role(did);
CREATE INDEX idx_admin_role_active ON admin_role(did, revoked) WHERE revoked = 0;

-- Account moderation actions (takedowns, suspensions)
CREATE TABLE IF NOT EXISTS account_moderation (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    did TEXT NOT NULL,
    action TEXT NOT NULL CHECK(action IN ('takedown', 'suspend', 'flag', 'warn', 'restore')),
    reason TEXT NOT NULL,
    moderated_by TEXT NOT NULL,  -- Admin DID
    moderated_at TEXT NOT NULL,  -- ISO 8601 timestamp
    expires_at TEXT,  -- For temporary suspensions
    reversed INTEGER NOT NULL DEFAULT 0,
    reversed_at TEXT,
    reversed_by TEXT,
    reversal_reason TEXT,
    -- Metadata
    report_id INTEGER,  -- Link to report if this was from a report
    notes TEXT,
    FOREIGN KEY (moderated_by) REFERENCES admin_role(did)
);

CREATE INDEX idx_moderation_did ON account_moderation(did);
CREATE INDEX idx_moderation_action ON account_moderation(action);
CREATE INDEX idx_moderation_active ON account_moderation(did, reversed) WHERE reversed = 0;

-- Labels applied to content or accounts
CREATE TABLE IF NOT EXISTS label (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    uri TEXT NOT NULL,  -- AT-URI (at://did/collection/rkey or at://did for account labels)
    cid TEXT,  -- Optional CID for specific content version
    val TEXT NOT NULL,  -- Label value (e.g., 'porn', 'spam', 'violence')
    neg INTEGER NOT NULL DEFAULT 0,  -- Whether this is a negative label (removal)
    src TEXT NOT NULL,  -- DID of label source (usually the PDS itself)
    created_at TEXT NOT NULL,  -- ISO 8601 timestamp
    created_by TEXT NOT NULL,  -- Admin DID who applied the label
    expires_at TEXT,  -- Optional expiration
    -- Metadata
    sig BLOB,  -- Optional signature for label
    UNIQUE(uri, cid, val) ON CONFLICT REPLACE
);

CREATE INDEX idx_label_uri ON label(uri);
CREATE INDEX idx_label_val ON label(val);
CREATE INDEX idx_label_created_at ON label(created_at);

-- Invite code system
CREATE TABLE IF NOT EXISTS invite_code (
    code TEXT PRIMARY KEY,  -- The actual invite code
    available INTEGER NOT NULL DEFAULT 1,  -- Number of uses remaining
    disabled INTEGER NOT NULL DEFAULT 0,
    created_by TEXT NOT NULL,  -- DID or 'system'
    created_at TEXT NOT NULL,
    expires_at TEXT,  -- Optional expiration
    note TEXT,  -- Admin note about this code
    for_account TEXT  -- Optional: specific account this is for
);

CREATE INDEX idx_invite_code_available ON invite_code(available) WHERE available > 0 AND disabled = 0;

-- Invite code usage tracking
CREATE TABLE IF NOT EXISTS invite_code_use (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    code TEXT NOT NULL,
    used_by TEXT NOT NULL,  -- DID of account created
    used_at TEXT NOT NULL,
    FOREIGN KEY (code) REFERENCES invite_code(code),
    FOREIGN KEY (used_by) REFERENCES account(did)
);

CREATE INDEX idx_invite_use_code ON invite_code_use(code);
CREATE INDEX idx_invite_use_did ON invite_code_use(used_by);

-- Reports (user-submitted moderation reports)
CREATE TABLE IF NOT EXISTS report (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    subject_did TEXT,  -- DID of reported account
    subject_uri TEXT,  -- AT-URI of reported content
    subject_cid TEXT,  -- CID of reported content
    reason_type TEXT NOT NULL CHECK(reason_type IN (
        'spam',
        'violation',
        'misleading',
        'sexual',
        'rude',
        'other'
    )),
    reason TEXT,  -- Additional details
    reported_by TEXT NOT NULL,  -- Reporter DID
    reported_at TEXT NOT NULL,
    -- Status
    status TEXT NOT NULL DEFAULT 'open' CHECK(status IN ('open', 'acknowledged', 'resolved', 'escalated')),
    reviewed_by TEXT,  -- Admin DID
    reviewed_at TEXT,
    resolution TEXT,  -- Resolution notes
    FOREIGN KEY (reviewed_by) REFERENCES admin_role(did)
);

CREATE INDEX idx_report_status ON report(status);
CREATE INDEX idx_report_subject_did ON report(subject_did);
CREATE INDEX idx_report_subject_uri ON report(subject_uri);
CREATE INDEX idx_report_reported_at ON report(reported_at);

-- Audit log for admin actions
CREATE TABLE IF NOT EXISTS admin_audit_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    admin_did TEXT NOT NULL,
    action TEXT NOT NULL,  -- e.g., 'account.takedown', 'label.apply', 'invite.create'
    subject_did TEXT,  -- Affected DID if applicable
    details TEXT,  -- JSON with action details
    timestamp TEXT NOT NULL,
    ip_address TEXT,  -- Optional IP tracking
    FOREIGN KEY (admin_did) REFERENCES admin_role(did)
);

CREATE INDEX idx_audit_log_admin ON admin_audit_log(admin_did);
CREATE INDEX idx_audit_log_action ON admin_audit_log(action);
CREATE INDEX idx_audit_log_timestamp ON admin_audit_log(timestamp);
CREATE INDEX idx_audit_log_subject ON admin_audit_log(subject_did);
