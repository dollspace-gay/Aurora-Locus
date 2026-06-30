-- v0.10 Arc 1 Phase B (#414) — did:web account identity storage (Postgres tree).
--
-- Mirror of migrations/0027_did_web_account.sql. Public-key-only by design
-- (LOCKED §4): no private-key column, so the substrate cannot sign as the holder
-- (§6 Layer 2 under SD-1 (A)). `slug` is the stable minted DID segment (AD-3) and
-- the serve-route reverse lookup key; UNIQUE. `did` REFERENCES actor(did) ON
-- DELETE CASCADE (R2 F-1) — Postgres enforces FKs by default, so account deletion
-- cascades to this row. Write/mint path ships in Phase C (gated on Arc 2).
CREATE TABLE did_web_account (
    did                 TEXT PRIMARY KEY REFERENCES actor(did) ON DELETE CASCADE,
    domain              TEXT NOT NULL,
    slug                TEXT NOT NULL UNIQUE,
    identity_public_key TEXT NOT NULL,
    created_at          TEXT NOT NULL
);
