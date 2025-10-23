# Aurora Locus

**ATProto Personal Data Server Implementation in Rust**

Aurora Locus is a production-grade Personal Data Server (PDS) for the AT Protocol network, written in Rust. It provides secure, performant data storage and federation capabilities compatible with the Bluesky social network and other ATProto applications.

## Features

### Phase 1: Foundation (Current)
- [x] Configuration management from environment variables
- [x] HTTP server with Axum
- [x] Health check endpoint
- [x] Server description endpoint
- [x] Error handling framework
- [x] Logging and tracing

### Planned Features

**Phase 2: Account System**
- [ ] User registration
- [ ] Authentication (JWT)
- [ ] Session management
- [ ] Password hashing (Argon2id)

**Phase 3: Repository Operations**
- [ ] Per-user repositories
- [ ] Record CRUD operations
- [ ] MST (Merkle Search Tree) integration
- [ ] Schema validation

**Phase 4: Blob Storage**
- [ ] File upload support
- [ ] Disk storage backend
- [ ] S3 storage backend
- [ ] MIME type validation

**Phase 5: Synchronization**
- [ ] Repository synchronization
- [ ] CAR file export
- [ ] Event sequencer
- [ ] Firehose WebSocket

**Phase 6: Identity & Federation**
- [ ] DID resolution
- [ ] Handle management
- [ ] Cross-server identity
- [ ] PLC directory integration

**Phase 7: Admin & Moderation**
- [ ] Admin endpoints
- [ ] Account moderation
- [ ] Content labeling
- [ ] Invite code system

**Phase 8: Production Readiness**
- [ ] Rate limiting
- [ ] Email integration
- [ ] Metrics/observability
- [ ] Docker deployment

## Architecture

Aurora Locus is built on:

- **HTTP Server**: Axum (async, type-safe)
- **Database**: SQLite with sqlx
- **ATProto SDK**: Custom Rust implementation
- **Runtime**: Tokio async runtime

See [ARCHITECTURE.md](ARCHITECTURE.md) for detailed design documentation.

## Getting Started

### Prerequisites

- Rust 1.75+ (with Cargo)
- SQLite 3.35+

### Installation

```bash
# Clone the repository
cd "Aurora Locus"

# Copy environment template
cp .env.example .env

# Generate cryptographic keys (Linux/macOS)
openssl genrsa -out repo_key.pem 2048
openssl rsa -in repo_key.pem -outform DER | xxd -p -c 256 > repo_key.hex
openssl genrsa -out plc_key.pem 2048
openssl rsa -in plc_key.pem -outform DER | xxd -p -c 256 > plc_key.hex

# Update .env with the generated keys
# PDS_REPO_SIGNING_KEY_K256_PRIVATE_KEY_HEX=<contents of repo_key.hex>
# PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX=<contents of plc_key.hex>

# Build and run
cargo run
```

### Configuration

Configuration is managed through environment variables. See [.env.example](.env.example) for all available options.

**Required Configuration:**
- `PDS_HOSTNAME` - Server hostname
- `PDS_JWT_SECRET` - JWT signing secret (32+ chars)
- `PDS_ADMIN_PASSWORD` - Admin password (8+ chars)
- `PDS_REPO_SIGNING_KEY_K256_PRIVATE_KEY_HEX` - Repository signing key
- `PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX` - PLC rotation key

### Development

```bash
# Run in development mode with auto-reload
cargo watch -x run

# Run tests
cargo test

# Check code quality
cargo clippy -- -D warnings

# Format code
cargo fmt
```

## API Endpoints

### Current Endpoints

- `GET /health` - Health check
- `GET /xrpc/com.atproto.server.describeServer` - Server description

### Planned Endpoints

See [ARCHITECTURE.md](ARCHITECTURE.md) for the complete list of planned ATProto endpoints.

## Project Structure

```
Aurora Locus/
├── src/
│   ├── main.rs              # Application entry point
│   ├── config.rs            # Configuration management
│   ├── context.rs           # Dependency injection
│   ├── error.rs             # Error types
│   ├── server.rs            # HTTP server
│   ├── db/                  # Database layer (Phase 2+)
│   ├── account/             # Account management (Phase 2+)
│   ├── actor_store/         # Repository storage (Phase 3+)
│   ├── blob/                # Blob storage (Phase 4+)
│   ├── sequencer/           # Event sequencing (Phase 5+)
│   ├── identity/            # DID/handle resolution (Phase 6+)
│   └── api/                 # API endpoints (Phase 2+)
├── migrations/              # Database migrations
├── Cargo.toml               # Dependencies
├── .env.example             # Environment template
└── README.md                # This file
```

## Contributing

This project follows strict quality guidelines:

1. **No stubs** - All code must be fully implemented
2. **Comprehensive testing** - All features must have tests
3. **Best practices** - Follow Rust idioms and conventions
4. **Documentation** - All public APIs must be documented

See [CLAUDE.md](../CLAUDE.md) in the parent directory for detailed development guidelines.

## License

Dual-licensed under MIT OR Apache-2.0

## Acknowledgments

- Inspired by [bluesky-social/pds](https://github.com/bluesky-social/pds)
- Built with the [AT Protocol](https://atproto.com/)
- Uses the custom Rust ATProto SDK from the parent directory

## Status

**Current Phase**: Phase 1 (Foundation) - In Progress

**Version**: 0.1.0

**Production Ready**: No (under active development)
