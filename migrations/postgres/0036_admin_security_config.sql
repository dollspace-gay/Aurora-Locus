-- Phase 4 admin session hardening (chainlink #442). Postgres counterpart of
-- sqlite 0035. See that file for the full rationale. Dual-tree boolean
-- convention: pg BOOLEAN / sqlite INTEGER.
CREATE TABLE admin_security_config (
    did                   TEXT PRIMARY KEY,
    ip_binding_enabled    BOOLEAN NOT NULL DEFAULT FALSE,
    session_lifetime_secs BIGINT,
    totp_secret_encrypted TEXT,
    totp_confirmed_at     TEXT,
    updated_at            TEXT NOT NULL,
    FOREIGN KEY (did) REFERENCES actor(did)
);

ALTER TABLE session ADD COLUMN bound_ip TEXT;
