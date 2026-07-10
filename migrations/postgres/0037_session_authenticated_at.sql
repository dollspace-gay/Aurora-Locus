-- Phase 4 admin session hardening (chainlink #442). Postgres counterpart of
-- sqlite 0036. See that file for the full rationale: step-up freshness is
-- measured from the interactive-login time, carried across refreshes, so a
-- silent refresh does not reset the step-up clock.
ALTER TABLE session ADD COLUMN authenticated_at TEXT;
