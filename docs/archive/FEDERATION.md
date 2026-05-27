# Aurora Locus Federation Guide

**Comprehensive guide to ATProto federation with Aurora Locus**

---

## Table of Contents

1. [Overview](#overview)
2. [Quick Start](#quick-start)
3. [Configuration](#configuration)
4. [Architecture](#architecture)
5. [API Endpoints](#api-endpoints)
6. [Security](#security)
7. [Monitoring](#monitoring)
8. [Troubleshooting](#troubleshooting)
9. [Production Deployment](#production-deployment)

---

## Overview

Aurora Locus implements full AT Protocol (ATProto) federation, enabling your Personal Data Server (PDS) to:

- **Discover** other PDS instances on the ATProto network
- **Subscribe** to relay firehose for network-wide events
- **Search** across multiple federated PDS instances
- **Authenticate** users from remote PDS instances
- **Publish** events to the relay for network distribution

### What is Federation?

Federation allows multiple PDS instances to interoperate, forming a decentralized social network. Users on your PDS can discover and interact with users on other PDSs seamlessly.

### Key Components

1. **Relay Firehose**: Subscribe to network events (commits, identity changes, etc.)
2. **Federated Search**: Query multiple PDS instances in parallel
3. **PDS Discovery**: Automatically discover new instances from relay events
4. **Service Authentication**: Cryptographically verify cross-PDS requests
5. **Event Publishing**: Share your PDS events with the network

---

## Quick Start

### Enable Federation

Edit your `.env` file:

```bash
# Enable federation
AURORA_ENABLE_FEDERATION=true

# Relay URL (default: Bluesky relay)
AURORA_RELAY_URL=wss://bsky.network

# Discovered PDS instances (comma-separated, optional)
AURORA_FEDERATED_PDS_INSTANCES=https://pds1.example.com,https://pds2.example.com

# Federation settings
AURORA_FEDERATION_MAX_CONCURRENT=10
AURORA_FEDERATION_TIMEOUT_SECS=30
```

### Start Server

```bash
cargo run --release
```

Your PDS will automatically:
- Subscribe to the relay firehose
- Discover other PDS instances from events
- Enable federated search endpoints

---

## Configuration

### Environment Variables

| Variable | Description | Default | Required |
|----------|-------------|---------|----------|
| `AURORA_ENABLE_FEDERATION` | Enable/disable federation | `false` | No |
| `AURORA_RELAY_URL` | Relay firehose WebSocket URL | `wss://bsky.network` | No |
| `AURORA_FEDERATED_PDS_INSTANCES` | Initial PDS list (comma-separated) | Empty | No |
| `AURORA_FEDERATION_MAX_CONCURRENT` | Max parallel queries | `10` | No |
| `AURORA_FEDERATION_TIMEOUT_SECS` | Query timeout in seconds | `30` | No |

### Relay Configuration

```bash
# Primary relay (Bluesky network)
AURORA_RELAY_URL=wss://bsky.network

# Custom relay
AURORA_RELAY_URL=wss://relay.mynetwork.example.com

# Multiple relays (comma-separated)
AURORA_RELAY_URL=wss://relay1.example.com,wss://relay2.example.com
```

### PDS Discovery

You can bootstrap federation with an initial list of known PDSs:

```bash
AURORA_FEDERATED_PDS_INSTANCES=https://pds1.example.com,https://pds2.example.com,https://pds3.example.com
```

**Auto-Discovery**: New PDSs are automatically discovered from relay events.

### Performance Tuning

```bash
# Allow up to 20 parallel federated queries
AURORA_FEDERATION_MAX_CONCURRENT=20

# Increase timeout for slow networks
AURORA_FEDERATION_TIMEOUT_SECS=60
```

---

## Architecture

### Data Flow

```
┌─────────────────────────────────────────────────────────────┐
│                     ATProto Network                         │
│                                                             │
│  ┌─────────────┐      ┌─────────────┐      ┌─────────────┐│
│  │   PDS 1     │      │   PDS 2     │      │   PDS 3     ││
│  │  alice.com  │      │  bob.com    │      │charlie.com  ││
│  └──────┬──────┘      └──────┬──────┘      └──────┬──────┘│
│         │                    │                    │        │
│         └────────────────────┼────────────────────┘        │
│                              │                             │
│                     ┌────────▼────────┐                    │
│                     │  Relay Firehose  │                    │
│                     │  (bsky.network)  │                    │
│                     └────────┬────────┘                    │
│                              │                             │
│                     ┌────────▼────────┐                    │
│                     │  Aurora Locus   │                    │
│                     │  (Your PDS)     │                    │
│                     └─────────────────┘                    │
└─────────────────────────────────────────────────────────────┘
```

### Component Architecture

#### 1. Relay Subscription (Inbound Events)

```rust
// src/federation/relay.rs
RelayClient::subscribe()
    ↓
WebSocket connection to relay
    ↓
Receive events (commit, identity, account, handle, tombstone)
    ↓
Process events (cache invalidation, indexing, discovery)
    ↓
Record metrics (RELAY_EVENTS_TOTAL, processing duration)
```

#### 2. Federated Search (Outbound Queries)

```rust
// src/federation/search.rs
FederatedSearch::search_actors(query)
    ↓
Query all known PDS instances in parallel
    ↓
Aggregate results from successful responses
    ↓
Deduplicate by DID/URI
    ↓
Return unified result set
```

#### 3. Circuit Breaker (Fault Tolerance)

```
Healthy PDS → 3 consecutive failures → Circuit OPEN (60s cooldown)
    ↓                                         ↓
Track failures                         Skip requests
    ↓                                         ↓
Reset on success                      After cooldown → Circuit HALF-OPEN → Retry
```

#### 4. Service Authentication (Cross-PDS Auth)

```
User request to remote PDS
    ↓
Create service JWT (signed with user's signing key)
    ↓
Claims: iss=userDID, aud=serviceDID, exp=<60s, lxm=endpoint, jti=nonce
    ↓
Remote PDS verifies:
  - Resolve issuer DID → fetch signing key
  - Verify JWT signature cryptographically
  - Check expiration (<60s strict)
  - Validate audience matches
  - Check nonce for replay prevention
    ↓
Request authorized ✓
```

---

## API Endpoints

### Federated Search

#### Search Actors (Users)

```http
GET /xrpc/app.bsky.actor.searchActors?q=alice&limit=50
Authorization: Bearer <token>
```

**Parameters:**
- `q` (string, required): Search query
- `limit` (integer, optional): Max results per PDS (default: 25, max: 100)

**Response:**
```json
{
  "actors": [
    {
      "did": "did:plc:abc123",
      "handle": "alice.pds1.example",
      "displayName": "Alice",
      "avatar": "https://..."
    }
  ]
}
```

#### Search Posts

```http
GET /xrpc/app.bsky.feed.searchPosts?q=atproto&limit=50
Authorization: Bearer <token>
```

**Parameters:**
- `q` (string, required): Search query
- `limit` (integer, optional): Max results per PDS (default: 25, max: 100)

**Response:**
```json
{
  "posts": [
    {
      "uri": "at://did:plc:abc123/app.bsky.feed.post/3k...",
      "cid": "bafyrei...",
      "author": {...},
      "record": {...},
      "indexedAt": "2025-11-15T12:00:00Z"
    }
  ]
}
```

#### Aggregate Timeline

```http
GET /xrpc/com.aurora.federation.aggregateTimeline?limit=50
Authorization: Bearer <token>
```

**Description**: Aggregates recent posts from all known federated PDSs.

**Parameters:**
- `limit` (integer, optional): Max results (default: 50, max: 100)

**Response:**
```json
{
  "feed": [
    {
      "post": {...},
      "reason": {...}
    }
  ],
  "cursor": "next-page-token"
}
```

### Sync Endpoints (For Crawlers/Relays)

#### Get Repository

```http
GET /xrpc/com.atproto.sync.getRepo?did=did:plc:abc123
Authorization: Bearer <serviceAuthToken>
```

**Description**: Export user's repository as CAR file.

**Response**: `application/vnd.ipld.car` binary data

#### Get Blocks

```http
GET /xrpc/com.atproto.sync.getBlocks?did=did:plc:abc123&cids=cid1,cid2
Authorization: Bearer <serviceAuthToken>
```

**Description**: Get specific IPLD blocks by CID.

**Response**: `application/vnd.ipld.car` binary data

---

## Security

### Service Authentication

Aurora Locus uses **DID-based cryptographic verification** for cross-PDS authentication (not callback-based).

#### How It Works

1. **JWT Creation**: When making requests to another PDS, Aurora creates a service JWT:
   ```json
   {
     "iss": "did:plc:user123",        // Issuer (user DID)
     "aud": "did:web:pds2.example",   // Audience (target PDS)
     "exp": 1234567890,               // Expires <60 seconds from now
     "lxm": "com.atproto.repo.get",   // Lexicon method
     "jti": "unique-nonce-abc"        // Nonce for replay prevention
   }
   ```

2. **JWT Signing**: Signed with user's atproto signing key (ES256).

3. **JWT Verification** (Remote PDS):
   - Resolve issuer DID → fetch DID document
   - Extract signing key from `verificationMethod`
   - Verify JWT signature cryptographically
   - Check expiration (<60 seconds, strict)
   - Validate audience matches this PDS
   - Check nonce hasn't been used (replay prevention)

#### Security Features

- ✅ **Short-lived tokens**: <60 second expiration (strict enforcement)
- ✅ **Nonce-based replay prevention**: Each JWT usable once
- ✅ **Cryptographic trust**: No callback to origin PDS needed
- ✅ **DID-based verification**: Trust derived from DID documents
- ✅ **Audience validation**: Prevents token misuse
- ✅ **Rate limiting**: 10x stricter for cross-PDS requests

### Rate Limiting

Federated endpoints have strict rate limits to prevent abuse:

| Endpoint | Rate Limit | Scope |
|----------|------------|-------|
| `searchActors` | 30/5min, 300/day | Per IP + DID |
| `searchPosts` | 30/5min, 300/day | Per IP + DID |
| `aggregateTimeline` | 10/min, 500/day | Per IP + DID |

**Note**: Cross-PDS requests have 10x stricter limits than local requests.

### Audit Logging

All cross-PDS actions are logged with:
- Timestamp
- Issuer DID
- Endpoint called
- Success/failure status
- Error details (if any)

---

## Monitoring

### Prometheus Metrics

Aurora Locus exposes federation metrics at `/metrics`:

#### Federation Requests

```prometheus
# Federated search requests by endpoint and status
federation_requests_total{endpoint="searchActors",status="success"} 1234

# Federated search latency histogram
federation_latency_seconds{endpoint="searchActors",quantile="0.95"} 1.2

# Number of known federated PDS instances
known_instances 42
```

#### Relay Metrics

```prometheus
# Relay events received by type
relay_events_total{event_type="commit"} 50000
relay_events_total{event_type="identity"} 120

# Relay event processing duration
relay_event_processing_duration_seconds{event_type="commit",quantile="0.95"} 0.015

# Relay connection status (0=down, 1=up)
relay_connection_status 1

# Total relay connections
relay_connections_total{relay_url="wss://bsky.network",status="success"} 15

# Events published to relay
relay_events_published_total{event_type="commit",status="success"} 8000
```

### Grafana Dashboard

Import the provided Grafana dashboard (`grafana/federation-dashboard.json`) for visualization:

- **Federated Search Latency** (p50, p95, p99)
- **Known Instances Over Time**
- **Relay Event Rate** (events/sec)
- **Circuit Breaker Status** (open/closed by instance)
- **Cross-PDS Authentication Success Rate**

### Health Checks

Check federation health:

```bash
curl http://localhost:2583/xrpc/_health

# Response
{
  "version": "0.1.0",
  "federation": {
    "enabled": true,
    "relay_connected": true,
    "known_instances": 42,
    "last_event_at": "2025-11-15T12:34:56Z"
  }
}
```

---

## Troubleshooting

### Federation Not Working

**Problem**: Federated search returns empty results.

**Solutions**:
1. Verify federation is enabled:
   ```bash
   grep AURORA_ENABLE_FEDERATION .env
   # Should show: AURORA_ENABLE_FEDERATION=true
   ```

2. Check relay connection:
   ```bash
   curl http://localhost:2583/metrics | grep relay_connection_status
   # Should show: relay_connection_status 1
   ```

3. Verify known instances:
   ```bash
   curl http://localhost:2583/metrics | grep known_instances
   # Should show: known_instances >0
   ```

4. Check logs for errors:
   ```bash
   tail -f logs/aurora-locus.log | grep -i federation
   ```

### Relay Connection Failures

**Problem**: `relay_connection_status 0` (disconnected).

**Solutions**:
1. Check relay URL:
   ```bash
   grep AURORA_RELAY_URL .env
   # Verify URL is correct: wss://bsky.network
   ```

2. Test WebSocket connectivity:
   ```bash
   curl -i -N -H "Connection: Upgrade" -H "Upgrade: websocket" \
     -H "Sec-WebSocket-Version: 13" -H "Sec-WebSocket-Key: test" \
     https://bsky.network
   ```

3. Check firewall/proxy:
   - Ensure outbound WebSocket (wss://) connections are allowed
   - Check corporate proxy settings

4. Review connection logs:
   ```bash
   grep "relay.*connect" logs/aurora-locus.log
   ```

### Slow Federated Search

**Problem**: Federated search takes >2 seconds.

**Solutions**:
1. Check federation timeout:
   ```bash
   grep AURORA_FEDERATION_TIMEOUT_SECS .env
   # Increase if needed: AURORA_FEDERATION_TIMEOUT_SECS=60
   ```

2. Review circuit breaker status:
   ```bash
   # Check logs for "circuit open" messages
   grep "circuit.*open" logs/aurora-locus.log
   ```

3. Monitor latency by instance:
   ```bash
   curl http://localhost:2583/metrics | grep federation_latency_seconds
   ```

4. Reduce concurrent queries:
   ```bash
   # Lower max concurrent to reduce load
   AURORA_FEDERATION_MAX_CONCURRENT=5
   ```

### Authentication Failures

**Problem**: Cross-PDS requests fail with 401 Unauthorized.

**Solutions**:
1. Verify service JWT creation:
   ```bash
   # Check logs for JWT creation errors
   grep "service.*jwt.*error" logs/aurora-locus.log
   ```

2. Check signing key configuration:
   ```bash
   grep AURORA_SIGNING_KEY_HEX .env
   # Ensure key is properly formatted (64 hex chars)
   ```

3. Verify DID resolution:
   ```bash
   curl https://plc.directory/did:plc:abc123
   # Should return valid DID document
   ```

4. Check token expiration:
   ```bash
   # Service JWTs must have exp <60 seconds from now
   # Review logs for "expired" errors
   grep "token.*expired" logs/aurora-locus.log
   ```

### High Error Rates

**Problem**: `federation_requests_total{status="error"}` is high.

**Solutions**:
1. Identify failing instances:
   ```bash
   grep "federation.*error" logs/aurora-locus.log | grep -o "pds[0-9]*.example.com" | sort | uniq -c
   ```

2. Remove problematic instances:
   ```bash
   # Edit .env to remove unreliable PDSs from initial list
   AURORA_FEDERATED_PDS_INSTANCES=https://reliable1.example.com,https://reliable2.example.com
   ```

3. Let circuit breaker handle it:
   - After 3 consecutive failures, instance is automatically excluded for 60s
   - Monitor circuit breaker events:
     ```bash
     grep "circuit.*open" logs/aurora-locus.log
     ```

---

## Production Deployment

### Prerequisites

- **Public hostname**: Your PDS must be accessible via HTTPS
- **Valid TLS certificate**: Let's Encrypt or commercial cert
- **Relay access**: Ensure outbound WebSocket connections allowed
- **Monitoring**: Prometheus + Grafana for observability

### Deployment Checklist

- [ ] **Configure federation**: Set `AURORA_ENABLE_FEDERATION=true`
- [ ] **Set relay URL**: Use production relay (e.g., `wss://bsky.network`)
- [ ] **Configure signing keys**: Generate production signing key
- [ ] **Enable rate limiting**: Verify strict limits for federated endpoints
- [ ] **Set up monitoring**: Deploy Prometheus + Grafana
- [ ] **Configure alerts**: Alert on relay disconnect, high error rates
- [ ] **Test health checks**: Verify `/xrpc/_health` returns federation status
- [ ] **Verify TLS**: Ensure HTTPS is properly configured
- [ ] **Review logs**: Set `AURORA_LOG_LEVEL=info` for production
- [ ] **Backup plan**: Document rollback procedure

### Performance Targets

Aurora Locus aims for these federation performance targets:

| Metric | Target (p95) | Notes |
|--------|--------------|-------|
| Federated search latency | <2 seconds | With 10+ PDSs |
| Relay event processing | <100 ms | Per event |
| PDS discovery refresh | <30 seconds | Background job |
| Known instances | 100+ | Auto-discovered from relay |
| Concurrent federated requests | 100 | Without degradation |

### Scaling Recommendations

| Deployment Size | Max Concurrent | Timeout | Known Instances |
|-----------------|----------------|---------|-----------------|
| Small (1-100 users) | 10 | 30s | 50 |
| Medium (100-1K users) | 20 | 45s | 100 |
| Large (1K-10K users) | 50 | 60s | 200 |
| Enterprise (10K+ users) | 100 | 90s | 500+ |

### Security Hardening

1. **Service Auth**:
   - Enforce <60s expiration strictly (no grace period)
   - Track nonces for replay prevention (60s TTL)
   - Validate audience matches this PDS exactly

2. **Rate Limiting**:
   - Apply 10x stricter limits for cross-PDS requests
   - Use composite keys: `IP + DID + endpoint`
   - Monitor rate limit violations

3. **Audit Logging**:
   - Log all cross-PDS actions with full context
   - Retain logs for compliance (90 days minimum)
   - Alert on suspicious patterns

4. **Network Security**:
   - Restrict relay URLs to trusted relays only
   - Use TLS 1.3 for all WebSocket connections
   - Implement connection limits per IP

### Maintenance

#### Weekly Tasks
- Review circuit breaker events
- Check for new discovered PDSs
- Monitor relay connection stability
- Review error rates by instance

#### Monthly Tasks
- Analyze federation latency trends
- Update known PDS list (if needed)
- Review security audit logs
- Update relay URL if migrating

#### Quarterly Tasks
- Performance benchmark federation
- Security audit of service auth
- Capacity planning based on growth
- Update documentation

---

## Support

### Documentation
- [Architecture](ARCHITECTURE.md) - System architecture overview
- [Security](SECURITY.md) - Security considerations
- [Quick Start](QUICKSTART.md) - Get started guide

### Community
- GitHub Issues: [Report bugs](https://github.com/your-org/aurora-locus/issues)
- Discord: [Join community](https://discord.gg/aurora-locus)

### Commercial Support
Contact: support@aurora-locus.example.com

---

**Last Updated**: 2025-11-15
**Aurora Locus Version**: 0.1.0
**ATProto Spec Version**: 2024
