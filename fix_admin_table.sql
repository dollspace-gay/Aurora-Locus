-- Emergency fix for admin_roles table
-- Run this on the VPS: sqlite3 data/account.sqlite < fix_admin_table.sql

-- Create admin_roles table if it doesn't exist
CREATE TABLE IF NOT EXISTS admin_roles (
    did TEXT PRIMARY KEY NOT NULL,
    role TEXT NOT NULL CHECK(role IN ('superadmin', 'admin', 'moderator')),
    granted_by TEXT,
    granted_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (did) REFERENCES account(did) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_admin_role_did ON admin_roles(did);

-- Insert your DID as superadmin
-- Replace 'did:plc:dzvxvsiy3maw4iarpvizsj67' with your actual DID
INSERT OR IGNORE INTO admin_roles (did, role, granted_at)
VALUES ('did:plc:dzvxvsiy3maw4iarpvizsj67', 'superadmin', datetime('now'));

-- Register this in sqlx migrations table so it doesn't try to run again
INSERT OR IGNORE INTO _sqlx_migrations (version, description, installed_on, success, checksum, execution_time)
VALUES (
    20250106000001,
    'admin_moderation',
    CURRENT_TIMESTAMP,
    1,
    X'', -- Empty checksum since we're manually applying
    0
);

SELECT 'Admin table created and ' || did || ' added as superadmin' as result FROM admin_roles WHERE did = 'did:plc:dzvxvsiy3maw4iarpvizsj67';
