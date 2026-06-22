-- Postgres variant of migrations/0020_phase_b_reviewer_assignment.sql.
-- See that file for the chainlink #346 / §5.5.4 Phase B motivation.
ALTER TABLE report ADD COLUMN assigned_operator_did TEXT;
ALTER TABLE report ADD COLUMN assignment_source TEXT;

CREATE INDEX idx_report_assigned_operator ON report (assigned_operator_did);

INSERT INTO runtime_settings (key, value, last_modified, last_modified_by) VALUES
  ('moderation.defaults.reviewer-rotation-cursor', '0', '2026-06-22T00:00:00Z', 'did:system'),
  ('moderation.defaults.reviewer-category-rotation-cursors', '{}', '2026-06-22T00:00:00Z', 'did:system'),
  ('moderation.defaults.reviewer-mode-version', '0', '2026-06-22T00:00:00Z', 'did:system'),
  ('moderation.defaults.escalation-superadmin-cursor', '0', '2026-06-22T00:00:00Z', 'did:system');
