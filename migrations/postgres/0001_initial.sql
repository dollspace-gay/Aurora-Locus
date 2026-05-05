-- Aurora-Locus initial schema (Postgres)
-- Migration 0001
--
-- Translation of migrations/0001_initial.sql per
-- POSTGRES_BACKEND_ASSESSMENT.md §3. Originally landed in chainlink
-- #74 (Phase 1) with DATETIME → TIMESTAMPTZ; amended in #76 sub-phase
-- 3a triage to TEXT for all timestamp columns (see below).
--
-- Type translation summary:
--   SQLite                                 -> Postgres
--   ----------------------------------     -----------------------------
--   TEXT                                   TEXT
--   DATETIME                               TEXT          (see note below)
--   INTEGER PRIMARY KEY AUTOINCREMENT      BIGSERIAL PRIMARY KEY
--   INTEGER (count / FK)                   INTEGER (or BIGINT for FK→BIGSERIAL)
--   INTEGER NOT NULL DEFAULT 0/1 (boolean) BOOLEAN NOT NULL DEFAULT false/true
--   BLOB                                   BYTEA
--
-- TIMESTAMP HANDLING (chainlink #76 amendment):
-- All timestamp columns are TEXT, not TIMESTAMPTZ. Rationale: sqlx's
-- `Any` driver — which Aurora-Locus uses to abstract the SQLite/Postgres
-- backend choice — has a deliberately small type-compatibility set that
-- excludes chrono types. Binding a `chrono::DateTime<Utc>` to an
-- `AnyPool` query is unsupported. The codebase therefore binds RFC3339
-- strings everywhere; uniform TEXT columns make that pattern work
-- consistently across both backends.
--
-- Trade-off: this gives up Postgres-native TIMESTAMPTZ features
-- (timezone-aware comparisons, binary representation, native indexing).
-- Aurora's query patterns are not currently exercising these features
-- (existing comparisons are lexicographic on RFC3339 strings, which
-- sort correctly within a single timezone). If/when the query layer
-- starts needing native timestamp semantics, this is the migration to
-- revisit.
--
-- Partial-index predicates that compared boolean-as-integer (`= 0` /
-- `= 1`) are rewritten using `NOT col` / `col` for proper Postgres
-- boolean semantics.

-- ============================================================================
-- ACCOUNT & ACTOR (split: actor=public identity, account=private auth)
-- ============================================================================

CREATE TABLE actor (
    did              TEXT PRIMARY KEY,
    handle           TEXT UNIQUE NOT NULL,
    created_at       TEXT NOT NULL,
    takedown_ref     TEXT,
    deactivated_at   TEXT,
    delete_after     TEXT
);

CREATE INDEX idx_actor_handle ON actor(handle);

CREATE TABLE account (
    did                 TEXT PRIMARY KEY,
    email               TEXT UNIQUE,
    password_hash       TEXT NOT NULL,
    email_confirmed_at  TEXT,
    invites_disabled    BOOLEAN NOT NULL DEFAULT false,
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
    created_at           TEXT NOT NULL,
    expires_at           TEXT NOT NULL,
    app_password_name    TEXT,
    FOREIGN KEY (did) REFERENCES actor(did)
);

CREATE INDEX idx_session_did ON session(did);
CREATE INDEX idx_session_expires ON session(expires_at);

CREATE TABLE refresh_token (
    id           TEXT PRIMARY KEY,
    did          TEXT NOT NULL,
    token        TEXT UNIQUE NOT NULL,
    created_at   TEXT NOT NULL,
    expires_at   TEXT NOT NULL,
    used         BOOLEAN NOT NULL DEFAULT false,
    used_at      TEXT,
    next_id      TEXT,
    FOREIGN KEY (did) REFERENCES actor(did)
);

CREATE INDEX idx_refresh_token_did ON refresh_token(did);

CREATE TABLE app_password (
    did             TEXT NOT NULL,
    name            TEXT NOT NULL,
    password_hash   TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    privileged      BOOLEAN NOT NULL DEFAULT false,
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
    created_at   TEXT NOT NULL,
    expires_at   TEXT NOT NULL,
    used         BOOLEAN NOT NULL DEFAULT false
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
    seq             BIGSERIAL PRIMARY KEY,
    did             TEXT NOT NULL,
    event_type      TEXT NOT NULL,
    event           BYTEA NOT NULL,
    invalidated     BOOLEAN NOT NULL DEFAULT false,
    sequenced_at    TEXT NOT NULL
);

CREATE INDEX idx_repo_seq_did ON repo_seq(did);
CREATE INDEX idx_repo_seq_seq ON repo_seq(seq) WHERE NOT invalidated;

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
    takedown     BOOLEAN NOT NULL DEFAULT false
);

CREATE INDEX idx_blob_did ON blob(did);

CREATE TABLE blob_metadata (
    cid              TEXT PRIMARY KEY,
    mime_type        TEXT NOT NULL,
    size             INTEGER NOT NULL,
    creator_did      TEXT NOT NULL,
    created_at       TEXT NOT NULL,
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
    created_at       TEXT NOT NULL,
    width            INTEGER,
    height           INTEGER
);

CREATE INDEX idx_temp_blob_creator ON temp_blob_metadata(creator_did);
CREATE INDEX idx_temp_blob_created ON temp_blob_metadata(created_at);

CREATE TABLE blob_quarantine (
    id                 BIGSERIAL PRIMARY KEY,
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
    indexed_at   TEXT NOT NULL,
    PRIMARY KEY (blob_cid, record_uri)
);

CREATE INDEX idx_record_blob_uri ON record_blob(record_uri);

-- ============================================================================
-- MODERATION
-- ============================================================================

CREATE TABLE account_moderation (
    id                BIGSERIAL PRIMARY KEY,
    did               TEXT NOT NULL,
    action            TEXT NOT NULL,
    reason            TEXT NOT NULL,
    moderated_by      TEXT NOT NULL,
    moderated_at      TEXT NOT NULL,
    expires_at        TEXT,
    reversed          BOOLEAN NOT NULL DEFAULT false,
    reversed_at       TEXT,
    reversed_by       TEXT,
    reversal_reason   TEXT,
    -- FK target is `report.id` (BIGSERIAL); widened from SQLite INTEGER
    -- to BIGINT so the reference type matches.
    report_id         BIGINT,
    notes             TEXT
);

CREATE INDEX idx_account_moderation_did ON account_moderation(did) WHERE NOT reversed;
CREATE INDEX idx_account_moderation_expires ON account_moderation(expires_at) WHERE expires_at IS NOT NULL AND NOT reversed;

CREATE TABLE label (
    id            BIGSERIAL PRIMARY KEY,
    uri           TEXT NOT NULL,
    cid           TEXT,
    val           TEXT NOT NULL,
    neg           BOOLEAN NOT NULL DEFAULT false,
    src           TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    created_by    TEXT NOT NULL,
    expires_at    TEXT,
    sig           BYTEA
);

CREATE INDEX idx_label_uri ON label(uri);
CREATE INDEX idx_label_val ON label(val);

CREATE TABLE report (
    id              BIGSERIAL PRIMARY KEY,
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
    id               BIGSERIAL PRIMARY KEY,
    -- FK targets: account_moderation.id, report.id, blob_quarantine.id —
    -- all BIGSERIAL. Widened from SQLite INTEGER for type match.
    moderation_id    BIGINT,
    report_id        BIGINT,
    quarantine_id    BIGINT,
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
    id              BIGSERIAL PRIMARY KEY,
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
    id            BIGSERIAL PRIMARY KEY,
    did           TEXT NOT NULL UNIQUE,
    role          TEXT NOT NULL,
    granted_by    TEXT,
    granted_at    TEXT NOT NULL,
    revoked       BOOLEAN NOT NULL DEFAULT false,
    revoked_at    TEXT,
    revoked_by    TEXT,
    notes         TEXT
);

CREATE TABLE admin_audit_log (
    id              BIGSERIAL PRIMARY KEY,
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
    -- `available` is a remaining-uses count, not a boolean — kept INTEGER.
    available      INTEGER NOT NULL DEFAULT 1,
    disabled       BOOLEAN NOT NULL DEFAULT false,
    created_by     TEXT NOT NULL,
    created_at     TEXT NOT NULL,
    expires_at     TEXT,
    note           TEXT,
    for_account    TEXT
);

CREATE INDEX idx_invite_code_created_by ON invite_code(created_by);

CREATE TABLE invite_code_use (
    id          BIGSERIAL PRIMARY KEY,
    code        TEXT NOT NULL,
    used_by     TEXT NOT NULL,
    used_at     TEXT NOT NULL
);

CREATE INDEX idx_invite_code_use_code ON invite_code_use(code);

-- ============================================================================
-- EMAIL DELIVERY TRACKING
-- ============================================================================

CREATE TABLE email_delivery (
    id               BIGSERIAL PRIMARY KEY,
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
    last_seen_at        TEXT,
    dpop_public_key     TEXT,
    created_at          TEXT NOT NULL
);

CREATE INDEX idx_device_session ON device(session_id);

CREATE TABLE account_device (
    id                BIGSERIAL PRIMARY KEY,
    did               TEXT NOT NULL,
    device_id         TEXT NOT NULL,
    authorized_at     TEXT NOT NULL,
    device_name       TEXT,
    is_active         BOOLEAN NOT NULL DEFAULT true,
    revoked_at        TEXT,
    FOREIGN KEY (device_id) REFERENCES device(id)
);

CREATE INDEX idx_account_device_did ON account_device(did) WHERE is_active;

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
    code_used               BOOLEAN NOT NULL DEFAULT false,
    created_at              TEXT NOT NULL,
    expires_at              TEXT NOT NULL
);

CREATE INDEX idx_auth_request_code ON authorization_request(authorization_code) WHERE authorization_code IS NOT NULL;
CREATE INDEX idx_auth_request_expires ON authorization_request(expires_at);

CREATE TABLE authorized_client (
    id                       BIGSERIAL PRIMARY KEY,
    did                      TEXT NOT NULL,
    client_id                TEXT NOT NULL,
    scope                    TEXT NOT NULL,
    first_authorized_at      TEXT NOT NULL,
    last_used_at             TEXT NOT NULL,
    is_active                BOOLEAN NOT NULL DEFAULT true,
    UNIQUE (did, client_id)
);

CREATE INDEX idx_authorized_client_did ON authorized_client(did) WHERE is_active;

-- ============================================================================
-- OAUTH: TOKENS & ROTATION
-- ============================================================================

CREATE TABLE token (
    token_id                  TEXT PRIMARY KEY,
    did                       TEXT NOT NULL,
    client_id                 TEXT NOT NULL,
    current_refresh_token     TEXT,
    scope                     TEXT NOT NULL,
    created_at                TEXT NOT NULL,
    updated_at                TEXT NOT NULL,
    expires_at                TEXT NOT NULL,
    dpop_thumbprint           TEXT,
    device_id                 TEXT,
    revoked                   BOOLEAN NOT NULL DEFAULT false,
    revoked_at                TEXT
);

CREATE INDEX idx_token_did ON token(did) WHERE NOT revoked;
CREATE INDEX idx_token_refresh ON token(current_refresh_token) WHERE current_refresh_token IS NOT NULL;
CREATE INDEX idx_token_expires ON token(expires_at) WHERE NOT revoked;

CREATE TABLE used_refresh_token (
    refresh_token    TEXT PRIMARY KEY,
    token_id         TEXT NOT NULL,
    did              TEXT NOT NULL,
    used_at          TEXT NOT NULL
);

CREATE INDEX idx_used_refresh_did ON used_refresh_token(did);
