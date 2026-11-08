-- Operations tables: runtime settings, leases, job state, env tracking, audit.
-- Times are milliseconds since the Unix epoch, from the database clock.

CREATE TABLE setting (
    key         TEXT PRIMARY KEY NOT NULL,
    value_json  TEXT NOT NULL,
    version     INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    updated_by  TEXT NOT NULL
) STRICT;

CREATE TABLE settings_version (
    id       INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    version  INTEGER NOT NULL
) STRICT;
INSERT INTO settings_version (id, version) VALUES (1, 0);

-- Named leases for singleton jobs, and per-node slots (clock ids). Every
-- acquisition increments epoch; transactions of a leased job re-check
-- holder, epoch and expiry before committing.
CREATE TABLE job_lease (
    name        TEXT PRIMARY KEY NOT NULL,
    holder      TEXT NOT NULL,
    epoch       INTEGER NOT NULL,
    expires_at  INTEGER NOT NULL
) STRICT;

CREATE TABLE job_state (
    name         TEXT PRIMARY KEY NOT NULL,
    last_run_at  INTEGER,
    last_ok_at   INTEGER,
    next_run_at  INTEGER,
    last_error   TEXT,
    runs         INTEGER NOT NULL DEFAULT 0
) STRICT;

-- The hash of the env/file value last applied install-wide, per setting key.
CREATE TABLE env_applied (
    key         TEXT PRIMARY KEY NOT NULL,
    hash        TEXT NOT NULL,
    applied_at  INTEGER NOT NULL
) STRICT;

-- Each node's current env/file values, by hash. Only rows of live nodes count.
CREATE TABLE env_report (
    node_id      TEXT NOT NULL,
    key          TEXT NOT NULL,
    hash         TEXT NOT NULL,
    reported_at  INTEGER NOT NULL,
    PRIMARY KEY (node_id, key)
) STRICT;

CREATE TABLE audit_log (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    at          INTEGER NOT NULL,
    actor       TEXT NOT NULL,
    action      TEXT NOT NULL,
    target      TEXT,
    detail_json TEXT
) STRICT;
CREATE INDEX audit_log_at ON audit_log (at);
