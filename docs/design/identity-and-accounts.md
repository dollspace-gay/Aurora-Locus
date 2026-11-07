# Identity, auth and accounts

This document describes how aurora-locus (locus) handles identity, sign-in,
accounts and moderation in 0.1. It is for operators running a locus PDS and
for developers working on it.

Locus aims to behave like Bluesky's reference PDS (`@atproto/pds` 0.5.51) as
seen from the network and from clients. Where it differs on purpose, this
document says so. Security mechanisms get a sentence each here; the security
design document covers them in depth.

## Identity

### DID methods

Every account is either `did:plc` or `did:web`. Both are supported at
account creation, in migration and in the operator console. `did:plc` is the
default (`accounts.default_did_method`, `plc` or `web`).

| | did:plc | did:web |
|---|---|---|
| Who controls it | Holders of the rotation keys in the PLC log | Whoever controls the hostname |
| Hostname | Free to change | Fixed forever (`did:web:<host>`) |
| Recovery | User or operator recovery key | None |
| Moving away | Signed PLC operation | New host must serve the document |

### did:plc

Locus builds, signs and verifies PLC operations itself. It encodes the
operation as DAG-CBOR, signs it, and derives the DID from the hash of the
signed genesis operation. It talks to the PLC directory set in
`identity.plc_url` (`PDS_DID_PLC_URL`; the public directory by default) to
submit operations and fetch documents and logs. Locus validates every
operation it signs before signing it: rotation keys and verification methods
must be `did:key`s, `alsoKnownAs` entries must be `at://` URIs, and service
endpoints must be http(s) URLs.

**Rotation keys on a new account**, in order:

1. the user's recovery key, if `createAccount` supplied `recoveryKey`;
2. the operator's recovery key, if `identity.recovery_did_key`
   (`PDS_RECOVERY_DID_KEY`) is set;
3. the PDS's current rotation key.

**The PDS rotation key** is a secp256k1 key. Locus generates it on first
boot and keeps it in the encrypted key store, or imports it from
`PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX`. It can also live in a KMS and
sign there (`PDS_PLC_ROTATION_KEY_KMS_KEY_ID`, as in the reference); locus
then never holds the private key.

**Changing the rotation key.** The reference reads its key at each boot, so
changing it means editing the env file and restarting, and the old key is
forgotten. Accounts whose documents list only the old key can then no longer
be signed for. Locus changes this:

- `aurora-locus keys set plc-rotation-key --generate | --file | --kms-key-id`,
  or a changed `PDS_PLC_ROTATION_KEY_*` value, stores the new key as
  **pending**. It becomes **current** only once a key backup that contains it
  exists, or the operator passes `--acknowledge-no-backup`. A backup taken
  earlier could not sign for accounts that later list the new key.
- **The previous key stays as a secondary signer.** For every operation
  locus signs (handle changes, `signPlcOperation`, identity repair, a
  tombstone), it uses whichever held key (current, pending or secondary) the
  account's latest PLC document lists first. Existing accounts keep working
  with nobody rewriting their documents.
- **New accounts and `getRecommendedDidCredentials`** name the current key,
  as the reference does.
- **Checks that need the current key** match the reference: `activateAccount`,
  the `validDid` field of `checkAccountStatus`, and `submitPlcOperation` all
  require the document (or submitted operation) to list the current key. A
  document that lists only a secondary key is refused at activation, with a
  message pointing at `signPlcOperation` to add the current key.
- **A key scan** runs in the background after a key becomes current, and
  daily while a secondary key is held. It fetches each hosted did:plc
  account's document (within PLC rate limits) and records which held keys it
  lists. `aurora-locus keys show` and the console report how many accounts
  list the current key, only an older one, or none.
- **Removing a key** is explicit: `keys drop-plc-key <did:key>`, or
  `--discard-pending` for a pending key. Before removing, locus re-fetches
  the documents that might still list the key and refuses if any lists it
  and no other held key, or if a fetch fails. The current key can't be
  dropped. A KMS key is only forgotten; the console tells the operator to
  disable it in the KMS.

Key changes need the operator's password and sign-in code over HTTP, or a
typed confirmation over the local admin socket, and are audited. Every node
reloads keys within seconds.

After a key compromise, replacing the key in existing documents is the
operator's task, as in the reference: someone holding a recovery key submits
an operation that lists the current key. The key scan shows who is left.
Locus does not automate that rewrite in 0.1.

**PLC endpoints.** `requestPlcOperationSignature`, `signPlcOperation`,
`submitPlcOperation` and `getRecommendedDidCredentials` behave as in the
reference, including the emailed `plc_operation` code needed to sign. A
submitted operation must keep the current rotation key, this PDS's
endpoint, the account's signing key and its handle.

**Handle changes on did:plc** go to PLC first, then the database, then the
`#identity` event. If a later step fails, or the directory's answer is
unclear, a repair job finishes the change. The reference can be left
inconsistent in that case. The operator can also run
`aurora-locus repair identity <did>`, which sets the document back to the
account's handle, this PDS and its signing key in one operation.

**Deletion does not tombstone.** Like the reference, locus does not tombstone
a PLC DID when its account is deleted; a tombstone would block a later
migration back. Locus tombstones a DID only when it created the DID for a
sign-up that then failed (see [Creation](#creation)).

### did:web accounts

An account can be created as `did:web:<handle>`, where the handle is on one
of the PDS's handle domains (for example `did:web:alice.example.com`). Locus
serves that account's `/.well-known/did.json` on that host. Path-based
did:web isn't supported by atproto, so locus doesn't offer it.

The trade-offs, which the console's create-account form shows next to the
choice:

- the DID is tied to that hostname forever; the handle can change, the DID
  can't;
- there are no rotation keys; whoever controls the host controls the
  identity;
- moving to another PDS needs the document served from that host by someone
  else, or by the user.

Because locus serves the document, key rotation, service changes and handle
changes take effect at once. For `updateHandle`, locus updates
`alsoKnownAs` in the document first, so the resolved document matches the
new handle, then the database, then emits `#identity`.

A **bring-your-own did:web** account can migrate in, as in the reference: it
proves control with a service JWT from the DID, and at activation its
document must list this PDS and the account's signing key.

### Documents locus serves

| Path | Host | Content |
|---|---|---|
| `/.well-known/did.json` | PDS hostname | The service DID document (`did:web:<pds host>`): `#atproto_pds` service, the service key, and an assertion key used by the private tier |
| `/.well-known/did.json` | A hosted did:web account's host | That account's DID document |
| `/.well-known/did.json` | A former PDS hostname | That host's service document, pointing at the current hostname; served indefinitely |
| `/.well-known/atproto-did` | A handle host | The active account's DID as `text/plain`; 404 otherwise |
| `/.well-known/oauth-protected-resource`, `/.well-known/oauth-authorization-server` | PDS hostname | OAuth metadata |

**Routing by host.** Locus classifies each request by its `Host` header:

- **The PDS hostname** gets every route.
- **Internal names** (`localhost`, IP addresses, single-label names such as a
  compose service name, and `http.internal_hosts`) also get every route, so
  local tools and container probes work. They are not a trust boundary: a
  request on an internal name acts under the PDS hostname's identity and gets
  nothing more. Privileged decisions key on the peer address or a
  credential, never on `Host`.
- **Handle hosts and hosted did:web hosts** get only
  `/.well-known/atproto-did` and `/.well-known/did.json`.
- **Health endpoints and `/tls-check`** answer on every host.
- **Anything else** gets 404.

So the console, OAuth pages and account portal can't be reached on a
user-chosen name. A handle can't become a sign-in page with a valid
certificate.

### Handles

Handle rules follow the reference:

- normalized to lowercase; whitespace, bad syntax and disallowed TLDs are
  refused;
- the reference's slur filter applies;
- on the PDS's own handle domains, the first label has no dots and is 3 to 18
  characters;
- the reference's reserved names are refused, plus the operator's
  `identity.reserved_handles`. Setup adds the PDS hostname's label and the
  domain's existing subdomains.

**Handle domains** are a live list (`identity.handle_domains`,
`PDS_SERVICE_HANDLE_DOMAINS`), each starting with a dot. Empty means
`.<hostname>`. Removing a domain stops new handles on it but doesn't break
existing ones.

**External handles** are verified by a DNS `_atproto` TXT record or by HTTPS
`/.well-known/atproto-did`, fetched fresh, not from cache. As in the
reference, new accounts start on a handle domain and can switch to an
external handle afterwards. An account migrating in can keep an external
handle if it resolves to its DID.

**Reserved hostnames.** Some names can never become handles:

- a hosted did:web host stays tied to its DID, even after that account
  changes handle or is deleted;
- every PDS hostname: the current one, a hostname being prepared for a
  migration, and every old one;
- every `http.internal_hosts` entry. In reverse, saving an internal host that
  is already a handle, did:web or PDS hostname is refused.

Each of these would otherwise give one name two routing classes.

**resolveHandle** looks up local active accounts first. A handle on one of
this PDS's handle domains with no local account returns `HandleNotFound`
without a network lookup. Anything else goes to the cache, then DNS and
HTTPS. Unlike the reference, locus does not ask the AppView.

### Resolution and caching

- **DID documents** resolve through proto-blue's resolver, with a cache in
  the database shared by every node. A cached document is refreshed in the
  background after `identity.did_cache_stale_ms` (1 hour) and not used after
  `identity.did_cache_max_ms` (1 day). These map to the reference's
  `PDS_DID_CACHE_STALE_TTL` and `PDS_DID_CACHE_MAX_TTL`.
- **Handles** resolve by DNS `_atproto` TXT raced against HTTPS, with a shared
  cache: 10 minutes for a hit, 1 minute for a miss. When the system resolver
  finds nothing, `identity.handle_backup_nameservers` is tried.
- Each lookup times out after `identity.resolver_timeout_ms` (3 s).
- On a signature failure, locus refreshes the signer's document once and
  retries, as the reference does.
- did:web documents and handle lookups go through an SSRF-safe client
  (HTTPS, public addresses only, no redirects, size-capped). did:plc
  documents come from the configured PLC directory through the operator's
  client, so a local directory on plain HTTP works.

## Auth

### Credential types

| Credential | Form | Used for |
|---|---|---|
| Legacy session | HS256 access + refresh JWTs | `createSession`, `refreshSession` |
| App password | Same tokens, narrower scope | Privileged or non-privileged, as the reference |
| OAuth | DPoP-bound access token | Third-party apps |
| Service auth | ES256K or ES256 JWT from another service | Inter-service calls |
| Operator | Console sign-in, API token, or local admin socket | The admin API |
| Legacy admin password | `admin:<password>` Basic auth | Existing tooling |

### Legacy sessions (stateful)

Locus keeps the reference's token shapes so clients don't notice, but checks
them differently.

- The access JWT is HS256 under a server secret, `typ: at+jwt`, with the
  reference's claims plus `jti` set to the session id. It lasts 2 hours.
- **Every authenticated request reads the session row and the account
  status.** The reference trusts the JWT until it expires. With locus,
  logout, password reset, app-password revocation and takedown take effect on
  the next request. That is a firm requirement for working with cairn.
- Public reads (`getRecord`, `listRecords`, sync reads, `getBlob`) check
  account, record and blob takedown status on every request, uncached.
- **Deactivation keeps sessions**, so the user can still call
  `activateAccount`, `getSession`, `checkAccountStatus` and the account
  management endpoints. Writes and public reads are refused.
- Refresh tokens last 90 days and rotate on use, with the reference's 2-hour
  grace for a replayed refresh (it returns the same next token).
- A taken-down account can still sign in, but only with the `takendown`
  scope, and its refresh token isn't stored, as in the reference.
- Passwords and app passwords are hashed with Argon2id. Sign-in responses are
  padded to a constant floor of 350 ms so timing doesn't reveal whether an
  account exists.
- Email sign-in codes (`AuthFactorTokenRequired`) work as in the reference,
  and app passwords skip them.
- Accounts imported from a reference PDS keep their sessions. Access tokens
  the reference issued are accepted until they expire, with the status check
  but no per-session revocation; the first refresh issues locus tokens.
  Imported scrypt password hashes become Argon2id on the first successful
  sign-in.

### Scopes and permissions

Locus uses the reference's scope model: `com.atproto.access`,
`com.atproto.appPassPrivileged`, `com.atproto.appPass`,
`com.atproto.takendown`, `com.atproto.refresh` and
`com.atproto.signupQueued`. The lists of full-access-only and
privileged-only methods are copied from the reference at 0.5.51. Some
examples:

- non-privileged app passwords are refused for every `chat.bsky.*` method,
  and for `getServiceAuth` where the reference refuses them;
- a taken-down session reaches only the endpoints meant for it, including
  the `tools.ozone.inbox.*` appeal methods;
- `activateAccount` and `checkSignupQueue` refuse OAuth credentials.

Under OAuth, each endpoint checks the token's granular scopes as the
reference does. A missing scope is 403 `ScopeMissingError`. Locus's own
procedures need the granular scope that fits them: the private-post
procedures need `repo:` scopes on their collections, and locus's
account-holder queries need their `rpc:` scope.

### OAuth authorization server

Locus is a full OAuth authorization server. proto-blue's OAuth support is
client-side only, so locus builds the server itself. Behaviour follows the
reference:

- pushed authorization requests, the authorization-code grant with PKCE
  (`S256` only), refresh and revocation;
- DPoP is mandatory, with server nonces and a `jti` replay check shared by
  every node;
- client metadata is fetched through the SSRF-safe client and cached;
  loopback clients work for development; confidential clients use
  `private_key_jwt`;
- refresh tokens rotate on use; a replayed refresh token revokes the grant;
- granular scopes `repo:`, `blob:`, `rpc:`, `account:`, `identity:`,
  `include:` (permission sets resolved from published lexicons, or from
  `oauth.lexicon_authority_did`), and `transition:*`;
- consent records the scopes the user approved, not every scope the client
  asked for;
- session lifetimes as the reference: 2 weeks for public clients; 2 years,
  with 3-month refresh, for confidential clients and those listed in
  `oauth.trusted_clients` (`PDS_OAUTH_TRUSTED_CLIENTS`).

The sign-in pages are server-rendered: sign in (by email, handle or DID),
choose a remembered account, approve or deny. The email code is skipped on a
device the account used in the last 30 days; a password is asked again after
7 days. `prompt=create`, or "Create an account" on the sign-in page, signs a
new user up and continues to consent. The pages use CSRF double-submit,
`Sec-Fetch-*` checks and constant-time padding.

Deliberate differences:

- **OAuth access tokens are signed with their own key**, not the legacy
  session secret, so `/oauth/jwks` publishes a real public key.
- **Password reset, account deletion and takedown revoke OAuth grants.** The
  reference's password reset leaves them.

### Service auth

**Outbound**, locus signs ES256K JWTs with the account's signing key:
`iss`, `aud` (the bare DID when proxying, as the reference), `lxm`,
`exp` 60 seconds after `iat`, and a random `jti`.

**Inbound**, locus refuses the token types meant for other uses (access,
refresh, DPoP), and requires `iss`, `aud` equal to this PDS's service DID,
`exp`, and `lxm` equal to the called method. It resolves the issuer's key
(`#atproto`, or `#atproto_label` for a labeler) and accepts ES256K and
ES256, refreshing the document once on a bad signature.

Locus is stricter than the reference about lifetime and replay:

- a token may live at most 1 hour, plus 60 seconds of clock skew;
- **procedures**, including every `com.atproto.admin.*` procedure, need a
  `jti` and accept it once. The reference always sets `jti`, so its tokens
  pass;
- **queries**, and idempotent procedures such as `uploadBlob`, may reuse a
  token until it expires, as the reference allows.

`getServiceAuth` follows the reference: a token without a method lasts at
most 60 seconds, one bound to a method up to an hour; protected methods are
refused; privileged methods need a privileged credential; a taken-down
session may mint only `createAccount` tokens (to migrate away). An OAuth
caller needs the matching `rpc:` scope.

### Moderation and admin trust

- **The moderation service** is configured by `moderation.service_did` and
  `moderation.service_url` (`PDS_MOD_SERVICE_DID`, `PDS_MOD_SERVICE_URL`). A
  service JWT from that DID gets the reference's mod-service access to
  `com.atproto.admin.*`. cairn works as that service.
- **Reports** go to `moderation.report_service_*`, or to the moderation
  service when no report service is set, as in the reference.
- **Operators** reach the admin API with a console sign-in or an API token,
  each method gated by a minimum role (see [Admin API](#admin-api)).
- **The legacy admin password** (`PDS_ADMIN_PASSWORD`, otherwise generated)
  counts as the `admin` role on the reference admin surface only.
  `admin.basic_auth_enabled` turns it off.
- **The local admin socket** (`<data>/admin.sock`, mode 0600, checked by
  peer credentials) counts as `owner`. The CLI uses it.

## Accounts

### Creation

`com.atproto.server.createAccount` takes the reference's input: `email`,
`handle`, `password`, `inviteCode`, optional `recoveryKey`, and optional
`did` for migrating in. Locus adds one optional field, `locusDidMethod`
(`plc` or `web`), which stock clients don't send; without it,
`accounts.default_did_method` applies.

Checks before anything is written:

- the email is valid, and its domain isn't on the built-in disposable-address
  list or `accounts.disposable_email_domains`;
- the handle passes the rules above;
- an invite code is present when `accounts.invite_required` is on (the
  default), otherwise `InvalidInviteCode`;
- the handle and email aren't taken (`InvalidRequest` "Handle already taken"
  / "Email already taken").

The order follows the reference. No database transaction is open during
network calls, and later failures are compensated:

1. Validate locally and generate the signing key. For did:plc, build and sign
   the genesis operation and derive the DID.
2. For a new did:plc, submit the genesis operation to PLC. A clear refusal
   returns an error; no DID exists. If the failure is ambiguous (timeout,
   reset, 5xx), locus asks PLC whether the DID now exists: if it does,
   creation continues; if not, it fails with `UpstreamFailure`.
3. One transaction writes the account, the signing key, the empty signed
   repository, the invite use and the first session. For an account created
   active, it also writes the `#identity`, `#account`, `#commit` and `#sync`
   events, in the reference's order.
4. If step 3 fails after locus registered a DID, locus tombstones that DID.
   If PLC can't be reached, the identity repair job retries until it
   succeeds. A DID supplied by a migrating account is never tombstoned.

Other details:

- Handle and email are unique across **all** accounts, including
  deactivated and taken-down ones. Unique indexes are the real guard, so a
  race returns a clean 400, never a 500.
- The password is optional at the API, as in the reference; an account
  without one signs in only after setting one through password reset.
  Passwords longer than 256 bytes are refused.
- `createAccount` for a DID that already has an account is `AlreadyExists`.
- `createAccount` is rate-limited to 100 per 5 minutes per IP.
- With `accounts.did_doc_with_session` (`PDS_ENABLE_DID_DOC_WITH_SESSION`),
  `createSession` and `getSession` include `didDoc`; `createAccount`
  returns it.
- Web sign-up (the account portal and the OAuth sign-up page) can ask for
  hCaptcha when `accounts.hcaptcha_site_key` and its secret are set.

The console's "Create an account" form offers did:plc or did:web, with
did:web's trade-offs shown next to the choice. When invites are required, it
makes a one-use code for the new account.

### Status model

| Status | Set by | Sign-in | Writes | Public reads |
|---|---|---|---|---|
| `active` | — | yes | yes | yes |
| `deactivated` | the user, or an operator | yes | no | `RepoDeactivated` |
| `takendown` | a moderator or operator | `takendown` scope only | no | `RepoTakendown` |
| `suspended` | a moderator or operator, until a set time | `takendown` scope only | no | `RepoSuspended` |
| `deleted` | the user or an operator | no | no | `RepoNotFound` |

The account's owner can still read their own repo in any state but deleted.

`suspended` is a locus addition: a takedown with an end time. A moderator
suspends an account for a number of days or until a time (at most ten
years), with an optional note, from the console, the CLI (`account suspend
<did> --days <n>` or `--until <time>`) or `moderator.account.suspend`. The
`#account` event says `suspended`. When the time comes, a background job
lifts it and emits `#account` as active again; a moderator can also lift it
early (`account unsuspend`). Sessions are kept and work again once it lifts.
Only an active account can be suspended, so a suspension never lifts a
takedown. A reference entryway or shard sees a suspension as a takedown.

### Deactivate, activate, delete

These work as in the reference, with these additions:

- **Every activation checks the DID document**, including reactivating a
  self-deactivated account: the document must name this PDS's endpoint and
  the account's signing key, and for did:plc the current PDS rotation key.
  Activation then emits `#account`, `#identity` and `#sync` together.
- **Deactivating a taken-down account keeps the takedown.**
- **`deleteAfter` is enforced.** A scheduled job deletes deactivated accounts
  once their `deleteAfter` time passes. The reference stores the value and
  does nothing with it.
- **Deletion is a resumable job.** The first step marks the account deleted,
  ends its sessions and OAuth grants, and emits `#account` with status
  `deleted`. Batches then remove the repository, blobs, app passwords,
  invites, keys and private-tier data, and the account row last, which frees
  the handle and email. The reference leaves app passwords and invite rows
  behind. A node that stops mid-way leaves the job to another.
- `requestAccountDelete` and `deleteAccount` use an emailed code, as in the
  reference.

### Migration

**In**, the reference's flow works unchanged:

1. `createAccount` with `did` and a service JWT from that DID;
2. `importRepo`, `uploadBlob`, `putPreferences`;
3. `checkAccountStatus` and `listMissingBlobs` to check progress;
4. `getRecommendedDidCredentials`, then `submitPlcOperation` (or update the
   did:web document);
5. `activateAccount`.

The account starts deactivated and emits nothing until activation, which
checks the DID document as above. `accounts.accept_imports`
(`PDS_ACCEPTING_REPO_IMPORTS`) turns imports off. The account portal has a
guided page for moving an account here.

**Out**, the reference's flow works too: `getServiceAuth` for
`createAccount` at the new PDS, `requestPlcOperationSignature`,
`signPlcOperation`, then `deactivateAccount`. `signPlcOperation` works while
the account is deactivated.

**A whole reference PDS** can become a locus install with
`aurora-locus import reference-pds`. That is covered in the operations
documentation.

### Invites

Invites work as in the reference:

- codes with a number of uses, created by operators (`server.createInviteCode`,
  `createInviteCodes`) or from the console;
- per-account codes earned on an interval (`accounts.invite_interval_ms` /
  `PDS_INVITE_INTERVAL`, counting from `accounts.invite_epoch_ms` /
  `PDS_INVITE_EPOCH`; at most 5 unused; 0 turns it off). Codes an operator
  gives don't count toward the 5;
- disabling codes, and disabling or enabling an account's invites.

Each use decrements the code's remaining uses in the same transaction that
creates the account, so two sign-ups can't both take the last use. The
console lists the uses left and marks a code with none left as "used up".

### Email

Email goes over SMTP: `smtp://` with STARTTLS when the server offers it, or
`smtps://`. The SMTP URLs live in the encrypted key store
(`PDS_EMAIL_SMTP_URL` and `PDS_MODERATION_EMAIL_SMTP_URL` are read on first
boot). `email.from_address` and `email.moderation_from_address` set the
senders. A separate moderation sender handles `admin.sendEmail`, and falls
back to the main one.

**Locus doesn't pretend to send email.** The reference, with no SMTP
configured, logs messages and answers as if it sent them. Locus treats
email as configured only once the sender's connect-and-auth check passes.
Until then, every feature that needs email — confirmation, password reset,
the PLC signing code, the deletion code, sign-in codes, email change —
answers `InvalidRequest` "Email is not configured on this server", and the
console warns. The setup wizard sends a test message.

Other email behaviour:

- Codes are `XXXXX-XXXXX`, valid 15 minutes, one per account and purpose,
  stored as keyed hashes.
- **`requestPasswordReset` answers the same whether or not the address has
  an account.** The reference reveals it.
- A password reset ends every session and OAuth grant.
- `updateEmail` can turn email sign-in codes on and off, as in the
  reference.

Without email, an operator can still reset a password, change an email or
handle, and delete an account from the console or CLI. For a did:plc account
that needs a new PLC operation, the operator can run
`aurora-locus repair identity`.

### Server description and signup

`com.atproto.server.describeServer` returns:

| Field | Source |
|---|---|
| `did` | The service DID (`did:web:<hostname>` unless set at setup) |
| `availableUserDomains` | The handle domains |
| `inviteCodeRequired` | `accounts.invite_required` |
| `blobUploadLimit` | `blobs.upload_limit` |
| `links.privacyPolicy`, `links.termsOfService` | `server.privacy_policy_url`, `server.terms_of_service_url` |
| `contact.email` | `server.contact_email` (`PDS_CONTACT_EMAIL_ADDRESS`) |

Locus has no signup queue. `com.atproto.temp.checkSignupQueue` answers
`{activated: true}` and accepts the `signupQueued` scope, as the reference.

Sign-up is controlled by `accounts.invite_required`: open, or invite-only
(the default). There is no separate "closed" mode in 0.1; requiring invites
and issuing none has that effect.

## Moderation and the admin API

### What a moderation service needs

Locus implements what cairn needs from a PDS, so it can be a cairn community
PDS target. The same surface serves any reference-compatible moderation
service.

| Endpoint | Behaviour |
|---|---|
| `admin.updateSubjectStatus` | Takedowns of an account, a record (by URI, whatever the CID) or a blob, with `takedown {applied, ref}`. Takes effect on the next request. An account takedown ends its sessions and OAuth grants and emits `#account`. It also accepts `deactivated`. |
| `admin.getSubjectStatus` | For a DID, a record URI or a blob, returns that subject's own takedown status and `ref`. |
| `admin.sendEmail` | Sent through the moderation sender. |
| `admin.getAccountInfo(s)`, invite methods | As the reference. |
| Public reads | A taken-down record is `RecordNotFound` from `repo.getRecord` for every caller and missing from `listRecords`. A taken-down repo is refused to everyone but its owner. |
| `moderation.createReport` | Proxied to the report service with a service JWT, honouring `atproto-proxy`. |

Taken-down blobs move to quarantine storage, and back when the takedown is
reversed.

### Admin API

Operators are separate from atproto accounts. Each has one of four roles:
**owner > admin > moderator > viewer**. The reference has a single admin
credential, so locus maps each reference admin method to a minimum role:

| Minimum role | Methods |
|---|---|
| viewer | `admin.getAccountInfo`, `admin.getAccountInfos`, `admin.getSubjectStatus`, `admin.searchAccounts` (a locus addition behind the console's account search), and the sync reads the reference opens to its admin token |
| moderator | `admin.updateSubjectStatus`, `admin.sendEmail`, `admin.getInviteCodes` (unused codes can create accounts), `admin.disableAccountInvites`, `admin.enableAccountInvites`, `admin.disableInviteCodes`, `server.createInviteCode`, `server.createInviteCodes` |
| admin | `admin.deleteAccount`, `admin.updateAccountPassword`, `admin.updateAccountEmail`, `admin.updateAccountHandle`, and any other `com.atproto.admin.*` method |

The moderation service gets moderator-level access to the methods the
reference opens to mod services. The legacy admin password counts as admin.

**Locus's own operator methods** live under `app.nearhorizon.locus`, grouped
by the role they need: `.viewer.*` (read-only views, such as the audit log),
`.moderator.*`, `.admin.*` (settings, jobs, private-tier controls) and
`.owner.*` (operators and API tokens). The router refuses a caller below the
group's role before the handler runs or parses the body, and each handler
checks again. `describeCapabilities` is open. The namespace is a single
build-time constant, so it can be changed.

The console and the CLI both call this API as the signed-in operator, so the
same roles, checks and audit entries apply whichever is used. The console
needs a password and a TOTP code; sign-ins end after
`admin.session_idle_minutes` (30) idle, and always after 12 hours. Managing
operators, deleting accounts and changing keys ask for the password and code
again.

### Audit log

Every operator action and every moderation action is appended to the audit
log: who, what, on which subject, when, and, for settings, the old and new
values. Secrets are recorded only as fingerprints. Operators read it in the
console, with filters, or through `viewer.getAuditLog`, and can export it as
JSON lines.

## Not done in 0.1

- `resolveHandle` doesn't fall back to the AppView.
- After a rotation-key compromise, rewriting existing PLC documents is a
  manual task.
- Handle length limits (3–18) are fixed, not settings.
- There is one moderation service, not a list.
- The console can't issue a PLC operation code to an account, or clear an
  account's email sign-in codes, when email is unconfigured. Operators use
  `repair identity` and the email-change methods instead.
