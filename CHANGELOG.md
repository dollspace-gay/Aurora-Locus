# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.9.0] - 2026-06-19

### Added
- Admin UI reorganized into four domains (Moderation, Operations, Configuration, Kryphocron) with role- and mode-based visibility, a role-tiered dashboard, and a reshaped sidebar, breadcrumbs, and routing
- Customizable theming: theme manifests with inheritance, a design-token contract, WCAG 2.2 contrast checking, an effect-class library, and theme selection in the UI settings (a personal-preference picker plus a row list of installed themes)
- Login page now matches the deployment-default theme
- Login splash branding is operator-customizable — logo and banner (direct upload or URL), plus title/subtitle text and text colors, with a live preview
- Ten bundled themes — Dark, Light, Stack Classic, Ember, Emerald, Glacier, Meridian, Pride, and High Contrast Dark/Light — five dark and five light, with Stack Classic as the deployment default
- All ten themes pass WCAG 2.2 AA contrast checks via the substrate's verifier; High Contrast Dark and High Contrast Light additionally clear AAA for text (7:1). Programmatic contrast only — not a full focus-indicator, keyboard, or screen-reader audit
- Theme extension points — themes can declare and provide named extension points that surfaces opt into at runtime
- Themes can declare lifecycle hooks (install, activate, deactivate); the substrate recognizes and lists them but does not execute them yet — script execution waits on a security-reviewed sandbox in a later version
- Theme authoring documentation and a reference example theme
- Kryphocron admin surface: overview, deployment-wide audiences, laquna status with rotation history, and tier-activity pages, plus a per-account drawer, a policy page, and a dashboard summary block
- Per-account kryphocron overrides on the Account Detail page: operators can block a specific account from issuing kryphocron capabilities (and flag a rate-limit exemption), audited with rationale
- New-account access policy: operators can require new accounts to wait a configurable number of days before posting to the private tier (off by default — accounts can post immediately)
- Default audience for new accounts: operators can have each new account start with a chosen kryphocron audience mode, created automatically at signup (off by default — new accounts start with no audience)
- Encryption-at-rest for private-tier posts — encoded on write and transparently decoded for authorized readers — with a standard rotation oracle, automatic re-encoding on key rotation, and operator read endpoints
- Admin authentication: refresh-token flow with rotation on use, and per-operator session management with a Sessions page to view and revoke active sessions
- SuperAdmin can revoke all of an operator's active sessions in one action (for suspected compromise or operator departure), audited with rationale
- Recovery surfaces: recovery-mode status display, single-repository rebuild, bulk repository repair, and sequencer integrity validation — which can now route the accounts it flags with malformed events straight to a per-account rebuild
- Blocking a subject now also removes them from the blocker's audiences, with an audit log of the cascade
- Moderation list pages (Reports, Appeals, Events, Audit) share unified pagination and filtering behavior
- The moderation queue can be filtered by report status (open, acknowledged, escalated, resolved, or all), with the selection preserved in the URL across navigation and reload
- The Kryphocron Overview shows recent audience-oracle consultation activity — how often private-tier writes and reads are checked against audiences and how those checks resolved (aggregate counts only)
- Forensic export now includes the account's full repository (as a CAR file) and uploaded blobs alongside audit events, in one verifiable archive
- A Registration policy page consolidating the deployment's account-registration settings — registration mode, new-account access, and default audience — into one overview, with links to where each is managed
- An Observability page: a read-only overview of the deployment's monitoring surfaces (Prometheus metrics endpoint, system health, database, audit log, substrate metrics) with notes on the env-scoped logging and telemetry configuration
- A Federation policy page where SuperAdmins manage the deployment's federation at runtime — no restart. It also shows the read-only peer-visible posture (exactly what this PDS advertises to peers) and the boot-seed status; server discovery (describeServer) advertises a minimal federation posture, and a federation-scoped describe endpoint exposes richer posture to federation-aware tooling
- Runtime-mutable trusted peer allowlist: add, remove, and modify federation peers (the trusted issuers for cross-PDS auth) without a restart. Every change is audited; trust changes take effect immediately
- Peer discovery modes: choose allowlist-only (discovered peers wait in a review list before you trust them), auto-accept (discovered peers are trusted automatically — the UI warns that this delegates trust to your relays), or discovery-disabled (no scheduled discovery). Mode changes apply forward only; an in-flight scan finishes under the mode it started with
- Pending-discovery review: peers seen during scans are surfaced with accept and dismiss controls. The list is bounded to the 100 most-recently-seen, de-duplicated by DID with a last-seen refresh so a noisy peer can't flush the list
- Runtime-mutable relay set: add or remove individual relays, or replace the whole set, without a restart. The change is recorded first, then the live firehose is re-pointed at the new relays. At least one relay is always required
- Boot-seed safety: if federation policy can't be seeded at startup (for example, federation is enabled but no relays are configured), the deployment surfaces the failure in the audit log, the policy page, and the describe endpoint, and blocks federation-policy changes until you fix the configuration and restart — other operations keep working
- The audit log can be filtered to federation activity (a "Federation management" filter), and federation peers added automatically in auto-accept mode are tagged with a "Discovery" source you can filter on
- All federation-policy changes are blocked while the deployment is in recovery mode, consistent with the rest of the admin surface
- UI building blocks: loading skeletons, spinners, inline errors, error boundaries, consistent timestamps, source-tier indicators, and a save-with-rationale confirmation
- The repository-rebuild deep preflight now reports how many times an account has rotated its signing key (read from the full PLC audit-log history), giving operators forensic visibility into rotated accounts before a rebuild
- The repository-rebuild deep preflight now runs history-aware verification — every commit is checked against the signing key that was valid at its revision per the account's PLC key history — so an operator sees whether a rotated account verifies cleanly across its full history before triggering a rebuild
- Repository rebuild (and bulk repo-repair, which repairs through the same path) now verifies the reconstructed repo history-aware — every commit against the key valid at its revision from PLC history — instead of checking only the head commit against the current key
- A new SuperAdmin dry-run endpoint lets operators validate a signing-key rotation before committing to it — reporting the key the PDS would generate, or checking an operator-supplied keypair (and catching mismatched keys) — without mutating anything or publishing to PLC
- A new SuperAdmin "Key rotation policy" page controls whether operator-supplied keypairs are accepted by the rotation flow (the `key_rotation.operator_supplied_keys_enabled` setting). Off by default; flip it on for HSM-backed or pre-generated rotation paths. Enabling it asks for a confirmation and rationale, and the change is recorded in the audit log
- Account signing-key rotation now mints a fresh per-account key by default (or accepts an operator-supplied keypair when the runtime gate is on), publishes it to PLC, stores it, and signs the rotation's empty commit with the new per-account key. The rotation audit records the generation source (PDS-generated vs operator-supplied) and the old and new public keys. **Breaking:** the endpoint no longer takes a `signingKey` field (the old single-operator-key model is removed) — it takes the account DID, an optional rationale, and an optional operator keypair; the admin Account page's rotation form is updated to match
- The `aurora-cli rotate-keys` command now mints a fresh per-account key per DID by default (bulk rotation still supported), or — for a single DID — accepts an operator-supplied keypair via `--public-key` and `--private-key-hex` when the runtime gate is on, with an optional `--rationale`. CLI rotations now publish, store, sign the empty commit with the new key, and emit an audit entry (previously the CLI rotated without one); the old server-wide signing-key shortcut is removed
- The Key rotation policy page has a "Run migration check" button (backed by a SuperAdmin endpoint) that verifies every account's locally-stored signing key matches what PLC publishes, reporting any divergences for review. Read-only; expected to find none

### Changed
- The deployment moderation tier (full, reduced, disabled) is now set on the Moderation policy page instead of UI & modes; switching to the disabled tier requires a typed confirmation
- Kryphocron is now enabled by default on fresh deployments; operators can disable it by setting PDS_KRYPHOCRON_ENABLED=false
- Operator role changes now take effect on the next request, without requiring re-login
- proto-blue dependency pin updated to 0.3.3 to match the already-resolved version (no API impact)

### Fixed
- Kryphocron Policy settings now save instead of erroring (new-account access, default audience mode, deployment process-shape, and per-account cadence range); the process-shape declaration drives the Overview's single-/multi-process mismatch warning
- Admin UI displayed all operators as moderator regardless of their actual role
- Moderation metrics failed to load on the dashboard
- Configuration fields with no backing store are now shown read-only instead of offering saves that always fail
- Runtime-setting errors now report the specific reason instead of a generic moderation message
- Subject context and history drawers failed to load on the account-detail page
- Report-detail pages returned not-found
- Destructive operator actions (repository rebuild, repair, and manual rotation) are now recorded in the tamper-evident audit chain
- Corrected the laquna rotation-history empty-state copy
- Switching the default theme now repaints colors immediately instead of only after a reload
- The reduced-motion preference now also suppresses smooth scrolling
- Cosmetic settings such as the theme save with a lighter confirmation instead of requiring a typed rationale
- Account search now filters by the search term instead of returning every account
- Restored the installed-themes listing to the server's advertised capabilities
- Hardened intermittent time- and ordering-sensitive test failures

## [0.8.0] - 2026-06-08

### Added
- Persistent forensic record of writes whose downstream commit failed, with automatic reconciliation against actor-store state
- Recovery mode for restoring otherwise-denied private-tier writes during operator recovery — off unless explicitly enabled, with every recovery write recorded as an audit event

### Changed
- Session refresh and logout now read credentials from the standard Authorization header (breaking change — clients sending credentials in the request body must update); logout fully revokes the session, and app-password revocation now also revokes that app password's active sessions and refresh tokens

### Fixed
- Login endpoints now accept DIDs in addition to handles; email addresses containing ':' are no longer accepted
- Rotated refresh tokens are now correctly invalidated, closing a replay path where a rotated token stayed usable
- Account restoration now emits a firehose event, so downstream subscribers no longer remain stuck in a stale takedown state
- applyWrites now accepts both the standard atproto request shape and the existing flat shape, so standard PDS-shaped requests no longer fail
- Bulk session revocation in the OAuth migration tool now also removes paired refresh tokens
- Operators are now warned at startup when orphan recovery or its reconciliation job is disabled, and when a client write produces an unusually large repository commit

## [0.7.0] - 2026-06-02

### Added
- Private-tier posting: dedicated endpoints for creating and deleting private posts, joining a private audience, and managing audiences
- Writes to a private audience are checked against the audience and rejected when the author isn't a member (cross-instance audiences deferred)
- Generic record writes to private-tier collections are redirected to their dedicated endpoints
- Audit-first write ordering — a write's audit record is committed before the write, so the audit trail is never lost if the write fails partway

## [0.6.0] - 2026-05-27

### Added
- Operator-tunable retention window for rate-limit buckets
- Configuration validation now warns about a service-DID form that breaks cross-PDS auth, and about test-only overrides that shouldn't be set in production

### Changed
- Federation is now enabled by default; opt out via configuration
- Service-auth tokens are now signed with the per-account key so receiving servers can verify them
- Fetched lexicons are verified against the publishing authority's signature before being trusted
- Requests authenticated by a tombstoned DID now return a clear 400 error instead of a server error

### Fixed
- Fixed malformed did:web handle generation under the default domain configuration
- Blob upload now accepts application/octet-stream
- Blob staging cleans up its temporary file when the write fails
- Role grant and revoke errors now return structured JSON instead of plain text
- Unresolvable handles now return a 400 error instead of a server error

## [0.5.0] - 2026-05-23

### Added
- Repository import: upload a full repository, with structure validation and blob pre-fetch from the origin server
- Account-lifecycle changes (creation, deactivation, reactivation, takedown, deletion, identity submission) now emit firehose events matching the reference PDS
- Dynamic lexicon loading with on-disk and in-memory caching and configurable failure handling, plus admin endpoints to inspect, refresh, and evict the cache
- Postgres backend coverage across every shipped surface
- Blob writes are now durable before their metadata is recorded
- Metrics for lexicon fetching and validation

### Changed
- Record writes are signed with the per-account repository key rather than a server-wide key
- Tombstoned DIDs now return a typed error instead of a server error

### Fixed
- Fixed a Postgres login failure caused by a timestamp type mismatch
- Missing-blob requests now return the spec-correct error code

## [0.4.0] - 2026-05-13

### Added
- Multi-instance support: a distributed state backend keeps auth state and rate-limit buckets coherent across instances, with configurable backends, a dedicated maintenance database pool, and background cleanup of expired DPoP records, OAuth flow state, and rate-limit buckets
- Optional background blob garbage-collection sweep (off by default), plus a CLI command for one-off operator-initiated sweeps
- Admin UI: reusable modal dialogs with validation, typed-confirmation, and required rationale for destructive actions; readable mapping of server error codes; and operator role grant/revoke behind that confirmation gate
- Audit and dashboard: a chain-verification indicator with detail, subject-CID filtering, a time-range preset selector, and success notifications that link through to the audit entry
- Moderation event and subject-status endpoints accept both current and legacy request shapes (sending both is rejected)

### Changed
- Forensic-export bundles now use the same audit-entry shape as the audit-trail API (breaking change for scripts parsing the previous bundle format)
- Settings screens now show the source (runtime / file / default / recovery) of each value
- Bulk-action batch sizes are now configured per action rather than as a single shared limit

## [0.3.0] - 2026-05-10

### Added
- File-based runtime configuration (YAML), layered between the live settings API and the built-in defaults
- API stability contracts committed for subject types, capability descriptions, action-ID surfacing, audit-trail reads, and multi-subject event emission

### Changed
- Moderation event emission now takes a list of subjects and returns a snapshot per subject (breaking change — single-subject callers must wrap their subject in a one-element list)
- Batch moderation operations are now all-or-nothing: any per-subject failure rolls back the whole batch (breaking change — the per-subject failures field is removed from responses)
- Role grant and revoke responses now use camelCase fields (breaking change against the previous ad-hoc JSON)
- Moderation metrics accept both the current time-range shape and the legacy start/end shape

### Removed
- Admin authority now comes only from the roles table; the environment-variable admin list is removed and the first super-admin is bootstrapped via CLI (breaking change for deployments that relied on the env-var admin list)

## [0.2.0] - 2026-05-04

### Added
- Live moderation-event subscription backed by a retention-bounded feed, with an operator-configurable retention window and background cleanup of older entries (subscribers whose cursor has aged out receive an explicit outdated-cursor signal)
- Audit-chain entries can be streamed live over the moderation-event subscription
- The audit-trail API reports chain-verification status, backed by per-row hash and linkage checks
- Audit-chain coverage extended across administrative endpoints, with a per-subject snapshot captured for batch operations
- DPoP proof-of-possession enforced on resource requests, closing a stolen-token replay path

### Changed
- Moderation metrics moved from POST to GET (breaking change — POST clients receive 405)
- Sending email via a moderation event now requires Admin, and several operator endpoints (password reset, forensic export, runtime settings) now require admin server scope (breaking change for moderation-only tokens)
- The moderation-event subscription's action filter is now a list and gains a record-level subject filter (breaking change for clients sending a scalar filter)
- Invalid DPoP proofs are rejected with a 400 instead of silently downgrading to Bearer
- Unknown runtime-setting keys are rejected with a 400
- The admin audit-log endpoint now reads from the hash-chained audit store (IP address omitted)

### Fixed
- Administrative actions now write their audit-chain entry atomically with the action, so an action can't land un-audited
- Concurrent audit-chain writes are serialized, eliminating silent loss of entries under load
- Forensic-export integrity hash now covers the entire bundle, not just the manifest
- Admin error pages no longer render URL-derived input as HTML (cross-site-scripting fix)
- Debug admin pages are no longer reachable in production builds
- Fixed admin-UI role grant and revoke (a field-name mismatch)

### Removed
- The legacy admin audit-log table is removed; every administrative decision is recorded in the hash-chained audit store
- The environment-variable admin list no longer grants admin authority — authority comes from the roles table
- The admin UI no longer stores the refresh token in browser local storage

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
