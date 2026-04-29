-- Aurora-Locus initial schema
-- Migration 0001
--
-- Harvested from test-helper schemas in:
--   src/account/manager.rs
--   src/admin/{appeals,events,invites,moderation,roles}.rs
--   src/blob_store/{quarantine,store}.rs
--   src/identity/cache.rs
--   src/mailer/tracking.rs
--   src/sequencer/sequencer.rs
--
-- Reconstructed from production query patterns in:
--   src/account/manager.rs (account/actor split, plc_keys, email_token)
--   src/admin/{labels,reports}.rs
--   src/api/admin.rs (sequencer_config)
--   src/blob_store/store.rs (record_blob, temp_blob_metadata)
--   src/oauth/{authorize,client,consent,device,token,token_rotation}.rs
--
-- ============================================================================
-- ACCOUNT & ACTOR (split: actor=public identity, account=private auth)
-- ============================================================================

CREATE TABLE actor (
    did              TEXT PRIMARY KEY,
    handle           TEXT UNIQUE NOT NULL,
    created_at       DATETIME NOT NULL,
    takedown_ref     TEXT,
    deactivated_at   DATETIME,
    delete_after     DATETIME
);

CREATE INDEX idx_actor_handle ON actor(handle);

CREATE TABLE account (
    did                 TEXT PRIMARY KEY,
    email               TEXT UNIQUE,
    password_hash       TEXT NOT NULL,
    email_confirmed_at  DATETIME,
    invites_disabled    INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (did) REFERENCES actor(did)
);

CREATE INDEX idx_account_email ON account(email);

CREATE TABLE plc_keys (
    did                  TEXT PRIMARY KEY,
    rotation_key         TEXT NOT NULL,
    rotation_key_public  TEXT NOT NULL,
    last_operation_cid   TEXT,
    FOREIGN KEY (did) REFERENCES actor(did)
);

-- ============================================================================
-- SESSIONS & REFRESH TOKENS
-- ============================================================================

CREATE TABLE session (
    id                   TEXT PRIMARY KEY,
    did                  TEXT NOT NULL,
    access_token         TEXT UNIQUE NOT NULL,
    refresh_token        TEXT UNIQUE NOT NULL,
    created_at           DATETIME NOT NULL,
    expires_at           DATETIME NOT NULL,
    app_password_name    TEXT,
    FOREIGN KEY (did) REFERENCES actor(did)
);

CREATE INDEX idx_session_did ON session(did);
CREATE INDEX idx_session_expires ON session(expires_at);

CREATE TABLE refresh_token (
    id           TEXT PRIMARY KEY,
    did          TEXT NOT NULL,
    token        TEXT UNIQUE NOT NULL,
    created_at   DATETIME NOT NULL,
    expires_at   DATETIME NOT NULL,
    used         INTEGER NOT NULL DEFAULT 0,
    used_at      DATETIME,
    next_id      TEXT,
    FOREIGN KEY (did) REFERENCES actor(did)
);

CREATE INDEX idx_refresh_token_did ON refresh_token(did);

CREATE TABLE app_password (
    did             TEXT NOT NULL,
    name            TEXT NOT NULL,
    password_hash   TEXT NOT NULL,
    created_at      DATETIME NOT NULL,
    privileged      INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (did, name),
    FOREIGN KEY (did) REFERENCES actor(did)
);

-- ============================================================================
-- EMAIL TOKENS (confirm_email, reset_password, delete_account)
-- ============================================================================

CREATE TABLE email_token (
    token        TEXT PRIMARY KEY,
    did          TEXT NOT NULL,
    purpose      TEXT NOT NULL,  -- 'confirm_email', 'reset_password', 'delete_account'
    created_at   DATETIME NOT NULL,
    expires_at   DATETIME NOT NULL,
    used         INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX idx_email_token_did_purpose ON email_token(did, purpose);

-- ============================================================================
-- IDENTITY CACHE
-- ============================================================================

CREATE TABLE did_doc (
    did          TEXT PRIMARY KEY,
    doc          TEXT NOT NULL,
    updated_at   TEXT NOT NULL,
    cached_at    TEXT NOT NULL
);

CREATE TABLE did_handle (
    handle        TEXT PRIMARY KEY,
    did           TEXT NOT NULL,
    declared_at   TEXT,
    updated_at    TEXT NOT NULL
);

CREATE INDEX idx_did_handle_did ON did_handle(did);

-- ============================================================================
-- SEQUENCER
-- ============================================================================

CREATE TABLE repo_seq (
    seq             INTEGER PRIMARY KEY AUTOINCREMENT,
    did             TEXT NOT NULL,
    event_type      TEXT NOT NULL,
    event           BLOB NOT NULL,
    invalidated     INTEGER NOT NULL DEFAULT 0,
    sequenced_at    TEXT NOT NULL
);

CREATE INDEX idx_repo_seq_did ON repo_seq(did);
CREATE INDEX idx_repo_seq_seq ON repo_seq(seq) WHERE invalidated = 0;

-- Key/value store used by api/admin.rs for sequencer pause toggle.
CREATE TABLE sequencer_config (
    key     TEXT PRIMARY KEY,
    value   TEXT NOT NULL
);

-- ============================================================================
-- BLOBS
-- ============================================================================

CREATE TABLE blob (
    cid          TEXT PRIMARY KEY,
    did          TEXT NOT NULL,
    size         INTEGER NOT NULL,
    mime_type    TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    takedown     INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX idx_blob_did ON blob(did);

CREATE TABLE blob_metadata (
    cid              TEXT PRIMARY KEY,
    mime_type        TEXT NOT NULL,
    size             INTEGER NOT NULL,
    creator_did      TEXT NOT NULL,
    created_at       DATETIME NOT NULL,
    width            INTEGER,
    height           INTEGER,
    alt_text         TEXT,
    thumbnail_cid    TEXT
);

CREATE INDEX idx_blob_metadata_creator ON blob_metadata(creator_did);

CREATE TABLE temp_blob_metadata (
    cid              TEXT PRIMARY KEY,
    mime_type        TEXT NOT NULL,
    size             INTEGER NOT NULL,
    creator_did      TEXT NOT NULL,
    created_at       DATETIME NOT NULL,
    width            INTEGER,
    height           INTEGER
);

CREATE INDEX idx_temp_blob_creator ON temp_blob_metadata(creator_did);
CREATE INDEX idx_temp_blob_created ON temp_blob_metadata(created_at);

CREATE TABLE blob_quarantine (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    cid                TEXT NOT NULL,
    reason             TEXT NOT NULL,
    details            TEXT,
    quarantined_by     TEXT NOT NULL,
    quarantined_at     TEXT NOT NULL,
    restored_at        TEXT,
    restored_by        TEXT,
    legal_reference    TEXT
);

CREATE INDEX idx_blob_quarantine_cid ON blob_quarantine(cid);

-- Junction table: which blobs are referenced by which records.
CREATE TABLE record_blob (
    blob_cid     TEXT NOT NULL,
    record_uri   TEXT NOT NULL,
    indexed_at   DATETIME NOT NULL,
    PRIMARY KEY (blob_cid, record_uri)
);

CREATE INDEX idx_record_blob_uri ON record_blob(record_uri);

-- ============================================================================
-- MODERATION
-- ============================================================================

CREATE TABLE account_moderation (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    did               TEXT NOT NULL,
    action            TEXT NOT NULL,
    reason            TEXT NOT NULL,
    moderated_by      TEXT NOT NULL,
    moderated_at      TEXT NOT NULL,
    expires_at        TEXT,
    reversed          INTEGER NOT NULL DEFAULT 0,
    reversed_at       TEXT,
    reversed_by       TEXT,
    reversal_reason   TEXT,
    report_id         INTEGER,
    notes             TEXT
);

CREATE INDEX idx_account_moderation_did ON account_moderation(did) WHERE reversed = 0;
CREATE INDEX idx_account_moderation_expires ON account_moderation(expires_at) WHERE expires_at IS NOT NULL AND reversed = 0;

CREATE TABLE label (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    uri           TEXT NOT NULL,
    cid           TEXT,
    val           TEXT NOT NULL,
    neg           INTEGER NOT NULL DEFAULT 0,
    src           TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    created_by    TEXT NOT NULL,
    expires_at    TEXT,
    sig           BLOB
);

CREATE INDEX idx_label_uri ON label(uri);
CREATE INDEX idx_label_val ON label(val);

CREATE TABLE report (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    subject_did     TEXT,
    subject_uri     TEXT,
    subject_cid     TEXT,
    reason_type     TEXT NOT NULL,
    reason          TEXT,
    reported_by     TEXT NOT NULL,
    reported_at     TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'open',
    reviewed_by     TEXT,
    reviewed_at     TEXT,
    resolution      TEXT
);

CREATE INDEX idx_report_status ON report(status);
CREATE INDEX idx_report_subject_did ON report(subject_did);
CREATE INDEX idx_report_subject_uri ON report(subject_uri);

CREATE TABLE appeal (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    moderation_id    INTEGER,
    report_id        INTEGER,
    quarantine_id    INTEGER,
    appellant_did    TEXT NOT NULL,
    reason           TEXT NOT NULL,
    details          TEXT,
    submitted_at     TEXT NOT NULL,
    status           TEXT NOT NULL,
    reviewed_by      TEXT,
    reviewed_at      TEXT,
    decision         TEXT,
    notes            TEXT
);

CREATE INDEX idx_appeal_status ON appeal(status);
CREATE INDEX idx_appeal_appellant ON appeal(appellant_did);

CREATE TABLE moderation_event (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    event_type      TEXT NOT NULL,
    actor_did       TEXT NOT NULL,
    subject_did     TEXT,
    subject_uri     TEXT,
    subject_cid     TEXT,
    details         TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    meta            TEXT
);

CREATE INDEX idx_moderation_event_subject ON moderation_event(subject_did);
CREATE INDEX idx_moderation_event_type ON moderation_event(event_type);

-- ============================================================================
-- ADMIN
-- ============================================================================

CREATE TABLE admin_roles (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    did           TEXT NOT NULL UNIQUE,
    role          TEXT NOT NULL,
    granted_by    TEXT,
    granted_at    TEXT NOT NULL,
    revoked       INTEGER NOT NULL DEFAULT 0,
    revoked_at    TEXT,
    revoked_by    TEXT,
    notes         TEXT
);

CREATE TABLE admin_audit_log (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    admin_did       TEXT NOT NULL,
    action          TEXT NOT NULL,
    subject_did     TEXT,
    details         TEXT,
    timestamp       TEXT NOT NULL,
    ip_address      TEXT
);

CREATE INDEX idx_admin_audit_admin ON admin_audit_log(admin_did);
CREATE INDEX idx_admin_audit_timestamp ON admin_audit_log(timestamp);

-- ============================================================================
-- INVITES
-- ============================================================================

CREATE TABLE invite_code (
    code           TEXT PRIMARY KEY,
    available      INTEGER NOT NULL DEFAULT 1,
    disabled       INTEGER NOT NULL DEFAULT 0,
    created_by     TEXT NOT NULL,
    created_at     TEXT NOT NULL,
    expires_at     TEXT,
    note           TEXT,
    for_account    TEXT
);

CREATE INDEX idx_invite_code_created_by ON invite_code(created_by);

CREATE TABLE invite_code_use (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    code        TEXT NOT NULL,
    used_by     TEXT NOT NULL,
    used_at     TEXT NOT NULL
);

CREATE INDEX idx_invite_code_use_code ON invite_code_use(code);

-- ============================================================================
-- EMAIL DELIVERY TRACKING
-- ============================================================================

CREATE TABLE email_delivery (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    recipient        TEXT NOT NULL,
    subject          TEXT NOT NULL,
    template_type    TEXT NOT NULL,
    status           TEXT NOT NULL,
    sent_at          TEXT,
    created_at       TEXT NOT NULL,
    error_message    TEXT,
    retry_count      INTEGER NOT NULL DEFAULT 0,
    message_id       TEXT
);

CREATE INDEX idx_email_delivery_recipient ON email_delivery(recipient);
CREATE INDEX idx_email_delivery_status ON email_delivery(status);

-- ============================================================================
-- OAUTH: DEVICES
-- ============================================================================

CREATE TABLE device (
    id                  TEXT PRIMARY KEY,
    session_id          TEXT NOT NULL,
    user_agent          TEXT,
    ip_address          TEXT,
    last_seen_at        DATETIME,
    dpop_public_key     TEXT,
    created_at          DATETIME NOT NULL
);

CREATE INDEX idx_device_session ON device(session_id);

CREATE TABLE account_device (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    did               TEXT NOT NULL,
    device_id         TEXT NOT NULL,
    authorized_at     DATETIME NOT NULL,
    device_name       TEXT,
    is_active         INTEGER NOT NULL DEFAULT 1,
    revoked_at        DATETIME,
    FOREIGN KEY (device_id) REFERENCES device(id)
);

CREATE INDEX idx_account_device_did ON account_device(did) WHERE is_active = 1;

-- ============================================================================
-- OAUTH: AUTHORIZATION FLOW
-- ============================================================================

CREATE TABLE authorization_request (
    request_id              TEXT PRIMARY KEY,
    did                     TEXT NOT NULL,
    client_id               TEXT NOT NULL,
    code_challenge          TEXT NOT NULL,
    code_challenge_method   TEXT NOT NULL,
    scope                   TEXT NOT NULL,
    redirect_uri            TEXT NOT NULL,
    state                   TEXT,
    authorization_code      TEXT,
    code_used               INTEGER NOT NULL DEFAULT 0,
    created_at              DATETIME NOT NULL,
    expires_at              DATETIME NOT NULL
);

CREATE INDEX idx_auth_request_code ON authorization_request(authorization_code) WHERE authorization_code IS NOT NULL;
CREATE INDEX idx_auth_request_expires ON authorization_request(expires_at);

CREATE TABLE authorized_client (
    id                       INTEGER PRIMARY KEY AUTOINCREMENT,
    did                      TEXT NOT NULL,
    client_id                TEXT NOT NULL,
    scope                    TEXT NOT NULL,
    first_authorized_at      DATETIME NOT NULL,
    last_used_at             DATETIME NOT NULL,
    is_active                INTEGER NOT NULL DEFAULT 1,
    UNIQUE (did, client_id)
);

CREATE INDEX idx_authorized_client_did ON authorized_client(did) WHERE is_active = 1;

-- ============================================================================
-- OAUTH: TOKENS & ROTATION
-- ============================================================================

CREATE TABLE token (
    token_id                  TEXT PRIMARY KEY,
    did                       TEXT NOT NULL,
    client_id                 TEXT NOT NULL,
    current_refresh_token     TEXT,
    scope                     TEXT NOT NULL,
    created_at                DATETIME NOT NULL,
    updated_at                DATETIME NOT NULL,
    expires_at                DATETIME NOT NULL,
    dpop_thumbprint           TEXT,
    device_id                 TEXT,
    revoked                   INTEGER NOT NULL DEFAULT 0,
    revoked_at                DATETIME
);

CREATE INDEX idx_token_did ON token(did) WHERE revoked = 0;
CREATE INDEX idx_token_refresh ON token(current_refresh_token) WHERE current_refresh_token IS NOT NULL;
CREATE INDEX idx_token_expires ON token(expires_at) WHERE revoked = 0;

CREATE TABLE used_refresh_token (
    refresh_token    TEXT PRIMARY KEY,
    token_id         TEXT NOT NULL,
    did              TEXT NOT NULL,
    used_at          DATETIME NOT NULL
);

CREATE INDEX idx_used_refresh_did ON used_refresh_token(did);
