-- v0.9 Arc E 0.9.3 / chainlink #271 — per-operator session store (§8.1.7).
--
-- Substrate for per-operator session management: list active operator
-- sessions, force-logout (revoke) a specific one, and rotate refresh
-- tokens on use. Until now AS-only admin operators (those authenticating
-- via their atproto identity with NO local `actor` row) had ZERO
-- server-side session state — their access/refresh tokens were stateless
-- HS256 JWTs (see src/api/oauth_admin.rs). The existing `session` /
-- `refresh_token` tables can't hold them: those FK to actor(did), which
-- AS-only operators don't have. This table is a dedicated, FK-free store
-- keyed by an opaque session id (`sid`, a UUID) that the access + refresh
-- tokens carry as a claim.
--
-- Dual-backend type discipline (mirrors 0013 / the #130 invariant):
--   * Every timestamp is TEXT (RFC3339), never TIMESTAMPTZ — the read
--     path goes through sqlx::Any, whose type-compat set excludes
--     chrono::DateTime<Utc>; TIMESTAMPTZ silently breaks PG reads.
--   * `revoked` is INTEGER (SQLite) / BOOLEAN (PG); read via db::read_bool.
--   * No FK on `did` — AS-only operators have no actor row (the whole
--     reason this table is separate from `session`).
--
-- `current_refresh_id` / `prev_refresh_id` carry the rotation chain
-- (chainlink #272, rotation-on-use): the refresh token embeds its id as a
-- claim; a refresh is accepted only when that id matches `current` (or
-- `prev` within the grace window), then the columns advance. `revoked`
-- and the per-request `sid` lookup back the SuperAdmin force-logout
-- (chainlink #273); enforcement lives in auth.rs's admin path.

CREATE TABLE operator_session (
    id                    TEXT PRIMARY KEY,
    did                   TEXT NOT NULL,
    created_at            TEXT NOT NULL,
    last_active_at        TEXT NOT NULL,
    expires_at            TEXT NOT NULL,
    source_ip             TEXT,
    user_agent            TEXT,
    current_refresh_id    TEXT,
    prev_refresh_id       TEXT,
    revoked               INTEGER NOT NULL DEFAULT 0,
    revoked_at            TEXT,
    revoked_by            TEXT,
    revoke_reason         TEXT
);

-- Listing for a single operator (self-service) and the keyset-friendly
-- created_at ordering the session list renders by.
CREATE INDEX idx_operator_session_did ON operator_session (did);
-- Expiry sweep / "active sessions" filter.
CREATE INDEX idx_operator_session_expires ON operator_session (expires_at);
