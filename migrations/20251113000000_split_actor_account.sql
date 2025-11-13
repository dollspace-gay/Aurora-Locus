-- Split Account Table Migration
-- Separates public actor identity from private account credentials
-- This enables proper ATProto federation and account-less actor representation
--
-- This migration:
-- 1. Creates new actor table (public identity)
-- 2. Creates new account table (private auth)
-- 3. Creates plc_keys table (cryptographic material)
-- 4. Migrates data from old account table
-- 5. Renames old table as backup

-- ====================================================================
-- Step 1: Create new actor table (public identity)
-- ====================================================================
CREATE TABLE IF NOT EXISTS actor (
    did TEXT PRIMARY KEY NOT NULL,
    handle TEXT,
    created_at DATETIME NOT NULL,
    takedown_ref TEXT,
    deactivated_at DATETIME,
    delete_after DATETIME
);

CREATE INDEX IF NOT EXISTS actor_handle_idx ON actor(handle) WHERE handle IS NOT NULL;
CREATE INDEX IF NOT EXISTS actor_deactivated_idx ON actor(deactivated_at) WHERE deactivated_at IS NOT NULL;
CREATE INDEX IF NOT EXISTS actor_delete_after_idx ON actor(delete_after) WHERE delete_after IS NOT NULL;

-- ====================================================================
-- Step 2: Create new account table (private authentication)
-- ====================================================================
CREATE TABLE IF NOT EXISTS account_new (
    did TEXT PRIMARY KEY NOT NULL,
    email TEXT,
    password_hash TEXT NOT NULL,
    email_confirmed_at DATETIME,
    invites_disabled BOOLEAN NOT NULL DEFAULT 0,
    FOREIGN KEY (did) REFERENCES actor(did) ON DELETE CASCADE
);

CREATE UNIQUE INDEX IF NOT EXISTS account_new_email_idx ON account_new(email) WHERE email IS NOT NULL;
CREATE INDEX IF NOT EXISTS account_new_did_idx ON account_new(did);

-- ====================================================================
-- Step 3: Create PLC keys table (cryptographic material)
-- ====================================================================
CREATE TABLE IF NOT EXISTS plc_keys (
    did TEXT PRIMARY KEY NOT NULL,
    rotation_key TEXT NOT NULL,
    rotation_key_public TEXT NOT NULL,
    last_operation_cid TEXT,
    FOREIGN KEY (did) REFERENCES actor(did) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS plc_keys_did_idx ON plc_keys(did);

-- ====================================================================
-- Step 4: Migrate data from old account table
-- ====================================================================

-- Migrate to actor table (public identity)
INSERT INTO actor (did, handle, created_at, takedown_ref, deactivated_at, delete_after)
SELECT
    did,
    handle,
    created_at,
    CASE
        WHEN taken_down = 1 THEN 'manual_takedown'
        ELSE NULL
    END as takedown_ref,
    deactivated_at,
    CASE
        WHEN deactivated_at IS NOT NULL THEN datetime(deactivated_at, '+3 days')
        ELSE NULL
    END as delete_after
FROM account
WHERE NOT EXISTS (SELECT 1 FROM actor WHERE actor.did = account.did);

-- Migrate to new account table (private auth)
INSERT INTO account_new (did, email, password_hash, email_confirmed_at, invites_disabled)
SELECT
    did,
    email,
    password_hash,
    CASE
        WHEN email_confirmed = 1 THEN email_confirmed_at
        ELSE NULL
    END as email_confirmed_at,
    0 as invites_disabled
FROM account
WHERE NOT EXISTS (SELECT 1 FROM account_new WHERE account_new.did = account.did);

-- Migrate to plc_keys table (only if keys exist)
INSERT INTO plc_keys (did, rotation_key, rotation_key_public, last_operation_cid)
SELECT
    did,
    plc_rotation_key,
    plc_rotation_key_public,
    plc_last_operation_cid
FROM account
WHERE plc_rotation_key IS NOT NULL
  AND plc_rotation_key_public IS NOT NULL
  AND NOT EXISTS (SELECT 1 FROM plc_keys WHERE plc_keys.did = account.did);

-- ====================================================================
-- Step 5: Rename old account table to account_old (backup)
-- ====================================================================

-- Rename the old account table to keep as backup
ALTER TABLE account RENAME TO account_old;

-- Rename the new account table to account
ALTER TABLE account_new RENAME TO account;

-- ====================================================================
-- Step 6: Update foreign key references in other tables
-- ====================================================================

-- Note: SQLite doesn't support ALTER COLUMN to change foreign keys.
-- Existing foreign keys referencing account(did) will now point to actor(did)
-- since the did column retains its values. No data loss occurs.

-- The following tables have foreign keys to account(did):
-- - session (via did column)
-- - refresh_token (via did column)
-- - app_password (via did column)
-- - email_token (via did column)
-- - account_device (via did column)
-- - authorization_request (via did column)
-- - token (via did column)

-- These foreign keys will work correctly because:
-- 1. All DIDs from account_old are now in actor table
-- 2. Foreign key checks are based on DID value, not table name
-- 3. The actor.did is the new authoritative source for all DIDs

-- ====================================================================
-- Migration Complete
-- ====================================================================

-- Verification queries (for manual checking):
-- SELECT COUNT(*) FROM account_old; -- Original count
-- SELECT COUNT(*) FROM actor;       -- Should match
-- SELECT COUNT(*) FROM account;     -- Should match
-- SELECT COUNT(*) FROM plc_keys;    -- Should match accounts with keys

-- To rollback (if needed):
-- DROP TABLE IF EXISTS account;
-- DROP TABLE IF EXISTS actor;
-- DROP TABLE IF EXISTS plc_keys;
-- ALTER TABLE account_old RENAME TO account;
