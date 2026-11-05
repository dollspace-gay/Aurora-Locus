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
- Access tokens are checked for the scopes each endpoint allows, as the
  reference: takendown and signup-queued sessions only reach the endpoints
  meant for them.

### Changed

- proto-blue is pinned to `skydeval/proto-blue` `feat/lazy-mst` (`a88e52e`),
  which adds the lazily loaded MST.
