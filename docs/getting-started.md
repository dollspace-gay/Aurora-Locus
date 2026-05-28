# Getting Started

This walkthrough stands up a fresh Aurora-Locus PDS on SQLite, mints the first
SuperAdmin, and then shows the path to upgrade to a Postgres-backed
multi-instance deployment. SQLite is the default backend and the right choice
for development and single-host production deployments; Postgres is the
backend for multi-instance HA.

For the complete environment-variable surface, see
[operator/configuration.md](operator/configuration.md). For the model behind
admin authority and the full bootstrap, see
[operator/admin-auth.md](operator/admin-auth.md).

---

## 1. Prerequisites

- **Rust toolchain.** Stable Rust with Cargo. The workspace pins
  `edition = "2021"` and does not declare a minimum `rust-version`; any
  recent stable toolchain works.
- **System libraries.** OpenSSL development headers (for the `openssl` CLI
  used below to generate keys; the server itself links `rustls`).
- **Docker** *(optional, only for the Postgres upgrade in §4)*.
- **Outbound HTTPS** *(optional, only for federation; not required for a
  local-only install).*

Disk: a few GiB plus whatever you plan to store in `data/`.

---

## 2. Quick install (SQLite, default backend)

### 2.1 Clone and generate keys

```bash
git clone <repository-url> aurora-locus
cd aurora-locus

# Two secp256k1 private keys, hex-encoded, exactly 32 bytes each.
openssl rand -hex 32 > repo_key.hex
openssl rand -hex 32 > plc_key.hex
```

Both `*_K256_PRIVATE_KEY_HEX` variables expect **exactly 64 hex characters**;
`openssl rand -hex 32` produces precisely that.

### 2.2 Write `.env`

Copy the template and fill in the three required values:

```bash
cp .env.example .env
```

At minimum, set:

```bash
# .env — minimum viable single-host SQLite install.
PDS_HOSTNAME=localhost
PDS_PORT=2583
PDS_SERVICE_DID=did:web:localhost
PDS_DATA_DIRECTORY=./data

# 32+ char random string for HS256 session JWT signing.
PDS_JWT_SECRET=$(openssl rand -hex 32)

# Paste the two key files (no newlines, no leading 0x).
PDS_REPO_SIGNING_KEY_K256_PRIVATE_KEY_HEX=<contents of repo_key.hex>
PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX=<contents of plc_key.hex>
```

Defaults that you do **not** need to set for a local install: `PDS_DB_BACKEND`
(defaults to `sqlite`), per-component DB paths (auto-derive from
`PDS_DATA_DIRECTORY`), blob storage (defaults to disk under
`PDS_DATA_DIRECTORY/blobs`), federation (off by default).

### 2.3 Validate the config

```bash
cargo run --bin aurora-locus -- validate-config
```

This catches required-but-missing variables, mismatched backend selectors,
and the well-known operator-error footguns before runtime. Fix any errors it
reports before continuing.

### 2.4 Run the server

```bash
cargo run --release --bin aurora-locus
```

The server creates `./data/` on first start (account DB, sequencer DB,
DID cache, per-actor stores, blob storage) and listens on `PDS_PORT`. Probe
it:

```bash
curl -s http://localhost:2583/health
curl -s http://localhost:2583/xrpc/com.atproto.server.describeServer | jq
```

A healthy install returns `200 OK` on `/health` and a JSON `describeServer`
response with the configured service DID.

---

## 3. Grant the first SuperAdmin

Aurora-Locus does not have an env-driven admin password or a hardcoded
bootstrap credential. Admin authority lives in the `admin_roles` table; the
first row is inserted by the offline `aurora-locus grant-admin` CLI
subcommand. Stop the PDS first (the CLI takes the same liveness lock
`serve` holds), grant the role, then restart and mint a session.

The complete bootstrap sequence — including the dev-route shortcut for
debug builds, the `dev.aurora.mintToken` helper for CI flows, and the
five-layer auth-resolution chain — is documented in
[operator/admin-auth.md](operator/admin-auth.md).

---

## 4. Upgrading to Postgres (multi-instance)

Postgres is a first-class shared-DB backend and is required for the
multi-instance HA story (leader election, distributed-state substrate for
DPoP / rate-limit buckets / OAuth flow state, LISTEN/NOTIFY cache
invalidation). Per-actor stores remain SQLite regardless of backend.

To switch a fresh install from SQLite to Postgres:

```bash
# 1. Provision Postgres (any reachable instance; this example uses Docker).
docker run -d --name aurora-pg \
  -e POSTGRES_USER=aurora -e POSTGRES_PASSWORD=aurora -e POSTGRES_DB=aurora \
  -p 5432:5432 postgres:16-alpine

# 2. Point .env at it.
echo 'PDS_DB_BACKEND=postgres'                                      >> .env
echo 'PDS_DB_URL=postgres://aurora:aurora@localhost:5432/aurora'    >> .env

# 3. Re-validate, then start.
cargo run --bin aurora-locus -- validate-config
cargo run --release --bin aurora-locus
```

Migrations run automatically on first start against an empty Postgres
database. The full multi-instance topology — leader election, maintenance
pool sizing, LISTEN/NOTIFY wiring, transaction-isolation pinning — is
documented in
[operator/multi-instance-deployment.md](operator/multi-instance-deployment.md).

For Postgres WAL archiving and point-in-time recovery, see
[operator/wal-archiving.md](operator/wal-archiving.md).

---

## 5. Next steps

- **All env vars.** [operator/configuration.md](operator/configuration.md) is
  the exhaustive reference (24 sections, every variable the server reads).
- **Admin & moderation.** [operator/admin-auth.md](operator/admin-auth.md).
- **Multi-instance HA.**
  [operator/multi-instance-deployment.md](operator/multi-instance-deployment.md).
- **Backup & restore.** [operator/backup-restore.md](operator/backup-restore.md)
  and [operator/wal-archiving.md](operator/wal-archiving.md).
- **Audit-chain verification.**
  [operator/audit-chain-verification.md](operator/audit-chain-verification.md).
- **Blob GC sweep** (dry-run by default; promote when ready).
  [operator/blob-gc-sweep.md](operator/blob-gc-sweep.md).
- **File-tier runtime settings.**
  [operator/file-tier-config.md](operator/file-tier-config.md).
- **Deployment posture & forensic recovery runbook.**
  [operator/deployment-posture.md](operator/deployment-posture.md).
