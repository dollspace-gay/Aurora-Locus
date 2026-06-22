-- Postgres variant of migrations/0019_phase_a_reports_subject_created_idx.sql.
-- See that file for the chainlink #345 / §5.5.4 motivation and the
-- design-name → local-column translation (subject→subject_did,
-- created_at→reported_at).
CREATE INDEX idx_reports_subject_created ON report (subject_did, reported_at);
