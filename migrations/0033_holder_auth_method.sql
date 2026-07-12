-- Holder UI Phase 1 (chainlink #424, Arc 2 SD-A5 = flexible): per-holder auth
-- methods for the did:web/did:plc holder self-service UI.
--
-- SD-A5 was revised post-LOCK from "(b) not-offered" to "flexible": a did:web
-- holder authenticates to the OAuth authorization-server / holder UI via one of
-- three method types, holder's choice, managed per-account:
--
--   'password'    argon2id credential (crate::auth::PasswordHasher). The
--                 default floor; same hashing as did:plc app-passwords.
--   'passkey'     WebAuthn credential. DEFERRED in Phase 1 (needs webauthn-rs +
--                 a serialized-credential model); the columns exist so the
--                 follow-on adds no migration, but no 'passkey' rows are
--                 written yet.
--   'login_alpha' β.2's challenge-signed-by-#atproto-key shape. Substrate holds
--                 only `identity_public_key` (already in did_web_account); no
--                 method-specific column here. DEFERRED at the web-UI layer in
--                 Phase 1 (the browser needs a vendored secp256k1 lib to sign),
--                 gated off by `holder_login_alpha_enabled`.
--
-- Safety invariant (enforced in HolderAuthMethodManager::remove, not the
-- schema): never delete a holder's last remaining method — that would lock them
-- out.
--
-- Columns:
--   id                       opaque uuid PK.
--   did                      holder DID (FK to actor, ON DELETE CASCADE — Arc 1
--                            did:web pattern, mirrors atproto_device).
--   method_type              'password' | 'passkey' | 'login_alpha'.
--   is_primary               the holder's default method (INTEGER 0/1 sqlite;
--                            BOOLEAN pg — read via crate::db::read_bool). At
--                            most one primary per holder (partial unique index).
--   password_hash/_algo      argon2id PHC string + algo tag; NULL for non-password.
--   passkey_*                WebAuthn credential fields; all NULL in Phase 1
--                            (passkey deferred). Present so the follow-on needs
--                            no migration.
--   created_at               RFC3339.
--   last_used_at             RFC3339; touched on each successful auth; NULL until
--                            first use.

CREATE TABLE holder_auth_method (
    id                       TEXT PRIMARY KEY,
    did                      TEXT NOT NULL,
    method_type              TEXT NOT NULL,
    is_primary               INTEGER NOT NULL DEFAULT 0,
    password_hash            TEXT,
    password_algo            TEXT,
    passkey_credential_id    BLOB,
    passkey_public_key       BLOB,
    passkey_sign_count       INTEGER,
    passkey_device_name      TEXT,
    passkey_backup_eligible  INTEGER,
    passkey_backup_state     INTEGER,
    created_at               TEXT NOT NULL,
    last_used_at             TEXT,
    FOREIGN KEY (did) REFERENCES actor(did) ON DELETE CASCADE
);

CREATE INDEX idx_holder_auth_method_did ON holder_auth_method (did);
-- At most one primary method per holder.
CREATE UNIQUE INDEX idx_holder_auth_method_did_primary
  ON holder_auth_method (did) WHERE is_primary = 1;
-- WebAuthn credential IDs are globally unique. Partial (passkey-only) so the
-- many NULL passkey_credential_id rows (password / login_alpha) are exempt.
CREATE UNIQUE INDEX idx_holder_auth_method_passkey_credential_id
  ON holder_auth_method (passkey_credential_id) WHERE method_type = 'passkey';
