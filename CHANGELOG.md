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
- Login splash logo and banner image are operator-customizable via direct upload or URL settings
- Ten bundled themes — Dark, Light, Stack Classic, Ember, Emerald, Glacier, Meridian, Pride, and High Contrast Dark/Light — five dark and five light, with Stack Classic as the deployment default
- All ten themes pass WCAG 2.2 AA contrast checks via the substrate's verifier; High Contrast Dark and High Contrast Light additionally clear AAA for text (7:1). Programmatic contrast only — not a full focus-indicator, keyboard, or screen-reader audit
- Theme extension points — themes can declare and provide named extension points that surfaces opt into at runtime
- Theme authoring documentation and a reference example theme
- Kryphocron admin surface: overview, deployment-wide audiences, laquna status with rotation history, and tier-activity pages, plus a per-account drawer, a policy page, and a dashboard summary block
- Encryption-at-rest for private-tier posts — encoded on write and transparently decoded for authorized readers — with a standard rotation oracle, automatic re-encoding on key rotation, and operator read endpoints
- Admin authentication: refresh-token flow with rotation on use, and per-operator session management with a Sessions page to view and revoke active sessions
- Recovery surfaces: recovery-mode status display, single-repository rebuild, bulk repository repair, and sequencer integrity validation
- Blocking a subject now also removes them from the blocker's audiences, with an audit log of the cascade
- Moderation list pages (Reports, Appeals, Events, Audit) share unified pagination and filtering behavior
- UI building blocks: loading skeletons, spinners, inline errors, error boundaries, consistent timestamps, source-tier indicators, and a save-with-rationale confirmation

### Changed
- Kryphocron is now enabled by default on fresh deployments; operators can disable it by setting PDS_KRYPHOCRON_ENABLED=false
- Operator role changes now take effect on the next request, without requiring re-login

### Fixed
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
