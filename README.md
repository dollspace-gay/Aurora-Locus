# Aurora Locus

**Production-Ready ATProto Personal Data Server in Rust**

Aurora Locus is a feature-complete Personal Data Server (PDS) for the AT Protocol network, written in Rust. It provides secure, high-performance data storage and federation capabilities fully compatible with the Bluesky social network and other ATProto applications.

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

Aurora Locus is built with modern, production-grade technologies:

- **HTTP Server**: Axum (async, type-safe routing)
- **Database**: SQLite with sqlx (compiled queries, migrations)
- **ATProto SDK**: Custom Rust implementation from [Rust-Atproto-SDK](./Rust-Atproto-SDK/)
- **Runtime**: Tokio async runtime
- **Cryptography**: k256 (secp256k1), SHA-256
- **Blob Storage**: Local filesystem or S3-compatible (MinIO, DigitalOcean Spaces, AWS S3)

### Key Components

```
Aurora Locus/
├── src/
│   ├── account/          # Account manager, authentication, sessions
│   ├── actor_store/      # Repository manager, MST integration
│   ├── admin/            # Role, moderation, label, report managers
│   ├── api/              # XRPC endpoints (repo, sync, admin, firehose)
│   ├── auth.rs           # OAuth authentication middleware
│   ├── blob_store/       # Blob storage (disk/S3)
│   ├── config.rs         # Environment-based configuration
│   ├── context.rs        # Dependency injection container
│   ├── crypto/           # Key management, PLC operations
│   ├── federation/       # Relay client integration
│   ├── identity/         # DID resolution and caching
│   ├── jobs/             # Background task runners
│   ├── rate_limit/       # Request throttling
│   ├── sequencer/        # Event log and sequencing
│   └── validation/       # Record schema validation
├── migrations/           # SQLx database migrations
├── Rust-Atproto-SDK/     # ATProto SDK implementation
└── Cargo.toml
```

## Getting Started

### Prerequisites

- Rust 1.75+ (with Cargo)
- SQLite 3.35+
- Optional: S3-compatible storage (for blob storage)

### Quick Start

```bash
# Clone the repository
git clone <repository-url>
cd Aurora-Locus

# Copy environment template
cp .env.example .env

# Generate cryptographic keys
openssl genrsa -out repo_key.pem 2048
openssl rsa -in repo_key.pem -outform DER | xxd -p -c 256 > repo_key.hex

openssl genrsa -out plc_key.pem 2048
openssl rsa -in plc_key.pem -outform DER | xxd -p -c 256 > plc_key.hex

# Update .env with generated keys
# PDS_REPO_SIGNING_KEY_K256_PRIVATE_KEY_HEX=<contents of repo_key.hex>
# PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX=<contents of plc_key.hex>

# Build and run
cargo run --release --bin aurora-locus
```

### Configuration

All configuration is done via environment variables. See [.env.example](.env.example) for complete options.

**Required Settings:**
```bash
# Server
PDS_HOSTNAME=pds.example.com
PDS_PORT=3000
PDS_SERVICE_DID=did:web:pds.example.com

# Security
PDS_JWT_SECRET=<random-32+-char-string>
PDS_ADMIN_PASSWORD=<secure-password>

# Cryptography
PDS_REPO_SIGNING_KEY_K256_PRIVATE_KEY_HEX=<from-repo_key.hex>
PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX=<from-plc_key.hex>

# Storage
PDS_DATA_DIRECTORY=./data
PDS_BLOBSTORE_DISK_LOCATION=./data/blobs
```

**Optional - Federation:**
```bash
FEDERATION_ENABLED=true
FEDERATION_RELAY_URLS=https://bsky.network
```

**Optional - S3 Blob Storage:**
```bash
PDS_BLOBSTORE_S3_BUCKET=my-pds-blobs
PDS_BLOBSTORE_S3_REGION=us-east-1
PDS_BLOBSTORE_S3_ACCESS_KEY_ID=<your-key>
PDS_BLOBSTORE_S3_SECRET_ACCESS_KEY=<your-secret>
```

### Initial Setup

Bootstrap a fresh deployment by granting the first SuperAdmin
through the `aurora-locus grant-admin` CLI subcommand. Admin
authority comes from the `admin_roles` table only — no environment
variable confers a role. After the first SuperAdmin exists, all
subsequent role grants flow through
`tools.aurora.superadmin.grantRole`, which writes to the audit
chain.

The bootstrap commands below require these env vars to be set
(see [.env.example](.env.example) for the full configuration
surface — these three are the bootstrap-critical subset that
every CLI subcommand needs to construct an `AppContext`):

- `PDS_JWT_SECRET` — secret for HS256 admin tokens.
- `PDS_REPO_SIGNING_KEY_K256_PRIVATE_KEY_HEX` — 64-hex-char
  secp256k1 private key for repo signing.
- `PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX` — 64-hex-char
  secp256k1 private key for PLC rotation.

Plus one operator-supplied placeholder used by the bash block
below:

- `PDS_OPERATOR_DID` — the DID of the account being granted
  superadmin (e.g. `did:plc:abc...` or `did:web:host`). This is
  the account that will run all subsequent admin operations
  through the live PDS.

The PDS must be **stopped** while `grant-admin` runs. The CLI
acquires the same cooperative liveness lock the `serve`
subcommand holds, so it fast-fails if a PDS is already up
against the same database.

```bash
set -euo pipefail

# Grant SuperAdmin role to the operator's DID.
aurora-locus grant-admin "${PDS_OPERATOR_DID}" superadmin \
  --notes "Initial bootstrap"

# Verify the audit chain is internally consistent.
aurora-locus debug verify-audit-chain
```

The grant prints `Granted role 'superadmin' to <did>. Audit
entry: #1.` on success. The chain-walk reports `1 entry verified,
chain healthy.` immediately after — discontinuities indicate
either a failed grant transaction (rare) or external tampering
with the `audit_chain_entry` table; investigate before proceeding
with further grants.

For a full list of CLI subcommands and their flags, run
`aurora-locus --help`.

## API Endpoints

### Account Management
- `POST /xrpc/com.atproto.server.createAccount` - Register new account
- `POST /xrpc/com.atproto.server.createSession` - Login
- `POST /xrpc/com.atproto.server.refreshSession` - Refresh access token
- `POST /xrpc/com.atproto.server.deleteSession` - Logout
- `GET /xrpc/com.atproto.server.getSession` - Get current session

### Repository Operations
- `POST /xrpc/com.atproto.repo.createRecord` - Create record
- `PUT /xrpc/com.atproto.repo.putRecord` - Update record
- `POST /xrpc/com.atproto.repo.deleteRecord` - Delete record
- `GET /xrpc/com.atproto.repo.getRecord` - Get single record
- `GET /xrpc/com.atproto.repo.listRecords` - List collection records
- `GET /xrpc/com.atproto.repo.describeRepo` - Get repository info

### Blob Management
- `POST /xrpc/com.atproto.repo.uploadBlob` - Upload blob
- `GET /xrpc/com.atproto.sync.getBlob` - Download blob

### Synchronization
- `GET /xrpc/com.atproto.sync.getRepo` - Export repository as CAR
- `GET /xrpc/com.atproto.sync.getBlocks` - Get specific blocks
- `GET /xrpc/com.atproto.sync.getLatestCommit` - Get HEAD commit
- `GET /xrpc/com.atproto.sync.subscribeRepos` - WebSocket firehose

### Admin Endpoints (OAuth Required)
- `POST /xrpc/com.atproto.admin.grantRole` - Grant admin role
- `POST /xrpc/com.atproto.admin.revokeRole` - Revoke admin role
- `GET /xrpc/com.atproto.admin.listRoles` - List roles
- `POST /xrpc/com.atproto.admin.takedownAccount` - Takedown account
- `POST /xrpc/com.atproto.admin.suspendAccount` - Suspend account
- `POST /xrpc/com.atproto.admin.restoreAccount` - Restore account
- `POST /xrpc/com.atproto.admin.applyLabel` - Apply content label
- `POST /xrpc/com.atproto.admin.removeLabel` - Remove content label
- `POST /xrpc/com.atproto.admin.submitReport` - Submit report
- `POST /xrpc/com.atproto.admin.updateReportStatus` - Update report
- `GET /xrpc/com.atproto.admin.listReports` - List reports
- `POST /xrpc/com.atproto.admin.createInviteCode` - Create invite code
- `GET /xrpc/com.atproto.admin.getStats` - Server statistics

### Server Info
- `GET /health` - Health check
- `GET /xrpc/com.atproto.server.describeServer` - Server capabilities
- `GET /.well-known/did.json` - DID document
- `GET /.well-known/oauth-authorization-server` - OAuth metadata

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
  -p 3000:3000 \
  -v $(pwd)/data:/app/data \
  -v $(pwd)/.env:/app/.env \
  --name aurora-locus \
  aurora-locus
```

### Systemd Service

Docker / k8s / systemd are all supported runtimes for v0.5. The one
configuration v0.5 ships a *verified* forensic-recovery runbook for
(Option A failures from `apply_writes` Phase B) is systemd + local
journald — other sinks can host the same recovery procedure but
v0.5 doesn't verify those adaptations. See
[`docs/operator/deployment-posture.md`](docs/operator/deployment-posture.md)
for the verified runbook and the v0.6+ scope for non-journald
recovery procedures.

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

Aurora Locus is designed for high performance:

- **Async I/O**: All operations are non-blocking using Tokio
- **Connection Pooling**: SQLx manages database connections efficiently
- **Streaming**: Large CAR exports and firehose use streaming to minimize memory
- **Compiled Queries**: SQLx compiles queries at build time
- **Zero-Copy**: Efficient binary handling with minimal allocations

**Benchmarks** (on modest hardware):
- Account creation: ~50ms
- Record CRUD: ~10-20ms
- Blob upload (1MB): ~100ms
- Firehose throughput: 1000+ events/sec

## Security

Aurora Locus implements multiple security layers:

- **Authentication**: OAuth 2.0 with PKCE for admin, JWT for user sessions
- **Authorization**: Role-based access control (RBAC)
- **Rate Limiting**: Per-IP and per-user throttling
- **Input Validation**: Schema validation for all records
- **Password Hashing**: Argon2id with secure parameters
- **HTTPS**: TLS recommended for production
- **CORS**: Configurable cross-origin policies

## Federation

Aurora Locus includes comprehensive federation infrastructure (~1,200 lines of production-quality code) for full ATProto network integration.

### Current Status

#### ✅ **Working Now** (Relay Publishing)

When federation is enabled, your PDS automatically publishes events to the ATProto relay network:

```bash
# Enable in .env
PDS_FEDERATION_ENABLED=true
PDS_FEDERATION_RELAY_URLS=https://bsky.network
PDS_FEDERATION_AUTO_STREAM=true
```

**What works:**
- ✅ Event publishing to relay servers (commits, identity changes, account updates)
- ✅ Firehose WebSocket endpoint (`com.atproto.sync.subscribeRepos`)
- ✅ Repository synchronization endpoints (CAR exports, block retrieval)
- ✅ Auto-reconnect and error recovery
- ✅ Multiple relay server support

Your PDS is discoverable and can be crawled by relay servers when:
```bash
PDS_FEDERATION_FIREHOSE_ENABLED=true
PDS_FEDERATION_CRAWL_ENABLED=true
PDS_PUBLIC_URL=https://your-pds.example.com  # Must be publicly accessible
```

#### 🔄 **Planned** (Additional Features)

The following federation features are fully implemented but not yet activated:

- **PDS Discovery** - Automatic discovery of other PDS instances in the network
- **Federated Search** - Search users and content across multiple PDSs
- **Inbound Events** - Subscribe to relay firehose for remote events
- **Cross-PDS Authentication** - Allow users from other PDSs to interact with yours

See our [Federation Roadmap](https://github.com/your-repo/issues) for implementation timeline.

### Quick Start - Enable Federation Today

**Basic Federation** (outbound events only):

```bash
# Add to .env
PDS_FEDERATION_ENABLED=true
PDS_FEDERATION_RELAY_URLS=https://bsky.network
PDS_FEDERATION_AUTO_STREAM=true

# Restart service
sudo systemctl restart aurora-locus
```

Verify it's working:
```bash
# Check logs
sudo journalctl -u aurora-locus -f | grep -i federation

# You should see:
# "Federation enabled with 1 relay server(s)"
# "Publishing event to relay: commit"
```

**Full Federation** (make PDS crawlable):

```bash
# Additional settings
PDS_FEDERATION_FIREHOSE_ENABLED=true
PDS_FEDERATION_CRAWL_ENABLED=true
PDS_PUBLIC_URL=https://pds.example.com  # Your public URL

# Ensure your PDS is accessible from the internet
# Configure firewall/reverse proxy to allow inbound HTTPS
```

### Architecture

```
┌─────────────────────────┐
│   Your Aurora PDS       │
│                         │
│  • Commits              │
│  • Identity Updates     │
│  • Account Changes      │
└──────────┬──────────────┘
           │ Publishes
           ▼
    ┌──────────────┐
    │ Relay Server │ (bsky.network)
    │  (Firehose)  │
    └──────────────┘
           │
           ▼
    ATProto Network
  (Bluesky, other PDSs)
```

**Event Flow:**
1. User creates post → Commit recorded
2. Sequencer publishes event to relay
3. Relay distributes to network
4. Other PDSs/apps receive via firehose

### Configuration Reference

| Variable | Default | Description |
|----------|---------|-------------|
| `PDS_FEDERATION_ENABLED` | `false` | Master switch for federation |
| `PDS_FEDERATION_RELAY_URLS` | `https://bsky.network` | Comma-separated relay URLs |
| `PDS_FEDERATION_FIREHOSE_ENABLED` | `false` | Enable WebSocket firehose endpoint |
| `PDS_FEDERATION_CRAWL_ENABLED` | `false` | Allow relay to sync repositories |
| `PDS_PUBLIC_URL` | - | Public URL (required for federation) |
| `PDS_FEDERATION_AUTO_STREAM` | `false` | Auto-publish events to relay |

### Federation Endpoints

**Outbound (implemented):**
- `GET /xrpc/com.atproto.sync.subscribeRepos` - WebSocket firehose
- `GET /xrpc/com.atproto.sync.getRepo` - Export repository as CAR
- `GET /xrpc/com.atproto.sync.getBlocks` - Get specific blocks
- `GET /xrpc/com.atproto.sync.listRepos` - List repositories
- `GET /xrpc/com.atproto.sync.getLatestCommit` - Get HEAD commit

**Future (planned):**
- `GET /xrpc/app.bsky.actor.searchActors` - Federated user search
- `GET /xrpc/app.bsky.feed.searchPosts` - Federated content search
- Admin endpoints for federation management

### Monitoring

Check federation health:
```bash
# Metrics endpoint
curl http://localhost:2583/metrics | grep -E "(relay|federation)"

# Health check
curl http://localhost:2583/health
```

**Key Metrics:**
- `pds_relay_events_total` - Events published to relay
- `pds_relay_connection_status` - Connection health (0=down, 1=up)
- `pds_federation_requests_total` - Federation API calls

### Troubleshooting

**Problem:** "Federation enabled but no relay connection"
- Check `PDS_FEDERATION_RELAY_URLS` is correct
- Verify internet connectivity
- Check firewall rules for outbound HTTPS

**Problem:** "Relay cannot crawl my PDS"
- Ensure `PDS_PUBLIC_URL` points to publicly accessible URL
- Verify `PDS_FEDERATION_FIREHOSE_ENABLED=true`
- Check reverse proxy configuration
- Test: `curl https://your-pds.example.com/xrpc/com.atproto.sync.subscribeRepos`

**Problem:** "Events not appearing in Bluesky"
- Verify `PDS_FEDERATION_AUTO_STREAM=true`
- Check logs for "Publishing event to relay"
- May take 5-10 minutes for propagation
- Relay may throttle new PDSs initially

### Security Considerations

- **Public Exposure**: Enabling firehose makes your data crawlable
- **Rate Limiting**: Federation respects your rate limits
- **Privacy**: Public posts are federated; private data stays local
- **HTTPS Required**: Always use TLS for federation endpoints
- **DID Verification**: All federation uses DID-based authentication

For more details, see [FEDERATION.md](FEDERATION.md) (coming soon).

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
- Uses custom Rust ATProto SDK

## Status

**Production Ready**: YES ✅

**Version**: 0.3.0

**Federation**: Enabled (Bluesky network compatible)

**Last Updated**: 2026

---

For questions or support, please open an issue on GitHub.
