-- Initial account database schema
-- Based on TypeScript PDS account manager schema

-- Accounts table
CREATE TABLE IF NOT EXISTS account (
    did TEXT PRIMARY KEY NOT NULL,
    handle TEXT UNIQUE NOT NULL,
    email TEXT UNIQUE,
    password_hash TEXT NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    email_confirmed BOOLEAN NOT NULL DEFAULT 0,
    email_confirmed_at DATETIME,
    deactivated_at DATETIME,
    taken_down BOOLEAN NOT NULL DEFAULT 0
);

CREATE INDEX idx_account_handle ON account(handle);
CREATE INDEX idx_account_email ON account(email) WHERE email IS NOT NULL;

-- Sessions table
CREATE TABLE IF NOT EXISTS session (
    id TEXT PRIMARY KEY NOT NULL,
    did TEXT NOT NULL,
    access_token TEXT UNIQUE NOT NULL,
    refresh_token TEXT UNIQUE NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at DATETIME NOT NULL,
    app_password_name TEXT,
    FOREIGN KEY (did) REFERENCES account(did) ON DELETE CASCADE
);

CREATE INDEX idx_session_did ON session(did);
CREATE INDEX idx_session_access_token ON session(access_token);
CREATE INDEX idx_session_refresh_token ON session(refresh_token);
CREATE INDEX idx_session_expires_at ON session(expires_at);

-- Refresh tokens table
CREATE TABLE IF NOT EXISTS refresh_token (
    id TEXT PRIMARY KEY NOT NULL,
    did TEXT NOT NULL,
    token TEXT UNIQUE NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at DATETIME NOT NULL,
    used BOOLEAN NOT NULL DEFAULT 0,
    used_at DATETIME,
    FOREIGN KEY (did) REFERENCES account(did) ON DELETE CASCADE
);

CREATE INDEX idx_refresh_token_did ON refresh_token(did);
CREATE INDEX idx_refresh_token_token ON refresh_token(token);
CREATE INDEX idx_refresh_token_expires_at ON refresh_token(expires_at);

-- Email tokens (for confirmation and password reset)
CREATE TABLE IF NOT EXISTS email_token (
    token TEXT PRIMARY KEY NOT NULL,
    did TEXT NOT NULL,
    purpose TEXT NOT NULL, -- 'confirm_email' or 'reset_password'
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at DATETIME NOT NULL,
    used BOOLEAN NOT NULL DEFAULT 0,
    FOREIGN KEY (did) REFERENCES account(did) ON DELETE CASCADE
);

CREATE INDEX idx_email_token_did ON email_token(did);
CREATE INDEX idx_email_token_expires_at ON email_token(expires_at);

-- Invite codes
CREATE TABLE IF NOT EXISTS invite_code (
    code TEXT PRIMARY KEY NOT NULL,
    available_uses INTEGER NOT NULL,
    disabled BOOLEAN NOT NULL DEFAULT 0,
    created_by TEXT NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    created_for TEXT,
    FOREIGN KEY (created_by) REFERENCES account(did) ON DELETE CASCADE
);

CREATE INDEX idx_invite_code_created_by ON invite_code(created_by);

-- Invite code usage tracking
CREATE TABLE IF NOT EXISTS invite_code_use (
    code TEXT NOT NULL,
    used_by TEXT NOT NULL,
    used_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (code, used_by),
    FOREIGN KEY (code) REFERENCES invite_code(code) ON DELETE CASCADE,
    FOREIGN KEY (used_by) REFERENCES account(did) ON DELETE CASCADE
);

CREATE INDEX idx_invite_code_use_used_by ON invite_code_use(used_by);

-- App passwords (for OAuth/third-party apps)
CREATE TABLE IF NOT EXISTS app_password (
    did TEXT NOT NULL,
    name TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    privileged BOOLEAN NOT NULL DEFAULT 0,
    PRIMARY KEY (did, name),
    FOREIGN KEY (did) REFERENCES account(did) ON DELETE CASCADE
);

CREATE INDEX idx_app_password_did ON app_password(did);
