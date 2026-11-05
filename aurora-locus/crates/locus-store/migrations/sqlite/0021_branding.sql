-- Branding themes: one draft, and published versions (kept, so any can be
-- put back). branding_state names the version in use.
CREATE TABLE branding_version (
    version       INTEGER PRIMARY KEY AUTOINCREMENT,
    state         TEXT NOT NULL CHECK (state IN ('draft', 'published')),
    theme_json    TEXT NOT NULL,
    created_at    INTEGER NOT NULL,
    created_by    TEXT NOT NULL,
    published_at  INTEGER,
    published_by  TEXT
) STRICT;

CREATE TABLE branding_state (
    id      INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    active  INTEGER
) STRICT;
INSERT INTO branding_state (id, active) VALUES (1, NULL);

-- Uploads (logo, favicon, backgrounds, fonts), stored under branding/ in
-- the blob store outside any account. unused_since is set at upload and
-- when the last version using one goes; past the grace the row goes and
-- its bytes are queued for deletion.
CREATE TABLE branding_asset (
    id            TEXT PRIMARY KEY NOT NULL,
    kind          TEXT NOT NULL CHECK (kind IN ('logo', 'favicon', 'background', 'font')),
    name          TEXT NOT NULL,
    mime          TEXT NOT NULL,
    size          INTEGER NOT NULL,
    storage_key   TEXT NOT NULL,
    created_at    INTEGER NOT NULL,
    unused_since  INTEGER
) STRICT;

CREATE TABLE branding_asset_use (
    asset_id  TEXT NOT NULL,
    version   INTEGER NOT NULL,
    PRIMARY KEY (asset_id, version)
) STRICT;
