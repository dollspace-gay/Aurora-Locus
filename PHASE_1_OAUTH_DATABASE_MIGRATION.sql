-- ============================================================================
-- Phase 1 OAuth Database Migration
-- ============================================================================
-- This migration adds OAuth 2.1 support to Aurora Locus PDS
-- Based on Bluesky PDS OAuth implementation analysis
--
-- Reference: PHASE_6.2_OAUTH_DPOP_COMPARISON.md
--
-- Tables to be added:
-- 1. device - Multi-device support with DPoP key binding
-- 2. account_device - Account-to-device associations
-- 3. authorization_request - OAuth authorization flow tracking
-- 4. token - Transform session/refresh_token into proper OAuth tokens
-- 5. used_refresh_token - Refresh token rotation replay detection
-- 6. authorized_client - OAuth client authorization tracking
-- 7. lexicon - OAuth scope/lexicon definitions
-- ============================================================================

-- ----------------------------------------------------------------------------
-- 1. Device Table
-- ----------------------------------------------------------------------------
-- Tracks client devices with DPoP public key binding
-- Enables multi-device OAuth support per ATProto spec
-- ----------------------------------------------------------------------------
CREATE TABLE device (
    -- Device identifier (generated UUID)
    id TEXT PRIMARY KEY NOT NULL,

    -- Session this device belongs to
    session_id TEXT NOT NULL,

    -- User agent string for device identification
    user_agent TEXT,

    -- IP address for security tracking
    ip_address TEXT,

    -- Last activity timestamp
    last_seen_at DATETIME NOT NULL,

    -- DPoP public key (JWK format) for token binding
    -- This is the critical security feature that prevents token theft
    dpop_public_key TEXT,

    -- Device creation timestamp
    created_at DATETIME NOT NULL DEFAULT (datetime('now')),

    -- Index for session lookups
    FOREIGN KEY (session_id) REFERENCES token(token_id) ON DELETE CASCADE
);

CREATE INDEX device_session_id_idx ON device(session_id);
CREATE INDEX device_last_seen_idx ON device(last_seen_at);

-- ----------------------------------------------------------------------------
-- 2. Account-Device Association Table
-- ----------------------------------------------------------------------------
-- Maps accounts to their authorized devices
-- Enables device management (list devices, revoke access)
-- ----------------------------------------------------------------------------
CREATE TABLE account_device (
    -- Auto-increment primary key
    id INTEGER PRIMARY KEY AUTOINCREMENT,

    -- Account DID
    did TEXT NOT NULL,

    -- Device identifier
    device_id TEXT NOT NULL,

    -- When device was authorized
    authorized_at DATETIME NOT NULL DEFAULT (datetime('now')),

    -- When device was last used
    last_used_at DATETIME,

    -- Device nickname (optional, user-defined)
    device_name TEXT,

    -- Is this device currently active?
    is_active BOOLEAN NOT NULL DEFAULT 1,

    -- When device was revoked (if applicable)
    revoked_at DATETIME,

    FOREIGN KEY (did) REFERENCES account(did) ON DELETE CASCADE,
    FOREIGN KEY (device_id) REFERENCES device(id) ON DELETE CASCADE,

    -- One device can only be associated with one account at a time
    UNIQUE(did, device_id)
);

CREATE INDEX account_device_did_idx ON account_device(did);
CREATE INDEX account_device_device_id_idx ON account_device(device_id);
CREATE INDEX account_device_active_idx ON account_device(is_active);

-- ----------------------------------------------------------------------------
-- 3. Authorization Request Table
-- ----------------------------------------------------------------------------
-- Tracks OAuth authorization flow state (PKCE)
-- Stores authorization codes before token exchange
-- ----------------------------------------------------------------------------
CREATE TABLE authorization_request (
    -- Auto-increment primary key
    id INTEGER PRIMARY KEY AUTOINCREMENT,

    -- Authorization request identifier
    request_id TEXT UNIQUE NOT NULL,

    -- Account DID being authorized
    did TEXT NOT NULL,

    -- OAuth client ID
    client_id TEXT NOT NULL,

    -- PKCE code challenge (SHA-256)
    code_challenge TEXT NOT NULL,

    -- PKCE code challenge method (S256)
    code_challenge_method TEXT NOT NULL DEFAULT 'S256',

    -- Authorization code (generated after approval)
    authorization_code TEXT UNIQUE,

    -- Requested OAuth scopes
    scope TEXT NOT NULL,

    -- Redirect URI
    redirect_uri TEXT NOT NULL,

    -- Authorization request state parameter
    state TEXT,

    -- When request was created
    created_at DATETIME NOT NULL DEFAULT (datetime('now')),

    -- When code expires (typically 10 minutes)
    expires_at DATETIME NOT NULL,

    -- Was the code used? (prevents reuse)
    code_used BOOLEAN NOT NULL DEFAULT 0,

    -- When code was used
    code_used_at DATETIME,

    FOREIGN KEY (did) REFERENCES account(did) ON DELETE CASCADE
);

CREATE INDEX authorization_request_did_idx ON authorization_request(did);
CREATE INDEX authorization_request_code_idx ON authorization_request(authorization_code)
    WHERE authorization_code IS NOT NULL;
CREATE INDEX authorization_request_expires_idx ON authorization_request(expires_at);

-- ----------------------------------------------------------------------------
-- 4. Token Table (replaces session + refresh_token)
-- ----------------------------------------------------------------------------
-- OAuth 2.1 tokens with DPoP binding and rotation support
-- Consolidates session management into proper OAuth token lifecycle
-- ----------------------------------------------------------------------------
CREATE TABLE token (
    -- Auto-increment primary key
    id INTEGER PRIMARY KEY AUTOINCREMENT,

    -- Token identifier (UUID)
    token_id TEXT UNIQUE NOT NULL,

    -- Account DID
    did TEXT NOT NULL,

    -- OAuth client ID
    client_id TEXT NOT NULL,

    -- Client authentication method (JSON)
    -- Stores client credentials or DPoP proof info
    client_auth TEXT NOT NULL,

    -- Device this token is bound to
    device_id TEXT,

    -- OAuth parameters (JSON)
    -- Contains grant_type, redirect_uri, etc.
    parameters TEXT NOT NULL,

    -- Additional token details (JSON, optional)
    details TEXT,

    -- Authorization code (if from authorization flow)
    code TEXT,

    -- Current refresh token
    -- Updated on each rotation
    current_refresh_token TEXT UNIQUE,

    -- OAuth scopes granted
    scope TEXT NOT NULL,

    -- Token creation timestamp
    created_at DATETIME NOT NULL DEFAULT (datetime('now')),

    -- Token last update (rotation) timestamp
    updated_at DATETIME NOT NULL DEFAULT (datetime('now')),

    -- Access token expiration
    expires_at DATETIME NOT NULL,

    FOREIGN KEY (did) REFERENCES account(did) ON DELETE CASCADE,
    FOREIGN KEY (device_id) REFERENCES device(id) ON DELETE CASCADE
);

CREATE INDEX token_did_idx ON token(did);
CREATE INDEX token_token_id_idx ON token(token_id);
CREATE INDEX token_code_idx ON token(code) WHERE code IS NOT NULL;
CREATE INDEX token_refresh_token_unique_idx ON token(current_refresh_token)
    WHERE current_refresh_token IS NOT NULL;
CREATE INDEX token_expires_idx ON token(expires_at);

-- ----------------------------------------------------------------------------
-- 5. Used Refresh Token Table
-- ----------------------------------------------------------------------------
-- Tracks used/rotated refresh tokens for replay attack detection
-- When a used token is presented again, revoke ALL tokens for that account
-- ----------------------------------------------------------------------------
CREATE TABLE used_refresh_token (
    -- Auto-increment primary key
    id INTEGER PRIMARY KEY AUTOINCREMENT,

    -- The refresh token that was used
    refresh_token TEXT UNIQUE NOT NULL,

    -- Token ID it belonged to
    token_id TEXT NOT NULL,

    -- Account DID
    did TEXT NOT NULL,

    -- When token was used/rotated
    used_at DATETIME NOT NULL DEFAULT (datetime('now')),

    -- The new token that replaced it
    new_token_id TEXT,

    FOREIGN KEY (token_id) REFERENCES token(token_id) ON DELETE CASCADE,
    FOREIGN KEY (did) REFERENCES account(did) ON DELETE CASCADE
);

CREATE INDEX used_refresh_token_token_idx ON used_refresh_token(refresh_token);
CREATE INDEX used_refresh_token_did_idx ON used_refresh_token(did);
CREATE INDEX used_refresh_token_used_at_idx ON used_refresh_token(used_at);

-- ----------------------------------------------------------------------------
-- 6. Authorized Client Table
-- ----------------------------------------------------------------------------
-- Tracks which OAuth clients have been authorized by users
-- Enables "remember this device" functionality
-- ----------------------------------------------------------------------------
CREATE TABLE authorized_client (
    -- Auto-increment primary key
    id INTEGER PRIMARY KEY AUTOINCREMENT,

    -- Account DID
    did TEXT NOT NULL,

    -- OAuth client ID
    client_id TEXT NOT NULL,

    -- OAuth scopes granted
    scope TEXT NOT NULL,

    -- When client was first authorized
    first_authorized_at DATETIME NOT NULL DEFAULT (datetime('now')),

    -- When client was last used
    last_used_at DATETIME,

    -- Is authorization still active?
    is_active BOOLEAN NOT NULL DEFAULT 1,

    -- When authorization was revoked
    revoked_at DATETIME,

    FOREIGN KEY (did) REFERENCES account(did) ON DELETE CASCADE,

    -- One client per account (but can have multiple scopes)
    UNIQUE(did, client_id)
);

CREATE INDEX authorized_client_did_idx ON authorized_client(did);
CREATE INDEX authorized_client_client_id_idx ON authorized_client(client_id);
CREATE INDEX authorized_client_active_idx ON authorized_client(is_active);

-- ----------------------------------------------------------------------------
-- 7. Lexicon Table
-- ----------------------------------------------------------------------------
-- OAuth lexicon/scope definitions
-- Maps ATProto lexicons to OAuth scopes for granular permissions
-- ----------------------------------------------------------------------------
CREATE TABLE lexicon (
    -- Lexicon NSID (e.g., "com.atproto.repo.createRecord")
    nsid TEXT PRIMARY KEY NOT NULL,

    -- OAuth scope (e.g., "atproto:repo.create")
    oauth_scope TEXT NOT NULL,

    -- Human-readable description
    description TEXT NOT NULL,

    -- Is this a privileged scope?
    is_privileged BOOLEAN NOT NULL DEFAULT 0,

    -- Lexicon category (repo, identity, admin, etc.)
    category TEXT NOT NULL,

    -- Created timestamp
    created_at DATETIME NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX lexicon_scope_idx ON lexicon(oauth_scope);
CREATE INDEX lexicon_category_idx ON lexicon(category);

-- ----------------------------------------------------------------------------
-- Data Migration: Transform existing session/refresh_token tables
-- ----------------------------------------------------------------------------
-- NOTE: This migration strategy depends on whether we want to:
-- Option A: Migrate existing sessions to new OAuth tokens (smooth transition)
-- Option B: Invalidate all existing sessions (force re-auth, more secure)
--
-- Recommended: Option B for Phase 1, Option A for production deployment
-- ----------------------------------------------------------------------------

-- OPTION B: Clean slate approach (for Phase 1 development)
-- Simply drop old tables after OAuth is fully operational

-- DROP TABLE IF EXISTS session;
-- DROP TABLE IF EXISTS refresh_token;

-- OPTION A: Migration approach (for production)
-- This would require:
-- 1. Copy session data to token table
-- 2. Generate device records for each session
-- 3. Set appropriate OAuth scopes based on session type
-- 4. Preserve refresh tokens in token.current_refresh_token
-- Example migration query (commented out, needs careful implementation):

/*
INSERT INTO token (
    token_id,
    did,
    client_id,
    client_auth,
    device_id,
    parameters,
    scope,
    current_refresh_token,
    created_at,
    updated_at,
    expires_at
)
SELECT
    session.id,                                    -- token_id
    session.did,                                   -- did
    'legacy-session',                              -- client_id (placeholder)
    '{"method":"legacy"}',                         -- client_auth
    NULL,                                          -- device_id (would need to create)
    '{"grant_type":"legacy"}',                     -- parameters
    'atproto:*',                                   -- scope (full access for legacy)
    session.refresh_token,                         -- current_refresh_token
    session.created_at,                            -- created_at
    datetime('now'),                               -- updated_at
    session.expires_at                             -- expires_at
FROM session
WHERE session.expires_at > datetime('now');        -- Only migrate active sessions
*/

-- ----------------------------------------------------------------------------
-- Cleanup: Remove legacy tables (ONLY after OAuth is fully operational)
-- ----------------------------------------------------------------------------

-- DROP TABLE IF EXISTS session;
-- DROP TABLE IF EXISTS refresh_token;

-- ============================================================================
-- Migration Complete!
-- ============================================================================
-- Next steps:
-- 1. Create Rust structs for new tables (src/oauth/models.rs)
-- 2. Implement OAuth authorization server (Phase 2)
-- 3. Integrate DPoP verification with token endpoints
-- 4. Add device management API endpoints
-- 5. Implement token rotation with replay detection
-- ============================================================================
