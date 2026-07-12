-- Phase 4 admin session hardening (chainlink #442): per-DID opt-in security
-- settings + a per-session bound-IP column.
--
-- `admin_security_config` is a sibling to `admin_roles`, keyed by DID. Absence
-- of a row = all defaults (no IP binding, role-default session lifetime, no
-- TOTP). A row exists only for a DID that has opted into at least one feature.
--   ip_binding_enabled     — 0/1 (sqlite INTEGER); IP-bind the session at login.
--   session_lifetime_secs  — NULL = use the role-based default; else the
--                            refresh-token (idle) lifetime in seconds.
--   totp_secret_encrypted  — NULL = TOTP not enrolled; else the encrypted secret.
--   totp_confirmed_at      — NULL = enrolled-but-unconfirmed (NOT enforced at
--                            login, so a half-finished enrollment can't lock the
--                            operator out); a timestamp = TOTP enforced.
CREATE TABLE admin_security_config (
    did                   TEXT PRIMARY KEY,
    ip_binding_enabled    INTEGER NOT NULL DEFAULT 0,
    session_lifetime_secs INTEGER,
    totp_secret_encrypted TEXT,
    totp_confirmed_at     TEXT,
    updated_at            TEXT NOT NULL,
    FOREIGN KEY (did) REFERENCES actor(did)
);

-- Per-session bound IP (chainlink #442, wired by the IP-binding commit). NULL =
-- the session is not IP-bound. Nullable + additive, so existing sessions are
-- unaffected.
ALTER TABLE session ADD COLUMN bound_ip TEXT;
