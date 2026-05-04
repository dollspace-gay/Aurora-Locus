-- Session 8 / chainlink #111 — per-subject snapshot ids on batch chain rows.
--
-- Pre-CR-2, batch chain rows (the six tools.aurora.admin.batch* endpoints)
-- recorded `cascade_subjects` (JSON list of Subject) but no per-subject
-- snapshot ids. Per docs/AURORA_ADMIN_UI_DESIGN.md §3.4, snapshot + chain
-- together answer the forensic question; without per-subject snapshots
-- the chain points at subjects whose state-at-decision can't be
-- reconstructed.
--
-- New column `cascade_snapshot_ids` (JSON list of nullable i64). Same
-- index as `cascade_subjects`: cascade_subjects[i] pairs with
-- cascade_snapshot_ids[i]. Empty/NULL for single-subject entries (where
-- the scalar `snapshot_id` applies) and for legacy pre-CR-2 batch rows.
--
-- The field is included in the canonical hash so verify_chain_range
-- catches tampering with the snapshot linkage.

ALTER TABLE audit_chain_entry ADD COLUMN cascade_snapshot_ids TEXT;
