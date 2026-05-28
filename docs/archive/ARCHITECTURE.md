# Aurora Locus - ATProto PDS Implementation Plan

## Executive Summary

Aurora Locus is a Rust implementation of an ATProto Personal Data Server (PDS), inspired by the official TypeScript implementation (bluesky-pds). This document maps the TypeScript PDS features to our Rust SDK capabilities and outlines the implementation strategy.

---

## Feature Mapping: TypeScript PDS → Rust SDK → Implementation Plan

### ✅ FULLY COVERED BY SDK (Can Use Directly)

| TypeScript PDS Component | Rust SDK Component | Status | Notes |
|-------------------------|-------------------|--------|-------|
| **XRPC Client** | `src/xrpc/mod.rs` | ✅ Complete | HTTP client with retry logic |
| **Session Management** | `src/session_manager.rs` | ✅ Complete | Token management, refresh logic |
| **Repository** | `src/repo.rs` | ✅ Complete | Commit creation, record CRUD |
| **MST (Merkle Search Tree)** | `src/mst.rs` | ✅ Complete | Full MST implementation |
| **CAR Files** | `src/car.rs` | ✅ Complete | CAR v1 read/write |
| **DID Resolution** | `src/did_doc.rs`, `src/handle.rs` | ✅ Complete | DNS + HTTPS resolution |
| **JWT Auth** | `src/server_auth.rs` | ✅ Complete | RS256 signing, Argon2id hashing |
| **OAuth 2.0** | `src/oauth/` | ✅ Complete | PKCE, DPoP support |
| **Schema Validation** | `src/validation.rs` | ✅ Complete | Lexicon validation |
| **Rich Text** | `src/rich_text/` | ✅ Complete | Facet detection, mentions, links |
| **Moderation** | `src/moderation/` | ✅ Complete | Label system |
| **TID Generation** | `src/tid.rs` | ✅ Complete | Timestamp IDs |
| **Client API Methods** | `src/client/` (334 files) | ✅ Complete | All XRPC endpoints |

### 🟨 PARTIALLY COVERED (SDK Provides Building Blocks)

| TypeScript PDS Component | Rust SDK Status | What's Missing | Implementation Needed |
|-------------------------|-----------------|----------------|----------------------|
| **Account Management** | Auth utilities exist | No account database layer | Database schema + CRUD operations |
| **Actor Store** | Repo/MST exist | No per-user storage orchestration | Per-user SQLite databases + file management |
| **Blob Storage** | MIME detection exists | No storage backend | Disk + S3 implementations |
| **Sequencer/Firehose** | WebSocket subscription exists | No event log | Event database + outbox pattern |
| **DID Cache** | Resolution exists | No caching layer | TTL-based cache with database |
| **API Server** | XRPC types exist | No HTTP server framework | Axum/Actix server + routing |

### ❌ NOT IN SDK (Must Implement)

| TypeScript PDS Component | Implementation Approach |
|-------------------------|------------------------|
| **SQLite Database Layer** | Use `rusqlite` or `sqlx` with migrations |
| **HTTP Server** | Use `axum` (tokio-based) or `actix-web` |
| **Email/SMTP** | Use `lettre` crate |
| **Rate Limiting** | Use `governor` crate or custom implementation |
| **Background Jobs** | Use `tokio::spawn` or `tokio-cron-scheduler` |
| **Configuration Management** | Use `config` crate or custom env parsing |
| **S3 Integration** | Use `aws-sdk-s3` or `rusoto_s3` |
| **Redis (optional)** | Use `redis` crate for distributed rate limiting |
| **Invite System** | Custom implementation with database |
| **Well-Known Routes** | Custom HTTP handlers for `/.well-known/*` |

---

## Critical Gaps Analysis

### 1. Database Layer (HIGH PRIORITY)
**TypeScript**: Uses SQLite via `better-sqlite3` + Kysely ORM with 4 separate databases:
- Account DB (users, sessions, tokens, invites)
- Sequencer DB (event log)
- DID Cache DB (resolved DIDs)
- Per-user Actor DBs (repository state)

**Rust Options**:
- `sqlx` - Async SQLite with compile-time query checking
- `rusqlite` - Sync SQLite (same as TypeScript, simpler)
- `diesel` - Full ORM (heavier)

**Recommendation**: Use `sqlx` for async compatibility with Tokio runtime.

### 2. HTTP Server (HIGH PRIORITY)
**TypeScript**: Express.js with middleware stack

**Rust Options**:
- `axum` - Tokio-native, type-safe extractors, composable middleware
- `actix-web` - Mature, fast, actor-based
- `warp` - Filter-based routing

**Recommendation**: Use `axum` for seamless Tokio integration and type safety.

### 3. Blob Storage Backends (MEDIUM PRIORITY)
**TypeScript**:
- Disk: Simple filesystem storage
- S3: AWS SDK integration

**Rust Implementation**:
- Disk: Use `tokio::fs` for async file I/O
- S3: Use `aws-sdk-s3` crate

### 4. Event Sequencer (MEDIUM PRIORITY)
**TypeScript**: Custom sequencer with:
- SQLite event log
- In-memory outbox buffer
- WebSocket broadcast
- Cursor-based resume

**Rust Implementation**:
- Database table for events
- `tokio::sync::broadcast` channel for live events
- WebSocket handler with `axum-tungstenite`

### 5. Configuration System (LOW PRIORITY)
**TypeScript**: Environment variables with defaults

**Rust Implementation**:
- Use `dotenv` for `.env` file loading
- Custom struct with `serde` + validation
- Or use `config` crate for multiple sources

---

## Architecture Design for Aurora Locus

### Directory Structure
```
Aurora Locus/
├── src/
│   ├── main.rs                    # Server entry point
│   ├── config.rs                  # Configuration loading
│   ├── server.rs                  # HTTP server setup (Axum)
│   ├── context.rs                 # AppContext (DI container)
│   ├── error.rs                   # Unified error types
│   │
│   ├── db/
│   │   ├── mod.rs                 # Database connection management
│   │   ├── account.rs             # Account database schema
│   │   ├── sequencer.rs           # Sequencer database schema
│   │   ├── did_cache.rs           # DID cache schema
│   │   ├── actor.rs               # Actor store schema (per-user)
│   │   └── migrations/            # SQL migration files
│   │
│   ├── account/
│   │   ├── mod.rs                 # Account manager
│   │   ├── creation.rs            # Account creation logic
│   │   ├── authentication.rs      # Login/password verification
│   │   ├── tokens.rs              # Token generation/validation
│   │   └── invites.rs             # Invite code system
│   │
│   ├── actor_store/
│   │   ├── mod.rs                 # Actor store manager
│   │   ├── storage.rs             # Per-user SQLite management
│   │   ├── reader.rs              # Read operations
│   │   ├── writer.rs              # Write operations
│   │   └── transactor.rs          # Transactional writes
│   │
│   ├── blob/
│   │   ├── mod.rs                 # Blob store trait
│   │   ├── disk.rs                # Disk storage backend
│   │   └── s3.rs                  # S3 storage backend
│   │
│   ├── sequencer/
│   │   ├── mod.rs                 # Event sequencer
│   │   ├── events.rs              # Event types (commit, identity, etc.)
│   │   ├── outbox.rs              # In-memory event buffer
│   │   └── subscription.rs        # WebSocket firehose handler
│   │
│   ├── identity/
│   │   ├── mod.rs                 # DID/handle management
│   │   ├── resolver.rs            # DID resolution with caching
│   │   └── plc.rs                 # PLC directory integration
│   │
│   ├── api/
│   │   ├── mod.rs                 # Route registration
│   │   ├── middleware.rs          # Auth, rate limiting, logging
│   │   ├── well_known.rs          # .well-known endpoints
│   │   │
│   │   ├── com/
│   │   │   └── atproto/
│   │   │       ├── server/        # Auth endpoints
│   │   │       ├── repo/          # Repository endpoints
│   │   │       ├── sync/          # Sync endpoints
│   │   │       ├── identity/      # Identity endpoints
│   │   │       └── admin/         # Admin endpoints
│   │   │
│   │   └── app/
│   │       └── bsky/
│   │           ├── actor/         # Actor endpoints
│   │           ├── feed/          # Feed endpoints (proxy mostly)
│   │           └── notification/  # Notification endpoints
│   │
│   ├── mailer/
│   │   ├── mod.rs                 # Email sending
│   │   └── templates.rs           # Email templates
│   │
│   ├── rate_limit/
│   │   ├── mod.rs                 # Rate limiting logic
│   │   └── store.rs               # In-memory or Redis store
│   │
│   └── util/
│       ├── mod.rs
│       └── background.rs          # Background job runner
│
├── migrations/                    # Database migrations
├── Cargo.toml
├── .env.example
└── README.md
```

### Core Dependencies (Cargo.toml additions)
```toml
[dependencies]
# Use parent SDK
atproto = { path = ".." }

# HTTP Server
axum = "0.8"
axum-extra = "0.10"  # For middleware utilities
tower = "0.5"
tower-http = "0.6"   # CORS, tracing, etc.

# Database
sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite"] }

# Async runtime
tokio = { version = "1", features = ["full"] }

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# Configuration
dotenv = "0.15"

# Error handling
thiserror = "2"
anyhow = "1"

# Logging
tracing = "0.1"
tracing-subscriber = "0.3"

# Email
lettre = "0.11"

# Rate limiting
governor = "0.7"

# Blob storage (S3)
aws-sdk-s3 = "1"
aws-config = "1"

# Utilities
chrono = "0.4"
uuid = { version = "1", features = ["v4"] }
```

---

## Implementation Phases

### Phase 1: Foundation (Week 1)
**Goal**: Basic server with health check

- [x] Project structure setup
- [ ] Configuration system
- [ ] Database connection management
- [ ] Axum server with basic routing
- [ ] Error handling framework
- [ ] Logging setup
- [ ] Health check endpoint

**Deliverable**: Server that starts and responds to `GET /health`

### Phase 2: Account System (Week 2)
**Goal**: User registration and authentication

- [ ] Account database schema + migrations
- [ ] Account creation endpoint (`com.atproto.server.createAccount`)
- [ ] Session management (login/logout)
- [ ] JWT token generation/validation
- [ ] Password hashing (use SDK's Argon2id)
- [ ] Email verification flow (optional)

**Deliverable**: Users can register, login, and authenticate

### Phase 3: Repository Operations (Week 3)
**Goal**: Core data storage

- [ ] Actor store initialization (per-user SQLite)
- [ ] Repository creation on account signup
- [ ] `com.atproto.repo.createRecord` endpoint
- [ ] `com.atproto.repo.putRecord` endpoint
- [ ] `com.atproto.repo.deleteRecord` endpoint
- [ ] `com.atproto.repo.getRecord` endpoint
- [ ] Record validation (use SDK's validation)
- [ ] MST integration (use SDK's MST)

**Deliverable**: Users can create, read, update, delete records

### Phase 4: Blob Storage (Week 4)
**Goal**: File uploads

- [ ] Blob store trait
- [ ] Disk storage implementation
- [ ] `com.atproto.repo.uploadBlob` endpoint
- [ ] Blob reference tracking
- [ ] MIME type validation (use SDK's blob utilities)
- [ ] S3 storage implementation (optional)

**Deliverable**: Users can upload images/files

### Phase 5: Synchronization (Week 5)
**Goal**: Federation support

- [ ] Sequencer database schema
- [ ] Event log recording on commits
- [ ] `com.atproto.sync.getRepo` endpoint (CAR export)
- [ ] `com.atproto.sync.getLatestCommit` endpoint
- [ ] `com.atproto.sync.subscribeRepos` WebSocket
- [ ] Firehose outbox buffer

**Deliverable**: Other servers can sync repositories

### Phase 6: Identity & Federation (Week 6)
**Goal**: Cross-server identity

- [ ] DID cache database
- [ ] Handle resolution with caching
- [ ] `com.atproto.identity.resolveHandle` endpoint
- [ ] `com.atproto.identity.updateHandle` endpoint
- [ ] PLC directory integration
- [ ] Well-known DID document endpoint

**Deliverable**: Handles resolve, DIDs are discoverable

### Phase 7: Admin & Moderation (Week 7)
**Goal**: Server management

- [ ] Admin authentication
- [ ] Admin endpoints (account management)
- [ ] Account takedown/suspension
- [ ] Label application
- [ ] Invite code system

**Deliverable**: Server can be administered

### Phase 8: Production Readiness (Week 8)
**Goal**: Deploy-ready PDS

- [ ] Rate limiting implementation
- [ ] Email system integration
- [ ] Background job runner
- [ ] Comprehensive error responses
- [ ] Metrics/observability
- [ ] Docker containerization
- [ ] Documentation

**Deliverable**: Production-ready PDS

---

## Risk Assessment

### High Risk
1. **Database Performance**: Per-user SQLite files may have I/O bottlenecks
   - *Mitigation*: Connection pooling, WAL mode, async I/O

2. **Concurrent Actor Store Access**: Multiple requests to same user
   - *Mitigation*: Per-DID locks, transaction isolation

### Medium Risk
1. **Firehose Memory Usage**: Outbox buffer may grow large
   - *Mitigation*: Configurable buffer size, backpressure

2. **S3 Upload Performance**: Large file uploads
   - *Mitigation*: Multipart uploads, streaming

### Low Risk
1. **OAuth Complexity**: Full OAuth provider implementation
   - *Mitigation*: SDK already has OAuth support

---

## Success Criteria

### Minimum Viable PDS
- [ ] Users can create accounts
- [ ] Users can create, read, update, delete posts
- [ ] Users can upload images
- [ ] Other PDSs can sync repositories
- [ ] Handles resolve correctly
- [ ] Basic admin operations work

### Production PDS
- [ ] All com.atproto.* endpoints implemented
- [ ] Rate limiting enforced
- [ ] Email verification works
- [ ] Firehose streams reliably
- [ ] Monitoring/logging comprehensive
- [ ] Docker deployment ready
- [ ] Migration tools available

---

## Next Steps

1. Review this plan with team/user
2. Set up Cargo.toml with dependencies
3. Create initial module structure
4. Begin Phase 1 implementation

**Estimated Timeline**: 8 weeks for MVP, 12 weeks for production-ready

**Team Size**: 1-2 developers

**Complexity**: High - requires deep understanding of ATProto, async Rust, databases
