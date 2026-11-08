-- Email: the sign-in second factor, token lookup by hash (password reset
-- finds the account from the token alone), and whether each sender works.

ALTER TABLE account ADD COLUMN email_auth_factor_at INTEGER;

CREATE INDEX email_token_hash ON email_token (purpose, token_hash);

-- `main` sends account mail, `moderation` sends `admin.sendEmail`.
-- status: `verified` (a test email arrived), `unverified` (connect and
-- auth passed, delivery not yet confirmed) or `failed`.
CREATE TABLE email_sender (
    name        TEXT PRIMARY KEY NOT NULL CHECK (name IN ('main', 'moderation')),
    status      TEXT NOT NULL CHECK (status IN ('verified', 'unverified', 'failed')),
    url_hash    TEXT NOT NULL,
    checked_at  INTEGER NOT NULL,
    last_error  TEXT,
    failures    INTEGER NOT NULL DEFAULT 0
) STRICT;
