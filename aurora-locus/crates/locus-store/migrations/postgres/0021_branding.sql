-- Branding themes: one draft, and published versions (kept, so any can be
-- put back). branding_state names the version in use.
CREATE TABLE branding_version (
    version       BIGSERIAL PRIMARY KEY,
    state         TEXT NOT NULL CHECK (state IN ('draft', 'published')),
    theme_json    TEXT NOT NULL,
    created_at    BIGINT NOT NULL,
    created_by    TEXT NOT NULL,
    published_at  BIGINT,
    published_by  TEXT
);

CREATE TABLE branding_state (
    id      INTEGER PRIMARY KEY CHECK (id = 1),
    active  BIGINT
);
INSERT INTO branding_state (id, active) VALUES (1, NULL);

-- Uploads (logo, favicon, backgrounds, fonts), stored under branding/ in
-- the blob store outside any account. unused_since is set at upload and
-- when the last version using one goes; past the grace the row goes and
-- its bytes are queued for deletion.
CREATE TABLE branding_asset (
    id            TEXT PRIMARY KEY,
    kind          TEXT NOT NULL CHECK (kind IN ('logo', 'favicon', 'background', 'font')),
    name          TEXT NOT NULL,
    mime          TEXT NOT NULL,
    size          BIGINT NOT NULL,
    storage_key   TEXT NOT NULL,
    created_at    BIGINT NOT NULL,
    unused_since  BIGINT
);

CREATE TABLE branding_asset_use (
    asset_id  TEXT NOT NULL,
    version   BIGINT NOT NULL,
    PRIMARY KEY (asset_id, version)
);
