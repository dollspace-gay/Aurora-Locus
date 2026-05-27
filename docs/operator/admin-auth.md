# Admin authentication for Aurora-Locus

**Audience:** operators bootstrapping admin access to a fresh
Aurora-Locus instance, or wiring CI/test flows that need to call
admin endpoints (`/admin/...`, `/xrpc/com.atproto.admin.*`,
`/xrpc/dev.aurora.admin.*`).

**Scope:** chainlink #84 — this page documents the model end-to-end,
including the bootstrap flow that was previously undocumented and
blocked Arc 15 Phase B Scenario 2.3 (takedown).

---

## Model in one paragraph

Aurora-Locus does NOT bake admin authority into the JWT. JWTs are
plain session tokens (whoever holds a valid one is the DID owner).
At request time, admin endpoints take the bearer JWT, resolve it
to a DID via the normal session lookup, then ask the
`admin_roles` table whether that DID has an active (non-revoked)
admin role. Authority is database-side; the token is just an
identity assertion. Granting a role and minting a token are
independent operations.

This means:

- There is no `PDS_ADMIN_PASSWORD` env var or hardcoded bootstrap
  credential. There is no `PDS_ADMIN_DID` env var that auto-grants
  on startup.
- To get admin access on a fresh PDS, you (a) create a normal
  account and (b) grant it an admin role via the offline
  `grant-admin` CLI. Then a session JWT for that account
  authenticates to admin endpoints.
- Admin role lookups happen on every admin-endpoint call — granting
  or revoking a role takes effect immediately for new requests
  (existing in-flight requests aren't interrupted).

---

## Bootstrap: granting the first admin role

The `grant-admin` CLI subcommand inserts into the `admin_roles`
table and appends an audit-chain entry, all in one transaction. It
is **offline-only** — it acquires the same PDS-liveness lock that
`serve` would, so it cannot run while a live PDS is hitting the
same database.

### Step 1 — create an account on the PDS

Start the PDS and create the account that will become admin. The
account can be created via the standard `createAccount` flow, or
the dev-route `dev.aurora.createAccount` (debug builds only):

```bash
# Production-style.
curl -sX POST http://localhost:3000/xrpc/com.atproto.server.createAccount \
  -H 'content-type: application/json' \
  -d '{
    "handle": "admin.example.com",
    "email": "admin@example.com",
    "password": "<strong-password>"
  }' | jq

# Dev-route (debug builds only).
curl -sX POST http://localhost:3000/xrpc/dev.aurora.createAccount \
  -H 'content-type: application/json' \
  -d '{
    "handle": "admin.localhost",
    "email": "admin@localhost",
    "password": "test-password"
  }' | jq
```

Note the returned `did`.

### Step 2 — stop the PDS

`grant-admin` acquires the PDS-liveness lock; the PDS must be down:

```bash
docker-compose down
# OR if running directly:
# kill the process holding data/ + the SQLite/Postgres connection
```

### Step 3 — grant the role

Three roles available: `moderator`, `admin`, `superadmin`.
Case-insensitive.

```bash
cargo run --release --bin aurora-locus -- grant-admin \
    did:plc:<from-step-1> admin \
    --notes "bootstrap operator"
```

Output on success:

```
Granted role 'admin' to did:plc:abc.... Audit entry: #1.
```

On failure (active grant already present):

```
Error: Validation error: did:plc:abc already has active role 'admin'.
       Revoke first via the admin API before re-granting.
```

To re-grant a previously revoked role, add `--force`:

```bash
cargo run --release --bin aurora-locus -- grant-admin did:plc:abc admin --force
```

`--force` does NOT bypass an *active* grant — only a *revoked* one.

### Step 4 — restart the PDS + mint a session

```bash
docker-compose up -d
# OR re-run cargo run --release --bin aurora-locus

# Log in as the admin account.
curl -sX POST http://localhost:3000/xrpc/com.atproto.server.createSession \
  -H 'content-type: application/json' \
  -d '{"identifier": "admin.localhost", "password": "<password>"}' | jq
```

Extract the `accessJwt` — that's your admin bearer token. Pass it
in `authorization: Bearer <accessJwt>` on any admin endpoint.

### Dev shortcut: `dev.aurora.mintToken`

In debug builds, `dev.aurora.mintToken` mints a fresh local-session
JWT for a given DID without requiring a password. Useful for CI /
test flows that just granted a role and want to immediately use it:

```bash
curl -sX POST http://localhost:3000/xrpc/dev.aurora.mintToken \
  -H 'content-type: application/json' \
  -d '{"did": "did:plc:abc..."}' | jq
```

The returned `accessJwt` is exactly the same shape as a real
`createSession` token. The endpoint is gated behind
`#[cfg(debug_assertions)]` — release builds return 404.

---

## Calling admin endpoints

All admin endpoints expect:

```
authorization: Bearer <accessJwt>
```

The same JWT shape as a regular session token. The admin-or-not
distinction is invisible at the JWT layer — it surfaces only at
request time when the endpoint runs admin-role lookup.

Example — takedown an account (Arc 15 Phase B Scenario 2.3):

```bash
curl -sX POST http://localhost:3000/admin/accounts/takedown \
  -H "authorization: Bearer $ADMIN_JWT" \
  -H 'content-type: application/json' \
  -d '{"did": "did:plc:offender...", "reason": "phase-b test"}' | jq
```

---

## Auth-resolution layering

The `AdminAuthContext` extractor walks a 5-layer fallthrough chain
when verifying the bearer token (`src/auth.rs::admin_auth_from_token`):

| Layer | What it accepts | Outcome on match |
|---|---|---|
| 1 | Local-session JWT (the `session` table) | DID → continue to Layer 5 |
| 2 | HS256 JWT with `scope="admin"` claim, signed with `PDS_JWT_SECRET` | DID → continue to Layer 5 |
| 3 | ES256K pre-check (alg validation only) | continue to Layer 4 |
| 4 | ES256K service JWT, verified via identity resolver | DID → continue to Layer 5 |
| 5 | `admin_roles` table lookup for the resolved DID | role assigned → admin context; no row → 403 |

Layers 1-4 are alternative paths to **assert a DID**; Layer 5 is
the only path that grants admin authority. Layer 2's
`scope="admin"` HS256 token is a separate mechanism that doesn't
require a row in `admin_roles` — but the secret to mint it
(`PDS_JWT_SECRET`) is operator-controlled and not auto-bootstrapped,
so most operators won't use this path.

---

## Environment variables

Aurora-Locus has **no** admin-specific env vars. Relevant general
vars:

| Var | Purpose |
|---|---|
| `PDS_JWT_SECRET` | HS256 secret for issuing session JWTs (Layer 1) AND for verifying any operator-minted HS256 admin tokens (Layer 2). Set to a strong random string in production. |
| `PDS_ADMIN_PASSWORD` | **Not present.** No env-driven admin bootstrap; use the `grant-admin` CLI. |
| `PDS_ADMIN_DID` | **Not present.** Same reason. |

---

## Revoking an admin role

Either via the live admin API (preferred — no downtime) or via SQL
on the offline DB. The live API:

```bash
curl -sX POST http://localhost:3000/admin/roles/revoke \
  -H "authorization: Bearer $ADMIN_JWT" \
  -H 'content-type: application/json' \
  -d '{"did": "did:plc:to-revoke..."}'
```

Direct SQL (offline) for emergencies:

```sql
UPDATE admin_roles
   SET revoked = 1,
       revoked_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
       revoked_by = 'manual:operator'
 WHERE did = 'did:plc:to-revoke...' AND NOT revoked;
```

Revocation invalidates the role at the *next* request — the holder
keeps their session JWT but loses admin authority on the next admin
endpoint call.

---

## Troubleshooting

| Symptom | Likely cause |
|---|---|
| HTTP 401 on admin endpoint | JWT missing/invalid; `createSession` first to get a fresh token. |
| HTTP 403 on admin endpoint with valid JWT | DID has no active `admin_roles` row. Re-grant via `grant-admin`. |
| `grant-admin` fails with "Cannot grant admin role: PDS is running" | The PDS is up against the same DB. Stop it first. |
| Granted role doesn't take effect | Did you re-mint the session JWT after the grant? Layer 5 lookup uses the DID from Layer 1; old sessions are fine if their DID's row was added/restored. |
