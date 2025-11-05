-- OAuth 2.1 Tables Migration
-- Adds OAuth support with DPoP token binding, PKCE, and multi-device management

-- Device table - tracks client devices with DPoP public key binding
CREATE TABLE IF NOT EXISTS device (
    id TEXT PRIMARY KEY NOT NULL,
    session_id TEXT NOT NULL,
    user_agent TEXT,
    ip_address TEXT,
    last_seen_at DATETIME NOT NULL,
    dpop_public_key TEXT,
    created_at DATETIME NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS device_session_id_idx ON device(session_id);
CREATE INDEX IF NOT EXISTS device_last_seen_idx ON device(last_seen_at);

-- Account-Device association - maps accounts to authorized devices
CREATE TABLE IF NOT EXISTS account_device (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    did TEXT NOT NULL,
    device_id TEXT NOT NULL,
    authorized_at DATETIME NOT NULL DEFAULT (datetime('now')),
    last_used_at DATETIME,
    device_name TEXT,
    is_active BOOLEAN NOT NULL DEFAULT 1,
    revoked_at DATETIME,
    FOREIGN KEY (did) REFERENCES account(did) ON DELETE CASCADE,
    FOREIGN KEY (device_id) REFERENCES device(id) ON DELETE CASCADE,
    UNIQUE(did, device_id)
);

CREATE INDEX IF NOT EXISTS account_device_did_idx ON account_device(did);
CREATE INDEX IF NOT EXISTS account_device_device_id_idx ON account_device(device_id);
CREATE INDEX IF NOT EXISTS account_device_active_idx ON account_device(is_active);

-- Authorization request - OAuth authorization flow tracking with PKCE
CREATE TABLE IF NOT EXISTS authorization_request (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    request_id TEXT UNIQUE NOT NULL,
    did TEXT NOT NULL,
    client_id TEXT NOT NULL,
    code_challenge TEXT NOT NULL,
    code_challenge_method TEXT NOT NULL DEFAULT 'S256',
    authorization_code TEXT UNIQUE,
    scope TEXT NOT NULL,
    redirect_uri TEXT NOT NULL,
    state TEXT,
    created_at DATETIME NOT NULL DEFAULT (datetime('now')),
    expires_at DATETIME NOT NULL,
    code_used BOOLEAN NOT NULL DEFAULT 0,
    code_used_at DATETIME,
    FOREIGN KEY (did) REFERENCES account(did) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS authorization_request_did_idx ON authorization_request(did);
CREATE INDEX IF NOT EXISTS authorization_request_code_idx ON authorization_request(authorization_code)
    WHERE authorization_code IS NOT NULL;
CREATE INDEX IF NOT EXISTS authorization_request_expires_idx ON authorization_request(expires_at);

-- Token table - OAuth 2.1 tokens with DPoP binding
-- Simplified schema matching actual code usage
CREATE TABLE IF NOT EXISTS token (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    token_id TEXT UNIQUE NOT NULL,
    did TEXT NOT NULL,
    client_id TEXT NOT NULL,
    current_refresh_token TEXT UNIQUE,
    scope TEXT NOT NULL,
    created_at DATETIME NOT NULL DEFAULT (datetime('now')),
    updated_at DATETIME NOT NULL DEFAULT (datetime('now')),
    expires_at DATETIME NOT NULL,
    dpop_thumbprint TEXT,
    device_id TEXT,
    FOREIGN KEY (did) REFERENCES account(did) ON DELETE CASCADE,
    FOREIGN KEY (device_id) REFERENCES device(id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS token_did_idx ON token(did);
CREATE INDEX IF NOT EXISTS token_token_id_idx ON token(token_id);
CREATE INDEX IF NOT EXISTS token_refresh_token_idx ON token(current_refresh_token)
    WHERE current_refresh_token IS NOT NULL;
CREATE INDEX IF NOT EXISTS token_expires_idx ON token(expires_at);

-- Used refresh token - tracks rotated tokens for replay detection
CREATE TABLE IF NOT EXISTS used_refresh_token (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    refresh_token TEXT UNIQUE NOT NULL,
    token_id TEXT NOT NULL,
    did TEXT NOT NULL,
    used_at DATETIME NOT NULL DEFAULT (datetime('now')),
    new_token_id TEXT,
    FOREIGN KEY (did) REFERENCES account(did) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS used_refresh_token_token_idx ON used_refresh_token(refresh_token);
CREATE INDEX IF NOT EXISTS used_refresh_token_did_idx ON used_refresh_token(did);
CREATE INDEX IF NOT EXISTS used_refresh_token_used_at_idx ON used_refresh_token(used_at);

-- Authorized client - tracks OAuth client authorizations
CREATE TABLE IF NOT EXISTS authorized_client (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    did TEXT NOT NULL,
    client_id TEXT NOT NULL,
    scope TEXT NOT NULL,
    first_authorized_at DATETIME NOT NULL DEFAULT (datetime('now')),
    last_used_at DATETIME,
    is_active BOOLEAN NOT NULL DEFAULT 1,
    revoked_at DATETIME,
    FOREIGN KEY (did) REFERENCES account(did) ON DELETE CASCADE,
    UNIQUE(did, client_id)
);

CREATE INDEX IF NOT EXISTS authorized_client_did_idx ON authorized_client(did);
CREATE INDEX IF NOT EXISTS authorized_client_client_id_idx ON authorized_client(client_id);
CREATE INDEX IF NOT EXISTS authorized_client_active_idx ON authorized_client(is_active);

-- Lexicon table - OAuth scope/lexicon mapping
CREATE TABLE IF NOT EXISTS lexicon (
    nsid TEXT PRIMARY KEY NOT NULL,
    oauth_scope TEXT NOT NULL,
    description TEXT NOT NULL,
    is_privileged BOOLEAN NOT NULL DEFAULT 0,
    category TEXT NOT NULL,
    created_at DATETIME NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS lexicon_scope_idx ON lexicon(oauth_scope);
CREATE INDEX IF NOT EXISTS lexicon_category_idx ON lexicon(category);
