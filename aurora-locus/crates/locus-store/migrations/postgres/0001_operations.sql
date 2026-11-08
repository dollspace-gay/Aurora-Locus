-- Operations tables: runtime settings, leases, job state, env tracking, audit.
-- Times are milliseconds since the Unix epoch, from the database clock.

CREATE TABLE setting (
    key         TEXT PRIMARY KEY,
    value_json  TEXT NOT NULL,
    version     BIGINT NOT NULL,
    updated_at  BIGINT NOT NULL,
    updated_by  TEXT NOT NULL
);

CREATE TABLE settings_version (
    id       INTEGER PRIMARY KEY CHECK (id = 1),
    version  BIGINT NOT NULL
);
INSERT INTO settings_version (id, version) VALUES (1, 0);

-- Named leases for singleton jobs, and per-node slots (clock ids). Every
-- acquisition increments epoch; transactions of a leased job re-check
-- holder, epoch and expiry before committing.
CREATE TABLE job_lease (
    name        TEXT PRIMARY KEY,
    holder      TEXT NOT NULL,
    epoch       BIGINT NOT NULL,
    expires_at  BIGINT NOT NULL
);

CREATE TABLE job_state (
    name         TEXT PRIMARY KEY,
    last_run_at  BIGINT,
    last_ok_at   BIGINT,
    next_run_at  BIGINT,
    last_error   TEXT,
    runs         BIGINT NOT NULL DEFAULT 0
);

-- The hash of the env/file value last applied install-wide, per setting key.
CREATE TABLE env_applied (
    key         TEXT PRIMARY KEY,
    hash        TEXT NOT NULL,
    applied_at  BIGINT NOT NULL
);

-- Each node's current env/file values, by hash. Only rows of live nodes count.
CREATE TABLE env_report (
    node_id      TEXT NOT NULL,
    key          TEXT NOT NULL,
    hash         TEXT NOT NULL,
    reported_at  BIGINT NOT NULL,
    PRIMARY KEY (node_id, key)
);

CREATE TABLE audit_log (
    id          BIGSERIAL PRIMARY KEY,
    at          BIGINT NOT NULL,
    actor       TEXT NOT NULL,
    action      TEXT NOT NULL,
    target      TEXT,
    detail_json TEXT
);
CREATE INDEX audit_log_at ON audit_log (at);
