-- §5.5.4 Phase A / chainlink #345 — audit-chain `source` discriminator +
-- action-scalar `payload` (docs/internal/design/v09_configurable_moderation_defaults_design.md §6.1).
--
-- Every audit-chain entry now carries a `source` discriminator
-- (default_action | auto_label_rule | manual | stale_expiration |
-- operator_removal | escalation | system_diagnostic) so the §6.4
-- source-field filter is a queryable column rather than rationale-
-- encoded text. Action-specific scalars (Phase A: `applied: bool` on
-- moderation_auto_label_applied; later phases: caused_state_change,
-- triggering_event_id, pipeline, rule_id) ride a single JSON `payload`
-- column — one column/encoding pattern for all 22 action names rather
-- than a column per action-specific field.
--
-- Both fields ENTER THE CANONICAL HASH (audit_chain.rs write_chain_entry_inner
-- / verify_entry). This is the v0.9 chain-format bump: pre-v0.9 rows hash
-- under the pre-source/payload canonical form and will not re-verify under
-- the v0.9 form — the same accepted legacy-tradeoff documented for the CR-2
-- cascade_snapshot_ids addition (migration 0005). Acceptable pre-1.0: no
-- production chain data predates this bump.
--
-- `source` is NOT NULL (never absent — non-substrate operator calls pass
-- 'manual'). The DEFAULT backfills existing rows and is unreachable from
-- code (every INSERT binds `source` explicitly). `payload` is nullable
-- (NULL for the common case — only action-specific scalars populate it).
-- TEXT (holding JSON) mirrors the cascade_subjects/cascade_snapshot_ids
-- convention so the sqlx::Any String binding works identically on both
-- backends.

ALTER TABLE audit_chain_entry ADD COLUMN source TEXT NOT NULL DEFAULT 'manual';
ALTER TABLE audit_chain_entry ADD COLUMN payload TEXT;

CREATE INDEX idx_audit_chain_source ON audit_chain_entry(source);
