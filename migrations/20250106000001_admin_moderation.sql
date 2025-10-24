-- Admin & Moderation Schema
-- Tables for admin panel functionality that don't exist in other migrations

-- Admin roles and permissions
CREATE TABLE IF NOT EXISTS admin_roles (
    did TEXT PRIMARY KEY NOT NULL,
    role TEXT NOT NULL CHECK(role IN ('superadmin', 'admin', 'moderator')),
    granted_by TEXT,
    granted_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (did) REFERENCES account(did) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_admin_role_did ON admin_roles(did);

-- Moderation reports
CREATE TABLE IF NOT EXISTS moderation_reports (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    reason_type TEXT NOT NULL,
    reported_by TEXT NOT NULL,
    subject_uri TEXT NOT NULL,
    reason_text TEXT,
    status TEXT NOT NULL DEFAULT 'open' CHECK(status IN ('open', 'resolved', 'dismissed')),
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    resolved_at DATETIME,
    resolved_by TEXT
);

CREATE INDEX IF NOT EXISTS idx_moderation_reports_status ON moderation_reports(status);
CREATE INDEX IF NOT EXISTS idx_moderation_reports_subject ON moderation_reports(subject_uri);
CREATE INDEX IF NOT EXISTS idx_moderation_reports_reported_by ON moderation_reports(reported_by);

-- Account moderation actions (for suspension/takedown tracking)
CREATE TABLE IF NOT EXISTS account_moderation (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    did TEXT NOT NULL,
    action TEXT NOT NULL CHECK(action IN ('suspend', 'takedown', 'flag', 'warn')),
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
    FOREIGN KEY (did) REFERENCES account(did) ON DELETE CASCADE,
    FOREIGN KEY (report_id) REFERENCES moderation_reports(id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_account_moderation_did ON account_moderation(did);
CREATE INDEX IF NOT EXISTS idx_account_moderation_expires ON account_moderation(expires_at);
CREATE INDEX IF NOT EXISTS idx_account_moderation_reversed ON account_moderation(reversed);
CREATE INDEX IF NOT EXISTS idx_account_moderation_action ON account_moderation(action);

-- Content labels (for labeling posts/media)
CREATE TABLE IF NOT EXISTS content_labels (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    uri TEXT NOT NULL,
    cid TEXT,
    val TEXT NOT NULL,
    neg INTEGER NOT NULL DEFAULT 0,
    src TEXT NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at DATETIME
);

CREATE INDEX IF NOT EXISTS idx_content_labels_uri ON content_labels(uri);
CREATE INDEX IF NOT EXISTS idx_content_labels_src ON content_labels(src);
CREATE INDEX IF NOT EXISTS idx_content_labels_val ON content_labels(val);
