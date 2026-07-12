-- Holder UI Phase 1 (chainlink #424, Arc 2 SD-A5 = flexible): per-holder auth
-- methods. Postgres counterpart of sqlite 0033. See that file for the full
-- rationale (three method types password/passkey/login_alpha; passkey +
-- login_alpha deferred in Phase 1; last-method-remaining safety enforced in the
-- manager). Dual-tree boolean convention: pg BOOLEAN / sqlite INTEGER; BYTEA
-- here for the sqlite BLOB passkey columns.

CREATE TABLE holder_auth_method (
    id                       TEXT PRIMARY KEY,
    did                      TEXT NOT NULL,
    method_type              TEXT NOT NULL,
    is_primary               BOOLEAN NOT NULL DEFAULT false,
    password_hash            TEXT,
    password_algo            TEXT,
    passkey_credential_id    BYTEA,
    passkey_public_key       BYTEA,
    passkey_sign_count       BIGINT,
    passkey_device_name      TEXT,
    passkey_backup_eligible  BOOLEAN,
    passkey_backup_state     BOOLEAN,
    created_at               TEXT NOT NULL,
    last_used_at             TEXT,
    FOREIGN KEY (did) REFERENCES actor(did) ON DELETE CASCADE
);

CREATE INDEX idx_holder_auth_method_did ON holder_auth_method (did);
CREATE UNIQUE INDEX idx_holder_auth_method_did_primary
  ON holder_auth_method (did) WHERE is_primary = true;
CREATE UNIQUE INDEX idx_holder_auth_method_passkey_credential_id
  ON holder_auth_method (passkey_credential_id) WHERE method_type = 'passkey';
