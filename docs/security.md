# Security in aurora-locus

This document describes the security measures aurora-locus ("locus") takes,
what it trusts, and where its protections stop. It is written for operators
deciding how to deploy locus and for reviewers checking its design.

To report a vulnerability, see [SECURITY.md](../SECURITY.md). Please don't
open a public issue.

Locus is an atproto Personal Data Server (PDS). It aims to behave like the
reference PDS (`@atproto/pds` 0.5.51, Bluesky's implementation) wherever
clients and services can observe it. Where locus is deliberately stricter
than the reference, this document says so.

## Contents

1. [Threat model](#threat-model)
2. [Keys and secrets at rest](#keys-and-secrets-at-rest)
3. [Account credentials](#account-credentials)
4. [Operator console and admin API](#operator-console-and-admin-api)
5. [Network edge](#network-edge)
6. [Outbound requests](#outbound-requests)
7. [Input handling and limits](#input-handling-and-limits)
8. [Blobs](#blobs)
9. [The private tier](#the-private-tier)
10. [Moderation](#moderation)
11. [Failure behaviour](#failure-behaviour)
12. [Dependencies and supply chain](#dependencies-and-supply-chain)
13. [Operator hardening checklist](#operator-hardening-checklist)
14. [Summary of differences from the reference PDS](#summary-of-differences-from-the-reference-pds)

## Threat model

### What locus defends against

- **Remote attackers on the internet.** They can send any HTTP request, to
  any hostname that reaches the server, with any headers. They can also
  control DNS for their own domains, publish DID documents and OAuth client
  metadata, and run their own PDS, AppView or feed generator.
- **Malicious or compromised account holders.** They hold valid
  credentials for their own account and try to reach other accounts, act
  beyond their granted scopes, exhaust resources, or upload harmful
  content.
- **Malicious remote services.** A service that sends service-auth tokens,
  answers proxied requests or serves documents locus fetches may lie, stall
  or send oversized replies.
- **Neighbours on a shared network.** Other hosts or containers on the same
  private network should not be able to impersonate a trusted proxy or
  set the client IP that rate limits use.
- **Lower-privileged operators.** An operator with the viewer or moderator
  role should not be able to perform admin or owner actions.
- **Theft of the database alone.** A copy of the database without the
  master key should not reveal signing keys or other stored secrets.

### What locus trusts

- **The operator who controls the host.** Anyone with root on the machine,
  or with both the database and the master key, controls the PDS. They can
  sign as any hosted account, read every stored record and blob, and read
  account email addresses. Locus doesn't try to protect accounts from the
  operator; no PDS can. Locus does record operator actions in an audit log
  (see [Audit log](#audit-log)).
- **The host and its storage.** Locus assumes the kernel, the filesystem,
  the database server, the blob store and the container runtime are not
  hostile.
- **Configured services.** Endpoints an operator types into settings (the
  PLC directory, the AppView, the private AppView, moderation and report
  services, crawlers, the scanner webhook, SMTP, S3) are trusted to the
  degree their role needs. A configured moderation service, for example,
  can take accounts down. Choose them with care.
- **The reverse proxy you name.** Forwarded client-IP headers are believed
  only from addresses in the trusted-proxy list.

### Out of scope

- Confidentiality of content from the operator, relays or the network.
  Records are public by design in atproto. The private tier's encoding is
  not encryption (see [The private tier](#the-private-tier)).
- Volumetric denial of service. Rate limits and caps reduce abuse from
  single clients; absorbing a flood is the job of the network in front.
- Clients' own security (token storage on devices, browser compromise).

## Keys and secrets at rest

### The master key and envelope encryption

Every stored secret is sealed with XChaCha20-Poly1305 under one 32-byte
master key per install. This covers:

- account signing keys;
- the PDS's PLC rotation keys and its service key;
- the OAuth access-token signing key and the private tier's
  membership-assertion key;
- the legacy session secret, the DPoP nonce secret and the app-password
  lookup and email-token peppers;
- operators' TOTP secrets;
- SMTP and S3 credentials, the hCaptcha secret and other service
  credentials;
- secrets imported from a reference PDS's `pds.env`.

Each sealed value's associated data names the slot it belongs to, so a
ciphertext copied into another slot fails to decrypt.

**Stricter than the reference:** the reference PDS keeps account signing
keys in the database or key files without a separate encryption layer.
Locus never stores a private key in the clear.

### Where the master key comes from

| Source | How | Notes |
|---|---|---|
| File | `<data>/master.key`, 64 hex characters, mode `0600` | Default. Generated only on a fresh single-node SQLite install with a persistent data directory. |
| Environment | `LOCUS_MASTER_KEY` | For secret managers that inject env vars. |
| AWS KMS | `LOCUS_MASTER_KEY_KMS_ID` | KMS generates a data key; the database keeps only the KMS-wrapped copy. Each node unwraps it at boot. |

Locus never generates a key where a generated key could differ between
nodes or between restarts. With Postgres, more than one node, or a
non-persistent data directory, it refuses to start until you supply a key
from env, KMS, or a file you declare shared
(`LOCUS_MASTER_KEY_SHARED_FILE=true`, the same file on every node).

### One key per install, checked before writes

The key's id is derived from the key itself (a keyed hash), never assigned.
The database records that id with a MAC under the key. At boot, before
migrations or any other write, a node checks the record. A node holding a
different key refuses to start and names the source it used.

The boot check can't cover a brand-new install, so every transaction that
writes a secret checks the key again. On a mismatch the transaction rolls
back and the node exits without writing. If a stored secret ever fails to
decrypt, the node re-checks its key; if the key no longer verifies, it
exits.

### Key backups

`aurora-locus keys backup` (and the download during setup) writes every
stored secret, every account signing key and every PLC rotation key with
its role, encrypted under a passphrase you choose:

- passphrase of at least 12 characters, stretched with Argon2id (64 MiB,
  three passes by default);
- XChaCha20-Poly1305 frames, each bound to the backup's header, its index
  and whether it is the last, so frames can't be reordered, dropped,
  truncated or moved between backups without detection;
- KMS-backed keys appear as their KMS key id, not key material.

Setup asks you to download a key backup before it finishes. You can skip it
only with an explicit acknowledgement. **Losing the master key and every
key backup loses the PDS's ability to sign for its accounts.**

`keys import <backup>` reads a backup back (owner only, with
re-authentication, audited). A full restore runs only into an empty install
and refuses a master key that doesn't verify the snapshot.

### Rotation

| What | Command | Effect |
|---|---|---|
| Master key source | `keys rewrap --to file\|env\|kms` (offline) | Re-encrypts every secret under the new key in one transaction. A new key file goes to a new path; the old file is never touched, so a failure before commit leaves everything as it was. Take a new key backup afterwards. |
| Service key | `keys rotate-service-key` | Owner only, audited. |
| Account signing key | `keys rotate-account <did>` | Equivalent of the reference's rotate-keys scripts. |
| PDS PLC rotation key | `keys set plc-rotation-key` | The new key stays pending until a key backup that holds it exists (or you acknowledge the risk). The previous key stays as a secondary signer until `keys drop-plc-key`, which refuses while any account's DID document lists only that key. |
| Legacy session secret, DPoP secret, hCaptcha secret, rate-limit bypass key, metrics token | `keys set <name>` (`--remove` removes the last two) | Read from stdin or a file, never the command line. The audit log records only a fingerprint. Changing the session secret signs every legacy session out. |

## Account credentials

### Passwords

- New passwords are hashed with Argon2id with a random salt.
- Passwords imported from a reference PDS keep their scrypt hashes (the
  reference's parameters) and are rehashed with Argon2id on the next
  successful sign-in.
- Comparisons are constant-time. Login passwords over 512 bytes and new
  passwords over 256 bytes are refused, as the reference.
- `createSession` and the OAuth sign-in form take at least 350 ms whatever
  the outcome, so response time doesn't reveal whether an account exists.
  The console's sign-in spends the same work on an unknown username as on
  a wrong password.

### Legacy sessions: stateful and revocable

The reference's access and refresh tokens are stateless JWTs that stay
valid until they expire. **Locus is stricter:** every authenticated request
checks the session row and the account status in the database. So logout,
password reset, app-password revocation and takedown take effect on the
next request, not when the token expires.

- Access tokens: HS256 under a server secret, 120 minutes, with the session
  id as `jti`.
- Refresh tokens: 90 days, rotated on use, with the reference's 2-hour
  replay grace for client compatibility.
- A taken-down account's sign-in gets only the reference's `takendown`
  scope, which reaches only the endpoints meant for it (such as appeals),
  and its refresh token is not stored.
- Public reads (`getRecord`, `listRecords`, sync reads, `getBlob`) check
  account, record and blob takedown status from the database on every
  request.
- Deactivation doesn't revoke sessions, as the reference, so a user can
  reactivate. Writes and public reads are refused while deactivated.

Accounts imported from a reference PDS keep their sessions: outstanding
reference access tokens are accepted until they expire (at most 120
minutes), with the account-status check but without per-session
revocation. The first refresh issues locus tokens.

### App passwords

App passwords have the reference's shape and its privileged and
non-privileged levels. Each is stored as an Argon2id hash plus a keyed
lookup hash (HMAC-SHA256 under a secret pepper), so sign-in finds the one
candidate to verify. Non-privileged app passwords can't call `chat.bsky.*`
methods, proxy requests or mint service tokens, as the reference. The
account portal refuses app passwords for its own sign-in.

### OAuth

Locus runs its own OAuth authorization server, following the reference:

- pushed authorization requests, PKCE with `S256` only, `response_type=code`;
- DPoP required on every token, with server nonces and a proof replay check
  shared across nodes;
- client metadata fetched through the SSRF-safe client (see
  [Outbound requests](#outbound-requests)) and cached;
- refresh tokens rotated on use; a replayed refresh token revokes the
  grant; a replayed authorization code revokes the token it issued;
- consent records only the scopes the user approved;
- granular scopes (`repo:`, `blob:`, `rpc:`, `account:`, `identity:`,
  `include:`, `transition:*`), checked on every endpoint; a missing scope is
  403 `ScopeMissingError`.

**Stricter than the reference:**

- OAuth access tokens are signed with a dedicated ES256 key, not the legacy
  session secret, so `/oauth/jwks` publishes a real key.
- Password reset, account deletion and takedown revoke the account's OAuth
  grants too. The reference's password reset doesn't.

The sign-in, consent and account pages use CSRF double-submit tokens and
`Sec-Fetch-Site` checks, `frame-ancestors 'none'`, and no-store caching.

### Email codes and sign-in codes

Email tokens (confirmation, password reset, account deletion, PLC
operations, email change, sign-in codes) are `XXXXX-XXXXX`, valid for 15
minutes, one per account and purpose, and stored as a keyed hash.

**Stricter than the reference:**

- `requestPasswordReset` answers the same whether or not the email address
  has an account. The reference reveals it.
- If email isn't configured, features that need email say so ("Email is
  not configured on this server") instead of pretending to send. An account
  with email 2FA can't be silently downgraded; an operator can clear the
  factor from the console, and that action is audited.

### Service auth

Outbound service tokens are ES256K JWTs signed with the account's key,
with a 60-second lifetime and a random `jti`.

Inbound service tokens must have the right `aud` and `lxm`, and a token of
a session, refresh, DPoP or assertion type is refused. **Stricter than the
reference:**

- A token may live at most one hour (plus 60 seconds of skew).
- Tokens for procedures (state-changing methods, including every
  `com.atproto.admin.*` procedure) need a `jti` and work once per issuer,
  remembered until the token would expire. The reference mints a `jti`
  but doesn't reject a replayed token.
- Queries and the idempotent `uploadBlob` may reuse a token until it
  expires, as the reference allows.

### Sign-up controls

- Sign-up is invite-only by default; it can also be open or closed. Invite
  codes count their uses.
- Optional hCaptcha on the web sign-up pages (the account portal and the
  OAuth sign-up page), as the reference. `createAccount` itself is not
  captcha-gated, also as the reference.
- Disposable email domains are refused (an editable list).
- Handles go through the reference's reserved-name list, its slur filter,
  and the names you reserve. Handles and emails are unique across every
  account, including deactivated and taken-down ones.

## Operator console and admin API

### Operators are not accounts

Operators are separate from atproto accounts: their own table, username,
Argon2id password, optional TOTP code app, and one of four roles.

| Role | Can |
|---|---|
| owner | Everything, including operators, API tokens and keys |
| admin | Everything except operators and keys: account deletion, credential changes (password, email, handle), settings, services, limits, jobs, storage |
| moderator | Takedowns and their reversal, invites, `sendEmail`, reports, the local private-tier block, the scanner review queue, and the read-only views moderation needs |
| viewer | Read-only views |

Every `com.atproto.admin.*` method has a minimum role, checked per method.
Locus's own operator methods are grouped by role
(`app.nearhorizon.locus.viewer.*`, `.moderator.*`, `.admin.*`, `.owner.*`),
and a group is refused below its role before the handler parses the body.
Handlers check the role again. Reading unused invite codes needs the
moderator role, because an unused code can create an account.

The console is a client of the same admin API the CLI uses, so its forms
get the same role checks and audit entries.

### Console sign-in and sessions

- Password, then a 6-digit TOTP code when the operator has enrolled a code
  app. Enrolment shows a QR code. A code is accepted once: a step already
  used, or an earlier one, is refused. A code refused because the server's
  and the device's clocks disagree says by how much, to help fix the clock.
- Sign-in attempts (passwords and codes) are rate-limited per address and
  per operator name. These limits hold even when rate limits are turned off
  or a request carries the bypass key.
- Session ids are 256-bit random values. The database stores only a hash.
- Cookies: `HttpOnly`, `SameSite=Strict`, `Secure` when the public URL is
  HTTPS, scoped to `/admin`.
- Sessions end after 30 idle minutes (`admin.session_idle_minutes`) and
  after 12 hours regardless.
- Sensitive actions (managing operators, deleting accounts, key
  operations) need the password and code again if they weren't confirmed
  in the last 10 minutes. API tokens send the password and code with the
  request.
- Every state-changing form needs a CSRF token tied to the session and a
  same-origin `Sec-Fetch-Site`.

TOTP is per operator. Until an operator enrols a code app, the console
shows a banner on every page. Enrol one for every operator.

### The local admin socket

The CLI talks to a running node over a Unix socket at `<data>/admin.sock`
(mode `0600`). Locus checks the connecting process with `SO_PEERCRED` and
accepts only root and the service's own user. Those callers get the owner
role, because local access to the service account already means access to
the key material on disk. Where Unix sockets aren't available, the CLI uses
HTTP with an operator API token instead. API tokens are stored as hashes.

### The reference's admin password

The reference's `admin:<password>` Basic auth works for compatibility with
existing tooling. It covers exactly the reference's admin surface
(`com.atproto.admin.*`, the invite endpoints, and the sync reads the
reference opens to the admin token), and counts as the admin role there. It
can't reach settings, keys, operators or backups. Turn it off with
`admin.basic_auth_enabled` if you don't need it. When `PDS_ADMIN_PASSWORD`
isn't set, locus generates the password.

### Audit log

Every operator action and every moderation action is appended to the audit
log: who, what, when, and for settings the old and new values. Operators
can filter it in the console and export it as JSON lines. Key
changes record a fingerprint, never the key. The private tier keeps its own
audit events, with correlation ids keyed by an install secret, for 365 days
by default (`kryphocron.audit_retention_days`).

### Setup

Until first-run setup completes, a node serves only the setup wizard,
health checks and `/tls-check` for the bootstrap hostname, behind a
one-time token. The token is printed to the log and written to
`<data>/setup-token` (mode `0600`); the database stores only its hash.

## Network edge

### Recommended deployment

The shipped Docker Compose stack runs locus behind Caddy:

- Only Caddy publishes public ports (80 and 443).
- Locus publishes port 3000 on `127.0.0.1` only, for the local CLI. Never
  publish it on `0.0.0.0`; that would bypass the proxy.
- The stack's network has a fixed subnet, an explicit gateway, and gives
  Caddy a static address outside Docker's dynamic range, so no other
  container can take Caddy's address.
- The image runs as a non-root user.

For a bare binary, the systemd unit in `aurora-locus/deploy/systemd/`
listens on `127.0.0.1:3000`, runs as its own user with `UMask=0077` and a
`0700` state directory, and enables systemd's sandboxing options
(`ProtectSystem=strict`, `NoNewPrivileges`, an empty capability set, a
system-call filter and others).

### Trusted proxies and the client IP

`X-Forwarded-For` and `Forwarded` are honoured only when the TCP peer is in
`http.trusted_proxies` (env `LOCUS_TRUSTED_PROXIES`). The client IP is the
right-most address in the chain that isn't a trusted proxy.

**Stricter than the reference:** the default is loopback only
(`127.0.0.0/8`, `::1/128`). The reference trusts every private address, so
on a shared network any neighbour can set the client IP that rate limits
key on. The Compose installer trusts Caddy's one static address, not the
subnet: connections through Docker's published port arrive from the bridge
gateway, which must not be trusted.

### Cloudflare mode

With `deploy.proxy_mode = cloudflare`, locus reads `CF-Connecting-IP` only
when the hop before the trusted proxy is inside Cloudflare's published IP
ranges. The list is bundled and refreshed daily from Cloudflare, with the
bundled copy as the fallback. A client connecting directly can't set its
own IP with that header.

Cloudflare mode always uses a fixed origin certificate (a Cloudflare
Origin CA certificate or a DNS-01 wildcard), since on-demand certificates
can't work behind Cloudflare's proxy. Setup requires Cloudflare's "Full
(strict)" TLS mode: it detects "Flexible" by probing, and with a read-only
API token it reads the zone's mode. Bypass Cloudflare's cache for
`/xrpc/*`, `/oauth/*`, `/admin/*` and `/blob/*`; with the private media gate
on this is required.

### TLS and certificate gating

With on-demand TLS, Caddy asks locus's `/tls-check?domain=<name>` before
getting a certificate. It answers 200 only for:

- the PDS hostname (and, during a hostname migration, the prepared
  candidate and the old hostname);
- former PDS hostnames, so `did:web:<old host>` keeps resolving;
- hosted did:web hostnames;
- hostnames of existing handles.

Anything else gets 404, so no certificate is issued and the TLS handshake
for that name fails. In setup mode only the bootstrap hostname is approved.
Locus can also terminate TLS itself with ACME, using the same approval
rule.

Every Caddyfile locus generates sets `strict_sni_host on`, which makes Caddy
refuse a request whose `Host` differs from the TLS server name. Caddy's
default is off.

### Host-based routing

Locus classifies every request by its `Host` header:

| Host | Gets |
|---|---|
| The PDS hostname | Every route |
| Internal names (`localhost`, IP literals, single-label names such as a Compose service name, `http.internal_hosts`) | Every route, but only under the PDS hostname's identity |
| Handle hosts and hosted did:web hosts | Only `/.well-known/atproto-did` and `/.well-known/did.json` |
| A prepared hostname candidate | Only `/tls-check` and health |
| A former PDS hostname | Its service `did.json`, `/tls-check`, health; everything else redirects to the current hostname |
| Anything else | 404 |

So the console, the OAuth pages and the account portal are never served on
user-chosen names. A user can't get a valid certificate for a convincing
sign-in origin under your domain.

Internal names are **not** a trust boundary: a client can send any `Host`
through a proxy or a published port. Locus never derives an identity or a
privilege from `Host`. Decisions such as trusted proxies, rate-limit bypass
and metrics access key on the peer address or a credential. PDS hostnames
and internal names are reserved against handles and did:web hosts, so one
name can't hold two roles.

### CORS and security headers

| Surface | Headers |
|---|---|
| XRPC | `Access-Control-Allow-Origin: *` with the atproto header set, as the reference. Authenticated responses get `Cache-Control: private` and `Vary: Authorization`. |
| Console, portal, setup | `Content-Security-Policy` with `default-src 'none'`, styles, fonts and images from self only, `form-action 'self'`, `frame-ancestors 'none'`, `base-uri 'none'`; `X-Frame-Options: DENY`; `Referrer-Policy: same-origin`; `X-Content-Type-Options: nosniff`; `Cache-Control: no-store` |
| OAuth pages | The same, plus `script-src 'self'` |
| Blobs | `Content-Security-Policy: default-src 'none'; sandbox`, `X-Content-Type-Options: nosniff`, `Content-Disposition: attachment`, as the reference |

The web UI has no inline scripts. Console pages are plain server-rendered
HTML; the live status pages load one script from the same origin. Pages
with hCaptcha allow hCaptcha's origins and nothing else. The branding
editor's previews may be framed by the same origin only.

Locus doesn't set `Strict-Transport-Security` itself; set HSTS at your
proxy once HTTPS works for every name you serve.

### Metrics

`/metrics` (Prometheus) answers only to `metrics.allowed_ips` (loopback by
default) or to a bearer token (kept in the key store: `keys set
metrics-token`, or `LOCUS_METRICS_TOKEN` on first boot), compared in
constant time.
Turn it off with `metrics.enabled` if you don't scrape it.

## Outbound requests

Locus uses two HTTP clients, chosen by who supplied the URL.

### URLs users can influence

DID documents, did:web hosts, handle HTTPS lookups, OAuth client metadata,
`include:` permission sets, proxy targets from an `atproto-proxy` header,
and records and documents fetched from other PDSes all go through the
SSRF-safe client:

- HTTPS only.
- Every address a name resolves to must be public unicast. The check runs
  at connect time, inside the resolver, so a DNS answer can't change after
  the check. It covers loopback, RFC 1918, carrier-grade NAT, link-local,
  unique-local, documentation, benchmarking and multicast ranges, and
  IPv4-mapped IPv6 addresses. Local-network names are refused.
- Redirects are never followed.
- Responses are capped (512 KB for documents, as the reference) and every
  request has a timeout.

Document and handle fetches, OAuth client metadata and `include:`
resolution are stricter still, in every mode: no IP-literal hosts and none
of the reference's forbidden domains; with protection on, no custom ports.

### Turning SSRF protection off

`PDS_DISABLE_SSRF_PROTECTION=true`, or dev mode, turns these rules off, as
in the reference. Then plain HTTP, other ports and private addresses work,
for local development with `did:web:localhost` or a feed generator on a
private network. Locus honours dev mode only on a local hostname, and
`PDS_DISABLE_SSRF_PROTECTION=false` keeps the rules on even in dev mode.
Each node reports its SSRF state at boot, and the console shows a warning
on every page while any live node runs with protection off. **Never turn it
off on a public server.**

### Operator-configured endpoints

Endpoints you type into settings (the PLC directory, AppView, private
AppView, moderation and report services, crawlers, the scanner webhook,
S3, the hCaptcha verify URL) may be on private addresses, such as a scanner
in the same Compose stack. They must be HTTPS unless you allow plain HTTP
for that endpoint (`http.allow_insecure_http`).

**Stricter than the reference:** the reference accepts any `http://` URL.
Locus refuses one typed into the console, CLI or setup wizard unless you
allow it for that endpoint, and warns in the console about every endpoint
on plain HTTP. A value from env or a reference `pds.env` allows itself,
with a log line, so an existing install boots unchanged.

## Input handling and limits

| Input | Limit (default) |
|---|---|
| Record-write request body | 2,000,000 bytes (`limits.repo_write_body`) |
| Other JSON request bodies | 153,600 bytes (`limits.json_body`) |
| A single record | 1,100,000 bytes (`limits.record_size`) |
| Blob upload | 300 MiB (`blobs.upload_limit`) |
| Repository import (CAR) | 1 GiB (`limits.import_size`); each block at most 2 MB; at most 5,000,000 blocks |
| A `#commit` event | 2 MB of blocks; larger writes are refused |
| Request time | 60 s (`http.request_deadline_ms`); streams and uploads have their own caps |
| Firehose subscribers | 1,000 per node, 20 per client IP |

Uploads and imports stream; nothing larger than the JSON limits is held in
memory whole.

- **Records** are validated against the reference's known schemas, and
  record JSON is parsed as strictly as the reference (malformed `$type`,
  `$link`, `$bytes` and blob refs are refused).
- **CAR imports** check every block's CID against its bytes, and walk the
  tree with a bounded walker (visited set, depth, block count, key order).
  The import is staged and resumable, and rolls back cleanly before its
  final swap.
- **Commits** are checked, from the changed nodes alone, to change the tree
  exactly as the writes say before anything is stored.
- **Encoded private-tier content** is decoded only with a bounded trial
  decode.
- Malformed parameters and bodies get 400 `InvalidRequest`, not a server
  error. On Postgres, text with a NUL byte is refused with 400.

### Rate limiting

Rate limits are **on by default** (the reference's package default is off).
Locus has the reference's buckets: a global per-IP bucket (3,000 requests
per 5 minutes), `getRepo`, login, account creation, password reset, email
requests, handle changes, uploads and account deletion, plus repo-write
points per account (create 3, update 2, delete 1; 5,000 per hour, 35,000 per
day). Console sign-in has its own per-address and per-name limits.

Responses carry `RateLimit-*` headers; a refusal is 429
`RateLimitExceeded` with `Retry-After`. Counters live in each node, in the
database or in Redis, so several nodes can share them.

A request skips the limits with the `x-ratelimit-bypass` header or from an
address in `rate_limits.bypass_ips`. **Stricter than the reference:** CIDR
ranges in the bypass list are honoured as ranges; the reference drops the
suffix.

## Blobs

### Serving

Blobs are served with the headers in the table above, so a browser won't
render an uploaded file as a page on your origin. No `Cache-Control` is
sent by default, because a cached copy would outlive a takedown or account
deletion. A taken-down blob answers 400 "Blob not found" to every caller.

### Scanning hook

A scanner (`blobs.scanner`, an HTTP webhook at `blobs.scanner_url`) sees each
upload in temporary storage before it can be used. It answers allow,
reject or quarantine. On a match locus:

- refuses the upload;
- keeps the bytes in quarantine (never garbage-collected);
- alerts operators, writes the audit log, and adds the blob to a review
  queue in the console, because fuzzy matches can be false positives.

When the scanner can't be reached, `blobs.scan_unavailable` decides: refuse
uploads with a retryable error (the default), or store them unscanned and
scan them when the scanner returns.

### Quarantine

A blob takedown marks the blob's database row in one transaction, then
moves the file to quarantine storage; reversal moves it back. The row, not
the storage location, decides, and it is checked on every request, so a
takedown applies on the next request even before the file has moved.

### Gated private media

With `kryphocron.private_media_gated` on, media used only by an account's
private posts is served only to the owner, to audience members of a post
that uses it, and to services you list in
`kryphocron.media_trusted_services`. Every trusted-service fetch is
audited. To anyone else a gated blob looks exactly like a missing one, and
both answers carry `Cache-Control: private, no-store`.

The gate needs a scanner: it can't be turned on without one, and the
scanner can't be removed while the gate is on. Media is not encoded,
because encoded media couldn't be scanned. With the gate off (the default),
private-post media is public by CID like any other blob.

## The private tier

Locus has built-in support for the kryphocron private tier
(`tools.kryphocron.*` records), off until you configure it.

### What it is not

**The private tier's encoding is friction, not encryption.** Locus encodes
private post content with the kryphocron codec so it isn't readable at a
glance in a repository, a firehose dump or a generic atproto client. It is
not encryption: don't rely on it to keep a post secret from someone who has
a copy of the record. Don't describe it to your users as encrypted,
confidential or end-to-end protected.

Encoded records are served like any other record: from `getRecord`,
`listRecords`, `getRepo` and the firehose. Relays, the AppView and anyone
reading the network receive them.

### Who can read private posts

- **The private AppView** decides who sees a private post in an app. It
  decodes records and serves them to audience members.
- **Anyone holding a copy of the record** may be able to read it, since
  the encoding isn't encryption.
- **The operator** can read everything stored on the PDS.
- **Locus itself never decodes for a reader**, the author included, and
  never decodes for moderators. Moderation services decode on their own
  side.

### Guarantees locus does give

- **Its own AppView setting.** The private tier uses `appview.private_url`
  and `appview.private_did`, which must be entered explicitly; with them
  empty the private tier is off. Locus never sends private-tier traffic to
  the public AppView in their place.
- **Host rules on every write path.** Locus enforces the rules the lexicons
  can't express: exactly one of plain or encoded content (plain text is
  encoded on write and never stored), a supported codec, an audience in the
  writer's own repository, and derived fields recomputed by locus, not taken
  from the client.
- **Fail closed.** When locus can't prove a membership or block check
  (the private AppView is unreachable, answers with an error or times out,
  or a remote block scan exceeds its cap), it refuses rather than allows.
  The block check for a reply to an author on another PDS falls back to the
  public AppView or a configured block-source AppView, never the private
  AppView, and fails closed with neither.
- **Threadgates.** A private reply under a thread whose threadgate can't
  be fetched is refused; a thread with no threadgate allows it.
- **Non-enumeration.** Refusals to join a conversation don't reveal why.
  A hidden audience and a deleted one look the same.
- **Local block.** A moderator can block an account from the private tier
  locally, whatever the private AppView says.

## Moderation

### Takedowns

`com.atproto.admin.updateSubjectStatus` takes down accounts, records (by
URI, whatever the CID) and blobs. Takedowns apply on the next request. An
account takedown also ends the account's sessions and OAuth grants and
emits an `#account` event. A taken-down record answers `RecordNotFound` to
every caller and is left out of `listRecords`.

Accounts can also be suspended by a moderator until a set time, a status
locus adds beyond the reference: no sign-in, no writes, no public reads
until it lifts by itself.

### Trusted moderation services

A configured moderation service (`moderation.service_did` and
`moderation.service_url`) gets the reference's moderation-service access to
`com.atproto.admin.*`. Its service tokens are subject to the replay rule
above: a token for a procedure, such as a takedown, works once. Reports go
to the report service (or, when none is set, the moderation service), as
the reference.

## Failure behaviour

- **Fail closed** wherever a check can't complete: a master key that
  doesn't verify stops the node; an unreachable scanner refuses uploads by
  default; private-tier membership and block checks refuse when unproven;
  the media gate answers "not found" to anyone it can't place.
- **Generic errors.** Every 500 is logged with its detail. The client
  receives only `InternalServerError` with the reference's generic message.
  Storage and blob-store errors are never passed to the client.
- **No silent downgrades.** Features that depend on unavailable services
  (email, the scanner, the private AppView) say so instead of pretending to
  work.
- **Logging.** Error detail goes to the log, never to the client. Key
  changes are audited by fingerprint. Logs still contain DIDs, handles,
  client IPs and request paths: treat them as personal data, restrict who
  can read them, and set a retention period. The console's log view is
  available to operators.

## Dependencies and supply chain

- Written in Rust. The toolchain is pinned (`rust-toolchain.toml`), and the
  workspace's `Cargo.lock` is committed.
- Core atproto code comes from proto-blue and the kryphocron crates, each
  pinned to an exact revision or version.
- Docker images build with `cargo build --locked`, in a multi-stage build
  that ships a slim runtime image with the binary and runs as a non-root
  user.
- The web UI is server-rendered with its assets embedded in the binary. No
  JavaScript build step, no CDN, no third-party scripts except hCaptcha
  when you turn it on.
- Untrusted input is decoded with bounded, strict decoders; the repo and
  OAuth parsers have fuzz and property tests.

Before deploying a build, run `cargo audit` and `cargo deny check` against
the lockfile yourself, and watch the repository's security advisories.

## Operator hardening checklist

**Before going live**

- [ ] Download the key backup during setup and store it, with its
      passphrase, somewhere other than the server.
- [ ] Back up the master key (or use AWS KMS) separately from database
      backups. A database backup alone can't sign; with the key it can.
- [ ] Enrol a TOTP code app for every operator. Give each person the
      lowest role they need.
- [ ] Keep SSRF protection on and dev mode off. Check the console shows no
      warning about either.
- [ ] Use HTTPS for every operator-configured endpoint, or allow plain HTTP
      only for endpoints on a network you control.
- [ ] Set `http.trusted_proxies` to exactly your proxy's address, never a
      whole subnet that other hosts share.
- [ ] Publish locus's port on `127.0.0.1` only. Only the proxy faces the
      internet.
- [ ] Keep `strict_sni_host on` in Caddy (every generated Caddyfile has it).
      With another proxy, configure the equivalent check.
- [ ] Set HSTS at the proxy.
- [ ] Behind Cloudflare: use Cloudflare mode, "Full (strict)" TLS, and
      bypass the cache for `/xrpc/*`, `/oauth/*`, `/admin/*` and `/blob/*`.

**Accounts and content**

- [ ] Keep sign-up invite-only or closed unless you mean to run an open
      server. If open, turn on hCaptcha.
- [ ] Configure SMTP, so password reset and email 2FA work.
- [ ] Configure a blob scanner if you host other people's media.
- [ ] Keep rate limits on. Use a shared backend (database or Redis) with
      more than one node.

**Reducing surface**

- [ ] Turn off `admin.basic_auth_enabled` if no tooling needs the
      reference's admin password. If it does, set a long random
      `PDS_ADMIN_PASSWORD`.
- [ ] Leave the rate-limit bypass key unset unless you need it, and keep the
      bypass address list short.
- [ ] Turn off `/metrics` if you don't scrape it, or restrict it to your
      monitoring address.
- [ ] Turn off the account portal (`portal.enabled`) if your users manage
      their accounts elsewhere.

**Ongoing**

- [ ] Review the audit log regularly, and export it to storage the PDS
      can't modify.
- [ ] Take a new key backup after every key change; the console reminds you.
- [ ] Watch the console's warnings: SSRF state, plain-HTTP endpoints,
      scanner outages, email failures, nodes that disagree.
- [ ] Apply updates promptly. Read the changelog for security notes.

## Summary of differences from the reference PDS

Places where locus is deliberately stricter than `@atproto/pds` 0.5.51:

| Area | Reference | Locus |
|---|---|---|
| Account signing keys at rest | Stored without a separate encryption layer | Sealed under the master key |
| Legacy sessions | Stateless until expiry | Checked on every request; revocation is immediate |
| OAuth access-token signing | The legacy session secret | A dedicated ES256 key |
| Password reset and OAuth | Grants survive | Grants revoked |
| `requestPasswordReset` | Reveals whether the email has an account | Same answer either way |
| Inbound service auth | No replay check | `jti` single-use for procedures; lifetime at most one hour |
| Trusted proxies | Every private address | Loopback only, or the addresses you name |
| Rate limits | Off by default | On by default |
| Rate-limit bypass list | CIDR suffix dropped | CIDR ranges honoured |
| `http://` operator endpoints | Accepted silently | Allowed per endpoint, with a console warning |
| Host routing | Doesn't route by `Host` | Console, OAuth and portal only on the PDS hostname |
| Caddy `strict_sni_host` | Off | On in every generated Caddyfile |
| Email unconfigured | Email features don't say they can't deliver | Reported as unavailable |
