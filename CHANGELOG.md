# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.8.0] - 2026-06-08

### Added

- `bind_audit_orphan_marker` persistent forensic table replacing v0.7's tracing-only orphan emit. State lifecycle (`unresolved → confirmed_orphan | record_present`), `(state, id)` keyset index for sweep pagination, RFC3339 TEXT timestamps for dual-backend compatibility. Migrations 0013 (SQLite) and 0014 (Postgres).
- `bind_audit_orphan_reconcile` background job — 5-second default tick (env override `PDS_BIND_AUDIT_ORPHAN_RECONCILE_INTERVAL_SECS`), `MissedTickBehavior::Skip`, keyset pagination for cycle bounded-work guarantees. Conditional spawn on `bind_audit_orphan_marker.enabled` (default `true`). Per-cycle structured tracing emits `examined / marked_confirmed_orphan / marked_record_present / left_unresolved_for_retry / pages_scanned / duration_seconds`.
- `AURORA_DEBUG_FORCE_ACTOR_COMMIT_FAILURE` and `AURORA_DEBUG_FORCE_SHARED_COMMIT_FAILURE` env-var gates for Phase B Scenario 6 verification. Debug-build-only via `#[cfg(debug_assertions)]`; not compiled into release builds. First-trigger warn-log via `aurora_debug_force_actor_commit_failure_active` / `aurora_debug_force_shared_commit_failure_active` events.
- End-to-end Phase B coverage of the orphan-marker forensic emit — the hardening-cycle deliverable v0.7 deferred. Positive path (gated actor failure → marker landed `unresolved` → sweep transitions to `confirmed_orphan` with `resolution_detail="actor store reports record absent"`) and negative path (gated shared failure → no marker, no audit row — audit-first ordering invariant holds) both verified on SQLite and Postgres.
- Write-path recovery mode (`AURORA_RECOVERY_MODE=true`): `validate_write` now synthesizes a `RecoveryBypass` authorization for unauthorized `tools.kryphocron.*` writes that would otherwise be denied, routing them through `bind_pipeline` with full bypass within the kryphocron-prefix branch (no lexicon validation, no closed-namespace deny, no dedicated-endpoint requirement). Recovery mode does NOT override the kryphocron master switch (`PDS_KRYPHOCRON_ENABLED=false`), authentication, takedowns, or any deny mechanism upstream of that branch.
- `RecoveryBypass` arm of `bind_pipeline` upgraded from tracing-only to a persistent emit: every recovery write lands a `kryphocron_recovery_write` `moderation_event` row (the v0.7-deferred audit event type now has its production emit site), carrying `{subject_uri, requester_did, nsid, action, cascade_source}`. Queryable via `tools.aurora.moderator.queryEvents`.
- Recovery writes participate in the `bind_audit_orphan_marker` reconciliation sweep: a recovery write whose paired actor commit fails materializes an orphan marker joining back to the recovery audit row (`moderation_event_id`), swept to `confirmed_orphan` like any other orphan-able emit. Cross-arc invariant upheld — `subject_uri` is always populated (`at://<did>/<collection>/<rkey>`), never NULL.
- `AURORA_RECOVERY_MODE` parse semantics are fail-closed: only `"true"` and `"1"` enable recovery mode; everything else (`"TRUE"`, `" 1"`, `"true "`, `"True\n"`, `""`, unset, any other value) is OFF. First synthesis per process fires a `aurora_recovery_mode_write_active` warn log once.
- `CascadeSource` now derives `Serialize` with an explicit rustdoc infallibility invariant (all variants must be infallibly JSON-serializable); the `RecoveryBypass` arm bridges `Option<CascadeSource>` → `Option<serde_json::Value>` via `serde_json::to_value(...).expect("infallible")`. `cascade_source` is always `None`/null in this cycle — non-null payloads land when cascade-initiating handlers are wired in a later arc.

### Changed
- B-contrast-verifier — full WCAG 2.2 resolver (var/color-mix/inheritance) fail-closed + broken-test-themes (Rust, §11.10.4) (#214)
- B-backend-substrate — manifest parse/validate + themes/ enumeration + inheritance resolution + validation contract + theme.deployment-default key + themes.listInstalled XRPC (Rust, §11.2/3/10) (#213)
- B-tokens — 28-token contract + alias-layer migration + aurora-default values (§11.5/§4.1) (#212)
- Arc A — foundational IA reshape (v0.9.0 meta tracker) (#193)
- A-debt-roles — §9.1 SettingsRoles vs RolesMembers reconciliation (#203)
- A-domains — four-domain structure + Kryphocron label/stub (item 1) (#196)
- A-recon — Arc A recon report (#194)
- A-i18n-readiness — §10.3.2/.4/.5/.6 disciplines + §10.3.3 lint rule (pending lint-host decision) (#205)
- A-debt-pages — §9.4 §9.6 §10.1.1 §10.1.4 §10.1.6 §10.2.3 §10.2.4 (#204)
- A-debt-roles — §9.1 SettingsRoles vs RolesMembers reconciliation (#203)
- A-debt-auth — §8.1.1 token rename + §8.1.4 authHeaders + §8.1.5 endpoints fold-in (#202)
- A-urlstate — url-state.js substrate + list-page consumers + tooltip (item 9) (#200)
- A-dashboard — de-tab → role-tiered composition + §10.1.3/§10.1.5 (item 6) (#199)
- A-phaseb — construct Arc A Phase B harness (docs/internal/v09-phase-b/arc-a.md) (#206)
- A-mode-gating — role×mode dispatch + sidebar visibility (item 4, §5.7.4) (#198)
- A-sidebar — reshape + label-visibility + mode re-render + bell-badge relocation (item 5) (#197)
- A-breadcrumbs — per-page breadcrumb/title updates + §10.4.2 separator (item 7,8) (#201)
- A-routes — settings→configuration rename + new config routes + legacy redirects (item 2,3) (#195)

- `validate_write` and `bind_pipeline` thread an `&mut Vec<i64>` for moderation event-id capture into `commit_with_orphan_recovery`, populating the new orphan marker row's `moderation_event_id` foreign-key reference.
- `bind_pipeline` signature gains `recovery_override: Option<KryphocronWriteAuthorization>` with override-first auth precedence. Production cannot reach the both-`Some` state — synthesis only fires when `write_op.kryphocron_authorization.is_none()` — so the override never masks a real per-write authorization.
- **atproto wire-shape compliance for session endpoints** (#185). `com.atproto.server.refreshSession` now reads the refresh token from `Authorization: Bearer <jwt>` instead of a JSON body; the legacy `{"refreshJwt": …}` body shape is no longer accepted (HTTP 401). `com.atproto.server.deleteSession` now authenticates with the refresh token from `Authorization: Bearer <jwt>` instead of an access token; access-token auth is no longer accepted (HTTP 401). Logout now atomically revokes the refresh token alongside the session — no replay-mint after logout. `com.atproto.server.revokeAppPassword` extended symmetrically: it now revokes the app password's refresh tokens alongside its sessions, closing a pre-existing orphan. The internal `test_endpoints.sh` harness is updated in the same change. **Breaking change** for any client using the legacy body shape on `refreshSession` or the access-token credential on `deleteSession`. This change also fixes a pre-existing bug where `refreshSession`'s rotation never marked rotated tokens `used` (the mark-used `UPDATE` was missing its `WHERE id` bind, so it matched 0 rows), leaving rotated tokens `used=false` and replayable through the mint path (#191); the "logout fully revokes" guarantee now holds on disk.

### Fixed

- Identifier-login endpoints (createSession, app-password login, requestPasswordReset) now accept DIDs for locally-created accounts (#184). Email addresses may no longer contain ':'.
- gc_sweep startup log severity raised from debug to warn so operators see "orphan-recovery is off" without filter tuning (#112).
- migrate_oauth CLI `revoke_all_sessions` now deletes paired refresh_token rows alongside session rows in a single transaction, matching the Q8/Q9 paired-revoke chokepoint pattern from Arc 4 (#190).
- `restore_account` and the `updateSubjectStatus` reverse-takedown path now emit `AccountEvent{active:true}` to the sequencer/firehose after restore, symmetrizing with the takedown direction's emit. Previously the takedown event landed but the restoration didn't, leaving downstream subscribers (firehose, AppView indexers) in stale-takedown state (#179).
- Oversized-commit WARN now fires at 25KB threshold in the sequencer commit-event path, giving operators a signal when client write paths produce unusually large commits (#90).
- Bind-audit reconcile job disabled-startup log raised debug→warn (adjacent to #112's gc_sweep + row_sweep fixes from commit `af05ed1`; same operator-blindness class).
- applyWrites now accepts both the atproto-spec discriminated `$type`-tagged shape (`com.atproto.repo.applyWrites#{create,update,delete}`) AND the existing flat `{action, collection, rkey, value}` shape via a serde-untagged enum. Standard bsky-PDS-shaped POSTs no longer 422; existing internal consumers using the flat shape continue working. No deprecation planned (#110).

## [0.7.0] - 2026-06-02

### Added

- kryphocron substrate integration: four dedicated XRPC procedures under `tools.kryphocron.*` for the user-class capabilities — `tools.kryphocron.feed.createPostPrivate` (EditPrivatePost), `tools.kryphocron.feed.deletePostPrivate` (DeletePrivatePost), `tools.kryphocron.actor.participatePrivate` (ParticipatePrivate), and `tools.kryphocron.policy.manageAudience` (ManageAudience).
- Host-side audience-oracle pre-check on `participatePrivate` for local-DID parent posts; out-of-audience writes are rejected with HTTP 403 and an audit emit. Cross-DID parents are deferred with a `tracing::warn!` (federation-backed audience read-through is post-v0.7 work).
- `KryphocronWriteAuthorization` per-write authorization carrier on `WriteOp` with five variants — `DedicatedEndpoint`, `Cascade`, `AccountSetup`, `RecoveryBypass`, `SystemCleanup` — matching `v07_DESIGN.md` §5 exhaustively.
- `bind_pipeline` dispatch in `validate_write` with three new structured tracing events: `kryphocron_bind_pipeline_authorized`, `kryphocron_bind_pipeline_denied`, `kryphocron_cascade_token_invalid`.
- `CascadeContext` + `CascadeToken` mint/verify machinery for cascade-authorized writes: single-use spent marker, cross-context isolation, source-mismatch rejection, and a one-shot depth-2 cap per context.
- Twelve new `ModerationEventType` variants per `v07_DESIGN.md` §4 covering substrate-flusher binds, host-side audience denial, housekeeping audit emits, and forensic-fallback paths. Payload structs land in a new `kryphocron_audit` module.
- Audit-first relay-race transaction ordering across the per-actor SQLite and shared account-DB transactions, with `commit_with_orphan_recovery` and an operator-visible `bind_audit_orphan_marker` `tracing::error!` emit on the narrow window where the per-actor commit fails after the shared-DB audit commit succeeded.
- `kryphocron`-prefix deny-map source-1 overrides redirect generic `createRecord` traffic for the four NSIDs with dedicated endpoints to the appropriate dedicated procedure via `KryphocronRecordRequiresDedicatedEndpoint { suggested_endpoint: Some(...) }`.
- Two new `PdsError` variants: `KryphocronCascadeTokenInvalid` (HTTP 403) and `KryphocronBindPipelineOutsideScope` (HTTP 500).

### Changed

- `WriteOp` lost its `Clone` derive: the `CascadeToken` carried by the `Cascade` authorization variant is single-use by design, and cloning a write op would double-spend the token. The `kryphocron_authorization` field is `#[serde(skip)]` so the `applyWrites` wire shape is unchanged.
- `SqliteRepoStorage`'s `RepoStorage` trait methods (`get_block`, `put_block`, `get_root`, `update_root`, plus the already-aware `apply_commit`) are now lent-transaction-aware. proto-blue's commit-assembly reads now see writes staged earlier in the same scope.
- `apply_writes` opens both transactions up front, threads them through `with_lent_txns`, and commits in audit-first order. The validate loop moved into each branch of the relay-race vs. legacy split.
- `validate_write` signature gained optional `shared_tx: Option<&mut sqlx::Transaction<'_, sqlx::Any>>` and `cascade_context: Option<&mut CascadeContext>` parameters; the dispatcher's bind-pipeline branch routes through them.
- `kryphocron` and `kryphocron-lexicons` dependencies moved from git+branch to crates.io version-deps now that both are published at v0.2.0.

### Notes

- The cascade-context infrastructure ships in v0.7, but no production caller constructs a `CascadeContext` yet. The first production use lands in a future cycle alongside the bsky-side cascade integration (per-audience updates on `block.create`, audience-delete cascade-reassign, bsky-delete cascade completion).
- The `RecoveryBypass` write-authorization variant ships for exhaustive coverage; no production constructor exists in v0.7. Write-path recovery-mode integration is deferred to a future cycle.
- Six of the twelve new `ModerationEventType` variants ship enum + payload only, pending the post-v0.7 cycles that wire their emit sites (substrate async flusher for category A; block / mute / threadgate dedicated endpoints; recovery-mode write-path; cascade-initiating handlers + orphan-companion sweep; sentinel-sink + panic-guard infrastructure for `KryphocronFallback`).
- End-to-end Phase B coverage of the orphan-marker forensic emit is deferred to the hardening cycle that ships the reconciliation sweep. Unit-test coverage at `src/actor_store/repo_storage.rs::step_3_5_tests` (`step_3_5_t6`, `step_3_5_t7`) exercises the emit path and the clean-failure path directly.
- Non-`list` audience modes (`everyone`, `followers`, `following`, `nobody`) fail closed to `NoAudienceConfigured` in v0.7's `participatePrivate` audience check until per-mode logic ships in a follow-up.

## [0.6.0] - 2026-05-27

### Added

- Live coverage for required validation mode (a schema-violating createRecord on a Required-mode instance hard-rejects with HTTP 400, no commit).
- Live coverage for concurrent same-DID importRepo collisions (loser receives HTTP 409 ConcurrentMutation; no double-write).
- Phase B verification harness as a first-class shell-script library under `phase-b/lib/`.
- DNS responder binary for live federation-resolution scenarios.
- `PDS_RATE_LIMIT_BUCKETS_RETENTION_DAYS` for operator-tunable rate-limit reaper window.
- `validate-config` warning when `PDS_SERVICE_DID` uses a `did:plc:` form (cross-PDS service-JWT path requires `did:web:`).
- `validate-config` warning when `PDS_LEXICON_DNS_NAMESERVER` is set (test-harness affordance forbidden in production).

### Changed

- Service-auth JWTs (`getServiceAuth`) are now signed with the per-account signing key so receiving PDSes can verify them.
- `verify_service_jwt` propagates a typed `DidTombstoned` error end-to-end, surfacing HTTP 400 with a structured wire-error body when the issuer DID is tombstoned.
- `ProductionLexiconFetcher` verifies the CAR's commit signature against the authority DID's `#atproto` verification key before treating the fetched lexicon as trusted.
- Federation-trust failure-class taxonomy gains an `invalid_signature` value, surfaceable at both the log layer and the HTTP 502 `LexiconFetchFailed` response.
- `cargo run` invocations across operator-facing docs now carry `--bin aurora-locus` to disambiguate against the new dev binary.
- Bumped `hickory-resolver` to 0.26.
- Bumped `proto-blue` to 0.3.3, collapsing the transitive `hickory-resolver` duplicate to the single patched copy.
- Bumped `openssl` to 0.10.80.
- Reorganized documentation to match the current codebase.
- Federation is now enabled by default; set `PDS_FEDERATION_ENABLED=false` to opt out.

### Removed

- Unused `DistributedStore::cas` trait method and `CasResult` enum (no production consumer).
- Unused `TtlCache` parse-cache module (no production consumer).
- Unused route-registry forward-substrate accessors (`FamilyKind::Public`, `RouteEntry.methods`, `RouteEntry.version`, `RouteRegistryBuilder::merge`, `CapsBuilder.version`).

### Fixed

- `did:web` handle construction no longer produces malformed `usera..localhost` when `service_handle_domains` carries the default leading-dot shape.
- `stage_blob` best-effort removes the staging temp file when the `temp_blob_metadata` INSERT fails.
- `uploadBlob` accepts `application/octet-stream` (the detection fallback).
- `grant_role` / `revoke_role` error responses ship the structured `{error, message}` JSON envelope rather than plain-text bodies.
- `getAuditTrail` handler uses the shared `audit_chain::audit_entry_from_row` helper that `exportAccountForensic` already uses; wire shape unchanged.
- `resolveHandle` returns HTTP 400 `HandleNotFound` on unresolvable handles instead of HTTP 500.

## [0.5.0] - 2026-05-23

### Added

- Sequencer producer emits `#account` and `#sync` events on every account-lifecycle path (creation, deactivation, reactivation, takedown, deletion, PLC submission), matching the reference PDS frame sequences.
- `com.atproto.admin.updateSubjectStatus` emits `#account` events on canonical-path admin mutations.
- Dynamic lexicon loading: resolver, two-layer cache (in-memory + on-disk), single-flight de-dup, and explicit `HardFail` / `Warn` failure semantics.
- `tools.aurora.lexicon.*` admin endpoints (`getCacheState`, `fetchNow`, `evictCache`).
- `importRepo` endpoint accepting a full CAR upload, validating structure, pre-fetching referenced blobs from the origin PDS, and applying records under the actor's signing key.
- Two-phase commit on blob writes (bytes-durable-before-row, with fsync on the disk backend).
- TTL-bounded staged-orphan cleanup via a row-driven GC sweep.
- STRICT-before-unreference accounting on every record-write path.
- `BlobNotFound` typed error variant (404 → 400 at the wire per spec).
- Structured forensic-event log entries with consistent `failure_class` taxonomies (DNS, DID, PDS-unreachable, HTTP, timeout, authority-tombstoned, schema, invalid-NSID).
- Metrics for lexicon fetch attempts, cache hit/miss rates, single-flight collapse counts, and per-collection validation outcomes.
- Postgres backend matrix coverage on every shipped surface (backend-conditional `FOR UPDATE`, READ COMMITTED isolation pinning, type-compatible read layer).

### Changed

- PLC `410 Gone` routes to a typed `DidTombstoned` variant rather than HTTP 500.
- Record-write signing uses the per-account repo key rather than a server-wide key.
- `importRepo` forensic events report accurate per-CID failure counts.

### Fixed

- Postgres login decode tolerates the cross-backend `TIMESTAMPTZ` vs `Option<String>` divergence (silent-fail mask removed).

## [0.4.0] - 2026-05-13

### Added

- `DistributedStore` trait substrate (insert / get / delete / cas / reap_expired) and `PostgresCasStore` backend over `sqlx::Any`, making cross-instance auth state and rate-limit buckets coherent.
- `PDS_DISTRIBUTED_STATE_MODE` config (`distributed` / `single_instance_inmemory` / `redis` reserved).
- `dpop_jti_replay` and `rate_limit_buckets` tables (migration 0007, SQLite + Postgres).
- Dedicated maintenance pool with separate `PDS_MAINTENANCE_DB_*` sizing.
- Three background reapers for DPoP JTI replay, OAuth flow state, and rate-limit buckets.
- Operator guide for multi-instance deployment.
- Runtime `RouteRegistry` substrate populated at route registration; `describeCapabilities` reads from it instead of a hand-curated list.
- Clock trait abstraction for deterministic time-source injection (adopted by `identity::cache`).
- Optional background blob GC sweep (off by default; dry-run when enabled).
- `aurora-locus gc-sweep` CLI subcommand for operator-initiated one-off sweeps.
- `GcSweepConfig` env-var / file-tier surface, four `validate-config` safety warnings, and three Prometheus metrics.
- Operator guide for the blob GC sweep.
- Debug-build-only `dev.aurora.*` HTTP namespace (`grantAdmin`, `revokeAdmin`, `listAdmins`, `createAccount`, `mintToken`); release builds do not include the surface.
- `AuroraErrorTranslations` module mapping server structured-error codes to operator-friendly prose in the admin UI.
- `AuroraModal.form` and `AuroraModal.destructiveConfirm` helpers (promise-returning, with live validation, typed-confirm gates, required rationale, ack checkboxes).
- `chainVerified` indicator on the audit page with three-state semantics and detail panel.
- `cascadeSnapshotIds` cascade-subjects rendering on audit-entry detail.
- `subject_cid` filter and `timeRange` preset dropdown on the audit and Dashboard surfaces.
- `auditEntryId` click-through toast on 11 success toasts.
- Role-grant and role-revoke flows on the admin UI (canonical destructive-confirm pattern: typed-confirm + required rationale + audit-entry click-through).
- Dual-shape acceptance on `tools.aurora.admin.emitEvent` (canonical `subjects[]` + legacy `subject`) and `com.atproto.admin.updateSubjectStatus` (`record_uri` snake_case + legacy `recordUri` camelCase); requests sending both shapes return 400.
- `aurora_legacy_wire_ingest_total` Prometheus metric for tracking per-field legacy-shape ingest.
- Structured tracing event `legacy_wire_shape_ingested` at INFO level.
- JWT-deprecation middleware wired into the router stack with structural Authorization-header inspection.
- CLI sentinel rendering for `cli:`-prefixed actor strings as non-clickable badges.

### Changed

- `exportAccountForensic` bundle's `audit-entries.json` now uses the canonical `AuditEntry` wire shape, identical to `getAuditTrail`. Manifest `schemaVersion` bumped to `"2"`. **Breaking** for consumers scripted against the v1 forensic bundle.
- Identity cache time source migrated from direct `chrono::Utc::now()` to injected `Clock`.
- `SubscribeMessage::AuditEntry`'s `entry` field changed from `AuditEntry` to `Box<AuditEntry>`. Wire shape preserved via serde transparent `Box<T>`.
- OAuth handlers route through the `DistributedStore` trait for cross-instance-relevant operations.
- DPoP JTI replay routes through the trait in `distributed` mode (with the in-memory `HashMap` path retained for `single_instance_inmemory`).
- Rate-limit middleware adds a PRIORITY-0 distributed pre-check before the in-process governor; substrate-consult failure falls through to the governor non-fatally.
- `get_authorization_request` collapses "expired" and "not found" into a single `NotFound`.
- `describeCapabilities` handler reads from the `RouteRegistry` (byte-identical wire output preserved).
- 13 native `confirm()` / `prompt()` call sites in the admin UI migrated to `AuroraModal` helpers.
- `SettingsGeneral` and `SettingsUiModes` render all four `SettingSource` values (Runtime / File / Default / RecoveryMode) with informational suffixes.
- `BulkActionPanel` `MAX_BATCH_SIZE` switched from singleton constant to per-action lookup (`DeleteAccount` = 10, `DeleteBlob` = 25, default = 50).
- `ActionPanel.js` payload construction emits `subjects: [this.subject]` (v0.3 canonical).

### Removed

- Pre-existing Redis-backed `DistributedRateLimiter` module (replaced by the trait surface; `Redis` enum slot reserved).
- `RateLimitConfig.use_redis` and `RateLimitConfig.redis_url` env-var reads (no consumers post-removal).
- `crate::oauth::authorize::cleanup_expired_requests` (now routed through `store.reap_expired`).
- Hand-curated `aurora_capability_families()` and `aurora_capability_extensions()` functions; registry-driven generation replaces them.
- Three dead-code helper functions in `src/api/aurora_admin.rs` (`require_repo_did`, `subject_uri_cid`, `require_blob_cid`).

### Fixed

- 24 clippy `-D warnings` errors cleared (dead_code, manual_clamp, if_same_then_else, doc_lazy_continuation, useless_format, redundant_closure, large_enum_variant, doc_overindented_list_items).
- `test_stale_handle_detection` flakiness via programmatic `MockClock` advancement (runtime ~22s → ~0.15s deterministically).
- Pre-existing `authorization_request` schema/model mismatch (vestigial `id` and `code_used_at` model fields with no backing columns) worked around without schema change; consumers select only existing columns.

### Documentation

- `src/cli/validate_config.rs` audit-date comment confirming all 18 emitted warnings are still valid.
- Per-key value formats section in the file-tier-config operator doc.

## [0.3.0] - 2026-05-10

### Added

- File-tier YAML runtime configuration at `<data_directory>/runtime.yaml`, sitting between the runtime API and the compiled-in defaults. `PDS_RUNTIME_FILE` overrides the path; unknown keys warn and skip. New `serde_yaml` dependency.
- `getRuntimeSetting`'s response `source` field gains a fourth value, `"File"` (typed `SettingSource` enum).
- Audit-trail read contract committed as a stability surface: `tools.aurora.admin.getAuditTrail` ships `cascadeSnapshotIds` on the wire and pins the seven-filter set, pagination, and verification semantics.
- Six stability contracts committed: `Subject` and `ReportSubject` variant stability, `describeCapabilities` response shape, capability-string versioning, action-ID surfacing (`auditEntryId` / `eventId`), audit-trail read contract, and multi-subject `emitEvent`. Pinned by doc-comments + snapshot tests + structural lint.

### Changed

- `tools.aurora.admin.emitEvent` accepts `subjects: Vec<Subject>` on input and returns `snapshots: Vec<SnapshotRef>` paired 1:1-by-index. Per-action `MAX_BATCH_SIZE` caps (`DeleteAccount` = 10, `DeleteBlob` = 25, others = 50). **Breaking**: single-subject callers migrate by wrapping in a one-element array.
- Batch handlers (`batchTakedownAccounts`, `batchSuspendAccounts`, `batchRestoreAccounts`, `batchTakedownRecords`, `batchApplyLabel`, `batchRemoveLabel`) all adopt whole-batch atomicity; per-subject mutation failure aborts the wrapping tx. **Breaking**: `failures` field removed from output shapes; `affected_count` equals `cascade_subjects.len()` for successful responses.
- `_in_tx` companion variants added to `BlobQuarantine`, `BlobStore`, `AppealManager` so every `dispatch_action` arm runs through a tx-bound execution path.
- Single-subject chain entries now populate BOTH the flat `subject_did`/`subject_uri`/`subject_cid` columns AND `cascade_subjects: [s]`. External consumers can read either surface.
- `getModerationMetrics` accepts both canonical `timeRange` preset and legacy peer `start`/`end` shapes (dispatched via custom `Deserialize`).
- `GetQueueStatsOutput` selectively retypes six count and age fields from `i64` to `u32`; JSON wire shape unchanged.
- `TimeRange` validated newtype primitive (`crate::admin::TimeRange`) constructible from preset name or explicit object; rejects inverted ranges at deserialize time.
- `tools.aurora.superadmin.grantRole` and `revokeRole` responses are typed structs with camelCase wire fields (`audit_entry_id` → `auditEntryId`, `admin_role` → `adminRole`). **Breaking** wire shape against ad-hoc JSON.

### Removed

- `PDS_ADMIN_DIDS` env-var support and `admin_dids` field on `AuthConfig`. Admin authority comes from the `admin_roles` table only; the first SuperAdmin is bootstrapped via the `grant-admin` CLI.

### Documentation

- Flat-shape commitment for `ModEventAction`: the 16-variant enum is committed; compositional reshape is a later-cycle candidate.
- Runtime route enumeration deferred to v0.4; `describeCapabilities` continues to advertise a hand-curated list with snapshot-test drift detection.
- Operator-doc S3 framing corrected to reflect the as-built state (S3 backend already shipped).

## [0.2.0] - 2026-05-04

### Added

- `mod_event_seq` table mirroring the wire-emitted subset of `moderation_event` columns; `subscribeModEvents` now reads from this retention-bounded surface. Migration 0006 (SQLite + Postgres).
- `PDS_MOD_EVENT_RETENTION_DAYS` env-var (default 7) controls the `mod_event_seq` retention window; a background cleanup job deletes rows older than the window.
- `OutdatedCursor` wire-format variant in `subscribeModEvents`; emitted when the caller's cursor is older than the oldest retained `mod_event_seq.seq`.
- `failures: BatchFailure[]` field on batch op responses (interim shape; later refined).
- Per-subject snapshot capture for the six `tools.aurora.admin.batch*` endpoints; new `cascade_snapshot_ids` JSON column on `audit_chain_entry`. Migration 0005 (SQLite + Postgres).
- Audit chain coverage for 26 administrative call sites under `com.atproto.admin.*`.
- `chainVerified` and `chainVerifiedThrough` fields on `getAuditTrail`. Backed by a new `verify_chain_range` API that walks the chain checking both per-row hash and prior-row linkage.
- Audit chain coverage for nine previously-unhooked endpoints (six batch ops plus `triggerPasswordReset`, `grantRole`, `revokeRole`).
- `AuditEntry` real-time streaming in `subscribeModEvents` via `includeAuditChain: true`; resume via the new `auditChainCursor` parameter.
- DPoP proof-of-possession enforcement on resource requests per RFC 9449 §4.3.
- Design corpus updated to reflect as-built reality across five surfaces (forensic export account-state, describeCapabilities curated list, two-tier runtime settings, subject-aware ModEventAction, flat `start`/`end` request shape).

### Changed

- `tools.aurora.admin.getModerationMetrics` switched from POST to GET; inputs move from JSON body to query parameters. **Breaking**: POST clients receive 405.
- `audit_chain::append_entry` gains an `_in_tx` companion so admin handlers can land the chain entry atomically with their underlying mutation. Five sites migrated.
- `Subject::Blob` round-trips `record_uri` through `audit_chain_entry`'s flat columns (was previously dropped on the producer side).
- `getAuditTrail`'s `chainVerifiedThrough` surfaces `failing_sequence - 1` on chain verification failure (collapses the prior all-failures-equal-zero signal).
- `tools.aurora.admin.setRuntimeSetting` validates the `key` against an allowlist (`KNOWN_RUNTIME_KEYS`). Unknown keys return 400.
- Crate version bumped from 0.1.0 to 0.2.0 (propagates to `describeCapabilities.version` and the firehose `Hello` frame).
- Documentation refactored: cycle-authored design docs consolidated; pre-cycle docs untouched.
- `tools.aurora.admin.subscribeModEvents` cursor space changed (now identifies `mod_event_seq` rows). Old cursors trigger an `OutdatedCursor` frame on next connect.
- `tools.aurora.admin.emitEvent` `SendEmail` action now requires Admin+ role (was Moderator+). **Breaking** for any Moderator-emitted SendEmail caller.
- Batch op responses carry a `failures` array; per-subject failures land in the field rather than rolling back the chain entry. `affected_count` semantics updated to "subjects whose mutation actually applied".
- `com.atproto.admin.getAuditLog` reads from the hash-chained `audit_chain_entry` table (wire format preserved; `ip_address` always omitted).
- `apply_account_status` and `apply_blob_status` no longer write per-patch chain entries inline; the handler joins patch-effect descriptors into a single chain row.
- Four operator-flavored `tools.aurora.admin.*` endpoints (`triggerPasswordReset`, `exportAccountForensic`, `getRuntimeSetting`, `setRuntimeSetting`) require `AdminServer` scope. **Breaking** for `AdminModeration`-only tokens.
- `subscribeModEvents` `actionFilter` is now an array; scalar-string clients fail to deserialize. Added `subjectUri` filter for record-level events.
- DPoP proof issuance no longer silently downgrades to Bearer on invalid proof; invalid proofs return HTTP 400 per RFC 9449 §5.
- Sequencer leader-election advisory lock runs on a dedicated connection separate from the application pool. Pool sizing model: `pool_size + 2`.
- `subscribeModEvents` ships an `AuditEntry` wire variant alongside `Event` / `Hello` / `Heartbeat` / `Error`. Exhaustive `$type` consumers need to handle the new variant.
- Moderation-reason classifier generalized to match any non-canonical reason NSID; behavior preserved.
- Closed the v0.2 cycle handoff.
- Production-primitives phase (Postgres backend).
- Multi-instance support (Postgres backend).
- Query layer compatibility (Group A+B files, Postgres backend).
- Stale `ModEventAction` variant list replaced with a code reference in the design doc.
- Tightened override-mechanism citation in the endpoint inventory.
- Fixed `listAccounts` line citation drift in the endpoint inventory.
- Added Postgres-multi-instance + runtime-settings env vars to `.env.example`.
- Updated `aurora_admin.rs` module docstring for the shipped admin-extensions phase.
- Documented `getSubjectStatus` blob-branch 501 in the design doc.
- Noted the audit-chain visibility gate is structurally always-true today.
- Documented the `audit-trail.json` redirect-string pattern.
- Fixed a stale section citation in `src/cache/invalidation.rs`.
- Updated the LISTEN/NOTIFY backoff schedule documentation to match the as-built six-step.
- Dropped a stale `axum 0.7 → 0.8` dependency-bump claim from the design doc.
- Brought the Audit page prose into line with the no-sentinel-rows reality.
- Removed named external-tooling references in the design doc.
- Rewrote an `src/oauth/scope.rs` comment to match the AdminServer-only behavior.
- Updated `listRecentEvents` route comment to past tense after the admin-extensions phase shipped.
- Clarified the pre-chain sentinel as defensive-only.
- Rewrote the lexicon-shape audit against the as-shipped state.
- Renamed `entries` → `items` and documented chain-level fields on `GetAuditTrailOutput` in the design doc.
- Removed `src/api/admin_panel.rs.bak` from the tree.
- Verified and tightened `EmitEventOutput.audit_entry_id` after the audit-chain handlers shipped.
- Tightened batch endpoint `audit_entry_id` type from `Option<String>` to `String`.
- `subscribeModEvents` `AuditEntry` shape: wrap in `entry` field and add missing `AuditEntry` fields.

### Fixed

- 17 administrative call sites in `aurora_admin.rs` and `admin.rs` now perform chain-write paired with actor-state mutation atomically. New `_in_tx` variants on `AccountManager`, `ModerationManager`, `ReportManager`, `InviteCodeManager`. `sendEmail` and `triggerPasswordReset` reorder to chain-first so a mailer failure doesn't leave operator action un-audited.
- 12 additional administrative call sites (six batch handlers plus `updateAccountEmail`) write the audit chain entry atomically with the underlying mutation; `_in_tx` companions on `AccountManager` and `LabelManager`.
- `tests/multi_instance_test.rs` compiles after the dedicated-lock-connection refactor.
- Forensic export `bundle_hash` now covers the complete tar bytes (was previously covering only `manifest.json`).
- Audit chain `append_entry` serializes concurrent writers (in-process mutex + transaction wrap + Postgres advisory lock); silent chain-entry loss under bursty load eliminated.
- Admin router fallback pages (not-found, forbidden, error) no longer flow URL-derived input through `innerHTML`; static-source pin test asserts the change.
- Admin UI grant/revoke role calls now succeed (field renamed from `subject` to `did`); static-source pin test catches regression.

### Removed

- `admin_audit_log` table dropped via migration 0004 (SQLite + Postgres). `AdminRoleManager::log_action`, `get_audit_logs`, `get_audit_log_count` removed with it. Every administrative decision lands in `audit_chain_entry`.
- `PDS_ADMIN_DIDS` no longer grants admin authority. Admin authority comes from the `admin_role` table only.
- Admin UI no longer stores `adminRefreshToken` in localStorage.

### Security

- Live subscription channel storage bounded by operator-configured retention via the new `mod_event_seq` mirror; supports GDPR-style data-minimization windows.
- Shadow audit surface eliminated: every administrative decision now goes through `audit_chain::append_entry`'s serialized writer.
- DPoP proof verification wired end-to-end on resource requests, closing the stolen-token replay path.
- Audit chain forensic-export tampering window closed via the complete-tar `bundle_hash` fix.
- Audit chain `append_entry` race-loss eliminated; silent chain-entry loss under bursty admin load no longer possible.
- `/admin/debug.html` no longer reachable in production builds. Gated behind `PDS_ENABLE_DEBUG_PAGES`.
- `PDS_ADMIN_DIDS` shadow grant of admin authority eliminated.

## [0.1.0] - 2026-04-30

### Changed

- Fix rotted integration tests in tests/ (#14)
- Update deps + run clippy/fmt (#13)
- Migrate from embedded Rust-Atproto-SDK to proto-blue crate (#1)
- Delete Rust-Atproto-SDK directory and verify build (#12)
- Adapt cli/rotate_keys.rs to format_resign_commit (#11)
- Update api/repo.rs callers and signer plumbing (#10)
- Rewrite actor_store/repository.rs against proto_blue::repo::Repo (#9)
- Implement Signer wrapper around k256 PLC key (#8)
- Implement RepoStorage for ActorStore (SQLite-backed) (#7)
- Adapt actor_store/car.rs to proto-blue read_car/blocks_to_car (#6)
- Migrate leaf imports: did_doc, identity, oauth, syntax, tid (#5)
- Vendor blob mime/size helpers into src/blob_store/mime.rs (#4)
- Vendor PasswordHasher into src/auth/password.rs (#3)
- Add proto-blue dep, remove path dep on Rust-Atproto-SDK (#2)
