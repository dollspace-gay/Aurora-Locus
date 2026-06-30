-- v0.10 Arc 1 Phase B (#414) — did:web account identity storage.
--
-- A did:web account's sovereign identity is stored PUBLIC-KEY-ONLY: the
-- `identity_public_key` is the holder's #atproto verification method, and there
-- is deliberately NO private-key column (LOCKED design §4). That absence is the
-- structural shape that makes §6 Layer 2 hold under SD-1 (A) — the substrate
-- cannot sign as the holder because it never holds the key.
--
-- `slug` is the stable, minted DID segment (AD-3) and the serve-route reverse
-- lookup key (`/user/{slug}/did.json`); UNIQUE so the lookup is unambiguous.
-- `domain` is the did:web host. `did` REFERENCES actor(did) ON DELETE CASCADE
-- (R2 F-1) so account deletion removes the identity row rather than orphaning it
-- (SQLite enforces this — `PRAGMA foreign_keys=ON` is set at pool setup,
-- src/db/mod.rs:87).
--
-- The write helper (insert_did_web_account) and the minting path ship in Phase C
-- (gated on Arc 2 holder-mediated signing); Phase B ships storage + read path.
CREATE TABLE did_web_account (
    did                 TEXT PRIMARY KEY REFERENCES actor(did) ON DELETE CASCADE,
    domain              TEXT NOT NULL,
    slug                TEXT NOT NULL UNIQUE,
    identity_public_key TEXT NOT NULL,
    created_at          TEXT NOT NULL
);
