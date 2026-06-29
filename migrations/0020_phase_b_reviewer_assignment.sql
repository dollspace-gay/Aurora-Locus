-- §5.5.4 Phase B / chainlink #346 — reviewer assignment.
--
-- §4.4: the queue/report row gains an assignee + an assignment-provenance
-- discriminator. Both NULLABLE; existing rows initialize NULL (= manual /
-- unassigned). `assignment_source` ∈ {auto, manual_override} when set.
-- SQLite takes one ADD COLUMN per ALTER; both run in this migration's
-- single transaction.
ALTER TABLE report ADD COLUMN assigned_operator_did TEXT;
ALTER TABLE report ADD COLUMN assignment_source TEXT;

-- §4.5 queue filter (assigned_operator_did = me OR IS NULL). Plain index
-- (not partial) for sqlx::Any portability per design §10 item 4.
CREATE INDEX idx_report_assigned_operator ON report (assigned_operator_did);

-- §4.7 substrate-managed rotation/version counters. Seeded so the value-CAS
-- cursor-advance (§4.7) is a pure UPDATE … WHERE value=expected with no
-- INSERT-race — these rows always exist. Values are JSON-encoded (integer
-- counters; the category cursors are a category→int object). Operator-facing
-- settings (reviewer-assignment-mode, reviewer-routing-category-map) are NOT
-- seeded — they stay Default-tier until a SuperAdmin configures them.
INSERT INTO runtime_settings (key, value, last_modified, last_modified_by) VALUES
  ('moderation.defaults.reviewer-rotation-cursor', '0', '2026-06-22T00:00:00Z', 'did:system'),
  ('moderation.defaults.reviewer-category-rotation-cursors', '{}', '2026-06-22T00:00:00Z', 'did:system'),
  ('moderation.defaults.reviewer-mode-version', '0', '2026-06-22T00:00:00Z', 'did:system'),
  ('moderation.defaults.escalation-superadmin-cursor', '0', '2026-06-22T00:00:00Z', 'did:system');
