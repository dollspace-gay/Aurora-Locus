-- v0.9 Arc D / chainlink #316 — per-account kryphocron overrides.
--
-- The per-account override surface (design §6.6.2 item 4): SuperAdmin-set
-- exceptions to the deployment-wide kryphocron policy, expressed per account.
-- Recon (docs/internal/v09/v09_per_account_override_recon.md) confirmed the
-- design's override surface is exactly two fields — rate-limit exemption and a
-- capability-issuance block ("host-side capability-issuance gate, not a
-- kryphocron substrate concept") — and pared the kickoff's 5-field sketch back
-- to these two (per-account cadence is incoherent with the deployment-wide
-- Laquna slug; access-delay / audience-default are not design override fields).
--
-- Tri-state booleans stored as nullable INTEGER (the #130 sqlx::Any discipline
-- forbids BOOLEAN in dual-backend tables): NULL = unset (use the deployment
-- default), 1 = true, 0 = explicit false. So a partial override (only one field
-- set) leaves the other NULL → default. Consumers:
--   * capability_issuance — wired now: the private-tier dedicated-endpoint
--     write path rejects a write from a DID whose row sets this to 0 (blocked);
--     NULL/1 = allowed = current behavior.
--   * rate_limit_exempt — stored now, enforced when the per-tier kryphocron
--     rate-limit feature (§6.6.2 item 3, backend-prereq) lands; the thing it
--     exempts from does not exist yet.
--
-- No FK on `did` (AS-only accounts have no actor row; operators may pre-create
-- an override before the account exists). Timestamps are TEXT (RFC3339).

CREATE TABLE kryphocron_account_override (
    did                     TEXT PRIMARY KEY,
    rate_limit_exempt       INTEGER,
    capability_issuance     INTEGER,
    last_modified_at        TEXT NOT NULL,
    last_modified_by_did    TEXT NOT NULL,
    last_modified_rationale TEXT
);

CREATE INDEX idx_kryphocron_account_override_modified
    ON kryphocron_account_override (last_modified_at);
