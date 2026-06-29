-- §5.5.4 Phase A / chainlink #345 — report-by-subject lookup index.
--
-- The design names `idx_reports_subject_created ON <reports_table>(subject,
-- created_at)`. Aurora-Locus's report table is `report` with no columns
-- literally named `subject`/`created_at`; the local equivalents are
-- `subject_did` (the account/content DID a report targets) and
-- `reported_at` (the intake timestamp). The index supports the §3
-- report-count trigger's "N reports of category X for this subject within
-- window Y" lookup (Phase B+ consumer) and subject-scoped report history.
CREATE INDEX idx_reports_subject_created ON report (subject_did, reported_at);
