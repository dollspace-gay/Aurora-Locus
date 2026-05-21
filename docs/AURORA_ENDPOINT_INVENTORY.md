# Aurora-Locus Endpoint Inventory

**Initial generation:** 2026-05-03 mid-cycle, post-Phase-3.4 / pre-3.5.
**Status:** v0.2-cycle complete — admin/moderation Phases 1, 2, and 3.1
through 3.10 all shipped. Endpoint tables below reflect the as-shipped
v0.2 surface; the discrepancy notes at the end capture implementation-
vs-design divergences that may be of interest to UI designers and
downstream consumers.

**Companion docs:** [AURORA_DESIGN.md](AURORA_DESIGN.md) for the
server-side design that backs these endpoints;
[AURORA_ADMIN_UI_DESIGN.md](AURORA_ADMIN_UI_DESIGN.md) for the admin UI
surface that consumes them.

**Lexicon convention:** Aurora-Locus does **not** ship JSON lexicon
files. Per CLAUDE.md, lexicons are defined as Rust types — request /
response shapes live in handler signatures and adjacent serde structs.
The "lexicon surface" enumerated here is the route table at
[src/api/admin.rs:38-350](../src/api/admin.rs#L38-L350) plus handler
modules under [src/api/](../src/api/). NSID descriptions below come from
the leading `///` doc comment on each handler (where present); empty
cells indicate the handler has no explicit one-liner — derive from
the NSID or the section comment block.

**Auth scope source:** [src/oauth/scope.rs:714-748](../src/oauth/scope.rs#L714).
Mapping is uniform per namespace prefix — namespace-level scope
enforcement happens in `namespace_scope_check` middleware before the
handler runs. Within-tier role checks (Moderator vs Admin vs
SuperAdmin) happen at the handler level via
`AdminAuthContext::role.can_act_as(...)`.

**Handler-shipped column:** every NSID in the route table has a wired
handler (no stub-only routes — `unimplemented!()` and `todo!()` are
banned by CLAUDE.md). The column captures `✅ shipped` everywhere
under the current build.

---

## com.atproto.admin.*

Parity floor with bsky-PDS plus Aurora-specific extensions inherited
from the initial Phase 1 work. **Actual count: 34 routes (33 unique
handlers; `listAccounts` is an alias to `getUsers`).** The expected
~15 in the brief reflects the bsky-PDS upstream baseline; Aurora's
surface adds invite admin, role/audit, label, report, and sequencer-
event endpoints atop that.

**Auth:** `AdminServer` OR `AdminModeration` (either accepted; some
endpoints are operator-flavored, some moderation-flavored, and
upstream's design didn't draw a lexicon-level distinction).

| NSID | Type | Description | Auth | Last commit | Shipped |
|---|---|---|---|---|---|
| com.atproto.admin.getUsers | query | Get list of users | AdminServer\|AdminModeration | initial commit | ✅ |
| com.atproto.admin.listAccounts | query | bsky-PDS-compat alias to getUsers (operator-flavored listing lives at tools.aurora.ops.listAccounts) | AdminServer\|AdminModeration | Phase 2.4: Remove legacy operator endpoints | ✅ |
| com.atproto.admin.getAccount | query | Get single account details | AdminServer\|AdminModeration | fixes | ✅ |
| com.atproto.admin.searchAccounts | query | Cursor-paginated account search; shared `accountView` shape | AdminServer\|AdminModeration | Phase 1.5: Add searchAccounts XRPC endpoint | ✅ |
| com.atproto.admin.getAccountInfo | query | Single-account info via `build_account_info` helper | AdminServer\|AdminModeration | Phase 1.4: Add getAccountInfo (singular) XRPC endpoint | ✅ |
| com.atproto.admin.getAccountInfos | query | Batched account info; uses axum_extra Query for repeated keys | AdminServer\|AdminModeration | refactor(sdk): proto-blue 0.2.6 | ✅ |
| com.atproto.admin.updateSubjectStatus | procedure | Polymorphic subject-status update (Repo / Record / Blob) | AdminServer\|AdminModeration | refactor(sdk): proto-blue 0.2.6 | ✅ |
| com.atproto.admin.getSubjectStatus | query | Current moderation status of a subject (takedown / deactivation / suspension) | AdminServer\|AdminModeration | refactor(sdk): proto-blue 0.2.6 | ✅ |
| com.atproto.admin.createInviteCode | procedure | Create an invite code | AdminServer\|AdminModeration | refactor(sdk): proto-blue 0.2.6 | ✅ |
| com.atproto.admin.getInviteCodes | query | List invite codes (lexicon's sort/limit/cursor; legacy `includeDisabled` removed in Phase 1.10) | AdminServer\|AdminModeration | refactor(sdk): proto-blue 0.2.6 | ✅ |
| com.atproto.admin.listInviteCodes | query | Aurora-Locus surface paralleling getInviteCodes; shares pagination machinery | AdminServer\|AdminModeration | refactor(sdk): proto-blue 0.2.6 | ✅ |
| com.atproto.admin.disableInviteCode | procedure | Disable an invite code | AdminServer\|AdminModeration | refactor(sdk): proto-blue 0.2.6 | ✅ |
| com.atproto.admin.disableInviteCodes | procedure | Bulk-disable; transactional all-or-nothing per Phase 1.3 | AdminServer\|AdminModeration | Phase 1.3: Add disableInviteCodes (plural) XRPC endpoint | ✅ |
| com.atproto.admin.enableAccountInvites | procedure | Enable invite-code creation for an account | AdminServer\|AdminModeration | refactor(sdk): proto-blue 0.2.6 | ✅ |
| com.atproto.admin.disableAccountInvites | procedure | Disable invite-code creation for an account | AdminServer\|AdminModeration | refactor(sdk): proto-blue 0.2.6 | ✅ |
| com.atproto.admin.listRoles | query | List admin roles (kept here intentionally per Phase 3.6 — Moderators may need role visibility without SuperAdmin) | AdminServer\|AdminModeration | fixes and SDK | ✅ |
| com.atproto.admin.updateAccountEmail | procedure | Update account email address | AdminServer\|AdminModeration | refactor(sdk): proto-blue 0.2.6 | ✅ |
| com.atproto.admin.updateAccountHandle | procedure | Update account handle | AdminServer\|AdminModeration | refactor(sdk): proto-blue 0.2.6 | ✅ |
| com.atproto.admin.updateAccountPassword | procedure | Update account password (admin override) | AdminServer\|AdminModeration | refactor(sdk): proto-blue 0.2.6 | ✅ |
| com.atproto.admin.deleteAccount | procedure | Delete account permanently (admin operation) | AdminServer\|AdminModeration | refactor(sdk): proto-blue 0.2.6 | ✅ |
| com.atproto.admin.updateAccountSigningKey | procedure | Update account signing key (Aurora-architecture safety constraint in strict mode) | AdminServer\|AdminModeration | Phase 1.2: Add updateAccountSigningKey XRPC endpoint | ✅ |
| com.atproto.admin.takedownAccount | procedure | Takedown an account (remove from public view) | AdminServer\|AdminModeration | refactor(sdk): proto-blue 0.2.6 | ✅ |
| com.atproto.admin.suspendAccount | procedure | Suspend an account temporarily | AdminServer\|AdminModeration | refactor(sdk): proto-blue 0.2.6 | ✅ |
| com.atproto.admin.restoreAccount | procedure | Restore an account after takedown / suspension | AdminServer\|AdminModeration | refactor(sdk): proto-blue 0.2.6 | ✅ |
| com.atproto.admin.getModerationHistory | query | Moderation history for an account | AdminServer\|AdminModeration | refactor(sdk): proto-blue 0.2.6 | ✅ |
| com.atproto.admin.getModerationQueue | query | Reports needing review | AdminServer\|AdminModeration | refactor(sdk): proto-blue 0.2.6 | ✅ |
| com.atproto.admin.applyLabel | procedure | Apply a label to content | AdminServer\|AdminModeration | fixes and SDK | ✅ |
| com.atproto.admin.removeLabel | procedure | Remove a label from content | AdminServer\|AdminModeration | fixes and SDK | ✅ |
| com.atproto.admin.submitReport | procedure | Submit a report | AdminServer\|AdminModeration | fixes and SDK | ✅ |
| com.atproto.admin.updateReportStatus | procedure | Update report status | AdminServer\|AdminModeration | refactor(sdk): proto-blue 0.2.6 | ✅ |
| com.atproto.admin.listReports | query | List reports | AdminServer\|AdminModeration | fixes and SDK | ✅ |
| com.atproto.admin.sendEmail | procedure | Send admin email (warnings, notifications, etc.) | AdminServer\|AdminModeration | Multiple issue commit | ✅ |
| com.atproto.admin.getAuditLog | query | Filterable audit log (admin DID / action type / subject DID) | AdminServer\|AdminModeration | Multiple issue commit | ✅ |
| com.atproto.admin.listRecentEvents | query | Sequencer event review (moderation-flavored; ops controls live at tools.aurora.ops.{getSequencerStatus, pauseSequencer, ...}) | AdminServer\|AdminModeration | Phase 2.4: Remove legacy operator endpoints | ✅ |

---

## tools.aurora.{describeCapabilities, moderator, admin, superadmin}.*

Aurora moderation/admin extension surface — the federated
moderation pairing target. **Post-cycle count: 25 (1 top-level + 7
moderator + 2 superadmin + 15 admin),** spanning Phases 3.2 through
3.10. The initial generation captured Phases 3.2-3.4 + 3.6 only;
post-cycle the `tools.aurora.admin.*` namespace is heavily populated.

**Auth:** `AdminModeration` (namespace-level). Within-tier checks
(Moderator vs Admin vs SuperAdmin) happen at the handler via
`AdminAuthContext::role.can_act_as(...)`. The four
`tools.aurora.admin.*` operator-flavored endpoints
(`triggerPasswordReset`, `exportAccountForensic`, `getRuntimeSetting`,
`setRuntimeSetting`) require `AdminServer` scope per UI design
§8.6/§8.7/§8.16. The override is a per-NSID lookup that runs before
the namespace prefix match, **replacing** (not augmenting) the
namespace default — `AdminModeration` alone is insufficient. See
[src/oauth/scope.rs:848-865](../src/oauth/scope.rs#L848-L865) for the
operator-NSID table and [AURORA_DESIGN.md §4.3.4](AURORA_DESIGN.md)
for the auth-tier framing the override implements.

| NSID | Type | Description | Auth (within-tier) | Last commit | Shipped |
|---|---|---|---|---|---|
| tools.aurora.describeCapabilities | query | Capability probe — clients discover which Aurora extensions this instance supports without trial-and-error | AdminModeration (any role) | Admin/moderation Phase 3.2: tools.aurora.describeCapabilities | ✅ |
| tools.aurora.moderator.queryEvents | query | Paginated query of moderation events with rich-context handle resolution (Phase 3.3) | AdminModeration (Moderator+) | Admin/moderation Phase 3.3: Moderator-tier read endpoints | ✅ |
| tools.aurora.moderator.getEvent | query | Single moderation event by ID with resolved actor / subject handles (Phase 3.3) | AdminModeration (Moderator+) | Admin/moderation Phase 3.3: Moderator-tier read endpoints | ✅ |
| tools.aurora.moderator.queryStatuses | query | Paginated query of subject statuses; subject_type=Record\|Blob short-circuits to empty pending per-record/per-blob status surfaces (Phase 3.3) | AdminModeration (Moderator+) | Admin/moderation Phase 3.3: Moderator-tier read endpoints | ✅ |
| tools.aurora.moderator.getSubjectContext | query | Comprehensive view of a subject — actor row + recent actions + recent reports + recent appeals (Phase 3.3) | AdminModeration (Moderator+) | Admin/moderation Phase 3.3: Moderator-tier read endpoints | ✅ |
| tools.aurora.moderator.getSubjectHistory | query | Chronological action history for one subject; sortable asc/desc, action-filterable (Phase 3.3) | AdminModeration (Moderator+) | Admin/moderation Phase 3.3: Moderator-tier read endpoints | ✅ |
| tools.aurora.moderator.listAppeals | query | Paginated appeals query with status / appellant / reviewer / date-range filters; embeds AppealView shape (Phase 3.4) | AdminModeration (Moderator+) | Admin/moderation Phase 3.4: Moderator-tier appeals reads | ✅ |
| tools.aurora.moderator.getAppeal | query | Single appeal by ID with full timeline (lifecycle entries) and original-action summary (Phase 3.4) | AdminModeration (Moderator+) | Admin/moderation Phase 3.4: Moderator-tier appeals reads | ✅ |
| tools.aurora.admin.emitEvent | procedure | Unified action surface — ModEvent + Subject → moderation_event + audit_chain_entry in one tx (Phase 3.5) | AdminModeration (Admin+) | Admin/moderation Phase 3.5A: tools.aurora.admin.emitEvent | ✅ |
| tools.aurora.admin.batchTakedownAccounts | procedure | Multi-DID account takedown; one chain entry, per-subject failures (Phase 3.5) | AdminModeration (Moderator+) | Admin/moderation Phase 3.5B: Six batch endpoints + triggerPasswordReset | ✅ |
| tools.aurora.admin.batchSuspendAccounts | procedure | Multi-DID account suspension (Phase 3.5) | AdminModeration (Moderator+) | Admin/moderation Phase 3.5B: Six batch endpoints + triggerPasswordReset | ✅ |
| tools.aurora.admin.batchRestoreAccounts | procedure | Multi-DID account restore; per-DID `UPDATE actor SET takedown_ref = NULL` failures land in `failures` (Phase 3.5) | AdminModeration (Moderator+) | Admin/moderation Phase 3.5B: Six batch endpoints + triggerPasswordReset | ✅ |
| tools.aurora.admin.batchTakedownRecords | procedure | Multi-record takedown via `!takedown` self-label (Phase 3.5) | AdminModeration (Moderator+) | Admin/moderation Phase 3.5B: Six batch endpoints + triggerPasswordReset | ✅ |
| tools.aurora.admin.batchApplyLabel | procedure | Multi-subject label application (Phase 3.5) | AdminModeration (Moderator+) | Admin/moderation Phase 3.5B: Six batch endpoints + triggerPasswordReset | ✅ |
| tools.aurora.admin.batchRemoveLabel | procedure | Multi-subject label removal; subjects without the label land in `skipped` (Phase 3.5) | AdminModeration (Moderator+) | Admin/moderation Phase 3.5B: Six batch endpoints + triggerPasswordReset | ✅ |
| tools.aurora.admin.triggerPasswordReset | procedure | Trigger password-reset flow for an account; rationale-required (Phase 3.5) | AdminServer (Admin+ role) | Admin/moderation Phase 3.5B: Six batch endpoints + triggerPasswordReset | ✅ |
| tools.aurora.superadmin.grantRole | procedure | Grant admin role to a user (relocated from com.atproto.admin per Phase 3.6) | AdminModeration + SuperAdmin role | Admin/moderation Phase 3.6: Relocate role management to tools.aurora.superadmin.* | ✅ |
| tools.aurora.superadmin.revokeRole | procedure | Revoke admin role from a user (Phase 3.6) | AdminModeration + SuperAdmin role | Admin/moderation Phase 3.6: Relocate role management to tools.aurora.superadmin.* | ✅ |
| tools.aurora.admin.getQueueStats | query | Pending appeals + open reports counts; latency percentiles (Phase 3.7) | AdminModeration (Moderator+) | Admin/moderation Phase 3.7A: getQueueStats + getModerationMetrics | ✅ |
| tools.aurora.admin.getModerationMetrics | query | Aggregate metrics: events_total, events_by_type, appeals_by_resolution, takedowns_applied, top_moderators (Phase 3.7) | AdminModeration (Moderator+) | Admin/moderation Phase 3.7A: getQueueStats + getModerationMetrics | ✅ |
| tools.aurora.admin.getAuditTrail | query | Paginated audit_chain_entry rows with per-row `verified` and chain-level `chainVerified`/`chainVerifiedThrough` (Phase 3.8) | AdminModeration (Moderator+) | Admin/moderation Phase 3.8A: Audit chain + snapshot infrastructure + getAuditTrail | ✅ |
| tools.aurora.admin.exportAccountForensic | procedure | Tamper-evident metadata bundle; bundle hash recorded in chain (Phase 3.8). v0.2 ships metadata-only — see UI design §8.7. | AdminServer (Admin+ role; SuperAdmin gates `includeAccountMetadata`/`includeAuditChain`) | Admin/moderation Phase 3.8C: tools.aurora.admin.exportAccountForensic | ✅ |
| tools.aurora.admin.subscribeModEvents | subscription (WebSocket) | Live event tail; reads from retention-bounded `mod_event_seq`; Hello/Event/AuditEntry/Heartbeat/OutdatedCursor/Error frames (Phase 3.9) | AdminModeration (Moderator+) | Admin/moderation Phase 3.9: subscribeModEvents + subscription substrate | ✅ |
| tools.aurora.admin.getRuntimeSetting | query | Read a runtime configuration setting (Phase 3.10) | AdminServer (Admin+ except `moderation-mode` which is Moderator+) | Admin/moderation Phase 3.10: Runtime settings + UI & modes settings page | ✅ |
| tools.aurora.admin.setRuntimeSetting | procedure | Set a runtime configuration setting; validates known keys (Phase 3.10) | AdminServer (Admin+ except `moderation-mode` which is Moderator+) | Admin/moderation Phase 3.10: Runtime settings + UI & modes settings page | ✅ |

---

## tools.aurora.ops.*

Aurora operator surface — relocated from legacy
`com.atproto.admin.*` operator endpoints during Phase 2.3, plus 2 new
endpoints (listAccounts with broader filters, getInstanceMetrics).
**Actual count: 32 — matches brief expectation exactly.**

**Auth:** `AdminServer` (operator / infrastructure tier).

| NSID | Type | Description | Auth | Last commit | Shipped |
|---|---|---|---|---|---|
| tools.aurora.ops.getStats | query | Server statistics | AdminServer | Phase 2.4: Remove legacy operator endpoints | ✅ |
| tools.aurora.ops.listAccounts | query | Operator-flavored account listing (broader filters than com.atproto.admin.searchAccounts) | AdminServer | Phase 2.4: Remove legacy operator endpoints | ✅ |
| tools.aurora.ops.getInstanceMetrics | query | Instance metrics (counts, percentiles); zero-counts return None rather than zero-fill | AdminServer | Phase 2.4: Remove legacy operator endpoints | ✅ |
| tools.aurora.ops.getValidationFailures | query | Recent validation failures across the instance | AdminServer | Phase 2.3.6: Relocate health/metrics ops | ✅ |
| tools.aurora.ops.getSystemHealth | query | Overall system health status | AdminServer | Phase 2.3.6: Relocate health/metrics ops | ✅ |
| tools.aurora.ops.getDatabaseStatus | query | Database connection pool status | AdminServer | Phase 2.3.6: Relocate health/metrics ops | ✅ |
| tools.aurora.ops.getResourceUsage | query | Resource usage metrics (CPU, memory) | AdminServer | Phase 2.3.6: Relocate health/metrics ops | ✅ |
| tools.aurora.ops.listBackgroundJobs | query | Background jobs status | AdminServer | Phase 2.3.6: Relocate health/metrics ops | ✅ |
| tools.aurora.ops.runHealthChecks | query | Comprehensive health checks | AdminServer | Phase 2.3.6: Relocate health/metrics ops | ✅ |
| tools.aurora.ops.getVersionInfo | query | Version and build information | AdminServer | Phase 2.3.6: Relocate health/metrics ops | ✅ |
| tools.aurora.ops.getSystemMetrics | query | Comprehensive system metrics | AdminServer | Phase 2.3.6: Relocate health/metrics ops | ✅ |
| tools.aurora.ops.getNonceStoreStatus | query | Service-auth + DPoP nonce store statistics | AdminServer | Phase 2.4: Remove legacy operator endpoints | ✅ |
| tools.aurora.ops.cleanupNonceStores | procedure | Trigger expired-nonce cleanup (normally automatic) | AdminServer | Phase 2.4: Remove legacy operator endpoints | ✅ |
| tools.aurora.ops.getBlobStatistics | query | Blob storage statistics | AdminServer | Phase 2.3.2: Relocate blob ops | ✅ |
| tools.aurora.ops.listBlobs | query | List blobs with optional filtering | AdminServer | Phase 2.3.2: Relocate blob ops | ✅ |
| tools.aurora.ops.deleteBlob | procedure | Delete a specific blob | AdminServer | Phase 2.3.2: Relocate blob ops | ✅ |
| tools.aurora.ops.quarantineBlob | procedure | Quarantine a blob (mark as taken down) | AdminServer | Phase 2.3.2: Relocate blob ops | ✅ |
| tools.aurora.ops.restoreBlob | procedure | Restore a quarantined blob | AdminServer | Phase 2.3.2: Relocate blob ops | ✅ |
| tools.aurora.ops.runBlobGC | procedure | Run blob garbage collection | AdminServer | Phase 2.3.2: Relocate blob ops | ✅ |
| tools.aurora.ops.getBlobQuotas | query | Per-account blob quotas | AdminServer | Phase 2.3.2: Relocate blob ops | ✅ |
| tools.aurora.ops.getSequencerStatus | query | Sequencer status and statistics | AdminServer | Phase 2.3.3: Relocate sequencer ops | ✅ |
| tools.aurora.ops.pauseSequencer | procedure | Pause sequencer event streaming | AdminServer | Phase 2.3.3: Relocate sequencer ops | ✅ |
| tools.aurora.ops.resumeSequencer | procedure | Resume sequencer event streaming | AdminServer | Phase 2.3.3: Relocate sequencer ops | ✅ |
| tools.aurora.ops.resetSequencerCursor | procedure | Reset sequencer cursor position | AdminServer | Phase 2.3.3: Relocate sequencer ops | ✅ |
| tools.aurora.ops.rebuildSequencer | procedure | Rebuild or verify sequencer integrity | AdminServer | Phase 2.3.3: Relocate sequencer ops | ✅ |
| tools.aurora.ops.getRateLimitConfig | query | Per-type rate-limit config + custom-limit endpoints | AdminServer | Phase 2.3.5: Relocate rate-limit ops | ✅ |
| tools.aurora.ops.getRateLimitStatus | query | Current request counts and tracked identifiers | AdminServer | Phase 2.3.5: Relocate rate-limit ops | ✅ |
| tools.aurora.ops.cleanupRateLimitState | procedure | Manual rate-limit-state cleanup trigger | AdminServer | Phase 2.3.5: Relocate rate-limit ops | ✅ |
| tools.aurora.ops.getFederationStatus | query | Federation configuration and connection status | AdminServer | Phase 2.3.4: Relocate federation ops | ✅ |
| tools.aurora.ops.getRelayConfig | query | Relay client configuration and server list | AdminServer | Phase 2.3.4: Relocate federation ops | ✅ |
| tools.aurora.ops.listKnownInstances | query | All PDS instances discovered through relay servers | AdminServer | Phase 2.3.4: Relocate federation ops | ✅ |
| tools.aurora.ops.triggerPdsDiscovery | procedure | Initiate PDS-instance discovery from configured relays | AdminServer | Phase 2.3.4: Relocate federation ops | ✅ |

---

## com.atproto.repo.* (record-write surface — error contracts)

Scope: the four record-write endpoints whose wire-error contracts
were updated by Arc 16e §9.5.4 Step 3.8. Per the lexicons-as-Rust-
types convention noted at the top of this file, the full request /
response shapes live in the handler signatures at
[src/api/repo.rs](../src/api/repo.rs); the table below enumerates
the wire-error codes each handler can emit so the contract is
discoverable from the endpoint surface without grep-tracing
[`PdsError`](../src/error.rs)'s `IntoResponse` mapping.

The Arc 16e additions on the write paths are `InvalidCid` (400 —
validate-phase walker rejection) and `BlobNotFound` (400 — Phase B
STRICT-missing-row); both are wire-pinned per V05_DESIGN.md
§9.5.3.5 R0c.A to match bsky-PDS verbatim.

Other `com.atproto.repo.*` endpoints (`getRecord`, `listRecords`,
`describeRepo`, `listMissingBlobs`) are read-side and not in
Arc 16e's scope; they can be added incrementally to this section
as their error contracts get audited.

| NSID | Type | Auth scope | Wire-error codes (HTTP status) |
|---|---|---|---|
| com.atproto.repo.createRecord | procedure | `RepoCreate` | `AuthRequired` (401), `InsufficientScope` / `Forbidden` (403), `RateLimitExceeded` (429), `InvalidCid` (400), `BlobNotFound` (400), `Validation` (400), `Database` / `Internal` (500) |
| com.atproto.repo.putRecord | procedure | `RepoUpdate` | createRecord's set + `NotFound` (404, swap-CID against missing record) |
| com.atproto.repo.deleteRecord | procedure | `RepoDelete` | `AuthRequired` (401), `InsufficientScope` / `Forbidden` (403), `RateLimitExceeded` (429), `Validation` (400, swap-CID mismatch), `NotFound` (404), `Database` / `Internal` (500). Phase B's `unreference_blob` six-variant `UnreferenceOutcome` is log-and-continue, so no Arc 16e-introduced error surfaces on this path. |
| com.atproto.repo.applyWrites | procedure | `RepoAll` | putRecord's set + `Validation` (400) covers batch size limit (>200 ops) and duplicate-op detection. A malformed CID anywhere in the batch aborts the whole batch before Phase A opens — partial state mutation is structurally impossible. |

---

## Discrepancies

1. **com.atproto.admin.* count higher than the upstream baseline**
   (34 actual vs the ~15 of bsky-PDS-2025-Q1). Aurora's parity floor
   adds invite admin, role + audit, label, report, and sequencer-
   event surfaces. The full surface is intentional; the count
   reflects accumulated parity work rather than scope drift.

2. **`com.atproto.admin.listAccounts` is an alias to `getUsers`**
   ([src/api/admin.rs:54](../src/api/admin.rs#L54)). Both routes wire
   to the same handler. The operator-flavored `listAccounts` (broader
   filters) is at `tools.aurora.ops.listAccounts` instead. UI should
   not display both — pick one or treat them as a single endpoint
   with two URLs.

3. **`com.atproto.admin.listRoles` kept at moderation tier
   intentionally** ([src/api/admin.rs:107-111](../src/api/admin.rs#L107-L111)).
   Phase 3.6 relocated `grantRole` and `revokeRole` to
   `tools.aurora.superadmin.*` but explicitly left `listRoles` at
   `com.atproto.admin.*` so Moderators can see who has what role
   without holding SuperAdmin themselves. UI should mirror this
   asymmetry — read at one tier, write at another.

4. **`com.atproto.admin.getAuditLog` vs `tools.aurora.admin.getAuditTrail`.**
   Two audit surfaces ship: the parity-floor `getAuditLog` (now
   reading from `audit_chain_entry` per cycle's audit-chain
   migration; legacy wire shape preserved for back-compat) and the
   Phase 3.8 `getAuditTrail` (rich-context, hash-chain-aware,
   `verified`/`chainVerified` flags). The UI design merges both into
   a unified audit page; see [AURORA_ADMIN_UI_DESIGN.md §5.4.5](AURORA_ADMIN_UI_DESIGN.md).

5. **`com.atproto.admin.{getModerationHistory, getModerationQueue,
   listReports, listRecentEvents}` overlap with Phase 3 reads.**
   Several parity-floor endpoints overlap conceptually with the
   moderator-tier reads (e.g., `getModerationHistory` overlaps with
   `tools.aurora.moderator.getSubjectHistory`;
   `getModerationQueue` overlaps with
   `tools.aurora.moderator.queryStatuses`). The Phase 3 endpoints
   carry richer context (resolved handles, paginated, batched
   metadata), but both surfaces are live. UI should pick one per
   workflow rather than expose duplicates.

6. **Lexicon convention.** Aurora-Locus has no JSON lexicon files.
   Per CLAUDE.md "Rust-types-as-lexicon convention" — handlers + their
   serde structs are the source of truth. Captured above as the
   lexicon convention note.

7. **"Last commit" entries are mid-cycle snapshots.** The "Last
   commit" column in tables above reflects the introducing or last-
   touching phase. For routes that pre-date the cycle, the
   introducing commit was "initial commit" or pre-Phase-1 work; for
   routes touched by the proto-blue 0.2.6 SDK refactor (`c2d6fd2`),
   that mass refactor shows up because it modified most handler
   signatures. The values are useful for UI designers tracking
   surface evolution but don't reflect every audit-pass amendment.

---

## Notes for UI design pass

### Natural endpoint clusters → UI surfaces

A reasonable first-cut UI grouping based on the inventory:

- **Account browser** → `getUsers / listAccounts / searchAccounts +
  getAccount + getAccountInfo + getAccountInfos` plus the operator-
  flavored `tools.aurora.ops.listAccounts`. Six read endpoints, one
  search box, paginated list, click-through to detail view.
- **Account-mgmt drawer** (within account detail) →
  `updateAccountEmail / updateAccountHandle / updateAccountPassword
  / updateAccountSigningKey / deleteAccount` plus
  `enableAccountInvites / disableAccountInvites`. Seven procedures
  on one subject; collapsible action panel.
- **Account moderation drawer** (within account detail) →
  `takedownAccount / suspendAccount / restoreAccount +
  updateSubjectStatus + getSubjectStatus + getModerationHistory`.
  Six endpoints; the existing UI under `static/admin/` already has
  partial coverage but treats them piecemeal.
- **Reports queue** → `listReports + submitReport +
  updateReportStatus + getModerationQueue`. Four endpoints; queue
  + detail view + status-update action. UI partially exists.
- **Mod Events page** (already shipped from Phase 3.3) →
  `tools.aurora.moderator.{queryEvents, getEvent}`. Filter bar +
  paginated table + detail modal.
- **Appeals page** (already shipped from Phase 3.4) →
  `tools.aurora.moderator.{listAppeals, getAppeal}`. Same pattern
  as Mod Events.
- **Subject-context deep-dive** (deferred per Phase 3.4 close) →
  `tools.aurora.moderator.{getSubjectContext, getSubjectHistory,
  queryStatuses}`. One-page comprehensive view per DID.
- **Invite admin** → `createInviteCode / getInviteCodes /
  listInviteCodes / disableInviteCode / disableInviteCodes`. Five
  endpoints; UI exists.
- **Label admin** → `applyLabel / removeLabel`. Two procedures;
  needs a target-picker UI.
- **Audit** → `getAuditLog + listRecentEvents` (parity floor) plus
  Phase 3.8's `getAuditTrail` (when shipped). Reconcile per
  Discrepancy #5.
- **Email** → `sendEmail`. Single procedure; ad-hoc compose form.
- **Roles** → `listRoles` (read, Moderator+) plus
  `tools.aurora.superadmin.{grantRole, revokeRole}` (write,
  SuperAdmin only). Asymmetric tier per Phase 3.6 — UI should hide
  the write actions from non-SuperAdmin sessions.
- **Settings / capabilities** → `tools.aurora.describeCapabilities`.
  Already wired into the existing Settings page.
- **Operator dashboard** → all 32 `tools.aurora.ops.*` endpoints.
  Big surface; obvious sub-clusters:
  - System health (getSystemHealth, runHealthChecks, getResourceUsage,
    getDatabaseStatus, getVersionInfo, getSystemMetrics) → 6
  - Sequencer (getSequencerStatus, pause, resume, reset, rebuild) → 5
  - Blob ops (getBlobStatistics, listBlobs, deleteBlob,
    quarantineBlob, restoreBlob, runBlobGC, getBlobQuotas) → 7
  - Federation (getFederationStatus, getRelayConfig,
    listKnownInstances, triggerPdsDiscovery) → 4
  - Rate-limit (getRateLimitConfig, getRateLimitStatus,
    cleanupRateLimitState) → 3
  - Validation + jobs + nonce (getValidationFailures,
    listBackgroundJobs, getNonceStoreStatus, cleanupNonceStores) → 4
  - Stats / accounts / metrics (getStats, listAccounts,
    getInstanceMetrics) → 3

### Pagination patterns

- **Cursor-based (Phase 3 standard, opaque base64 with composite
  created_at+id):** all `tools.aurora.moderator.*` query endpoints
  (queryEvents, queryStatuses, getSubjectHistory, listAppeals).
- **Cursor-based (legacy, trailing-DID):** com.atproto.admin's
  `searchAccounts`, `getAccountInfos`. Different cursor shape.
- **Limit-only (no cursor):** `getInviteCodes`, `listInviteCodes`
  (Phase 1.10 wired the lexicon's params but the cursor scheme
  may differ from Phase 3's).
- **Bounded (not paginated):** `getSubjectContext` returns 50-item
  bounded categories; `listRecentEvents` is bounded.
- **Unbounded / single-result:** all `getX`-by-id endpoints
  (`getEvent`, `getAppeal`, `getAccount`, `getAccountInfo`,
  `getSubjectStatus`).

UI should reuse Phase 3's cursor-based table component for the
moderator endpoints and not assume the legacy DID-cursor pattern
generalizes.

### Confusing names worth flagging

- **`com.atproto.admin.listAccounts` vs `tools.aurora.ops.listAccounts`**
  — same name, different filter sets (parity vs operator).
- **`com.atproto.admin.getModerationHistory` vs
  `tools.aurora.moderator.getSubjectHistory`** — overlap; latter is
  Aurora's enriched version.
- **`com.atproto.admin.getModerationQueue` vs
  `tools.aurora.moderator.queryStatuses`** — partially overlapping
  semantics.
- **`com.atproto.admin.listRecentEvents` (sequencer events) vs
  `tools.aurora.moderator.queryEvents` (moderation events)** —
  similarly named, different domains.
- **`com.atproto.admin.getAuditLog` vs Phase 3.8's
  `getAuditTrail`** (forthcoming) — two audit surfaces.

### Methodological note for the UI design pass

The "Phase that introduced or last modified it" data in the tables
above is `git blame` truth — many cells point to the proto-blue 0.2.6
refactor, which was a mass touch but not the introducing change. If
the UI design pass needs introducing-commit attribution per endpoint,
re-run with `git log --follow --diff-filter=A -- src/api/admin.rs`
filtered to handler-introducing changes; this inventory uses last-
touch since the brief asked for "most recent commit subject."
