-- Account portal sign-ins. Each points at an ordinary account session
-- (`session`), so revoking that session (a password change, an admin
-- reset) ends the portal sign-in too. Only a hash of the id is kept.
CREATE TABLE portal_session (
    id_hash       TEXT PRIMARY KEY NOT NULL,
    did           TEXT NOT NULL,
    session_id    TEXT NOT NULL,
    created_at    INTEGER NOT NULL,
    last_seen_at  INTEGER NOT NULL,
    expires_at    INTEGER NOT NULL
) STRICT;
CREATE INDEX portal_session_did ON portal_session (did);

-- Each account's own look of the portal.
ALTER TABLE account ADD COLUMN theme_mode TEXT NOT NULL DEFAULT 'system';
ALTER TABLE account ADD COLUMN theme_scheme TEXT NOT NULL DEFAULT 'sky';
ALTER TABLE account ADD COLUMN theme_font TEXT NOT NULL DEFAULT 'hyperlegible';
