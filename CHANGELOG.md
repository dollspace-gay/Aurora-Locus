# Changelog

All notable changes to aurora-locus are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Work toward 0.1: a production Rust atproto PDS built on proto-blue, with
kryphocron built in.

### Added

- Cargo workspace in `aurora-locus/` with the crate layout for 0.1, pinned to
  proto-blue (`skydeval/proto-blue` at `7a3fb57`), kryphocron 0.4.5 and
  kryphocron-lexicons 0.3.5. Rust 1.88.
- `aurora-locus version` command.
- Bootstrap configuration from `locus.toml`, `LOCUS_*` variables and the
  reference PDS's `PDS_*` variables, including its SSRF-protection and
  dev-mode rules.
- Storage with SQLite and Postgres backends: embedded migrations, and a node
  refuses to start against a database schema newer than it knows.
- Runtime settings stored in the database, validated, audited and applied
  live on every node (Postgres `NOTIFY` plus a 5-second poll).
- Env values for runtime settings seed them at first boot and apply again
  when they change, once every live node agrees.
- Fenced leases for singleton jobs: a node that loses its lease can't commit.
- `aurora-locus serve` starts a node: opens and migrates the database, keeps a
  persistent node id, claims a TID clock id, applies env values, follows live
  settings (including the log level) and shuts down cleanly on SIGTERM.
- Job run bookkeeping (last run, last success, next run, last error).
- Repo engine: every record write is one transaction covering the MST
  blocks, the repository root, the record index and the `#commit` outbox
  event. The MST loads lazily from stored blocks, so a write reads only the
  paths it changes. Writes are checked in order against the record index
  (swapCommit and swapRecord, and create/update/delete conflicts return 400
  rather than 500). Every commit verifies, from the changed nodes alone, that
  the tree changed exactly as the writes say before anything is stored.
- Commits are signed v3 commits with `prev: null`; revs strictly increase per
  repository, even across clock steps.
- `#commit` events carry `since`, `prevData`, per-op `prev`, and a CAR with
  the commit, new MST nodes, written records and covering proofs, so
  consumers can check every op; writes whose event would exceed 2 MB are
  refused.
- Reference-counted repo blocks, collected by a leased GC job once
  unreferenced for 10 minutes.
- Encrypted key store: every stored secret and account signing key is sealed
  with XChaCha20-Poly1305 under one master key per install (from a file or
  `LOCUS_MASTER_KEY`). The key's id and a MAC are recorded in the database;
  a node with a different key refuses to start, and every secret write checks
  the key again. A file key is generated only on a fresh single-node SQLite
  install with a persistent data dir.
- did:plc operations: build, sign and verify operations and tombstones, derive
  a DID from its genesis operation, and read and write the PLC directory's
  JSON form, checked against real directory data. Operations locus signs are
  validated first (did:key rotation keys and verification methods, at://
  `alsoKnownAs`, http(s) service endpoints).
- Handle rules as the reference PDS: normalization and syntax, disallowed
  TLDs, the first-label length and no-dot rules on the PDS's own handle
  domains, the reference's reserved names plus operator-entered ones, and the
  reference's slur filter.
- Accounts, sessions, app passwords and invite codes in the database, with
  handle and email uniqueness across every account (a clash is "Handle
  already taken" / "Email already taken", never a server error).
- Legacy sessions with the reference's token shapes (HS256 access and
  refresh JWTs) but checked against the database on every request: logout,
  app-password revocation and takedown take effect on the next request.
  Refresh rotation keeps the reference's 2-hour replay grace. Passwords and
  app passwords are hashed with Argon2id.
- HTTP server on `serve`: `describeServer`, `createAccount` (did:plc), the
  session endpoints (`createSession`, `getSession`, `refreshSession`,
  `deleteSession`), app passwords (create, list, revoke), `resolveHandle` for
  local handles, `/.well-known/atproto-did` on handle hosts, the service's
  `/.well-known/did.json`, and `/xrpc/_health`. Errors use the XRPC
  `{error, message}` shape and the reference's error names.
- Account creation writes the account, its signing key, the invite use, the
  `#identity`, `#account`, `#commit` and `#sync` events, the empty repository
  and the first session in one transaction, after the PLC directory accepts
  the genesis operation; if the transaction fails, the new DID is tombstoned.
- Server secrets (JWT secret, app-password pepper, PLC rotation key, service
  signing key) are generated on first boot and kept in the key store;
  `PDS_JWT_SECRET` and `PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX` are
  imported when set.
- PLC rotation keys are held by role (current, pending, secondary). Every
  op locus signs uses the held key the account's PLC document lists first,
  and locus records which held keys each account's document lists.
- `com.atproto.identity.updateHandle`, for did:plc and did:web accounts and
  for handles on other domains (verified by DNS or HTTPS). On did:plc the
  PLC op goes first; if anything after it fails, or the directory's answer
  is unclear, a repair job finishes the change. The same job tombstones the
  DID of a sign-up that failed after its DID was registered.
- did:web accounts: `createAccount` with `locusDidMethod: "web"` (or the
  `accounts.default_did_method` setting) creates `did:web:<handle>`, whose
  document locus serves on that host and updates on handle changes. A
  did:web host stays tied to its DID: no other account can take it as a
  handle, nor the PDS hostname.
- `resolveHandle` resolves handles elsewhere on the network: DNS
  (`_atproto` TXT, with the reference's backup nameservers setting) raced
  against HTTPS, with a shared cache (10 minutes for a hit, 1 minute for a
  miss). DID documents are resolved through a cache shared by every node
  (`PDS_DID_CACHE_STALE_TTL`, `PDS_DID_CACHE_MAX_TTL`).
- Outbound requests to user-influenced URLs go through an SSRF-safe client,
  as the reference's: HTTPS and public unicast addresses only (checked when
  connecting), no redirects, 512 KB responses, and no IP-literal hosts or
  forbidden domains for documents and handle lookups.
  `PDS_DISABLE_SSRF_PROTECTION` and local dev mode relax it as the
  reference does.
- Email over SMTP (`smtp://` with STARTTLS when offered, or `smtps://`).
  SMTP URLs are kept in the encrypted key store; `PDS_EMAIL_SMTP_URL`,
  `PDS_MODERATION_EMAIL_SMTP_URL`, `PDS_EMAIL_FROM_ADDRESS` and
  `PDS_MODERATION_EMAIL_ADDRESS` are read as the reference reads them. A
  sender works once its connect-and-auth check passes; until then, features
  that need email say "Email is not configured on this server" instead of
  pretending to send.
- `requestEmailConfirmation`, `confirmEmail`, `requestEmailUpdate`,
  `updateEmail` (including turning email sign-in codes on and off),
  `requestPasswordReset` and `resetPassword`, with the reference's
  `XXXXX-XXXXX` tokens (15 minutes, one per account and purpose, stored as
  keyed hashes). `requestPasswordReset` answers the same whether or not the
  address has an account. A password reset ends every session.
- Email sign-in codes: `createSession` asks for `authFactorToken`
  (`AuthFactorTokenRequired`) when the account has them on, except for app
  passwords, as the reference.
- `getRecommendedDidCredentials`, `requestPlcOperationSignature`,
  `signPlcOperation` and `submitPlcOperation`, as the reference (an emailed
  code to sign; a submitted op must keep the current PDS rotation key, this
  PDS, the account's signing key and handle). `signPlcOperation` also
  checks the fields it signs: did:key rotation keys and verification
  methods, typed http(s) services, `at://` names.
- `deactivateAccount` (with `deleteAfter`), `activateAccount`,
  `checkAccountStatus`, `requestAccountDelete` and `deleteAccount`.
  Activation checks the DID document as the reference (this PDS's
  endpoint, the account's signing key, the current PDS rotation key) and
  emits `#account`, `#identity` and `#sync` together. Deactivating a
  taken-down account keeps the takedown.
- Account deletion runs as a resumable job: the first step marks the
  account deleted, ends its sessions and emits `#account deleted`; batches
  then remove its repository, sessions, app passwords, invites and keys,
  and the account row last, freeing its handle and email. A node that
  stops mid-way leaves the job to another. A deactivated account is
  deleted once its `deleteAfter` passes.
- `getServiceAuth`, and service-JWT checks for requests from users'
  services (signature against the issuer's DID document, refreshed once on
  a mismatch; audience; method), as the reference.
- Migrating an account in: `createAccount` with an existing `did` and a
  service JWT from it. The account starts deactivated and emits nothing
  until `activateAccount` finds its DID document pointing here.
- did:plc documents are fetched from the configured PLC directory through
  the operator client (so a local directory on plain HTTP works);
  did:web documents go through the SSRF-safe client.
- `getAccountInviteCodes`, with accounts earning codes on an interval as
  the reference (`PDS_INVITE_INTERVAL`, `PDS_INVITE_EPOCH`; at most 5 unused;
  admin-gifted codes don't count). `checkSignupQueue` answers activated.
  `describeServer` reports `phoneVerificationRequired: false`.
- Routing by host: the PDS hostname and internal names (`localhost`, IP
  addresses, single-label names, the `http.internal_hosts` setting) get
  every route; handle hosts and hosted did:web hosts get only their
  `/.well-known` documents; health answers on any host; other names get
  404.
- Disposable email addresses are refused at sign-up and email change, with
  the reference's list (`scripts/update-disposable-domains.sh` refreshes
  it) plus the operator's `accounts.disposable_email_domains`.
- Record validation against the reference's curated known-schema list
  (217 lexicons, vendored by `scripts/vendor-lexicons.py`) and the
  `tools.kryphocron.*` lexicons, as the reference: `validate: false` skips
  it, unset validates known types and accepts unknown ones, `true` refuses
  unknown ones. `$type` must match the collection, record keys must match
  the schema's key type and pass the slur filter, records are strict data
  model (no floats, safe integers), and legacy blob refs are refused.
- Record endpoints: `createRecord`, `putRecord`, `deleteRecord`,
  `applyWrites` (up to 200 writes), `getRecord`, `listRecords` and
  `describeRepo`, as the reference: an identical `putRecord` and a
  `deleteRecord` of a missing record commit nothing, and `createRecord`
  replaces the account's earlier follow, block, like or repost of the same
  subject in the same commit.
- Blobs: `uploadBlob` (streamed; a sniffed MIME type wins over the request's,
  as the reference; `PDS_BLOB_UPLOAD_LIMIT`, default 300 MiB; user
  credentials or service auth), `getBlob` and a `/blob/<did>/<cid>` alias
  with `Range` support and the reference's safety headers. Stored on disk
  under the data directory, each copy under its own key, written atomically
  and verified against its CID when copied.
- Records that reference blobs are checked in the commit: the blob must be
  uploaded and not taken down, with the same MIME type and size
  (`BlobNotFound`, `InvalidMimeType`, `InvalidSize`). The first reference
  makes a blob permanent; removing the last one starts a grace period.
  Unreferenced blobs are collected (uploads never used after 6 hours,
  dropped blobs after an hour), and their bytes are deleted after a further
  grace so reads in flight finish. Account deletion removes the account's
  blobs too.
- Live request and record limits: 2,000,000-byte bodies for record writes
  (the reference allows 1,000,000), 150 KB for other JSON bodies, and
  1.1 MB per record (`limits.*` settings).
- The firehose: `com.atproto.sync.subscribeRepos` on every node. A leased
  sequencer moves events from the outbox (written in the same transaction
  as each change) into a numbered log with gapless seqs, so a consumer can
  never skip an event that commits late. Cursors backfill from the log and
  switch to live; a cursor ahead of the log gets `FutureCursor`, one behind
  retention `#info OutdatedCursor` and the oldest retained event; a
  subscriber too far behind gets `ConsumerTooSlow`. WebSocket pings every
  30 s keep proxies from cutting idle subscribers.
- Event retention (`sync.retention_hours`, default 72) and removal of a
  deleted account's earlier events (after
  `sync.deleted_history_preservation_days`, default at once).
- Crawler notification: `requestCrawl` to each relay in `sync.crawlers`
  (`PDS_CRAWLERS`) on new events, at most every 20 minutes each, with a
  call inside the window scheduled rather than dropped.
- Sync reads: `getRepo` (streamed as a CAR, all live blocks or those since
  a rev), `getRecord` (the record with its proof, or a proof of absence),
  `getBlocks`, `getLatestCommit`, `getRepoStatus`, `listRepos`,
  `listBlobs`, and the deprecated `getHead` and `getCheckout`. A
  taken-down, suspended or deactivated repo is refused to everyone but its
  owner (`RepoTakendown`, `RepoSuspended`, `RepoDeactivated`);
  `getRepoStatus` always answers.
- Repository writes are refused (retryable `RepoWriteBlocked`) while a
  deletion holds the account.
- `importRepo`: the uploaded CAR (up to `limits.import_size`, 1 GB) is
  applied as a diff onto the account's repository by a resumable job. The
  upload is streamed and checked (CIDs against bytes, one root, the
  account's DID, a bounded tree walk that refuses a node referenced twice,
  keys out of order or on the wrong layer, and missing blocks); blocks the
  CAR leaves out are taken from the current repository. The import is
  re-signed with a fresh rev and emits `#sync` for an active account; a
  commit not signed by the account's current key is accepted and noted. Any
  failure before the new root is in place rolls back completely; after it,
  the job only finishes. Writes are blocked until the job ends.
  `accounts.accept_imports` (`PDS_ACCEPTING_REPO_IMPORTS`) turns imports off.
- `listMissingBlobs`, and uploads of a blob records already reference are
  made permanent at once.
- `app.nearhorizon.locus.account.getImportStatus`: the account's latest
  import, its phase and progress, and why it failed.
- Pipethrough: XRPC methods without a local handler go to another service,
  as the reference. `atproto-proxy` picks the service; otherwise moderation
  methods go to the moderation service, `createReport` to the report
  service, and the rest to the AppView (`PDS_BSKY_APP_VIEW_*`,
  `PDS_MOD_SERVICE_*`, `PDS_REPORT_SERVICE_*`). Requests carry a service
  token from the account and the reference's forwarded headers; responses
  stream back with its headers. Upstream errors keep their status and
  name (500 becomes 502), an unreachable service is 502 `UpstreamFailure`.
  Account-management methods are never proxied, and chat needs full access
  or a privileged app password. Targets named by `atproto-proxy` are held
  to the SSRF rules. Timeouts, response size, HTTP/2 and retries follow the
  reference's `PDS_PROXY_*` settings.
- `getRecord` for a repo not hosted here is answered by the AppView.
- Preferences: `app.bsky.actor.getPreferences` and `putPreferences` with
  the reference's rules (a write replaces the `app.bsky` namespace,
  personal details only with full access, a derived `declaredAgePref`,
  moderation-service reads by `did`). Served even before an AppView is
  configured; piped through when `atproto-proxy` names another service.
- Read-after-write: the requester's own `getProfile`, `getProfiles`,
  `getTimeline`, `getAuthorFeed`, `getActorLikes` and `getPostThread`
  include records written here that the AppView hasn't seen yet (from its
  `atproto-repo-rev`), with `Atproto-Upstream-Lag`; a just-written post's
  thread is built locally when the AppView doesn't have it.
  `appview.cdn_url_pattern` (`PDS_BSKY_APP_VIEW_CDN_URL_PATTERN`) builds
  their image URLs.
- Rate limits, on by default (the reference's package default is off): a
  global per-IP bucket on XRPC requests (`getRepo` has its own), the
  reference's per-endpoint buckets (login by identifier and IP, account
  creation, password reset, email requests, handle changes, uploads,
  account deletion) and repo-write points per account (create 3, update 2,
  delete 1). `RateLimit-*` headers on responses; 429 `RateLimitExceeded`
  with `Retry-After`. Counters in each node or in the database (shared by
  every node; the default on Postgres). Bypass by `x-ratelimit-bypass`
  (`PDS_RATE_LIMIT_BYPASS_KEY`) or by address, with CIDR ranges honored
  (`PDS_RATE_LIMIT_BYPASS_IPS`).
- The client IP comes from `X-Forwarded-For` / `Forwarded` only when the
  connection is from a trusted proxy (`http.trusted_proxies`,
  `LOCUS_TRUSTED_PROXIES`; loopback by default).
- Firehose connections are limited per node and per client IP
  (`sync.max_subscribers`, `sync.max_subscribers_per_ip`).
- CORS as the reference (any origin, preflights answered), and
  `Cache-Control: private` with `Vary: Authorization` on authenticated
  responses.
- A request deadline (`http.request_deadline_ms`, 60 s): a request that
  outlasts it gets a retryable 503. Uploads, imports, repo exports, blobs
  and the firehose have their own caps instead.
- `app.bsky.feed.getFeed` asks the feed generator with a token for it, and
  `registerPush`/`unregisterPush` go to the AppView or the named push
  service.
- Access tokens are checked for the scopes each endpoint allows, as the
  reference: takendown and signup-queued sessions only reach the endpoints
  meant for them.
- OAuth authorization server, as the reference: server and resource
  metadata, JWKS, pushed authorization requests, the authorization code
  grant with PKCE, refresh tokens (rotated on use; a replayed refresh token
  revokes the grant) and revocation. Every token is bound to a DPoP key,
  with server nonces and a replay check shared by every node.
- OAuth clients: discoverable client metadata (fetched under the SSRF
  rules and cached), loopback clients for development, and
  `private_key_jwt` client authentication. `oauth.trusted_clients`
  (`PDS_OAUTH_TRUSTED_CLIENTS`) names trusted clients, which may use
  silent sign-on as in the reference.
- Granular OAuth scopes (`repo:`, `blob:`, `rpc:`, `account:`, `identity:`,
  `transition:*`), and `include:` permission sets resolved from published
  lexicons. Every endpoint checks an OAuth token's scopes as the
  reference; a missing scope is 403 `ScopeMissingError`.
- OAuth sign-in pages: sign in (with the email code when email 2FA is on,
  skipped on a device the account used in the last 30 days), pick a
  remembered account, and approve or deny the requested permissions.
  Forms are protected by a CSRF token and same-origin checks; pages send a
  strict Content-Security-Policy. Responses follow the request's
  `response_mode` (query, fragment or form post).
- Password resets and account deletion revoke the account's OAuth grants.
- Kryphocron: every `tools.kryphocron.*` record write is checked against
  the kryphocron registry (unregistered collections are refused) and its
  host rules (TID keys; a private post's `audienceList` and
  `publicCompanion` point into the writer's repo; a threadgate's key is
  its post's and its list rules name `app.bsky.graph.list` lists).
- The private tier needs an AppView and `kryphocron.private_tier_enabled`
  (else `UnsupportedNamespace`), and the AppView's permission for the
  account, asked with `app.nearhorizon.actor.getMembership` as the account
  and cached for its `validForSecs` (else `Forbidden`). If the AppView is
  unreachable, its last answer stands. An operator can block an account
  locally. Deletes are never gated.
- Private posts are stored encoded: plaintext submissions are encoded with
  laquna (`laquna/0.2`); pre-encoded ones are decoded (bounded) to check
  their content, then re-encoded. `embedBlobs`, `mentionDids`, `quote` and
  `mediaKinds` are always derived from the content. Plaintext is never
  stored, logged or sent.
- `tools.kryphocron.feed.createPostPrivate`, `updatePostPrivate` (changes
  only the audience; the encoded content is kept byte for byte) and
  `deletePostPrivate`; `createRecord`, `putRecord` and `applyWrites` run
  the same checks for kryphocron collections.
- Rotation: a new generation every `kryphocron.rotation_period_days` (60;
  30–90), shared by every node. A leased job re-encodes private posts
  under it in batched commits that never overwrite a concurrent write,
  spread over the period and limited by
  `kryphocron.rotation_max_commits_per_minute`. Imported plaintext posts
  are encoded on the next pass; records that can't be decoded are flagged
  and skipped.
- Kryphocron audit events are stored, with correlation ids keyed by an
  install secret, and kept for `kryphocron.audit_retention_days` (365).
- Private audiences are kept off the network: never in a repo, the
  firehose or a CAR, readable only by their owner. Locus keeps a hidden
  copy of the AppView's audiences, refreshed by polling (within
  `kryphocron.prism_call_budget`) and on use; a private post naming an
  audience the copy lacks refreshes it first.
- `tools.kryphocron.policy.manageAudience` and `deleteAudience`, and
  `createRecord`/`putRecord`/`deleteRecord`/`applyWrites` on the audience
  collection, write through to the AppView, which mints audience keys.
  Locus applies the AppView's checks first (modes, at most
  `kryphocron.audience_max_members` members and
  `kryphocron.audience_max_per_account` audiences). Deletes are never
  gated. A create whose answer never comes is matched to the audience it
  made, never retried blindly.
- During an AppView outage the copy is the authority: audiences can be
  created (with locus-minted keys), edited and deleted, posts and replies
  keep working where the copy can answer, and the account's proxied
  `app.nearhorizon.private.*Audience` calls are answered locally. When the
  AppView is back, queued edits are synced (the newer `updatedAt` wins),
  outage audiences are created there, and posts naming them are re-pointed
  to the AppView's URI.
- `tools.kryphocron.actor.participatePrivate`: a private reply under the
  parent's audience, refused (always the same `Forbidden`) unless the
  writer is in it, neither side blocks the other, and the root's
  threadgate allows it. Membership is answered from the copy for list
  audiences and by the AppView for the others.
- `app.nearhorizon.locus.audience.getMembershipAssertion`: short-lived
  signed assertions that the asker is in an audience owned here, signed
  with a dedicated `#kryphocron_audience` key; locus verifies them from
  other hosts when one of its accounts replies under their audiences.
- Uploads go through a scanner when one is configured (`blobs.scanner`,
  an HTTP webhook at `blobs.scanner_url`). A match is refused, kept and
  queued for review. When the scanner is down, `blobs.scan_unavailable`
  refuses uploads or stores them to scan later.
- The private media gate (`kryphocron.private_media_gated`, off; needs a
  scanner): media only private posts use is served only to the owner,
  audience members and `kryphocron.media_trusted_services`, never cached;
  to anyone else it looks like a missing blob.

### Changed

- proto-blue is pinned to `skydeval/proto-blue` `feat/lazy-mst` (`a88e52e`),
  which adds the lazily loaded MST.
