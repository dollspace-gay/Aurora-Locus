-- Phase β.1 (chainlink #420, Arc 2): add `access_token_hash` to the OAuth
-- `token` table + a lookup index. Postgres counterpart of sqlite 0028.
-- Closes R1 F-3.1 — the bearer-validation path was structurally broken at
-- HEAD: `oauth/token.rs` issued an access token it never persisted, and
-- `validate_oauth_token` looked the bearer up against `token_id` (the row
-- PK), so no OAuth bearer ever validated on a clean install.
--
-- The bearer is now stored as a SHA-256 hash: fast on the hot validation
-- path, and a DB compromise never yields a usable raw bearer.
-- `validate_oauth_token` looks up by this hash.
--
-- Nullable on purpose: any pre-β.1 `token` rows get NULL and never match the
-- hash lookup — correct, since those tokens were minted by the broken path
-- and never validated anyway. `token_id` stays the PK and refresh/audit
-- identity; it is simply no longer the bearer-lookup key.

ALTER TABLE token ADD COLUMN access_token_hash TEXT;

CREATE INDEX idx_token_access_hash
    ON token (access_token_hash)
    WHERE access_token_hash IS NOT NULL AND NOT revoked;
