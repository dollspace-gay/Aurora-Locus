-- Actor Store Enhancements Migration
-- Adds features needed to complete Actor Store & Account Management Parity
--
-- This migration adds:
-- 1. Refresh token grace period (nextId for token chaining)
-- 2. Invite code tables (if not exists)
-- 3. Invite code usage tracking

-- ====================================================================
-- Refresh Token Grace Period
-- ====================================================================

-- Add nextId field to refresh_token for grace period support
-- This allows a 2-hour grace period where old tokens remain valid
ALTER TABLE refresh_token ADD COLUMN next_id TEXT DEFAULT NULL;

-- Create index for nextId lookups
CREATE INDEX IF NOT EXISTS refresh_token_next_id_idx ON refresh_token(next_id) WHERE next_id IS NOT NULL;

-- ====================================================================
-- Invite Code System
-- ====================================================================

-- Create invite_code table (for account growth control)
CREATE TABLE IF NOT EXISTS invite_code (
    code TEXT PRIMARY KEY NOT NULL,
    available_uses INTEGER NOT NULL DEFAULT 1,
    disabled BOOLEAN NOT NULL DEFAULT 0,
    created_by TEXT NOT NULL,
    created_at DATETIME NOT NULL,
    created_for TEXT,
    FOREIGN KEY (created_by) REFERENCES actor(did) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS invite_code_created_by_idx ON invite_code(created_by);
CREATE INDEX IF NOT EXISTS invite_code_disabled_idx ON invite_code(disabled);

-- Create invite_code_use table (tracking who used which codes)
CREATE TABLE IF NOT EXISTS invite_code_use (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    code TEXT NOT NULL,
    used_by TEXT NOT NULL,
    used_at DATETIME NOT NULL,
    FOREIGN KEY (code) REFERENCES invite_code(code) ON DELETE CASCADE,
    FOREIGN KEY (used_by) REFERENCES actor(did) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS invite_code_use_code_idx ON invite_code_use(code);
CREATE INDEX IF NOT EXISTS invite_code_use_used_by_idx ON invite_code_use(used_by);
CREATE UNIQUE INDEX IF NOT EXISTS invite_code_use_unique_idx ON invite_code_use(code, used_by);

-- ====================================================================
-- Migration Complete
-- ====================================================================
