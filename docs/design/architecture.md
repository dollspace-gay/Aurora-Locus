# aurora-locus architecture

This document describes how aurora-locus ("locus") is built: what it is for,
how a node is put together, how it stores and commits repositories, how
events reach the network, and how it scales. It is written for operators
deciding whether to run locus and for developers who want to work on it.

It describes 0.1 as it is. Where something is planned but not built, the
text says so.

## 1. What locus is

Locus is an atproto Personal Data Server (PDS) written in Rust. It hosts
accounts and their repositories, serves them to relays, AppViews, other
PDSes and OAuth clients, and proxies the rest of a client's requests to the
services that answer them.

The reference for its behaviour is Bluesky's PDS, `@atproto/pds` 0.5.51
(the distro image `ghcr.io/bluesky-social/pds:0.4.5051`). Locus aims to be
indistinguishable from it on the wire, except where it is deliberately
better (section 12 lists every difference).

### Goals

1. **Reference parity.** The network cannot tell locus from the reference
   PDS, apart from the deliberate differences. A differential conformance
   harness (`locus-conformance`) runs the same XRPC scenarios against both
   and compares the answers.
2. **First-class kryphocron.** Private-tier posts (`tools.kryphocron.*`)
   are written, encoded, validated and rotated by the PDS itself, with no
   plaintext private-post content at rest. Their encoding is friction, not
   encryption.
3. **One binary, any size.** The same binary runs one small node on a VPS
   or several nodes on Postgres, and can sit behind an entryway for very
   large deployments.
4. **Operable from a web UI.** Settings change live from the operator
   console, and everything the console does can also be done from the CLI.
5. **Safe defaults.** Setup refuses, or warns loudly about, configurations
   that would fail silently: no working email, an upload limit above the
   proxy's cap, Cloudflare in the wrong TLS mode, and so on.

### Non-goals for 0.1

- Decoding private content for readers or moderators. Locus stores and
  serves encoded records; decoding happens elsewhere (the AppView for the
  private tier, prism, and moderation tools such as cairn).
- Encryption of private content.
- Being an AppView, relay or labeler.
- kryphocron's peer-to-peer sync channel.
- Phone verification.

## 2. System overview

### Components

```
                 ┌──────────────── aurora-locus node ─────────────────┐
  clients ──►    │ HTTP layer (axum)                                   │
  (Horizon,      │  ├─ XRPC router ── local handlers ── pipethrough ──►│── AppView, prism,
   OAuth apps,   │  ├─ OAuth server + account portal (web)             │   moderation, report
   relays,       │  ├─ Operator console (web)                          │   services
   cairn)        │  ├─ Admin API (CLI and console backend)             │
                 │  └─ well-known, did.json, tls-check, health         │
                 │                                                     │
                 │ Core services                                       │
                 │  ├─ Accounts         ├─ Identity (PLC, did:web) ────│── PLC directory
                 │  ├─ Auth (sessions, OAuth, service auth)            │
                 │  ├─ Repo engine (proto-blue MST + commits)          │
                 │  ├─ Kryphocron (validate, encode, rotate)           │
                 │  ├─ Blobs            ├─ Mailer (SMTP)               │
                 │  ├─ Live settings    ├─ Rate limiter                │
                 │  └─ Sequencer + firehose ───────────────────────────│──► relays
                 │                                                     │
                 │ Job runner (leases): sequencer, GC, rotation, ...   │
                 └───────────────┬────────────────────┬────────────────┘
                                 │                    │
                       SQLite or Postgres      disk or S3 blobs   (optional Redis)
```

### Process model

- There is one binary, `aurora-locus`. `aurora-locus serve` runs a node;
  the other subcommands are the CLI.
- Every node serves every HTTP route. Nodes hold nothing durable: all state
  lives in the database and the blob store, and caches have bounded
  lifetimes.
- Singleton work runs under **leases** (section 10), so each job runs on
  exactly one node at a time and moves to another node if that one dies.
  This covers the sequencer, crawler notification, garbage collection,
  key rotation, retention pruning and other scheduled work.
- On a single SQLite node the leases are trivially held, but they still
  exist as rows, so the same fencing code runs in every shape.

### Deployment shapes

| | Small | Large |
|---|---|---|
| Nodes | 1 | 1..N behind a load balancer |
| Database | SQLite (WAL), one file | Postgres |
| Blobs | Disk | S3-compatible (or shared disk) |
| Rate-limit counters, replay windows | Database or in-process | Redis, or the database |
| Settings propagation | In-process | Postgres `NOTIFY`, plus a 5-second poll |
| Default proxy | Caddy (Docker Compose) | Operator's choice |

Moving from small to large is a supported migration, not a reinstall:
`aurora-locus migrate storage` copies SQLite into Postgres, and
`aurora-locus migrate blobs` moves blobs between disk and S3. Beyond one
large PDS, the **entryway** shape (section 10) puts a locus entryway in
front of many locus PDS shards.

Each node has a **role**, set at boot (`LOCUS_ROLE`): `pds` (standalone,
the default), `shard` (a PDS behind an entryway) or `entryway`.

## 3. Crates and build

### Workspace

The Cargo workspace lives in the `aurora-locus/` folder of the repository.
Crate boundaries follow ownership:

| Crate | Contents |
|---|---|
| `locus-core` | Domain types, errors, the settings model, bootstrap config, leases, metrics |
| `locus-store` | The SQLite and Postgres backends, their migrations, and every query |
| `locus-repo` | Repo engine on proto-blue: commit pipeline, block storage, verification, CAR import and export, the lexicon set |
| `locus-identity` | PLC operations, did:web hosting, handles, resolvers, the SSRF-safe fetcher |
| `locus-auth` | Sessions, app passwords, service auth, the OAuth server, scopes |
| `locus-kryphocron` | kryphocron integration: registry routing, write pipeline, codec wrapper, rotation, audit |
| `locus-blob` | Blob stores (disk, S3), moves, deletes and GC |
| `locus-sync` | Sequencer, firehose fan-out, crawler notification |
| `locus-http` | axum routing, XRPC plumbing, pipethrough, rate limiting, all handlers |
| `locus-web` | Server-rendered operator console, account portal, OAuth pages, setup wizard |
| `locus-mail` | Mailer and email templates |
| `locus-conformance` | Differential harness: the same scenarios against locus and the reference |
| `aurora-locus` | The binary: CLI, `serve`, wiring |

Storage is one `Store` type in `locus-store` with a variant per backend.
Each query is written for each backend with `sqlx` (no `AnyPool`), so each
can use its own features: advisory locks, `FOR UPDATE` and `LISTEN/NOTIFY`
on Postgres, `BEGIN IMMEDIATE` on SQLite. Handlers call `Store` methods and
never write SQL.

### What comes from proto-blue

Locus uses proto-blue, a Rust atproto library, for syntax types (DIDs,
handles, NSIDs, TIDs, AT-URIs), CIDs and the data model, DAG-CBOR and JSON
codecs, keys and signatures, lexicon validation, the MST and commit types,
firehose frame encoding, DID and handle resolution, the XRPC client, and
generated lexicon types.

Locus builds its own server layer instead of proto-blue's XRPC server: it
needs non-JSON responses, repeated query parameters, async auth, and
subscription frames with the `t` header. It also has its own streaming CAR
reader and writer, its own TID generator, and its own handle DNS lookups.

The MST loads lazily, so a commit reads and re-hashes only the paths it
changes. That change is carried on a proto-blue fork that locus pins until
it lands upstream. All proto-blue crates are pinned to one commit through a
single `[patch]` block, so `kryphocron-lexicons` shares the same types.

`kryphocron` and `kryphocron-lexicons` are pinned to exact versions. Locus
uses kryphocron as a codec, registry, validation and audit library; it does
its own authentication and authorization.

### Build requirements

- Rust edition 2024, toolchain 1.88 (pinned in `rust-toolchain.toml`).
- `proto-blue-codegen` on `PATH` at build time; `kryphocron-lexicons` runs
  it from its build script. The Dockerfile installs it.
- A C compiler, for the zstd library kryphocron links.
- rustls everywhere. Every proto-blue crate is used with
  `default-features = false`, so native-tls never enters the build.
- No Node.js or JavaScript build step. The web UI is server-rendered.

## 4. Storage

### Model

Locus uses one logical schema with two backends, SQLite and Postgres. Repo
data for every account lives in shared tables keyed by DID: blocks with
reference counts, the repo root, the record index, blob metadata, backlinks,
the event outbox and the numbered event log, accounts and sessions, OAuth
state, settings, leases and job state.

The reference keeps one SQLite file per account plus several shared files.
Locus does not, for two reasons. A commit, its record index changes and its
firehose event must be written in **one transaction**, which can't span
files. And per-account files can't be shared between nodes.

### Transactions and write serialization

- **One repo write is one transaction.** It covers the new MST blocks, the
  repo root, record and backlink changes, blob references, kryphocron
  indexes and the outbox row.
- **Every event for a DID is written under that DID's repo lock**,
  including `#identity` and `#account` events that don't touch the repo. So
  a DID's events reach the outbox in the order they happened.
- **No network calls inside a write transaction.** Calls to PLC, prism,
  scanners and other PDSes happen before the transaction opens or after it
  commits, with compensation where needed. For example, if account creation
  fails after its DID was registered with PLC, a repair job tombstones it.
- **Per-repo locking.** On Postgres, a transaction-scoped advisory lock on
  the DID, then `SELECT … FOR UPDATE` on the repo root. On SQLite,
  `BEGIN IMMEDIATE`, which takes the database's single write lock.
- **Short transactions.** On SQLite every writer shares one lock, so no
  transaction holds it for long. Bulk work on one account (`importRepo`,
  account deletion, `repo rebuild`) is never one big transaction. It runs as
  a **persisted, resumable job** in batches. The job blocks other writes to
  that account while it runs (they get a retryable 503 `RepoWriteBlocked`
  with `Retry-After: 5`), holds its own lease so a paused node can't commit
  after losing it, and records its phase and cursor with each batch so a
  crash resumes where it stopped.

SQLite runs in WAL mode with `synchronous=NORMAL`, a 5-second busy timeout
and foreign keys on.

### Blob backends

Each stored blob copy has its own key with a generation suffix, so a copy
is never overwritten in place. There are three stores: normal, gated (for
private-only media when the media gate is on) and quarantine (taken-down
or scanner-matched blobs).

- **Disk:** under the data directory, as `blobs/<did>/<cid>.<gen>`,
  `gated/…`, `quarantine/…`. Writes go to a temp file, are fsynced, then
  renamed.
- **S3-compatible:** the same keys, with a custom endpoint and path-style
  addressing for services such as MinIO, R2 and B2, and an optional
  separate bucket for gated media. Large uploads go up in parts.

Exactly one backend is active. `aurora-locus migrate blobs` copies every
blob to the other backend while the old one keeps serving, in repeated
passes, then verifies and cuts over. The cutover pauses only new key writes
(uploads get a retryable 503) for a pass bounded by
`blobs.cutover_max_pause_ms` (default 60 s); posts, deletes and imports keep
working. The old backend's objects are left for the operator to delete.

### Migrations

- Migrations are embedded (`sqlx::migrate!`), one tree per backend, and
  applied at startup under a migration lock.
- A node refuses to start against a schema newer than it knows, so an old
  node can't run against a new schema during a rolling upgrade.
- Migrations are forward-only.

## 5. Repo engine

### Commit pipeline

Every record write ends in the same commit function: `createRecord`,
`putRecord`, `deleteRecord`, `applyWrites`, the kryphocron procedures,
account creation and rotation rewrites. (`importRepo` has its own staged
path, below.)

1. **Prepare, outside the transaction.** Resolve `$type`, validate, extract
   blob references, run kryphocron preparation for kryphocron NSIDs, encode
   to DAG-CBOR and compute the CID. Size limits apply here.
2. **Open the transaction and take the repo lock.**
3. **Swap checks.** `swapCommit` must match the current commit and
   `swapRecord` the current record, or the write fails with `InvalidSwap`.
4. **Op checks.** Ops are checked in order against the record index with
   the batch's earlier ops applied. A create on an existing key, or an update
   or delete of a missing key inside `applyWrites`, is a 400 with a specific
   message (the reference answers 500). Newly referenced blobs are claimed
   here, under the lock: each must exist, not be taken down, and match the
   record's MIME type and size.
5. **MST update.** Load only the affected paths and apply the ops. The
   result gives the new data root, the nodes added, the nodes no longer
   referenced, and covering proofs for every changed key.
6. **Verify.** Check, from the diff alone, that the new tree's leaves equal
   the old leaves with the ops applied. A mismatch aborts with `InvalidMst`
   before anything is written.
7. **Commit object.** A v3 commit with `prev: null`, signed with the
   account's ES256K key.
8. **Write.** Update block reference counts, the repo root, records, blob
   references and backlinks, and promote newly referenced blobs from temp to
   permanent.
9. **Outbox.** Insert the `#commit` event: `rev`, `since`, `prevData`,
   per-op `prev` CIDs, and a CAR slice holding the commit, new MST nodes,
   written records and the covering proofs. If those blocks exceed 2 MB, the
   write fails with the reference's "Too many writes. Max event size: 2MB".
10. **Commit**, then wake the sequencer.

Because the MST loads lazily, the cost of a commit depends on the paths it
changes, not on the size of the repo.

### Blocks, revisions and clocks

- Blocks are reference-counted per repo by locus's own accounting, so two
  records that share a CID are counted twice. A block whose count reaches
  zero is deleted by a GC job after 10 minutes, not inline; a block
  re-referenced in the meantime survives.
- Each block records the rev that introduced it, so `getRepo?since=` can
  return only newer blocks.
- A new `rev` is always greater than the stored one: locus takes the larger
  of "now" and the stored rev plus one, so a clock step backwards can't make
  revs go back.
- Each node leases a 10-bit TID clock id at startup, so two live nodes never
  mint the same TID. A node that can't get or keep one stops minting.

### CAR export and import

- **`getRepo`** streams a CAR in pages: the commit as root, then every live
  block of the repo (or only those introduced after `since`). Memory use
  stays bounded however large the repo is.
- **`importRepo`** applies the uploaded CAR as a diff onto the account's
  existing repo, as the reference does. Blocks the CAR leaves out are taken
  from the current repo. Blob references are recorded but not checked,
  because during a migration the blobs usually arrive afterwards;
  `listMissingBlobs` reports what is still missing.

The import runs as a resumable job:

1. **Staging.** Stream the upload to a temp file under a size cap, read it
   with a streaming CAR reader, and walk the tree with a bounded walker (a
   visited set, a depth bound, a block-count bound). A tree that references
   a node twice, or has keys out of order or on the wrong layer, is refused.
   Compute the record diff and per-block count changes.
2. **Increments.** Add references for blocks and blobs the new tree needs.
   The old repo stays fully readable.
3. **Swap.** One short transaction moves the repo root to a freshly signed
   commit over the imported data, with a new rev, and writes a `#sync`
   event if the account is active.
4. **Reindex.** Update the record index. The `importRepo` request returns
   here.
5. **Decrements.** Release blocks and blobs only the old tree used.

A failure before the swap rolls everything back; after the swap the job
only goes forward. Writes to the account stay blocked until the job ends.
The job doesn't depend on the connection: if a proxy cuts a long upload,
the job carries on. `app.nearhorizon.locus.account.getImportStatus` reports
its phase and progress, and the account portal's migration flow polls it.

### Limits

| Limit | Default | Setting |
|---|---|---|
| JSON body for record writes | 2,000,000 bytes (reference: 1,000,000) | `limits.repo_write_body` |
| Other JSON bodies | 150 KB | `limits.json_body` |
| One record (CBOR) | 1,100,000 bytes (reference: none) | `limits.record_size` |
| Ops per `applyWrites` | 200 | fixed |
| `#commit` event blocks | 2,000,000 bytes | fixed |
| `importRepo` upload | 1 GiB (reference: none) | `limits.import_size` |

The record-write limit is higher than the reference's because a maximal
private post, base64-encoded in JSON, is about 1.34 MB. Operators can lower
any of these.

## 6. Records and validation

At startup locus builds one lexicon registry from two sources: the
reference PDS's curated list of known schemas, vendored in the repo
(`com.atproto.*`, `app.bsky.*`, `chat.bsky.*`, `tools.ozone.*` and the
third-party sets the reference includes), and the `tools.kryphocron.*` set
from `kryphocron-lexicons`. Lexicons are never fetched from the network for
record validation.

The `validate` flag works as in the reference:

| `validate` | Known NSID | Unknown NSID |
|---|---|---|
| `false` | not validated; no `validationStatus` | no `validationStatus` |
| unset | validated: `valid`, or 400 | accepted, `validationStatus: "unknown"` |
| `true` | validated | 400 "Unknown lexicon type" |

On top of proto-blue's validator, locus checks:

- the record's `$type` matches the collection;
- `unknown`-typed objects carry a `$type`;
- the data model for every record, known or not: an object at the top
  level, no floats, safe integers, strict DAG-CBOR, and complete blob refs
  (legacy blob refs are refused);
- record-key syntax, and the schema's key type for known lexicons;
- blob references point at uploaded blobs with matching MIME type and
  size (record writes only; imports record them unchecked).

kryphocron NSIDs get additional host rules, and private-tier procedures are
always validated, whatever `validate` says.

Reference special cases are kept: `createRecord` replaces the account's
earlier `app.bsky` follow, block, like or repost of the same subject in the
same commit; an identical `putRecord` and a `deleteRecord` of a missing
record commit nothing.

Records are held in memory as proto-blue's `LexValue` and encoded to CBOR
only from it. Generated serde types are used for JSON input and output,
never for CBOR, because they mis-encode CIDs and bytes.

## 7. Sync and the firehose

### Outbox and sequencing

Every change that produces an event writes an **outbox** row in the same
transaction as the change. A single **sequencer** (a leased, fenced job)
moves outbox rows into the numbered event log in batches, giving them
increasing, gapless sequence numbers, and deletes them from the outbox in
the same transaction.

Why not number events in the write transaction? On Postgres, concurrent
transactions can commit out of order, so a consumer could see seq 101
before 100 commits and skip 100 forever. One sequencer removes that race.

Per-DID order holds because every event for a DID is written under that
DID's lock. Writers wake the sequencer directly (in-process, or Postgres
`NOTIFY` across nodes).

Event kinds are `#commit`, `#sync`, `#identity`, `#account` and `#info`,
with the reference's shapes. Account creation emits `#identity`, `#account`,
`#commit` and `#sync` together.

### Firehose server

- `com.atproto.sync.subscribeRepos` runs on every node. Each node tails the
  event log and fans new events out to its subscribers.
- **Cursors:** no cursor streams live; a cursor ahead of the log gets
  `FutureCursor`; a cursor older than retention gets `#info OutdatedCursor`
  and then the oldest retained event; otherwise the subscriber is backfilled
  from the log in pages and then switched to live.
- **Slow consumers:** a subscriber more than `sync.subscriber_buffer`
  events (default 500) behind live gets `ConsumerTooSlow` and is closed.
- **Heartbeat:** a WebSocket ping every `sync.ws_ping_interval_s` (default
  30 s), so proxies with idle timeouts don't cut quiet subscribers. The
  reference sends no pings.
- **Connection limits:** `sync.max_subscribers` (default 1000) per node and
  `sync.max_subscribers_per_ip` (default 20).

### Retention and deletion

- Events are kept for `sync.retention_hours` (default 72; the reference
  keeps 24). A backfilling subscriber that falls below what has been pruned
  is told with `#info OutdatedCursor`, never silently skipped ahead.
- On account deletion, locus emits `#account` with status `deleted` and
  removes that DID's earlier events, as the reference does.
  `sync.deleted_history_preservation_days` (0 to 90, default 0) can keep
  them for a while first.

### Crawlers

On new events, locus calls `com.atproto.sync.requestCrawl` on each relay in
`sync.crawlers`, at most once every 20 minutes per relay. A call that falls
inside the window is scheduled for its end rather than dropped. (The
reference has one global window and drops calls inside it.) Setup adds
`https://bsky.network` unless `PDS_CRAWLERS` is set or the hostname is
local. The console and CLI can request a crawl at once.

### Sync reads

`getRepo`, `getRecord` (with a proof, or a proof of absence), `getBlocks`,
`getLatestCommit`, `getBlob`, `listBlobs`, `getRepoStatus`, `listRepos`,
and the deprecated `getHead` and `getCheckout` behave as the reference. A
taken-down, suspended or deactivated repo is refused to everyone but its
owner and operators; `getRepoStatus` always answers.

## 8. Blobs

### Upload

`uploadBlob` streams the body to temp storage while computing its SHA-256
and enforcing the size cap, then sniffs the MIME type. As in the reference,
a sniffed type wins over the request's `Content-Type`. Uploads accept user
credentials or user service auth, so a service such as a video service can
upload on a user's behalf. The limit is `blobs.upload_limit` (default
300 MiB, as the distro installer sets).

If a scanner is configured (`blobs.scanner = webhook`, at
`blobs.scanner_url`), it sees each upload before the blob can be used. A
match is refused, its bytes kept in quarantine, and it is queued for review
in the console. When the scanner is down, `blobs.scan_unavailable` decides:
`refuse` (the default; a retryable error) or `accept_unscanned` (stored
and scanned later). The scanner is optional unless the private media gate
is on.

### References, promotion and moves

A blob starts as **temp**. The first commit that references it makes it
**permanent**, inside the commit's transaction. Moving bytes between
stores (normal, gated, quarantine) uses a three-step protocol: record the
new key, copy and verify the bytes, then switch the row to the new key and
queue the old one for deletion. A crash at any step leaves the row pointing
at a complete copy. A daily sweep queues any stored object that no row has
named for over a day.

### Garbage collection

GC never deletes bytes directly. It deletes metadata rows and queues their
keys; a delete job removes queued keys after `blobs.delete_grace_ms`
(default 5 minutes), so reads in flight can finish.

- Temp blobs never referenced within `blobs.temp_ttl_ms` (default 6 hours)
  are collected. The reference never collects them.
- Permanent blobs whose last reference was removed are collected after
  `blobs.unreferenced_grace_ms` (default 1 hour), so a quick
  delete-and-recreate keeps the blob. The reference deletes them at once.
- Quarantined, taken-down and held blobs, and blobs mid-move, are never
  collected.

### Serving

`getBlob` streams the blob with the reference's safety headers
(`x-content-type-options: nosniff`, `content-security-policy: default-src
'none'; sandbox`, `content-disposition: attachment`). Locus adds `Range`
support and a `/blob/<did>/<cid>` alias. There is no `Cache-Control` by
default, as in the reference, because a cached copy would outlive a
takedown.

Takedowns are checked on every request and apply at once, before the bytes
have moved to quarantine. A taken-down blob answers "Blob not found" to
everyone. With the private media gate on, gated media is served only to
those allowed to see it; to anyone else it looks exactly like a missing
blob.

## 9. HTTP and XRPC

### Router

One axum router per node serves XRPC (`/xrpc/<nsid>`), OAuth, the
well-known documents, the web UI, the admin API, health and metrics.

Routing looks at `Host` first. The PDS hostname and internal names get
every route. Handle hosts and hosted did:web hosts get only their
well-known documents. A prepared new hostname gets only `/tls-check` and
health; a former hostname keeps serving its `did.json` and redirects the
rest. Anything else is 404.

XRPC dispatch is a table of local handlers by NSID. Anything not in the
table goes to pipethrough. Errors use the `{error, message}` shape with the
reference's error names. A missing or malformed parameter or body is a 400
`InvalidRequest`, never a plain-text error.

Every request has a deadline, `http.request_deadline_ms` (default 60 s); a
request that outlasts it gets a retryable 503. Uploads, imports, repo
exports, blob downloads and the firehose are bounded by their own caps
instead.

### Body limits

Record writes get `limits.repo_write_body`, uploads and imports their own
limits (checked while streaming), everything else `limits.json_body`. A
declared length over the limit is refused before reading; a chunked body is
cut off at the limit. Nothing over the JSON limits is buffered.

### Rate limiting

- The reference's buckets: a global per-IP bucket (`getRepo` has its own),
  login, account creation, password reset, email requests, handle changes,
  uploads, account deletion, and repo-write points per account (create 3,
  update 2, delete 1; `rate_limits.repo_write_hour_points` and
  `_day_points`).
- **On by default** (`rate_limits.enabled`), as the distro installer sets;
  the reference package defaults to off.
- Counters live in each node, in the database, or in Redis
  (`rate_limits.backend`, default `auto`: Redis when the install has one,
  else the database on Postgres).
- Keyed by the real client IP, taken from forwarded headers only when the
  connection comes from a trusted proxy (`http.trusted_proxies`, loopback by
  default), and by DID where relevant.
- Bypass with the `x-ratelimit-bypass` header (its key is a secret in the
  key store: `keys set rate-limit-bypass-key`, or `PDS_RATE_LIMIT_BYPASS_KEY`) or
  by address (`rate_limits.bypass_ips`), with CIDR ranges honoured. The
  reference strips the suffix.
- `RateLimit-*` headers on responses; 429 `RateLimitExceeded` with
  `Retry-After`.

### Pipethrough

Methods without a local handler are proxied, as in the reference:

- The `atproto-proxy: <did>#<service>` header picks the target. The DID is
  resolved and its service endpoint used; the configured AppView's DID uses
  the configured URL without resolution.
- Without the header, the reference's moderation methods go to the
  moderation service, `createReport` to the report service (or the
  moderation service), and everything else, chat included, to the AppView.
- Requests carry a service token from the account (bare-DID `aud`, method
  `lxm`) and the reference's forwarded headers; responses stream back with
  the reference's returned headers. An upstream 500 becomes 502, an
  unreachable service 502 `UpstreamFailure`; other statuses pass through.
- Account-management methods are never proxied, and chat needs full access
  or a privileged app password.
- Targets resolved from a header's DID go through the SSRF-safe client:
  HTTPS and public addresses only, no redirects. Configured services use
  the operator's client. Timeouts, response size, HTTP/2 and retries follow
  the reference's `PDS_PROXY_*` settings (`proxy.*`).
- `getRecord` for a repo not hosted here is answered by the AppView.

The private tier has its own AppView setting (`appview.private_url`,
`appview.private_did`), for a prism that doesn't serve public reads. With it
empty, the private tier is off.

### Local app.bsky handlers

- `actor.getPreferences` and `putPreferences` with the reference's rules (a
  write replaces the `app.bsky` namespace; personal details need full
  access; a derived `declaredAgePref`). Locus serves preferences even before
  an AppView is configured; the reference doesn't serve them at all then.
- **Read-after-write:** `getProfile`, `getProfiles`, `getTimeline`,
  `getAuthorFeed`, `getPostThread` and `getActorLikes` include the
  requester's own records that the AppView hasn't indexed yet (judged from
  its `atproto-repo-rev`), with `Atproto-Upstream-Lag`.
  `appview.cdn_url_pattern` builds their image URLs.
- `feed.getFeed` asks the feed generator with a token for it;
  `notification.registerPush` and `unregisterPush` go to the AppView or the
  named push service.

### CORS and headers

- XRPC: any origin, preflights answered with the requested headers and a
  one-day max age, as the reference.
- Authenticated responses: `Cache-Control: private` and
  `Vary: Authorization`.
- Web UI pages: a strict content security policy with
  `frame-ancestors 'none'`.

## 10. Scale

### Profiles

Only the backends and the node count change between small and large. The
code paths are the same, so a single SQLite node runs the same logic as a
Postgres cluster.

### Multi-node

- Any node serves any request; no sticky sessions are needed.
- Per-repo write serialization is in the database, so two nodes writing to
  one repo is safe.
- Firehose subscribers can connect to any node.
- Session and takedown checks hit the database on every request. Settings
  changes reach every node through `NOTIFY` and a 5-second version poll.
- Rate-limit counters and replay windows (DPoP proofs, service-auth `jti`)
  are shared through Redis when configured, else the database. A node that
  can't reach its configured Redis doesn't start.

### Leases and fencing

A lease is a database row: `(name, holder, epoch, expires_at)`. A node takes
a lease with a conditional update that bumps the epoch, renews it every
third of its TTL (default 30 s), and stops the job if a renewal fails or
doesn't answer in time.

Stopping isn't enough on its own: a paused process can wake mid-transaction
after its lease expired. So every transaction a leased job commits
re-checks, inside the transaction, that it still holds the lease at its
epoch, and rolls back otherwise. Work by a stale holder never commits.

If the sequencer's node dies, another node takes the lease and resumes from
the stored sequence state, adding at most one lease TTL of latency.

### Background jobs

Leased jobs include the sequencer, crawler notification, block GC, blob
moves, placement, deletes, GC and the orphan sweep, scanner rescans,
event-log retention, rate-limit pruning, the bulk-job resumer, `deleteAfter`
enforcement, kryphocron rotation and audience sync, audit retention,
identity repair, PLC key checks, hostname migration, scheduled backups and
the entryway's shard outbox.

Jobs are idempotent and resumable. The console shows each job's last run,
next run and last error. `jobs pause <name>` stops one within seconds and
`jobs run <name>` runs it now.

### Capacity

The design targets, not yet benchmarked:

| Shape | Target |
|---|---|
| Small | Up to about 1,000 accounts on 1 vCPU / 1 GB RAM |
| Large (one PDS) | 100,000 to 1,000,000 accounts on Postgres with 3+ nodes |
| Entryway | Millions of accounts across many shards |

The 2 MB event limit is reached only by a single huge write, never by repo
size.

### Entryway mode

The reference PDS can run as a **shard** behind an **entryway**: a front
door that owns accounts, logins, OAuth and handles for many PDS instances.
Bluesky's entryway isn't published, so locus implements both sides: the
shard side to the reference's contract, and its own entryway that speaks
that contract.

**Topology.** Each account's DID document names its shard as the PDS.
Clients, relays and AppViews talk to the shard for everything repo-related;
the shard forwards account-management calls to the entryway. The entryway
is the OAuth server and the account UI. Each shard keeps its own repos,
blobs, firehose and private audience data, and relays crawl each shard
directly.

**Shard side.** A shard is configured with the entryway's URL and DID
(`entryway.url`, `entryway.did`), its token verification key, its PLC
rotation key and an admin token. It accepts the entryway's ES256K access
tokens (legacy and OAuth). It forwards session and account-management calls
to the entryway, re-signing the ones whose bearer or DPoP tokens can't be
forwarded as service auth from the account. It applies the entryway's
status and handle callbacks and sequences the matching events. For account
creation it requires a genesis operation signed by the entryway's PLC
rotation key, then submits it to PLC itself. Behind a non-locus entryway, a
locus shard behaves as the reference shard does.

**Entryway side.** The entryway holds account records, sessions, app
passwords, invites, the OAuth server, handle resolution and the PLC
rotation key. `shard add` registers a shard; new accounts go to the shard
with the most room for its weight. `shard weight` and `shard drain` steer
placement.

**Revocation.** A reference shard checks entryway tokens statelessly, so a
revoked session works until its token expires. A locus entryway adds a
session id to every access token and pushes every revocation (logout,
password reset, app-password revoke, OAuth revoke, takedown) to the
account's shard through a durable per-shard outbox, delivered in order and
retried until acknowledged. The shard checks revocations on every request.
Entryway token-key rotation is pushed the same way, with an overlap period
so tokens signed with the old key keep working while shards catch up.

In 0.1, a shard can't change its hostname and can't move to a different
entryway; its accounts move with the standard account migration instead.

## 11. Observability

- **Logs:** `tracing`, JSON or human-readable, with the level live from
  settings (`log.level`). Auth headers and private-post content are never
  logged. Every 500 is logged.
- **Metrics:** Prometheus at `/metrics`: requests and latency by NSID,
  commit and MST-verify time, sequencer lag, firehose subscribers and drops,
  job runs, rate-limit hits, blob bytes, database pool stats, kryphocron
  encodes, scanner calls and gated media fetches. Readable from
  `metrics.allowed_ips` (loopback by default) or with the metrics token (a
secret: `keys set metrics-token`); off
  with `metrics.enabled`.
- **Health:** `/xrpc/_health` (503 when the database doesn't answer),
  `/healthz` for liveness and `/readyz` for the database and blob store.
- **Backups:** `aurora-locus backup create` takes an online, consistent
  snapshot of a SQLite install and copies exactly the blobs that snapshot
  names, with an encrypted key backup; it can be scheduled
  (`backups.every_hours`, `backups.keep`). `backup restore` runs only into
  an empty install and moves the firehose sequence past anything relays may
  already have seen. On Postgres, operators use point-in-time recovery,
  then `backup post-restore` to reconcile blobs and the firehose sequence.
- OTLP trace export is planned, not built.

## 12. Differences from the reference PDS

Locus matches the reference unless a difference is listed here. Each one
is deliberate.

### Storage and repos

| Reference | Locus |
|---|---|
| One SQLite file per account, plus shared files | One schema, shared tables, SQLite or Postgres |
| Firehose events written to a separate database, not atomic with the commit | Event outbox written in the commit's transaction |
| `importRepo` runs as one transaction | A resumable, batched job; writes blocked until it ends |
| `importRepo` stores the uploaded commit | Re-signs a fresh commit with a newer rev, so revs never go backwards for relays |
| `importRepo` emits nothing | Emits `#sync` for an active account |
| MST conflicts in `applyWrites` return 500 | 400 `InvalidRequest` with a specific message |
| Record-write bodies up to 1,000,000 bytes | 2,000,000 bytes, to fit a maximal private post |
| No per-record limit | 1.1 MB per record |
| No default import size limit | 1 GiB |

### Sync and blobs

| Reference | Locus |
|---|---|
| No firehose pings | WebSocket ping every 30 s |
| 24 h event retention | 72 h by default |
| One global 20-minute crawl window; calls inside it dropped | A window per relay; calls inside it scheduled |
| Never-referenced uploads kept forever | Collected after 6 hours |
| Dereferenced blobs deleted at once | Deleted after a 1-hour grace |
| `getBlob` only | Adds `Range` support and `/blob/<did>/<cid>` |

### HTTP, auth and operations

| Reference | Locus |
|---|---|
| Rate limits off by default (package) | On by default |
| CIDR suffix stripped from bypass IPs | CIDR ranges honoured |
| Forwarded headers trusted from every internal address | Trusted only from named proxies; loopback by default |
| No preferences without an AppView | Preferences always served locally |
| Legacy access tokens survive logout, password reset and app-password revoke until expiry | Sessions checked on every request; revocation applies on the next one |
| Password reset doesn't revoke OAuth grants | It does, as does account deletion |
| No replay check on inbound service auth | Procedures need a single-use `jti` |
| With no SMTP, email features silently "succeed" | They say "Email is not configured on this server" until a sender passes its check |
| `requestPasswordReset` reveals whether an address has an account | Same answer either way |
| `signPlcOperation` doesn't check the fields it signs | Validates rotation keys, services and `alsoKnownAs` |
| A replaced PLC rotation key is forgotten | Kept as a secondary signer for accounts whose documents still list it |
| Signing keys stored as raw files | Every secret sealed under one master key (XChaCha20-Poly1305) |
| Any configured `http://` endpoint accepted | Plain HTTP must be allowed per endpoint (`http.allow_insecure_http`); env values allow themselves |
| No did:web accounts; no `/.well-known/did.json` | Creates did:web accounts and serves their documents |
| `/tls-check` only in the distro wrapper | Built in |
| `deleteAfter` never enforced | Accounts deleted once it passes |
| A shard waits for token expiry to revoke | A locus entryway pushes revocations to locus shards |
| Config read once at boot | Runtime settings change live, from the console or CLI |
