-- Private-tier permission copied from the AppView, one row per source
-- (`membership`: getMembership; `probe`: the read probe and audience calls).
CREATE TABLE kr_permission (
    did              TEXT NOT NULL,
    source           TEXT NOT NULL,
    prism_did        TEXT NOT NULL,
    status           TEXT NOT NULL,
    private_indexed  INTEGER NOT NULL,
    private_read     INTEGER NOT NULL,
    expires_at       INTEGER NOT NULL,
    checked_at       INTEGER NOT NULL,
    PRIMARY KEY (did, source)
) STRICT;

-- The private audience store: a hidden copy of the AppView's audiences.
CREATE TABLE kr_audience (
    did                     TEXT NOT NULL,
    rkey                    TEXT NOT NULL,
    name                    TEXT,
    mode                    TEXT NOT NULL,
    members_json            TEXT NOT NULL,
    created_at              TEXT NOT NULL,
    updated_at              TEXT NOT NULL,
    origin                  TEXT NOT NULL,
    prism_did               TEXT NOT NULL,
    prism_cid               TEXT,
    local_cid               TEXT NOT NULL,
    value_cbor              BLOB NOT NULL,
    pending                 TEXT NOT NULL DEFAULT 'none',
    pending_at              INTEGER,
    flagged                 TEXT,
    row_version             INTEGER NOT NULL DEFAULT 1,
    adopted_at              INTEGER,
    adopted_by_attempt      TEXT,
    synced_from_appview_at  INTEGER,
    checked_at              INTEGER,
    PRIMARY KEY (did, rkey)
) STRICT;

-- Every private record in a repo hosted here, kept with the record index.
CREATE TABLE kr_private_record (
    did            TEXT NOT NULL,
    collection     TEXT NOT NULL,
    rkey           TEXT NOT NULL,
    generation     TEXT,
    legacy_text    INTEGER NOT NULL DEFAULT 0,
    flagged        TEXT,
    audience_did   TEXT,
    audience_rkey  TEXT,
    PRIMARY KEY (did, collection, rkey)
) STRICT;
CREATE INDEX kr_private_record_generation ON kr_private_record (generation);
CREATE INDEX kr_private_record_audience ON kr_private_record (audience_did, audience_rkey);

-- Rotation generations; the newest row is current.
CREATE TABLE kr_rotation_state (
    generation  TEXT PRIMARY KEY NOT NULL,
    created_at  INTEGER NOT NULL
) STRICT;

-- Kryphocron audit events (append-only).
CREATE TABLE kr_audit (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    at              INTEGER NOT NULL,
    sink            TEXT NOT NULL,
    event           TEXT NOT NULL,
    correlation_id  TEXT,
    detail_json     TEXT NOT NULL
) STRICT;
CREATE INDEX kr_audit_at ON kr_audit (at);
