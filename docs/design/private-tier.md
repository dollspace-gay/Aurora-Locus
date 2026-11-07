# The private tier

aurora-locus has built-in support for kryphocron, the private-post
protocol. With the private tier on, an account can write private posts
shared with an audience, private blocks and mutes, and audiences. Locus
stores private posts encoded, keeps audiences off the network, can gate the
media those posts use, and re-encodes posts on a schedule. It keeps working
in a reduced form when the private AppView (prism) is down.

This document is for operators deciding whether to turn the private tier on,
and for developers of clients and AppViews that integrate with it.

## In short

- Private posts are **encoded, not encrypted**. The encoding is friction
  against casual reading. It is not end-to-end encryption, and it does not
  keep content from the PDS operator or the private AppView. Don't rely on
  it to keep a post secret from someone who has a copy of the record.
- Locus **never decodes for readers**, the author included. Readers get
  decoded posts from the private AppView, which checks the audience.
- **Audiences are hidden.** Who is in an audience is never written to a
  repository, the firehose or a CAR file. Only the audience URI on each post
  is public.
- **Refusals fail closed.** When locus can't prove a writer or reader is in
  an audience, or can't confirm there is no block, the answer is no.
- The private tier needs its own AppView setting. With that setting empty,
  the private tier is off.

## Who can read what

| Party | Private post content | Audience membership |
|---|---|---|
| The author | Through the private AppView, like any reader | Yes, their own audiences |
| Audience members | Through the private AppView | No (only "am I in it?", indirectly) |
| Anyone else on the network | The encoded record is public, on every read path and on the firehose. Encoding is not encryption, so don't treat it as secret. | No; the audience URI only |
| The PDS operator | Yes: the stored record and the codec are both on the host | Yes: the hidden audience store is on the host |
| The private AppView | Yes: it decodes for readers | Yes: it is the source of truth |
| Moderation services | Locus doesn't decode for them. A service with the codec decodes the record itself. Gated media goes to trusted services raw. | No |

Treat private posts as content that is hard to read in passing, not as
content that is safe from a determined reader.

Rotation (below) changes the stored bytes over time. It does not remove
earlier versions from firehose history or from relay archives.

## Turning it on

### Settings

| Setting | Env | Default | Meaning |
|---|---|---|---|
| `appview.private_url` | `LOCUS_PRIVATE_APPVIEW_URL` | empty | The private AppView (prism). Empty: no private tier. |
| `appview.private_did` | `LOCUS_PRIVATE_APPVIEW_DID` | empty | The private AppView's DID, the audience of the service tokens locus sends it. |
| `kryphocron.private_tier_enabled` | — | on | Host switch. Effective only with a private AppView set. |
| `kryphocron.block_appview_url` / `_did` | — | empty | Where `app.bsky` blocks are checked for a reply to an author on another PDS. Empty: the public AppView. |

The private AppView is separate from the public AppView (`appview.url` and
`appview.did`). It may name the same service, but it has to be entered
explicitly. Locus never falls back to the public AppView for private-tier
calls. The setup wizard's private posts step and the console's services
page both set it.

### The gates

Every private-tier write other than a delete passes these gates, in order:

1. **A private AppView is configured** and **the host switch is on**.
   Otherwise the write fails with `UnsupportedNamespace`. Reads of existing
   records keep working.
2. **The operator hasn't blocked the account** from the private tier (a
   per-account switch in the console). Otherwise `Forbidden`. A local block
   can only take away; it can't grant what the private AppView hasn't.
3. **The private AppView permits the account.** Otherwise `Forbidden`.

"Private-tier writes" covers `tools.kryphocron.feed.postPrivate`,
`graph.block`, `graph.mute` and audiences.

**How permission is learned.** Locus calls
`app.nearhorizon.actor.getMembership` on the private AppView, as the
account, using service auth. That method answers for the caller only and
may enrol an eligible caller, so locus calls it only when the account itself
starts a private-tier action: its first private-tier write, or a write after
the cached answer has expired. The answer is cached for the `validForSecs`
it carries. A write needs `permissions.privateIndexed` to be true.

- A 429, a 5xx or a timeout gives a retryable error and changes nothing.
- `AccountTakedown` from any call to the private AppView marks the account
  taken down there for an hour, and writes are refused.
- If the private AppView is unreachable, the last answer stands, expired or
  not. An account that never got an answer is refused until it's back.
- Pointing locus at a different private AppView (a different DID) discards
  the cached answers and the audience copy. The next action asks the new
  service.

**Deletes are never gated.** A user can always delete their own private
posts, blocks, mutes and audiences: with the tier off, with no private
AppView, with permission lapsed or refused, and with the local block set.
Ownership checks still apply.

## Records and procedures

All NSIDs below are relative to `tools.kryphocron.`.

| NSID | Kind | Tier | What locus does |
|---|---|---|---|
| `feed.postPublic`, `feed.like`, `feed.repost` | record | public | Normal records |
| `feed.threadgate` | record | public | Host rules (below) |
| `feed.postPrivate` | record | private | The write pipeline (below) |
| `graph.block`, `graph.mute` | record | private | Normal records with host rules, behind the gates. Not confidential. |
| `policy.audience` | record schema | private | Never stored in the repo; kept in the hidden audience store |
| `feed.createPostPrivate` | procedure | — | A private post under one of the writer's audiences |
| `feed.updatePostPrivate` | procedure | — | Changes only `audienceList` |
| `feed.deletePostPrivate` | procedure | — | Delete, never gated |
| `actor.participatePrivate` | procedure | — | A private reply under the parent's audience |
| `policy.manageAudience`, `policy.deleteAudience` | procedure | — | Audience writes, through to the private AppView |

The kryphocron registry decides each collection's tier. A
`tools.kryphocron.*` collection the registry doesn't know is refused on
every write path. The procedures need user auth, and `repo` must be the
authenticated account.

`com.atproto.repo.createRecord`, `putRecord`, `deleteRecord` and
`applyWrites` on a kryphocron collection run the same checks as the matching
procedure. A `feed.postPrivate` with `reply` set runs the
`participatePrivate` checks.

`updatePostPrivate` keeps `encodedContent` byte for byte; only the
record's CID changes. It
supports `swapRecord`. A reply's audience is its thread's, so it can't be
changed.

## Host rules

These apply on every write path, whatever `validate` says:

- **Record keys** on kryphocron record collections must be TIDs.
- **`feed.postPrivate`:**
  - exactly one of `text` and `encodedContent`; a codec name or generation
    mark without `encodedContent` is refused;
  - `encodedContentCodec` must name the codec locus supports;
  - `embedBlobs`, `mentionDids`, `quote` and `mediaKinds` are always derived
    by locus from the content, replacing whatever the client sent;
  - `audienceList` must name one of the writer's own audiences (or, for a
    reply, match the parent's);
  - `publicCompanion`, if present, must point into the writer's repo. It is
    not resolved, and deleting it doesn't cascade.
- **`feed.threadgate`:** its record key is its post's, the post is in the
  writer's repo, and a `listRule` names an `app.bsky.graph.list`. An
  audience URI there would match nobody, so it's refused.
- **`policy.audience`:** `mode` is one of `list` (the default when absent),
  `everyone`, `followers`, `following` or `nobody`. `members` are DIDs of at
  most 256 bytes, de-duplicated, with the owner dropped, and are required
  for `list`. `name`, when present, is 1–64 graphemes and at most 640 bytes
  after trimming. The body is at most 512 KiB. At most
  `kryphocron.audience_max_members` members (default and maximum 1,000) and
  `kryphocron.audience_max_per_account` audiences (default and maximum 200).
  These are the private AppView's own limits; locus applies them first, so
  a write should only fail there for state locus can't see. The caps can be
  lowered, never raised.

**Finding the writer's audience.** Clients often create an audience on the
private AppView directly, so a post can name an audience locus hasn't seen
yet. When the URI is missing from the writer's copy, locus refreshes the
copy from the private AppView first, paging until it finds the URI or the
listing completes. The write is refused with `InvalidRequest` only when a
complete listing lacks the URI. A refresh that fails, stops early or is
throttled gives a retryable error instead.

## Writing a private post

Used by `createPostPrivate`, `participatePrivate`, and generic writes to
`feed.postPrivate`.

**Plaintext submission** (`text`, `facets`, `embed`, no `encodedContent`):

1. The record key is chosen first (a new TID if none was given), because the
   encoding is bound to it.
2. The plaintext form is validated against the lexicon.
3. The clear fields are derived and set.
4. The content is encoded with the kryphocron codec, under the current
   rotation generation. The encoded bytes must be at most 1,000,000.
5. `text`, `facets` and `embed` are removed. `encodedContent`,
   `encodedContentCodec` and `encodedContentGeneration` are set.
   `createdAt`, `audienceList`, `reply`, `langs`, `labels` and
   `publicCompanion` stay in the clear, as the lexicon says.
6. The commit checks every referenced blob exists (`BlobNotFound`
   otherwise).

**Pre-encoded submission** (`encodedContent` set):

1. The codec must be the one locus supports.
2. Locus decodes it once, with a 1,000,000-byte bound, to check it. The
   client must name the record key it encoded for; content encoded for
   another key doesn't decode, and is refused with `InvalidRequest`.
3. The decoded `text`, `facets` and `embed` are validated exactly as a
   plaintext submission's.
4. Locus re-encodes the content itself, so the generation mark is the
   host's. The response returns the stored record's CID.

If the rotation state on a node is stale, encoding fails closed with a
retryable error rather than writing an old generation.

**Plaintext handling.** Plaintext exists only in memory for the request.
Locus never stores it, logs it, or puts it in an error message.

## Private replies

`participatePrivate` (or a generic write of a `feed.postPrivate` with
`reply`) writes a reply under the **parent's** audience. Locus checks:

- `reply.parent` resolves (from the local index, or `getRecord` on the
  parent's PDS) and is a `feed.postPrivate`;
- the reply's `audienceList` equals the parent's;
- neither the writer nor the parent's author blocks the other;
- the root's `tools.kryphocron.feed.threadgate`, if any, allows the writer;
- the writer is in the parent's audience (below).

Every refusal, whatever the reason, is the same `Forbidden`, so a refusal
doesn't reveal why.

**Blocks** count in either direction, through `tools.kryphocron.graph.block`
or `app.bsky.graph.block`. For accounts on this host, locus reads its own
index. For a parent author on another PDS:

- `app.bsky.graph.block`: `app.bsky.graph.getRelationships` on the
  block-source AppView (`kryphocron.block_appview_*`), or the public
  AppView when that is empty, called as the writer. It is never asked of the
  private AppView. With neither configured, or on any error, the reply is
  refused.
- `tools.kryphocron.graph.block` by the author: `listRecords` on the
  author's PDS. A scan past `kryphocron.remote_block_scan_max` (default
  50,000 records), or one that fails, refuses the reply.

Block answers for a remote pair are cached for 10 minutes.

**Threadgates.** A root with no threadgate allows any reply that passes the
other checks. A threadgate that can't be fetched refuses the reply. A
`mentionRule` on a private root uses the root's clear `mentionDids`. A
`followingRule` checks whether the root's author follows the writer: from
this host's index, or for an author on another PDS by asking the same
AppView as the block check (never the private AppView). A `listRule` fails
closed, since list membership isn't indexed here.

## Audiences

### Kept off the network

An audience is a `tools.kryphocron.policy.audience` value, addressed by the
usual AT-URI form, so `audienceList` on posts looks the same as ever. The
URI is an identifier only; it doesn't resolve publicly. This departs from
the kryphocron-lexicons description, which describes audiences as repo
records.

- **Writes** make no commit, no firehose event and no repo block. Responses
  carry the URI and the CID of the stored value, so clients written against
  the lexicon still work, and `swapRecord` compares against that CID
  (`InvalidSwap` on a mismatch).
- **Reads:** the owner, authenticated, reads their own audiences with
  `getRecord` and `listRecords`. Everyone else gets `RecordNotFound` from
  `getRecord`, the same as for a missing record, and an empty
  `listRecords`. `getRepo`, `sync.getRecord` and the firehose never include
  them.
- **What stays public:** the audience URI on each post, so it is visible
  which posts share an audience. Membership and mode are hidden. Another
  host can't tell a hidden audience from a deleted one.

### The private AppView is the authority

The private AppView holds the audiences and mints their record keys. Locus
keeps a **hidden copy**, bound to the configured private AppView's DID, and
talks to it only through its existing methods, always with a service token
for the account it acts as:

| Method | Locus uses it for |
|---|---|
| `app.nearhorizon.actor.getMembership` | The write gate (above) |
| `app.nearhorizon.private.listAudiences` | Refreshing an account's copy |
| `app.nearhorizon.private.createAudience` / `updateAudience` / `deleteAudience` | Writing audiences through |
| `app.nearhorizon.private.getPosts` | The read probe (membership, below) |

Locus never asks about any account except the one it acts as.

**Writes are write-through.** While the private AppView is up, locus runs
every local check, then calls the private AppView, then updates its copy
when the call succeeds. The private AppView's answers map to:
`LimitReached`, `InvalidRequest` and `NotReady` → `InvalidRequest` with its
message; `InvalidSwap` → `InvalidSwap`; `Reindexing` → a retryable 503;
429 → a retryable 429; `NotEnrolled` → `Forbidden`, and the account's copy
and cached permission are purged; `AccountTakedown` → `Forbidden`, with the
copy kept.

Some consequences:

- `manageAudience` **without** an rkey creates; **with** an rkey it updates
  an existing audience. A client can't choose an rkey. The same holds for
  `createRecord` on the collection: with an rkey, it is an update of that
  audience (needing `update` permission under OAuth), and refused if no
  such audience exists.
- An `applyWrites` with an audience op must contain only that op, and
  `swapCommit` doesn't apply. A call to another service can't roll back with
  a database transaction.
- Locus stamps `createdAt` and `updatedAt` itself.
- **A create whose answer never arrives** (a timeout after sending) is never
  retried blindly. Locus records the attempt before sending, refreshes the
  copy, and matches the attempt to the audience it made. If no match is
  found, the client gets a retryable error saying the outcome is unknown, so
  it lists its audiences before retrying. No duplicates.
- When a client calls the private AppView's audience methods **through**
  locus (with `atproto-proxy` naming the configured private AppView), locus
  refreshes that account's copy right after a successful write.

**Refreshing the copy.** The private AppView has no change feed, so locus
polls `listAudiences` as each account: in the background every
`kryphocron.audience_poll_interval_s` (default 15 minutes, stretched to stay
within `kryphocron.prism_call_budget`, default 5 calls per second per node),
and on use. Owner-triggered refreshes are limited to one per 5 seconds; those
caused by other accounts' checks to one per
`kryphocron.audience_check_ttl_s` (default 60 seconds) per owner, so another
account can't spend an owner's budget. Every call is bounded by
`kryphocron.prism_request_timeout_ms` (default 10 seconds), and one listing
by `kryphocron.audience_listing_deadline_s` (default 5 minutes). An audience
missing from a listing is treated as deleted only after a complete listing.

### Membership

The same check answers private replies and the media gate. For subject S
and an audience owned by O:

1. S is O → member.
2. **O is on this host:**
   - O's audience missing or deleted (after a refresh) → owner only;
   - mode `nobody` → owner only;
   - mode `list` → member if S is in `members`, checked against a copy no
     older than `audience_check_ttl_s` while the private AppView is up. When
     S is also on this host, S must also be a ready member of the private
     AppView; for a remote S locus can't learn that, and answers from the
     list alone;
   - modes `everyone`, `followers`, `following` → only the private AppView
     knows; see step 4.
3. **O is on another host** and S is on this host: locus asks O's PDS for a
   membership assertion on S's behalf (below). A valid assertion → member.
   Otherwise, step 4.
4. **The read probe**, only for a local S while the private AppView is up:
   locus calls `getPosts` as S for the referencing posts. If the private
   AppView serves a post, S is a member. Anything else fails closed. Locus
   reads only the status and whether a post came back. It discards the body,
   which holds decoded content, at once, and never logs, stores or returns
   it. Probes are limited to 3 per second per subject; hits are cached for
   60 seconds and misses for 10.
5. Otherwise → `Forbidden`.

A reader on another host can only be checked against `list` audiences owned
here. Everything else fails closed for them. Locus doesn't compute follower
graphs.

### Membership assertions

For audiences owned on another locus host, locus uses its own query,
`app.nearhorizon.locus.audience.getMembershipAssertion` (params
`audienceUri` and `aud`, returning `{token}`).

- The subject only ever asks about itself, with service auth from its own
  DID. Its own locus makes that request server-side; no client request
  carries an assertion in.
- The issuer answers only for audiences it holds, from what it can resolve
  itself, and issues only positive answers. A negative answer is the same
  `Forbidden`. Requests are rate-limited per subject and per owner.
- The token is a JWT with header `typ: kryphocron-audience+jwt` and claims
  `iss`, `sub`, `audienceUri`, `aud`, `iat`, `exp` (at most 5 minutes) and
  `jti`. Host identities are always `did:web:<hostname>`.
- It is signed with a dedicated key, published as `#kryphocron_audience` in
  the host's `/.well-known/did.json`, never the service-auth key. Locus's
  service-auth verifier rejects this `typ`, so an assertion can't pass as
  service auth.
- The verifier derives the expected issuer from the owner's DID document
  (its `#atproto_pds` host), never from the token, and checks `typ`, `iss`,
  the signature, `aud`, `sub`, `audienceUri` and expiry (30 seconds of
  skew).

Hosts that don't support this query can't check readers or writers against
a locus audience, and fail closed.

## Private media

Media in private posts is **not encoded**. It stays raw so it can be
hash-scanned. Clients upload it with `uploadBlob` as usual, and posts
reference it through the clear `embedBlobs` field.

**The private media gate** (`kryphocron.private_media_gated`, default
**off**) restricts who can fetch it.

| | Gate off (default) | Gate on |
|---|---|---|
| `getBlob`, `/blob/*` | Public by CID, like any blob | Served only to the owner, audience members of at least one referencing private post, and trusted services. Everyone else gets the same answer as for a missing blob. |
| `listBlobs` | Lists them | Omits them, except for the owner and trusted services |
| Storage | The normal blob store | A separate gated store |
| Outside scanning | Any service can fetch them | Not reachable; locus's own scan is required |
| Scanner | Optional | Required: the gate can't be turned on without `blobs.scanner`, and the scanner can't be removed while it's on |

- **Private-only** means referenced only by the same account's private-tier
  records. A blob any public record references is public. The gate reads the
  blob's state, not where its bytes are, so a blob is gated from the commit
  that makes it private-only.
- With the gate on, a commit that adds a private reference to an unscanned
  blob scans it first. If the scanner can't answer, the write fails with a
  retryable error. A blob that becomes private-only some other way (its
  last public reference deleted, an import, the gate switching on) is gated
  until scanned, never public.
- **Readers** on this host use their session or OAuth token. A reader on
  another PDS presents a service-auth token from their own PDS (`aud` =
  this host's DID, `lxm` = `com.atproto.sync.getBlob`). Membership is the
  check above, with the reader as subject; referencing posts are batched
  into one probe of at most 25.
- **Clients must fetch gated media with an `Authorization` header.** A plain
  `<img src>` can't load it. Turn the gate on only when your users' clients
  do this.
- **Caching:** while the gate is on, gated responses and every refusal
  (missing blobs included, so the two look alike) carry
  `Cache-Control: private, no-store` and `Vary: Authorization`. Proxies and
  CDNs in front of locus must not cache these paths.
- **Trusted services** (`kryphocron.media_trusted_services`): service DIDs,
  such as a moderation service like cairn or a scanning or labeling
  service, that may fetch gated media with service auth, bypassing the
  audience check. Every trusted fetch is audited. AppViews and CDNs can't
  fetch gated media unless listed.
- **Switching** is reversible. Turning the gate off makes gated media public
  by CID again. Copies fetched while the gate was off stay wherever they
  were copied.

## Rotation

Locus re-encodes private posts under a new **generation** on a schedule, so
their stored bytes change over time.

- A new generation starts every `kryphocron.rotation_period_days` (default
  60, range 30–90). Every node reads the current generation from the
  database every 30 seconds. A node whose copy is more than 5 minutes stale
  refuses to encode (retryably) rather than use an old one.
- A single leased job rewrites posts whose generation is older than the
  current one, or dated in the future. It spreads accounts across the period
  so the firehose doesn't spike, and is limited by
  `kryphocron.rotation_max_commits_per_minute` (default 120).
- It writes batched commits (at most 200 ops or about 1.5 MB per commit)
  through the normal write path, with compare-and-swap against the CIDs it
  read, so it never overwrites a concurrent user write.
- For encoded records only `encodedContent` and `encodedContentGeneration`
  change. Every other field, `createdAt` included, is kept byte for byte.
- Only active accounts are rotated. Records it can't decode are flagged one
  by one, skipped, and listed in the console.
- Admins can start a new generation early from the operator API.

## When the private AppView is down

Each node tracks whether the private AppView is up. A call that can't
connect, or times out, triggers an unauthenticated
`GET /.well-known/did.json` on the private AppView. If that also fails, the
node marks it **down** and probes again every 10 seconds; any HTTP answer
marks it up. An AppView that answers with errors, even 5xx or 429, is up:
those are retryable errors, not an outage. Nodes can briefly disagree,
which is harmless. The console shows the state.

**While it's down, the hidden copy is the authority:**

| | During an outage |
|---|---|
| Write gate | The last cached permission stands; an account with none is refused |
| Private posts and replies | Work wherever the copy can answer |
| `list` audiences owned here | Membership answered from the copy, with no staleness cutoff |
| Audiences owned on another locus host | Answered by that host's assertion |
| `everyone` / `followers` / `following` | Fail closed (need the probe) |
| Gated media | As membership above |
| Creating an audience | Creates a **local audience** with a locus-minted URI |
| Editing or deleting an audience | Accepted locally and queued |
| Proxied `app.nearhorizon.private.*Audience` calls | Answered by locus from its copy, in the private AppView's shapes |

**When it's back**, the sync job (leased, at most 2 writes per second per
account, within the call budget):

- **Queued edits:** the newer `updatedAt` wins. A newer local edit is pushed;
  a newer remote version replaces the local one and the dropped edit is
  flagged. Deletes always win. An audience the private AppView deleted
  meanwhile stays deleted.
- **Local audiences** are created on the private AppView, which mints a new
  URI. Locus maps the local URI to the new one; both keep resolving on
  locus.
- **Re-pointing:** a leased job rewrites posts in this host's repos that
  name a mapped local URI, so they name the new one. Only `audienceList`
  changes; the encoded content stays decodable. It is limited by
  `kryphocron.repoint_max_commits_per_minute` (default 120).

Posts naming a local URI are visible only to their author on the private
AppView until they are re-pointed. Records on other PDSes that name a local
URI can't be re-pointed by locus, and stay that way there. Last-writer-wins
depends on the clocks of locus and the private AppView agreeing.

**If the private AppView setting is removed**, the private tier turns off
and the copy is kept, with any queued edits. **If it is changed to a
different DID**, the copy, its queued edits and local audiences are
discarded. Posts naming those audiences fall back to owner only.

## Import and migration

- Records arriving by `importRepo` are stored as imported, since they are
  signed history. A private post that arrives with plaintext `text` is
  encoded by the next rotation pass. Its plaintext stays in the history it
  came from.
- Account migration keeps the DID, so encoded content stays decodable on the
  new host.
- Audiences are not in the repo CAR, so they don't travel with a standard
  migration. With the same private AppView on both hosts, nothing needs
  moving: the new host's copy fills from it. Local audiences not yet created
  on the private AppView, and local URI mappings, stay on the old host.

## Audit

Locus implements kryphocron's audit sinks as a bounded in-memory channel to
a writer task that stores events. A full channel rejects the event; events
that can't be stored go to the log. Correlation ids in audit rows are keyed
by an install secret generated at setup. Events are kept for
`kryphocron.audit_retention_days` (default 365). Rotation generations,
rotation passes and trusted-service media fetches are recorded too.

## Known limits

- The encoding is not encryption (see "Who can read what").
- A removal from a `list` audience on the private AppView reaches locus
  within `audience_check_ttl_s` while it's up; during an outage the copy
  doesn't expire.
- Readers on other hosts can only be admitted to `list` audiences owned
  here.
- At very large scale the background poll interval grows to stay within
  the call budget; on-use refreshes keep active accounts current.
- Gated media needs clients that send an `Authorization` header.
