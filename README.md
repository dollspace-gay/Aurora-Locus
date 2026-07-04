# Aurora Locus

**Production-Ready ATProto Personal Data Server in Rust**

Aurora Locus is a feature-complete Personal Data Server (PDS) for the AT Protocol network, written in Rust. Both SQLite (single-host) and Postgres (multi-instance HA) are first-class backends — the same binary adapts to whichever the operator configures. Federation, OAuth 2.1, and admin/moderation are all in the default build; compatible with the Bluesky social network and other ATProto applications.

## Features

### Core ATProto Functionality ✅
- [x] **Account Management** - OAuth 2.1 and JWT session auth; account lifecycle (create/deactivate/restore/delete) with refresh-token rotation
- [x] **Repository Operations** - Full CRUD with MST (Merkle Search Tree) integration
- [x] **Blob Storage** - Disk and S3-compatible storage backends
- [x] **Event Sequencing** - Monotonic event log for all repository operations
- [x] **Sync API** - CAR file export, repository synchronization
- [x] **Firehose** - Live WebSocket event streaming with backpressure handling
- [x] **Identity Resolution** - DID:PLC and DID:Web resolution with TTL-bounded handle and document cache

### Admin & Moderation ✅
- [x] **Role Management** - Moderator, Admin, SuperAdmin roles with granular permissions
- [x] **Account Moderation** - Discriminated `emitEvent` actions (takedown/suspend/restore/delete accounts; blob quarantine; record takedown; report and appeal resolution), single or batched
- [x] **Content Labels** - Apply and remove content labels
- [x] **Report System** - Submit and manage content/account reports
- [x] **Admin Namespace** - Canonical `tools.aurora.*` lex surface (admin/moderator/ops/superadmin)
- [x] **Hash-Chained Audit Log** - Tamper-evident per-row and chain-level verification; paginated trail; forensic tar-bundle export
- [x] **Runtime Settings** - `getRuntimeSetting`/`setRuntimeSetting` with four-tier config hierarchy

### Security & Performance ✅
- [x] **OAuth 2.1 with PKCE** - Mandatory S256 PKCE and refresh-token rotation
- [x] **DPoP** - Sender-constrained tokens with JTI replay tracking
- [x] **Rate Limiting** - Multi-axis throttling (global, per-endpoint, per-IP, per-user); distributed across instances
- [x] **Password Security** - Argon2id hashing
- [x] **JWT Sessions** - Refresh-token session auth; live with deprecation headers (OAuth 2.1 is the primary path)
- [x] **Required Validation Mode** - Hard-reject schema-violating writes before commit
- [x] **Optimistic Concurrency** - Swap CID validation for conflict prevention

### Production Features ✅
- [x] **Postgres Dual-Backend** - SQLite WAL and Postgres as first-class peers
- [x] **Multi-Instance Deployment** - HA via shared Postgres with leader election and LISTEN/NOTIFY cache invalidation
- [x] **Database Migrations** - Per-backend schema management for SQLite and Postgres
- [x] **Health Checks** - `/health` with live, ready, and detailed sub-endpoints
- [x] **GDPR Compliance** - Account deletion with grace period
- [x] **Background Jobs** - Grouped cleanup, cache reapers, sweeps, federation, and maintenance
- [x] **WAL Archiving + PITR** - Postgres backup and recovery surface

### Federation & Interop ✅
- [x] **Event-Stream Emission** - Account-lifecycle events (`#commit`/`#sync`/`#identity`/`#account`) on every path
- [x] **Dynamic Lexicon Loading** - DNS-TXT authority resolution, HTTP fetch, two-layer cache, single-flight de-dup
- [x] **Repository Import** - `importRepo` with per-blob pre-fetch and single-flight per-DID lock
- [x] **Cross-PDS Service Auth** - Per-account-signed service JWTs; tombstoned-issuer rejection
- [x] **Lexicon-Fetch Signature Verification** - Commit-signature check on fetched CARs

## Architecture

Aurora Locus is built on Rust with Tokio as the async runtime, Axum for type-safe HTTP routing, and sqlx (with the `any` feature) as the dual SQLite/Postgres database layer. The AT Protocol surface is provided by the [`proto-blue`](https://crates.io/crates/proto-blue) crate. Cryptography uses k256 (secp256k1) for repo signing and PLC rotation, p256 for DPoP, and Argon2id for password hashing.

For the full architecture — module layout, dual-backend design, multi-instance shape, federation surface — see [docs/architecture.md](docs/architecture.md).

## Getting Started

### Prerequisites

- Rust toolchain (stable, with Cargo)
- OpenSSL (for key generation below)
- Optional: Docker, for the Postgres upgrade path

### Quick start (SQLite, default backend)

```bash
git clone <repository-url> aurora-locus
cd aurora-locus

# Two secp256k1 private keys, hex-encoded — exactly 64 hex chars each.
openssl rand -hex 32 > repo_key.hex
openssl rand -hex 32 > plc_key.hex

cp .env.example .env

# Add these three lines to .env (the only values without sensible defaults):
#   PDS_JWT_SECRET=<32+ random chars>
#   PDS_REPO_SIGNING_KEY_K256_PRIVATE_KEY_HEX=<contents of repo_key.hex>
#   PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX=<contents of plc_key.hex>

cargo run --bin aurora-locus -- validate-config
cargo run --release --bin aurora-locus
```

The server creates `./data/` on first start and listens on `2583` by
default. Probe:

```bash
curl -s http://localhost:2583/health
curl -s http://localhost:2583/xrpc/com.atproto.server.describeServer | jq
```

### Further reading

- **[docs/getting-started.md](docs/getting-started.md)** — full install
  walkthrough, including the Postgres upgrade path.
- **[docs/operator/configuration.md](docs/operator/configuration.md)** —
  exhaustive environment-variable reference (24 sections, every variable
  the server reads).
- **[docs/operator/admin-auth.md](docs/operator/admin-auth.md)** —
  SuperAdmin bootstrap via `aurora-locus grant-admin` and the auth model.
- **[docs/operator/multi-instance-deployment.md](docs/operator/multi-instance-deployment.md)**
  — Postgres + leader election + LISTEN/NOTIFY topology.

## API Endpoints

Aurora Locus serves the standard ATProto XRPC surface (`com.atproto.*` for accounts, repo, sync, identity, and blob; OAuth at `/oauth/atproto/*`; well-known endpoints at `/.well-known/*`) plus the `tools.aurora.*` admin / moderator / ops / superadmin namespace with Aurora-specific extensions. The broad XRPC surface follows the ATProto spec and is discoverable at runtime from `com.atproto.server.describeServer`; the admin / moderation / ops surface is documented in detail.

See [docs/operator/admin-endpoint-reference.md](docs/operator/admin-endpoint-reference.md) for the admin / moderation / ops endpoint inventory.

## Development

```bash
# Run with auto-reload
cargo watch -x run

# Run lib tests (SQLite-backed; fast)
cargo test --lib

# Run Postgres integration tests (requires Docker; spins up
# postgres:16-alpine via testcontainers)
cargo test --test postgres_smoke_test -- --test-threads=1
cargo test --test multi_instance_test -- --test-threads=1

# Check code
cargo clippy

# Format
cargo fmt

# Build release
cargo build --release
```

### Dual-backend test setup

Aurora-Locus supports both SQLite (default, single-instance) and
Postgres (multi-instance) via `sqlx::Any`. CI runs both backends on
every commit:

- **SQLite job** — `cargo test --lib` covers the full test suite
  against SQLite (default backend; ~543 tests).
- **Postgres job** — `cargo test --test postgres_smoke_test`
  (per-manager round-trips, 6 tests) plus `cargo test --test
  multi_instance_test` (leader election + LISTEN/NOTIFY cache
  invalidation, 5 tests) against a real Postgres spun up via
  testcontainers. Catches placeholder-syntax, bool-decode, and
  bool-literal incompatibilities that SQLite-only testing can't
  surface.

Both jobs must pass for a commit to be considered green. Local
Postgres testing requires Docker daemon access — the test fixtures
panic with a clear message if Docker is unreachable. Promoting the
full lib suite to also run against Postgres (instead of just the
smoke + integration coverage) is a future-cycle concern; see the
header comment in `tests/postgres_smoke_test.rs` for the rationale.

## Deployment

### Docker (Recommended)

```bash
# Build image
docker build -t aurora-locus .

# Run container
docker run -d \
  -p 2583:2583 \
  -v $(pwd)/data:/app/data \
  -v $(pwd)/.env:/app/.env \
  --name aurora-locus \
  aurora-locus
```

The container listens on `PDS_PORT` (default `2583`). To run on a different
port, set `PDS_PORT` in `.env` and adjust the port mapping to match
(`-p <host-port>:<PDS_PORT>`) — the container-side port must equal
`PDS_PORT` or the published mapping won't reach the server.

### Systemd Service

Docker, Kubernetes, and bare-metal systemd are all supported runtimes.
The one configuration that ships a *verified* forensic-recovery runbook
for Option A failures (`apply_writes` Phase A committed, Phase B failed
mid-flight) is systemd + local journald — other sinks can host the same
recovery procedure but those adaptations aren't verified. See
[`docs/operator/deployment-posture.md`](docs/operator/deployment-posture.md)
for the verified runbook.

```ini
[Unit]
Description=Aurora Locus PDS
After=network.target

[Service]
Type=simple
User=pds
WorkingDirectory=/opt/aurora-locus
ExecStart=/opt/aurora-locus/target/release/aurora-locus
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
```

### Reverse Proxy (nginx)

```nginx
server {
    listen 443 ssl http2;
    server_name pds.example.com;

    ssl_certificate /etc/letsencrypt/live/pds.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/pds.example.com/privkey.pem;

    location / {
        proxy_pass http://127.0.0.1:3000;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}
```

## Performance

Aurora Locus is designed for low-latency async operation: non-blocking I/O via Tokio, connection pooling via sqlx, streaming for large CAR exports and the firehose, and compile-time query validation. Real-world performance is workload-and-hardware-dependent; this README does not carry canonical benchmark numbers.

See [docs/operator/performance.md](docs/operator/performance.md) for the observability surface (health endpoints, Prometheus `/metrics`, tracing) and profiling guidance.

## Security

OAuth 2.1 with mandatory PKCE, DPoP sender-bound tokens with JTI replay tracking, Argon2id password hashing, role-based access control, schema validation on writes, and multi-axis rate limiting (distributed across instances under the Postgres backend) — the security posture details and the disclosure process live in the security policy.

See [SECURITY.md](SECURITY.md) for the security policy and the disclosure process.

## Contributing

Contributions welcome! Please ensure:

1. **No stubs** - All code must be fully implemented
2. **Tests** - All features must have comprehensive tests
3. **Documentation** - Public APIs must be documented
4. **Best practices** - Follow Rust idioms and conventions

## License

Dual-licensed under MIT OR Apache-2.0

## Acknowledgments

- Inspired by [bluesky-social/pds](https://github.com/bluesky-social/pds)
- Built with the [AT Protocol](https://atproto.com/)
- Built on the [`proto-blue`](https://crates.io/crates/proto-blue) ATProto SDK

---

For questions or support, please open an issue on GitHub.
