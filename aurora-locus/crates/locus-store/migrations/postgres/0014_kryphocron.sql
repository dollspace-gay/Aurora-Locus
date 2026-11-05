-- Private-tier permission copied from the AppView, one row per source
-- (`membership`: getMembership; `probe`: the read probe and audience calls).
CREATE TABLE kr_permission (
    did              TEXT NOT NULL,
    source           TEXT NOT NULL,
    prism_did        TEXT NOT NULL,
    status           TEXT NOT NULL,
    private_indexed  BOOLEAN NOT NULL,
    private_read     BOOLEAN NOT NULL,
    expires_at       BIGINT NOT NULL,
    checked_at       BIGINT NOT NULL,
    PRIMARY KEY (did, source)
);

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
    value_cbor              BYTEA NOT NULL,
    pending                 TEXT NOT NULL DEFAULT 'none',
    pending_at              BIGINT,
    flagged                 TEXT,
    row_version             BIGINT NOT NULL DEFAULT 1,
    adopted_at              BIGINT,
    adopted_by_attempt      TEXT,
    synced_from_appview_at  BIGINT,
    checked_at              BIGINT,
    PRIMARY KEY (did, rkey)
);

-- Every private record in a repo hosted here, kept with the record index.
CREATE TABLE kr_private_record (
    did            TEXT NOT NULL,
    collection     TEXT NOT NULL,
    rkey           TEXT NOT NULL,
    generation     TEXT,
    legacy_text    BOOLEAN NOT NULL DEFAULT FALSE,
    flagged        TEXT,
    audience_did   TEXT,
    audience_rkey  TEXT,
    PRIMARY KEY (did, collection, rkey)
);
CREATE INDEX kr_private_record_generation ON kr_private_record (generation);
CREATE INDEX kr_private_record_audience ON kr_private_record (audience_did, audience_rkey);

-- Rotation generations; the newest row is current.
CREATE TABLE kr_rotation_state (
    generation  TEXT PRIMARY KEY,
    created_at  BIGINT NOT NULL
);

-- Kryphocron audit events (append-only).
CREATE TABLE kr_audit (
    id              BIGSERIAL PRIMARY KEY,
    at              BIGINT NOT NULL,
    sink            TEXT NOT NULL,
    event           TEXT NOT NULL,
    correlation_id  TEXT,
    detail_json     TEXT NOT NULL
);
CREATE INDEX kr_audit_at ON kr_audit (at);
