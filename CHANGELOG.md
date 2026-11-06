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
- Operators, separate from atproto accounts, with roles owner, admin,
  moderator and viewer, and API tokens. Every admin method has a minimum
  role; every operator and moderation action goes to the audit log.
- The reference admin API: `com.atproto.admin.getAccountInfo(s)`,
  `getSubjectStatus` and `updateSubjectStatus` (takedowns of accounts,
  records by URI whatever the CID, and blobs; they apply on the next
  request, and an account takedown ends its sessions and OAuth grants and
  emits `#account`), `sendEmail` (through the moderation sender),
  `deleteAccount`, `updateAccountPassword`/`Email`/`Handle`, the invite
  methods and `server.createInviteCode(s)`, plus `searchAccounts`. The
  `admin:<password>` Basic auth (`PDS_ADMIN_PASSWORD`, otherwise
  generated; `admin.basic_auth_enabled` turns it off) counts as admin, and
  the moderation service gets the reference's mod-service access. Admin
  credentials read taken-down repos through the sync methods.
- Locus operator methods under `app.nearhorizon.locus`, by role:
  settings, the audit log, jobs, import status and kryphocron status
  (viewer); the private-tier block and the scanner review queue
  (moderator); changing settings and starting a rotation generation
  (admin); operators and API tokens (owner); and `describeCapabilities`.
- A server-rendered web UI with no scripts and a strict Content Security
  Policy: one stylesheet of tokens, bundled fonts (Atkinson Hyperlegible,
  IBM Plex, JetBrains Mono), light, dark or the device's choice, four
  colour schemes, and a page shell with a sidebar. Its design comes from
  Horizon-Prism's admin UI.
- First-run setup: until it completes a node serves only the setup wizard,
  health and `/tls-check` (for the bootstrap hostname), behind a one-time
  token printed to the log and written to `<data>/setup-token`. The wizard
  covers the hostname, storage, proxy and TLS, handle domains (suggesting
  the parent's wildcard and reserving the domain's existing subdomains and
  the PDS's own label), the first operator, email with a test message,
  network services, private posts and keys, then runs checks (DNS,
  reachability through the public URL, Cloudflare's SSL mode, upload caps)
  whose warnings must be accepted. `aurora-locus setup` asks the same
  questions at a terminal, `--config setup.toml` takes them from a file,
  and `LOCUS_SETUP_AUTO=true` builds them from env. Completion is one
  transaction, so two nodes can't both apply it; every node then restarts
  into service on its own.
- The operator console at `/admin`: sign-in with a password and a TOTP
  code app (codes work once; idle and absolute session limits; a fresh
  password and code before managing operators or deleting accounts), and
  pages for Home, accounts, invites, operators, moderation, scanning,
  private posts, identity, network services, email (SMTP and a delivery
  test), proxy and TLS (with a DNS check), limits, storage, jobs, the audit
  log (filters and a JSON-lines export), this node's recent log lines, and
  every setting. Each operator chooses their own appearance. Console forms
  call the admin API as the signed-in operator, so its roles, checks and
  audit entries apply.
- The account portal at `/account` (`portal.enabled`, on): sign-up and
  sign-in (with email sign-in codes; app passwords are refused), handle,
  email confirmation and change, sign-in codes, password reset, app
  passwords, connected OAuth apps, a repo download, the DID document and a
  PLC recovery key, guidance for moving an account here, deactivation and
  deletion. A portal sign-in is an ordinary account session, so a password
  change ends it.
- OAuth sign-up: `prompt=create`, or "Create an account" on the sign-in
  page, makes an account and continues to consent.
- Branding (console): a draft theme with colours for light and dark
  (each checked for contrast before publishing), a logo, favicon,
  backgrounds, links, a bundled or uploaded font, corner and spacing
  presets, page text and custom CSS (parsed and re-serialized, rules and
  `@media` only, `url()` limited to uploads). Published versions are kept
  and can be put back. The OAuth sign-in and consent pages take only the
  checked variables, never custom CSS or uploaded fonts. Uploads are
  type-checked by their bytes and collected a week after no version uses
  them; emails carry the theme's footer.
- `identity.reserved_handles`, `deploy.proxy_mode` and
  `admin.session_idle_minutes` settings.
- The admin socket (`<data>/admin.sock`, mode 0600; root or the service's
  own user, checked by peer credentials, as the owner) and the CLI's online
  commands over it, or over HTTP with `--url` and `--token`: `status`,
  `account`, `invite`, `settings` (including export and import),
  `operator`, `moderation`, `crawl request`, `jobs list`, `kryphocron`,
  `call` for any XRPC method, and `healthcheck`.
- Health: `/xrpc/_health` answers 503 when the database doesn't,
  `/healthz` (liveness) and `/readyz` (database and blob store).
- `/tls-check?domain=` approves certificates only for the PDS hostname,
  hosted did:web hosts and existing handles.
- Cloudflare mode (`deploy.proxy_mode`, env `LOCUS_PROXY_MODE`): the
  client is `CF-Connecting-IP` only when the hop before the trusted proxy
  is one of Cloudflare's ranges (bundled, refreshed daily).
- `aurora-locus deploy caddyfile` prints a Caddyfile for on-demand
  certificates, a DNS-01 wildcard or Cloudflare's origin certificate, every
  variant with `strict_sni_host`.
- Direct TLS (`LOCUS_TLS=true`): locus serves HTTPS itself with ACME
  certificates (Let's Encrypt by default) ordered on demand for names
  `/tls-check` approves, the PDS hostname's at start, renewed after 60
  days; port 80 answers HTTP-01 challenges and redirects to HTTPS.
- Deployment files in `deploy/`: a multi-stage image running as a non-root
  user (with `goat`), a compose file with Caddy at a fixed address on its
  own network and locus published on localhost only, a Caddy image with
  DNS provider modules, an installer for a fresh server, a hardened
  systemd unit, and notes for Caddy, nginx, Traefik and HAProxy.
- S3-compatible blob storage (AWS, R2, B2 and similar, with a custom
  endpoint and path-style addressing) from the reference's
  `PDS_BLOBSTORE_S3_*` variables, with an optional separate bucket for
  gated media. A new install with a bucket configured stores its blobs
  there; large uploads go up in parts and appear only once complete.
- Blob bytes follow their state: private-only media moves to the gated
  store while the media gate is on (and back when it's switched off or a
  public record uses it), and taken-down or scanner-matched blobs move to
  quarantine (and back when the takedown is reversed or the match cleared).
  A background job catches any move a crash or a gate switch left.
- Key backups (`aurora-locus keys backup --out <file>`, and a download in
  setup): every stored secret, every account signing key and the PLC
  rotation keys' roles, encrypted under a passphrase (Argon2id,
  XChaCha20-Poly1305), streamed in frames that can't be reordered, cut
  short or mixed between backups unnoticed. The CLI reads each backup back
  before keeping it; `--passphrase-file` serves scheduled backups.
- `aurora-locus keys show`: the master key, service key and PLC rotation
  keys, the last key backup, and how many accounts' documents list the
  current key, only an older one, or none.
- Changing the PDS's PLC rotation key (`keys set plc-rotation-key
  --generate | --file`, or a changed `PDS_PLC_ROTATION_KEY_*` value): the
  new key is pending until a key backup holds it (or
  `--acknowledge-no-backup`); the previous key stays as a secondary
  signer for accounts whose documents list it. `--discard-pending` and
  `keys drop-plc-key` remove a key only after checking no account's
  document still needs it. Key changes need the operator's password and
  code over HTTP, or a typed confirmation over the socket, and are
  audited. Every node reloads its keys within seconds of a change.
- A background scan records which held PLC rotation keys each account's
  document lists, after a key becomes current and daily while an older
  key is held.
- `keys set jwt-secret` and `keys set dpop-secret` (read from a file or
  stdin; only a fingerprint is audited), and `keys rotate-service-key`. A
  changed `PDS_JWT_SECRET` or `PDS_DPOP_SECRET` is ignored with a warning
  that names the command.
- `keys rotate-account <did>… [--file]` and `identity publish <did>…
  [--file]`, the reference's `rotate-keys` and `publish-identity`
  scripts: the first points each account's PLC document at its stored
  signing key and announces `#identity` and a fresh commit's `#sync`.
- `keys import <backup>`: adds the PLC rotation keys this PDS lacks as
  secondary signers, and restores an account's signing key from the
  backup where its live DID document lists that key and not the stored
  one (with a fresh commit and `#sync` for active accounts); it reports
  accounts whose documents list neither.
- AWS KMS (or a compatible service, `LOCUS_KMS_ENDPOINT`): a master key
  from KMS (`LOCUS_MASTER_KEY_SOURCE=kms`, `LOCUS_MASTER_KEY_KMS_ID`), a
  data key kept wrapped in the database so every node unwraps the same
  one; and a PLC rotation key that signs in KMS
  (`PDS_PLC_ROTATION_KEY_KMS_KEY_ID`, as the reference, or `keys set
  plc-rotation-key --kms-key-id`), never stored here. Key backups name
  KMS keys by id.
- `aurora-locus keys rewrap --to file --path <new> | env --out <file> |
  kms --kms-key-id <id>` (offline): re-encrypts every stored secret under
  a new master key in one transaction; nothing changes if it fails.
- Backups for SQLite installs: `aurora-locus backup create
  [--passphrase-file] [--wait]` (or scheduled with
  `backups.every_hours`, keeping `backups.keep`) writes a consistent
  database snapshot, every object it names, a key backup and, last, a
  manifest; the delete job holds off while one runs, so nothing the
  snapshot names is deleted before it's copied. `backup list`.
- `backup restore <dir> --backend disk|s3` (offline, only into an empty
  install, every refusal before anything is written): restores the
  database and objects, restarts pending blob moves, releases the old
  install's leases, sets the next event's seq above what relays report
  (`--seq-floor`, `sync.restore_seq_margin`), lists accounts whose live
  DID documents don't list their restored signing key, re-announces every
  account (`#identity`, `#account`) and, with `--emit-sync`, writes a
  fresh commit and `#sync` for each active one.
- `backup post-restore` for after a Postgres point-in-time recovery:
  checks every tracked object, brings deleted ones back from a versioned
  bucket's earlier versions (or, with `--backend`, from the restored
  backend), stops when most are missing, ends a running blob migration,
  and finishes as `backup restore` does.
- `aurora-locus migrate blobs --to disk|s3` moves every blob to another
  backend while the node keeps serving: copy passes, a verify, then a
  short pause (bounded by `blobs.cutover_max_pause_ms`) for a final pass
  and the flip; every node then reloads onto the new backend. Uploads in
  the pause get a retryable 503. `--status` follows it, `--cancel` ends it
  without cutting over, and objects missing from the source hold the
  cutover until `--accept-missing`. The old backend's objects are left
  for the operator.
- `aurora-locus migrate storage --to postgres --url <url>` (offline)
  copies a SQLite install into an empty Postgres database in one
  transaction, with every sequence continuing above what was copied; it
  refuses a node-generated master key file (declare it shared, or rewrap
  first) and a running blob migration.
- `aurora-locus migrate hostname <new>`: prepares a new hostname
  (reserved against handles; `/tls-check` approves it so a certificate can
  be issued; it serves only health), checks it reaches this install over
  HTTPS, and `--confirm` makes it the hostname on every node. A job then
  moves every did:plc account's PDS endpoint and every hosted did:web
  document, with `#identity` for each, and lists accounts it can't move.
  The old hostname keeps full service, and its service DID stays an
  accepted token audience, while any DID document names it; then it
  becomes a former hostname that serves its `did.json` forever and
  redirects everything else. `--cancel`, `--close` (after
  `identity.hostname_transition_days`) and `--status`. Refused outside
  the standalone role. OAuth: each served host has its own
  protected-resource metadata, and the issuer lists every served origin.
- `aurora-locus import reference-pds <data dir> --env <pds.env>`: turns a
  stopped reference PDS (0.5.x) into a locus install with the same
  hostname, DIDs, handles, repos, blobs, sessions and firehose position.
  Offline, into an empty SQLite install (`migrate storage` moves it to
  Postgres after), refused while the reference has its databases open.
  It brings over settings through the env mapping, the JWT and DPoP
  secrets, the PLC rotation key (as current) and the admin password;
  accounts, app passwords, refresh tokens, emailed tokens, invites, OAuth
  state and preferences; each repo's signing key and the blocks its
  current tree reaches (reference counts rebuilt by walking the tree;
  unreachable blocks dropped), the record index with takedowns, and blob
  metadata; and the event log within retention, with seqs continuing from
  the reference's last. Disk blobs stay in place under the blob dir's
  parent (copied when its name is one of locus's prefixes); quarantined
  ones are copied into `quarantine/`. Trusted proxies are set to the
  reference's ranges, and setup is marked complete.
- Imported scrypt passwords log in and become Argon2id on the first
  successful login; imported app passwords are found the reference's way
  until first used, then get a lookup hash and an Argon2id hash. Access
  tokens a reference PDS issued before the import (legacy, and OAuth ones
  signed with its JWT secret) are accepted until they expire.
- The shard role (`LOCUS_ROLE=shard`), to the reference PDS's entryway
  contract: the entryway's URL and DID are settings
  (`PDS_ENTRYWAY_URL`, `PDS_ENTRYWAY_DID`); its token verify key, PLC
  rotation key and admin token come from env on the first boot and change
  after that with `keys set entryway-verify-key` (the previous key stays
  accepted 2 hours unless `--revoke-previous`),
  `keys set entryway-plc-rotation-key` (earlier keys stay accepted until
  `--revoke-previous`) and `keys set entryway-admin-token`. A shard
  accepts the entryway's ES256K access tokens (legacy and OAuth, the
  OAuth ones checked statelessly, `ref:` scopes dereferenced through the
  entryway), passes account-management calls to the entryway with the
  client's credentials or re-signed as the account, serves no OAuth
  server, invites or `/.well-known/atproto-did`, redirects
  `/.well-known/change-password`, names the entryway as its
  authorization server, and creates accounts from a genesis operation the
  entryway's PLC rotation key signed (submitting it to PLC itself). It
  holds no PLC rotation key of its own and runs no PLC key scan.
- Locus shard callbacks, for a locus entryway: `.shard.setCallbackToken`
  (only a single-use service JWT from the entryway's token key; the shard
  keeps a hash of the token), and, with that token, `.shard.revokeSessions`
  (a session, or every token of an account issued before a time; checked
  on every request for 2 hours), `.shard.setVerifyKeys` and
  `.shard.setPrivateTierBlock` (the shard's own operator API then only
  shows the block). `describeCapabilities` gives the role.
- Shard restore: `backup restore` and `backup post-restore` on a shard
  require `--entryway-verify-key` and `--entryway-plc-rotation-key` (the
  entryway's current keys replace the restored ones) and refuse every
  hosted account's tokens issued before the restore.
- The entryway role (`LOCUS_ROLE=entryway`): accounts, logins, handles and
  the PLC rotation key for shards behind it. `shard add <url>
  --admin-password-file <f>` probes a shard (`describeCapabilities`),
  checks its admin password and, on a locus shard, installs a callback
  token; `shard list`, `shard weight`, `shard drain`. New accounts go to
  the shard with the most room for its weight: the entryway reserves a
  signing key there, signs the genesis operation with its PLC rotation
  key and has the shard create the account, keeping the email, password,
  invite and sessions. Its tokens are ES256K with its token key (`keys
  show` gives it, as did:key and hex), access tokens addressed to the
  account's shard with a `sid`. It takes the calls shards re-sign as the
  account, tells the shard of status, handle and deletion changes
  (`admin.updateSubjectStatus`, `admin.updateAccountHandle`,
  `admin.deleteAccount`), and forwards repo and other calls to the
  account's shard. `com.atproto.temp.dereferenceScope` is served (a locus
  entryway sends full scopes).
- Entryway pushes to locus shards, through a durable per-shard outbox
  (delivered in order, kept until the shard acknowledges; a later
  private-tier block for the same account, or a later key set, replaces
  a queued one): every session revocation (logout, password change or
  reset, app-password revoke, OAuth revoke, takedown) is queued in the
  same transaction as `revokeSessions`, private-tier blocks as
  `setPrivateTierBlock`, and re-adding a shard re-sends the keys and
  every account's block. OAuth tokens from an entryway are ES256K,
  addressed to the account's shard with the full scope and `sid` = the
  grant, so shards check them statelessly. `keys rotate-entryway-key`
  sends shards `{old, new}`, switches signing once every shard has it
  (`--confirm <shard>` for a non-locus one) and sends `{new}` two hours
  later. `keys set plc-rotation-key --confirm <shard>` records shards set
  to a pending PLC rotation key, which becomes current only once every
  shard taking accounts is confirmed (or with `--acknowledge-unconfirmed`);
  once one is, the pending key can't be replaced or discarded. The
  console's Shards page lists the shards, their queues and a rotation's
  progress.
- Offline commands (`import reference-pds`, `keys rewrap`, `backup
  restore`, `backup post-restore`, `migrate storage`) refuse to run while a
  node is running: on SQLite every node holds an exclusive lock on
  `<data>/node.lock` (so one node per data dir), on Postgres a shared
  advisory lock that an offline command needs exclusively.
- `jobs pause <name> [--resume]` and `jobs run <name>`: a paused job stops
  within seconds and isn't taken up again; a run restarts it now.
  `jobs list` shows both.
- The orphan sweep (daily): lists the blob store under locus's prefixes
  (and an import's in-place prefixes, never the rest of a disk root) and
  queues for deletion every key older than a day that no blob, branding
  asset, move or queued delete names.
- Repair tools (admin): `repo rebuild <did>` (the reference's
  `rebuild-repo`: a new tree and signed commit from the record index, the
  account's blocks re-counted so the old tree's go to GC, `#sync`;
  records whose blocks are gone are listed), `repair repo <did>` (the
  record index made to match the tree, with blob references and
  backlinks) and `repair identity <did>` (a did:plc document set back to
  the account's handle, this PDS and its signing key in one operation,
  then `#identity`).
- `sequencer recover [--did] [--since]` (the reference's
  `sequencer-recovery`): replays the retained event log onto repositories,
  adopting each account's newest logged commit whose tree is whole (blocks
  re-counted, record index rebuilt, `#sync`). Accounts it can't recover
  are recorded, and `repair repos --from-sequencer` replays their logged
  record changes into the index and rebuilds them. `keys rotate-account
  --recovery` rotates the accounts recovery touched.
- Redis for rate limits and replay windows: `LOCUS_REDIS_URL`, or the
  reference's `PDS_REDIS_SCRATCH_ADDRESS` (`host:port`) and
  `PDS_REDIS_SCRATCH_PASSWORD`. With it, rate-limit counters (backend
  `auto` or `redis`) and every replay window (DPoP proofs, client
  assertions, request objects, code challenges, entryway service tokens)
  are shared through Redis; without it they stay in the database. A node
  that can't reach its Redis doesn't start. Nodes warn at boot when
  another live node has a different role or Redis URL.
- Prometheus metrics at `/metrics`: XRPC requests and latency by NSID
  and status, commit and import-verify time, sequencer lag and events,
  firehose subscribers and drops, job runs, rate-limit refusals, blob
  bytes, database pool connections, kryphocron encodes, scanner calls and
  gated media fetches. Readable from `metrics.allowed_ips` (loopback by
  default) or with `metrics.token` (`LOCUS_METRICS_TOKEN`); off with
  `metrics.enabled`.
- A differential conformance harness (`scripts/conformance.sh`,
  `crates/locus-conformance`): the same XRPC scenarios and firehose
  against a fresh node and the reference PDS (`@atproto/pds` 0.5.51 in
  Docker) on a shared mock PLC, with values that legitimately differ
  normalized and deliberate differences listed with their reasons in
  `conformance/allowlist.json`.
- `com.atproto.server.reserveSigningKey`, in every role (rate-limited per
  IP; unused keys expire after a day).

### Changed

- Answers now match the reference PDS where the conformance run found
  them apart: a missing or malformed query parameter or JSON body is a 400
  `InvalidRequest` (not a plain-text error); handle syntax errors on
  handle inputs are `InvalidRequest` (disallowed TLDs and service-domain
  rules stay `InvalidHandle`); `createAccount` with a taken handle is
  `InvalidRequest` and always returns `didDoc`; `refreshSession` includes
  the email fields; `describeServer` has `blobUploadLimit` and `contact`
  and no `phoneVerificationRequired`; `listRepos` gives a cursor on every
  page with rows; `getBlocks` serves only blocks a tree references; a
  duplicate app-password name is `InvalidRequest`.
- proto-blue pinned at a fix for its lazily loaded MST: a repository
  emptied of every record kept the last record's tree layer, so records
  written afterwards got a non-canonical data root (one other
  implementations wouldn't reproduce). Found by new property tests that
  also verify every `#commit` event from its own blocks and `getRepo`
  since every earlier rev.
- `backup post-restore` reports a blob migration it ended whether it was
  copying or paused (it reported only a paused one).
- Single-label handle domains (`.test`) are accepted in setup.
- Setup no longer adds the default relay when `PDS_CRAWLERS` is set
  (even empty) or the hostname is local.
- A leased job stops when a renewal hasn't answered before its lease could
  run out, so a slow database can't leave two nodes running it at once.
- Blobs uploaded after an import whose records already use them, and blobs
  an import brings in, are now checked for being private-only.
- The blob delete job skips keys a branding upload still uses.

- proto-blue is pinned to `skydeval/proto-blue` `feat/lazy-mst` (`a88e52e`),
  which adds the lazily loaded MST.
