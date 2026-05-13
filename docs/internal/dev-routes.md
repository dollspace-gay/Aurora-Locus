# Dev curl framework

Localhost-only HTTP endpoints for development workflow.
Compiled into debug builds only via `#[cfg(debug_assertions)]`;
release builds do not expose this surface.

The endpoint set is intentionally narrow — five operations that
collapse the most painful PDS-restart loops during Phase B
sweeps and local development:

| Endpoint | Method | Body | Purpose |
|---|---|---|---|
| `dev.aurora.grantAdmin` | POST | `{did, role, notes?}` | Grant admin role without stopping the PDS. |
| `dev.aurora.revokeAdmin` | POST | `{did, role?, reason?}` | Revoke an active admin grant. |
| `dev.aurora.listAdmins` | GET | — | Enumerate every `admin_roles` row, active + revoked. |
| `dev.aurora.createAccount` | POST | `{handle, email, password}` | Throwaway test account; bypass handler-layer invite + email-verification. |
| `dev.aurora.mintToken` | POST | `{did}` | Fresh local-session JWT (admin authority queried from `admin_roles` at request time). |

## When to use these

- Granting admin role without stopping the running PDS (the CLI
  `cargo run -- grant-admin` holds a PDS-liveness lock that
  fails fast when a PDS is up).
- Bypassing the `com.atproto.server.createAccount` ceremony's
  invite-code + email-verification gates for throwaway test
  accounts.
- Minting a fresh JWT after a grant lands, without going
  through `createSession`'s email/password flow.

## When NOT to use these

- Production. Release builds don't include the surface anyway
  (`#[cfg(debug_assertions)]` strips the module at compile
  time); the section below has a verification recipe.
- Phase B against a shared dev environment. Only your local
  127.0.0.1 PDS exposes these.
- Anything you'd want auditable. The endpoints write through
  the existing audit-chain machinery (grants land an audit
  entry via `AdminRoleManager::grant_role`), but the `dev:`
  actor-DID prefix on the rationale signals "developer
  convenience" rather than operator intent.

## Threat model

The `#[cfg(debug_assertions)]` gate IS the auth. Localhost
development is the trusted environment; release builds never
include the surface, so production deployment risk is zero.
Path namespace `dev.aurora.*` is List C by design — NEVER
registered in `RouteRegistry`, never advertised by
`tools.aurora.describeCapabilities`. Operators running release
builds against these paths will see 404.

## Setup

Start the PDS in debug mode (the default for `cargo run`).
Release builds (`cargo build --release` / `cargo run --release`)
do not expose dev routes.

```bash
cargo run -- serve
```

Expected log lines include the usual:

```
🚀 Aurora Locus PDS listening on 0.0.0.0:2583
```

No additional log line announces the dev surface — its
presence is verifiable via the endpoint itself (see "Verifying
the dev surface is on / off" below).

## Endpoint reference

### `dev.aurora.grantAdmin`

Grant admin role to a DID without stopping the PDS.

```bash
curl -s -X POST http://localhost:2583/xrpc/dev.aurora.grantAdmin \
  -H 'Content-Type: application/json' \
  -d '{
    "did": "did:web:alice.localhost",
    "role": "superadmin",
    "notes": "Phase B sweep"
  }' | jq
```

Response (camelCase):

```json
{
  "did": "did:web:alice.localhost",
  "role": "superadmin",
  "grantedAt": "2026-05-13T03:00:00+00:00"
}
```

Role values: `moderator` | `admin` | `superadmin`
(case-insensitive at the parse layer per
`Role::from_str` in `src/admin/roles.rs:70`).

The grant routes through `AdminRoleManager::grant_role`. If a
revoked row already exists for the DID, the underlying
`grant_role_in_tx` re-grants in place (UPDATEs the existing
row); if an active row exists, the call returns 409 Conflict.

### `dev.aurora.revokeAdmin`

Revoke the active admin role for a DID.

```bash
curl -s -X POST http://localhost:2583/xrpc/dev.aurora.revokeAdmin \
  -H 'Content-Type: application/json' \
  -d '{
    "did": "did:web:alice.localhost",
    "reason": "test cleanup"
  }' | jq
```

Response:

```json
{
  "did": "did:web:alice.localhost",
  "revokedAt": "2026-05-13T03:00:01+00:00"
}
```

404 if no active role exists for the DID
(`AdminRoleManager::revoke_role_in_tx` returns `NotFound`).

### `dev.aurora.listAdmins`

Enumerate every row in `admin_roles` — active and revoked.

```bash
curl -s http://localhost:2583/xrpc/dev.aurora.listAdmins | jq
```

Response:

```json
{
  "admins": [
    {
      "did": "did:web:alice.localhost",
      "role": "superadmin",
      "grantedBy": "dev:grant-admin",
      "grantedAt": "2026-05-13T03:00:00+00:00",
      "revoked": false,
      "revokedAt": null,
      "revokedBy": null,
      "notes": "Phase B sweep"
    }
  ]
}
```

Ordered by `granted_at DESC`. The dev surface intentionally
surfaces revoked rows too — sanity-checking grant history
without `sqlite3 data/account.sqlite` in a second terminal.

### `dev.aurora.createAccount`

Create a test account without invite-code or email-
verification gates. Returns a usable access JWT immediately.

```bash
curl -s -X POST http://localhost:2583/xrpc/dev.aurora.createAccount \
  -H 'Content-Type: application/json' \
  -d '{
    "handle": "victim.localhost",
    "email": "victim@localhost",
    "password": "TestPassword123!"
  }' | jq
```

Response:

```json
{
  "did": "did:plc:abc...",
  "handle": "victim.localhost",
  "accessJwt": "eyJ..."
}
```

Preserves the DB-invariant checks inside
`AccountManager::create_account` (handle uniqueness, email
uniqueness, password hashing, DID generation, actor row
insert). Initialises the repository so the account is usable
for record writes. Skips the email-verification token
generation that `com.atproto.server.createAccount` performs.

Caveat: if `config.invites.required = true`, the underlying
manager's check still fires — that flag is enforced inside
`AccountManager::create_account` itself, not at the handler
layer. Local-dev configs typically leave `invites.required =
false` (the default).

### `dev.aurora.mintToken`

Mint a fresh local-session JWT for the given DID.

```bash
curl -s -X POST http://localhost:2583/xrpc/dev.aurora.mintToken \
  -H 'Content-Type: application/json' \
  -d '{"did": "did:web:alice.localhost"}' | jq
```

Response:

```json
{
  "did": "did:web:alice.localhost",
  "accessJwt": "eyJ..."
}
```

`AdminAuthContext`'s Layer 1 path (`src/auth.rs:230-332`)
validates the JWT against the `session` table, then queries
`admin_role_manager` for the DID's current grant. Admin
authority is NOT baked into the JWT itself — it lives in
`admin_roles`. So minting a fresh session for a DID that
already has an admin grant produces a token that passes the
admin-tier auth check on subsequent requests, without needing
any scope claim baked into the JWT.

Returns 404 if no actor row exists for the DID.

## Typical workflow

```bash
# 1. Start the PDS (once per session)
cargo run -- serve

# 2. Create a test admin account.
RESP=$(curl -s -X POST http://localhost:2583/xrpc/dev.aurora.createAccount \
  -H 'Content-Type: application/json' \
  -d '{"handle":"alice.localhost","email":"alice@localhost","password":"pw"}')
ADMIN_DID=$(echo "$RESP" | jq -r '.did')
ADMIN_TOKEN=$(echo "$RESP" | jq -r '.accessJwt')

# 3. Grant admin role (no PDS restart needed).
curl -s -X POST http://localhost:2583/xrpc/dev.aurora.grantAdmin \
  -H 'Content-Type: application/json' \
  -d "{\"did\":\"$ADMIN_DID\",\"role\":\"superadmin\"}"

# 4. Mint a fresh JWT so the new admin grant is picked up by
#    AdminAuthContext's role lookup. (The token from step 2 also
#    works — the role lookup happens per-request — but minting a
#    fresh one mirrors the workflow operators expect after a grant.)
ADMIN_TOKEN=$(curl -s -X POST http://localhost:2583/xrpc/dev.aurora.mintToken \
  -H 'Content-Type: application/json' \
  -d "{\"did\":\"$ADMIN_DID\"}" | jq -r '.accessJwt')

# 5. Create a sacrificial subject account (so admin doesn't
#    takedown themselves).
SUBJECT_DID=$(curl -s -X POST http://localhost:2583/xrpc/dev.aurora.createAccount \
  -H 'Content-Type: application/json' \
  -d '{"handle":"victim.localhost","email":"victim@localhost","password":"pw"}' \
  | jq -r '.did')

# 6. Now run Phase B exercises against the admin/subject pair.
#    Example: emit a moderation event (Arc 9 Phase B Section A
#    seed). Note: ModEventAction's wire form is {"kind": "..."}.
curl -s -X POST http://localhost:2583/xrpc/tools.aurora.admin.emitEvent \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H 'Content-Type: application/json' \
  -d "{
    \"action\": {\"kind\": \"TakedownAccount\"},
    \"subjects\": [{\"\$type\": \"com.atproto.admin.defs#repoRef\", \"did\": \"$SUBJECT_DID\"}],
    \"rationale\": \"phase-b seed\",
    \"snapshotCapture\": true
  }" | jq
```

## Verifying the dev surface is on / off

The dev surface only exists in debug builds. Confirm:

```bash
# Debug build (default): dev routes are mounted.
cargo build
curl -s -o /dev/null -w "%{http_code}\n" \
  http://localhost:2583/xrpc/dev.aurora.listAdmins
# Expect: 200

# Release build: routes do not exist.
cargo build --release
# (then run target/release/aurora-locus serve)
curl -s -o /dev/null -w "%{http_code}\n" \
  http://localhost:2583/xrpc/dev.aurora.listAdmins
# Expect: 404
```

Symbol check against the release binary:

```bash
nm target/release/aurora-locus 2>/dev/null | grep dev_routes
# Expect: no output (zero symbols)
```

## Out of scope

The following endpoints were considered but explicitly excluded
from Arc 11's initial surface. They are v0.6 candidates:

- `dev.aurora.inspectState` — read substrate state
  (`dpop_jti_replay`, `rate_limit_buckets`, etc.) without
  `sqlite3` in another terminal.
- `dev.aurora.triggerReaper` — fire background reapers
  manually for tests that need an immediate sweep.
- `dev.aurora.inspectInMemory` — read in-process state
  (`DPopNonceStore.nonces`, governor counters) that doesn't
  live in the DB.

These were excluded for cycle scope, not for design objection.
A future cycle can add any of them under the same
`#[cfg(debug_assertions)]` gate following the pattern this
arc establishes.
