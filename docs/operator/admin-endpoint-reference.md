# Admin & Moderation Endpoint Reference

**Scope:** Aurora-Locus's admin / moderation / ops endpoint surface —
the `tools.aurora.{describeCapabilities, moderator, admin, superadmin,
ops}.*` namespaces plus the parity-floor `com.atproto.admin.*` namespace
they extend and partially alias. The §Discrepancies section reconciles
the alias relationships, tier asymmetries, and overlapping reads
between the two namespaces.

**Not covered here:** the broader `com.atproto.server.*` (account /
session / auth), `com.atproto.sync.*` (firehose, repo export),
`com.atproto.identity.*`, OAuth endpoints (`/oauth/*`), `/health`, and
`/.well-known/*` surfaces. Those follow the upstream ATProto spec and
are not enumerated here.

**As of:** 2026-05-03. Tables below capture the as-shipped surface at
that snapshot; spot-checks against current source remain accurate for
the endpoints inventoried, but an exhaustive re-audit covering any
additions made since this snapshot (e.g. the `tools.aurora.lexicon.*`
namespace) is future work. Source-link line numbers in this doc are
similarly point-in-time; follow the file references and search for the
named symbols if the exact line has shifted.

**Lexicon convention:** Aurora-Locus does **not** ship JSON lexicon
files. Per CLAUDE.md, lexicons are defined as Rust types — request /
response shapes live in handler signatures and adjacent serde structs.
The route table lives in [src/api/admin.rs](../../src/api/admin.rs)
plus handler modules under [src/api/](../../src/api/). NSID
descriptions below come from the leading `///` doc comment on each
handler (where present); empty cells indicate the handler has no
explicit one-liner — derive from the NSID or the section comment block.

**Auth scope source:** [src/oauth/scope.rs](../../src/oauth/scope.rs).
Mapping is uniform per namespace prefix — namespace-level scope
enforcement happens in `namespace_scope_check` middleware before the
handler runs. Within-tier role checks (Moderator vs Admin vs
SuperAdmin) happen at the handler level via
`AdminAuthContext::role.can_act_as(...)`.

**Handler-shipped column:** every NSID in the route table has a wired
handler (no stub-only routes — `unimplemented!()` and `todo!()` are
banned by CLAUDE.md). The column captures `✅ shipped` everywhere
under the snapshot build.

---

## com.atproto.admin.*

Parity floor with bsky-PDS plus Aurora-specific extensions. **Actual
count: 34 routes (33 unique handlers; `listAccounts` is an alias to
`getUsers`).** The bsky-PDS upstream baseline is ~15; Aurora's surface
adds invite admin, role/audit, label, report, and sequencer-event
endpoints atop that.

**Auth:** `AdminServer` OR `AdminModeration` (either accepted; some
endpoints are operator-flavored, some moderation-flavored, and
upstream's design didn't draw a lexicon-level distinction).

| NSID | Type | Description | Auth | Shipped |
|---|---|---|---|---|
| com.atproto.admin.getUsers | query | Get list of users | AdminServer\|AdminModeration | ✅ |
| com.atproto.admin.listAccounts | query | bsky-PDS-compat alias to getUsers (operator-flavored listing lives at tools.aurora.ops.listAccounts) | AdminServer\|AdminModeration | ✅ |
| com.atproto.admin.getAccount | query | Get single account details | AdminServer\|AdminModeration | ✅ |
| com.atproto.admin.searchAccounts | query | Cursor-paginated account search; shared `accountView` shape | AdminServer\|AdminModeration | ✅ |
| com.atproto.admin.getAccountInfo | query | Single-account info via `build_account_info` helper | AdminServer\|AdminModeration | ✅ |
| com.atproto.admin.getAccountInfos | query | Batched account info; uses axum_extra Query for repeated keys | AdminServer\|AdminModeration | ✅ |
| com.atproto.admin.updateSubjectStatus | procedure | Polymorphic subject-status update (Repo / Record / Blob) | AdminServer\|AdminModeration | ✅ |
| com.atproto.admin.getSubjectStatus | query | Current moderation status of a subject (takedown / deactivation / suspension) | AdminServer\|AdminModeration | ✅ |
| com.atproto.admin.createInviteCode | procedure | Create an invite code | AdminServer\|AdminModeration | ✅ |
| com.atproto.admin.getInviteCodes | query | List invite codes (lexicon's sort/limit/cursor; legacy `includeDisabled` removed) | AdminServer\|AdminModeration | ✅ |
| com.atproto.admin.listInviteCodes | query | Aurora-Locus surface paralleling getInviteCodes; shares pagination machinery | AdminServer\|AdminModeration | ✅ |
| com.atproto.admin.disableInviteCode | procedure | Disable an invite code | AdminServer\|AdminModeration | ✅ |
| com.atproto.admin.disableInviteCodes | procedure | Bulk-disable; transactional all-or-nothing | AdminServer\|AdminModeration | ✅ |
| com.atproto.admin.enableAccountInvites | procedure | Enable invite-code creation for an account | AdminServer\|AdminModeration | ✅ |
| com.atproto.admin.disableAccountInvites | procedure | Disable invite-code creation for an account | AdminServer\|AdminModeration | ✅ |
| com.atproto.admin.listRoles | query | List admin roles (kept here intentionally — Moderators may need role visibility without SuperAdmin) | AdminServer\|AdminModeration | ✅ |
| com.atproto.admin.updateAccountEmail | procedure | Update account email address | AdminServer\|AdminModeration | ✅ |
| com.atproto.admin.updateAccountHandle | procedure | Update account handle | AdminServer\|AdminModeration | ✅ |
| com.atproto.admin.updateAccountPassword | procedure | Update account password (admin override) | AdminServer\|AdminModeration | ✅ |
| com.atproto.admin.deleteAccount | procedure | Delete account permanently (admin operation) | AdminServer\|AdminModeration | ✅ |
| com.atproto.admin.updateAccountSigningKey | procedure | Update account signing key (Aurora-architecture safety constraint in strict mode) | AdminServer\|AdminModeration | ✅ |
| com.atproto.admin.takedownAccount | procedure | Takedown an account (remove from public view) | AdminServer\|AdminModeration | ✅ |
| com.atproto.admin.suspendAccount | procedure | Suspend an account temporarily | AdminServer\|AdminModeration | ✅ |
| com.atproto.admin.restoreAccount | procedure | Restore an account after takedown / suspension | AdminServer\|AdminModeration | ✅ |
| com.atproto.admin.getModerationHistory | query | Moderation history for an account | AdminServer\|AdminModeration | ✅ |
| com.atproto.admin.getModerationQueue | query | Reports needing review | AdminServer\|AdminModeration | ✅ |
| com.atproto.admin.applyLabel | procedure | Apply a label to content | AdminServer\|AdminModeration | ✅ |
| com.atproto.admin.removeLabel | procedure | Remove a label from content | AdminServer\|AdminModeration | ✅ |
| com.atproto.admin.submitReport | procedure | Submit a report | AdminServer\|AdminModeration | ✅ |
| com.atproto.admin.updateReportStatus | procedure | Update report status | AdminServer\|AdminModeration | ✅ |
| com.atproto.admin.listReports | query | List reports | AdminServer\|AdminModeration | ✅ |
| com.atproto.admin.sendEmail | procedure | Send admin email (warnings, notifications, etc.) | AdminServer\|AdminModeration | ✅ |
| com.atproto.admin.getAuditLog | query | Filterable audit log (admin DID / action type / subject DID) | AdminServer\|AdminModeration | ✅ |
| com.atproto.admin.listRecentEvents | query | Sequencer event review (moderation-flavored; ops controls live at tools.aurora.ops.{getSequencerStatus, pauseSequencer, ...}) | AdminServer\|AdminModeration | ✅ |

---

## tools.aurora.{describeCapabilities, moderator, admin, superadmin}.*

Aurora moderation/admin extension surface — the federated
moderation pairing target. **Count: 25 (1 top-level + 7 moderator + 2
superadmin + 15 admin).**

**Auth:** `AdminModeration` (namespace-level). Within-tier checks
(Moderator vs Admin vs SuperAdmin) happen at the handler via
`AdminAuthContext::role.can_act_as(...)`. The four
`tools.aurora.admin.*` operator-flavored endpoints
(`triggerPasswordReset`, `exportAccountForensic`, `getRuntimeSetting`,
`setRuntimeSetting`) require `AdminServer` scope. The override is a per-NSID lookup that runs before
the namespace prefix match, **replacing** (not augmenting) the
namespace default — `AdminModeration` alone is insufficient. See
[src/oauth/scope.rs](../../src/oauth/scope.rs) for the operator-NSID
table; the auth-tier model behind the override is documented in
[admin-auth.md](admin-auth.md).

| NSID | Type | Description | Auth (within-tier) | Shipped |
|---|---|---|---|---|
| tools.aurora.describeCapabilities | query | Capability probe — clients discover which Aurora extensions this instance supports without trial-and-error | AdminModeration (any role) | ✅ |
| tools.aurora.moderator.queryEvents | query | Paginated query of moderation events with rich-context handle resolution | AdminModeration (Moderator+) | ✅ |
| tools.aurora.moderator.getEvent | query | Single moderation event by ID with resolved actor / subject handles | AdminModeration (Moderator+) | ✅ |
| tools.aurora.moderator.queryStatuses | query | Paginated query of subject statuses; subject_type=Record\|Blob short-circuits to empty pending per-record/per-blob status surfaces | AdminModeration (Moderator+) | ✅ |
| tools.aurora.moderator.getSubjectContext | query | Comprehensive view of a subject — actor row + recent actions + recent reports + recent appeals | AdminModeration (Moderator+) | ✅ |
| tools.aurora.moderator.getSubjectHistory | query | Chronological action history for one subject; sortable asc/desc, action-filterable | AdminModeration (Moderator+) | ✅ |
| tools.aurora.moderator.listAppeals | query | Paginated appeals query with status / appellant / reviewer / date-range filters; embeds AppealView shape | AdminModeration (Moderator+) | ✅ |
| tools.aurora.moderator.getAppeal | query | Single appeal by ID with full timeline (lifecycle entries) and original-action summary | AdminModeration (Moderator+) | ✅ |
| tools.aurora.admin.emitEvent | procedure | Unified action surface — ModEvent + Subject → moderation_event + audit_chain_entry in one tx | AdminModeration (Admin+) | ✅ |
| tools.aurora.admin.batchTakedownAccounts | procedure | Multi-DID account takedown; one chain entry, per-subject failures | AdminModeration (Moderator+) | ✅ |
| tools.aurora.admin.batchSuspendAccounts | procedure | Multi-DID account suspension | AdminModeration (Moderator+) | ✅ |
| tools.aurora.admin.batchRestoreAccounts | procedure | Multi-DID account restore; per-DID `UPDATE actor SET takedown_ref = NULL` failures land in `failures` | AdminModeration (Moderator+) | ✅ |
| tools.aurora.admin.batchTakedownRecords | procedure | Multi-record takedown via `!takedown` self-label | AdminModeration (Moderator+) | ✅ |
| tools.aurora.admin.batchApplyLabel | procedure | Multi-subject label application | AdminModeration (Moderator+) | ✅ |
| tools.aurora.admin.batchRemoveLabel | procedure | Multi-subject label removal; subjects without the label land in `skipped` | AdminModeration (Moderator+) | ✅ |
| tools.aurora.admin.triggerPasswordReset | procedure | Trigger password-reset flow for an account; rationale-required | AdminServer (Admin+ role) | ✅ |
| tools.aurora.superadmin.grantRole | procedure | Grant admin role to a user (relocated from com.atproto.admin) | AdminModeration + SuperAdmin role | ✅ |
| tools.aurora.superadmin.revokeRole | procedure | Revoke admin role from a user | AdminModeration + SuperAdmin role | ✅ |
| tools.aurora.admin.getQueueStats | query | Pending appeals + open reports counts; latency percentiles | AdminModeration (Moderator+) | ✅ |
| tools.aurora.admin.getModerationMetrics | query | Aggregate metrics: events_total, events_by_type, appeals_by_resolution, takedowns_applied, top_moderators | AdminModeration (Moderator+) | ✅ |
| tools.aurora.admin.getAuditTrail | query | Paginated audit_chain_entry rows with per-row `verified` and chain-level `chainVerified`/`chainVerifiedThrough` | AdminModeration (Moderator+) | ✅ |
| tools.aurora.admin.exportAccountForensic | procedure | Tamper-evident metadata bundle; bundle hash recorded in chain. | AdminServer (Admin+ role; SuperAdmin gates `includeAccountMetadata`/`includeAuditChain`) | ✅ |
| tools.aurora.admin.subscribeModEvents | subscription (WebSocket) | Live event tail; reads from retention-bounded `mod_event_seq`; Hello/Event/AuditEntry/Heartbeat/OutdatedCursor/Error frames | AdminModeration (Moderator+) | ✅ |
| tools.aurora.admin.getRuntimeSetting | query | Read a runtime configuration setting | AdminServer (Admin+ except `moderation-mode` which is Moderator+) | ✅ |
| tools.aurora.admin.setRuntimeSetting | procedure | Set a runtime configuration setting; validates known keys | AdminServer (Admin+ except `moderation-mode` which is Moderator+) | ✅ |

---

## tools.aurora.ops.*

Aurora operator surface — relocated from legacy
`com.atproto.admin.*` operator endpoints, plus 2 endpoints unique to
Aurora (listAccounts with broader filters, getInstanceMetrics).
**Actual count: 32.**

**Auth:** `AdminServer` (operator / infrastructure tier).

| NSID | Type | Description | Auth | Shipped |
|---|---|---|---|---|
| tools.aurora.ops.getStats | query | Server statistics | AdminServer | ✅ |
| tools.aurora.ops.listAccounts | query | Operator-flavored account listing (broader filters than com.atproto.admin.searchAccounts) | AdminServer | ✅ |
| tools.aurora.ops.getInstanceMetrics | query | Instance metrics (counts, percentiles); zero-counts return None rather than zero-fill | AdminServer | ✅ |
| tools.aurora.ops.getValidationFailures | query | Recent validation failures across the instance | AdminServer | ✅ |
| tools.aurora.ops.getSystemHealth | query | Overall system health status | AdminServer | ✅ |
| tools.aurora.ops.getDatabaseStatus | query | Database connection pool status | AdminServer | ✅ |
| tools.aurora.ops.getResourceUsage | query | Resource usage metrics (CPU, memory) | AdminServer | ✅ |
| tools.aurora.ops.listBackgroundJobs | query | Background jobs status | AdminServer | ✅ |
| tools.aurora.ops.runHealthChecks | query | Comprehensive health checks | AdminServer | ✅ |
| tools.aurora.ops.getVersionInfo | query | Version and build information | AdminServer | ✅ |
| tools.aurora.ops.getSystemMetrics | query | Comprehensive system metrics | AdminServer | ✅ |
| tools.aurora.ops.getNonceStoreStatus | query | Service-auth + DPoP nonce store statistics | AdminServer | ✅ |
| tools.aurora.ops.cleanupNonceStores | procedure | Trigger expired-nonce cleanup (normally automatic) | AdminServer | ✅ |
| tools.aurora.ops.getBlobStatistics | query | Blob storage statistics | AdminServer | ✅ |
| tools.aurora.ops.listBlobs | query | List blobs with optional filtering | AdminServer | ✅ |
| tools.aurora.ops.deleteBlob | procedure | Delete a specific blob | AdminServer | ✅ |
| tools.aurora.ops.quarantineBlob | procedure | Quarantine a blob (mark as taken down) | AdminServer | ✅ |
| tools.aurora.ops.restoreBlob | procedure | Restore a quarantined blob | AdminServer | ✅ |
| tools.aurora.ops.runBlobGC | procedure | Run blob garbage collection | AdminServer | ✅ |
| tools.aurora.ops.getBlobQuotas | query | Per-account blob quotas | AdminServer | ✅ |
| tools.aurora.ops.getSequencerStatus | query | Sequencer status and statistics | AdminServer | ✅ |
| tools.aurora.ops.pauseSequencer | procedure | Pause sequencer event streaming | AdminServer | ✅ |
| tools.aurora.ops.resumeSequencer | procedure | Resume sequencer event streaming | AdminServer | ✅ |
| tools.aurora.ops.resetSequencerCursor | procedure | Reset sequencer cursor position | AdminServer | ✅ |
| tools.aurora.ops.rebuildSequencer | procedure | Rebuild or verify sequencer integrity | AdminServer | ✅ |
| tools.aurora.ops.getRateLimitConfig | query | Per-type rate-limit config + custom-limit endpoints | AdminServer | ✅ |
| tools.aurora.ops.getRateLimitStatus | query | Current request counts and tracked identifiers | AdminServer | ✅ |
| tools.aurora.ops.cleanupRateLimitState | procedure | Manual rate-limit-state cleanup trigger | AdminServer | ✅ |
| tools.aurora.ops.getFederationStatus | query | Federation configuration and connection status | AdminServer | ✅ |
| tools.aurora.ops.getRelayConfig | query | Relay client configuration and server list | AdminServer | ✅ |
| tools.aurora.ops.listKnownInstances | query | All PDS instances discovered through relay servers | AdminServer | ✅ |
| tools.aurora.ops.triggerPdsDiscovery | procedure | Initiate PDS-instance discovery from configured relays | AdminServer | ✅ |

---

## com.atproto.repo.* (record-write surface — error contracts)

Scope: the four record-write endpoints whose wire-error contracts are
documented here. Per the lexicons-as-Rust-types convention noted at the
top of this file, the full request / response shapes live in the
handler signatures at [src/api/repo.rs](../../src/api/repo.rs); the
table below enumerates the wire-error codes each handler can emit so
the contract is discoverable from the endpoint surface without
grep-tracing [`PdsError`](../../src/error.rs)'s `IntoResponse` mapping.

The write-path additions for `apply_writes` integrity are `InvalidCid`
(400 — validate-phase walker rejection) and `BlobNotFound` (400 —
Phase B STRICT-missing-row); both are wire-pinned to match bsky-PDS
verbatim.

Other `com.atproto.repo.*` endpoints (`getRecord`, `listRecords`,
`describeRepo`, `listMissingBlobs`) are read-side and not in scope;
they can be added incrementally to this section as their error
contracts get audited.

| NSID | Type | Auth scope | Wire-error codes (HTTP status) |
|---|---|---|---|
| com.atproto.repo.createRecord | procedure | `RepoCreate` | `AuthRequired` (401), `InsufficientScope` / `Forbidden` (403), `RateLimitExceeded` (429), `InvalidCid` (400), `BlobNotFound` (400), `Validation` (400), `Database` / `Internal` (500) |
| com.atproto.repo.putRecord | procedure | `RepoUpdate` | createRecord's set + `NotFound` (404, swap-CID against missing record) |
| com.atproto.repo.deleteRecord | procedure | `RepoDelete` | `AuthRequired` (401), `InsufficientScope` / `Forbidden` (403), `RateLimitExceeded` (429), `Validation` (400, swap-CID mismatch), `NotFound` (404), `Database` / `Internal` (500). Phase B's `unreference_blob` six-variant `UnreferenceOutcome` is log-and-continue, so no new error surfaces on this path. |
| com.atproto.repo.applyWrites | procedure | `RepoAll` | putRecord's set + `Validation` (400) covers batch size limit (>200 ops) and duplicate-op detection. A malformed CID anywhere in the batch aborts the whole batch before Phase A opens — partial state mutation is structurally impossible. |

### Aurora-owned `tools.aurora.repo.*` (importRepo error vocabulary)

Aurora ships lexicons as Rust types, not JSON. The
`tools.aurora.repo.importRepo` namespace exists as a documentation
contract here (and as a rustdoc `# Errors` section on the handler
function) — there is no `lexicons/tools/aurora/repo/importRepo.json`
file. The wire route is registered at
`/xrpc/com.atproto.repo.importRepo` (the standard ATProto NSID); the
`tools.aurora.repo.importRepo` name addresses Aurora's
implementation-specific *error vocabulary* — wire codes that bsky-PDS
does not define and that downstream tooling needs to discriminate.

Handler at [src/api/repo_import.rs](../../src/api/repo_import.rs).

| NSID | Type | Auth scope | Wire-error codes (HTTP status) |
|---|---|---|---|
| tools.aurora.repo.importRepo | procedure | `RepoAll` | `ActorNotInitialized` (400, no `plc_keys` row), `ConcurrentMutation` (409, single-flight lock contended), `InvalidCar` (400, structural CAR failure or root-DID mismatch or `max_import_size` exceeded), `InvalidCommitSignature` (400, `verify_diff_car` sig check failed against per-account key), `InvalidCid` (400, validate-phase walker), `QuarantinedBlobReferenced` (400, with coarse `public_reason` only), `BlobTooLarge` (413, fetched blob exceeds `max_blob_fetch_size`), `OriginFetchClientError` (502, origin 4xx — durable), `OriginFetchExhausted` (502, retry budget exhausted or per-CID failures aggregated), `ServiceUnavailable` (503, `accepting_imports = false`). |

---

## Discrepancies

1. **com.atproto.admin.* count higher than the upstream baseline**
   (34 actual vs the ~15 of bsky-PDS-2025-Q1). Aurora's parity floor
   adds invite admin, role + audit, label, report, and sequencer-
   event surfaces. The full surface is intentional; the count
   reflects accumulated parity work rather than scope drift.

2. **`com.atproto.admin.listAccounts` is an alias to `getUsers`**
   ([src/api/admin.rs:54](../../src/api/admin.rs#L54)). Both routes wire
   to the same handler. The operator-flavored `listAccounts` (broader
   filters) is at `tools.aurora.ops.listAccounts` instead. UI should
   not display both — pick one or treat them as a single endpoint
   with two URLs.

3. **`com.atproto.admin.listRoles` kept at moderation tier
   intentionally** ([src/api/admin.rs:107-111](../../src/api/admin.rs#L107-L111)).
   `grantRole` and `revokeRole` live at `tools.aurora.superadmin.*` but
   `listRoles` was left at `com.atproto.admin.*` so Moderators can see
   who has what role without holding SuperAdmin themselves. UI should
   mirror this asymmetry — read at one tier, write at another.

4. **`com.atproto.admin.getAuditLog` vs `tools.aurora.admin.getAuditTrail`.**
   Two audit surfaces ship: the parity-floor `getAuditLog` (now reading
   from `audit_chain_entry`; legacy wire shape preserved for
   back-compat) and the richer `getAuditTrail` (rich-context,
   hash-chain-aware, `verified`/`chainVerified` flags). Consumers should
   pick one per workflow rather than expose both.

5. **`com.atproto.admin.{getModerationHistory, getModerationQueue,
   listReports, listRecentEvents}` overlap with the moderator-tier
   reads.** Several parity-floor endpoints overlap conceptually with the
   `tools.aurora.moderator.*` reads (e.g., `getModerationHistory`
   overlaps with
   `tools.aurora.moderator.getSubjectHistory`;
   `getModerationQueue` overlaps with
   `tools.aurora.moderator.queryStatuses`). The Aurora extensions
   carry richer context (resolved handles, paginated, batched
   metadata), but both surfaces are live. UI should pick one per
   workflow rather than expose duplicates.

6. **Lexicon convention.** Aurora-Locus has no JSON lexicon files.
   Per CLAUDE.md "Rust-types-as-lexicon convention" — handlers + their
   serde structs are the source of truth. Captured above as the
   lexicon convention note.
