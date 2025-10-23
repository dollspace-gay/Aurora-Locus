# Aurora Locus - Bluesky Federation Guide

This document explains how to configure Aurora Locus PDS for federation with the Bluesky network.

## Current Status

### ✅ IMPLEMENTED - Core Federation Components

**Event Streaming & Firehose** (612 lines, production-ready)
- ✅ **Firehose WebSocket endpoint** (`com.atproto.sync.subscribeRepos`) - FULLY IMPLEMENTED
- ✅ Event sequencer database with monotonic sequence numbers
- ✅ Historical event playback with cursor support
- ✅ Backpressure handling (buffered channels, timeouts)
- ✅ Ping/pong keepalive for connection health
- ✅ Exponential backoff on database errors
- ✅ CBOR event serialization for ATProto protocol
- ✅ JSON frame format with event type discriminators

**Repository Sync** (endpoints exist, need block fetch)
- ✅ `com.atproto.sync.getRepo` - Export repository as CAR
- ✅ `com.atproto.sync.getLatestCommit` - Get current commit CID
- ✅ `com.atproto.sync.getBlocks` - Fetch specific blocks
- ✅ `com.atproto.sync.listRepos` - List all repositories
- ⚠️ CAR export works but needs block data from repo_block table

**Core PDS Functionality**
- ✅ Account creation, authentication, sessions
- ✅ Repository writes (create, update, delete records)
- ✅ Block storage in repo_block table (JUST FIXED!)
- ✅ MST integration via SDK
- ✅ Local SQLite storage
- ✅ DID resolution (can read from PLC Directory)
- ✅ Federation configuration structure

### ⚠️ PARTIALLY IMPLEMENTED

**CAR File Export** (needs completion)
- ✅ CAR encoder exists
- ✅ Endpoints structure in place
- ❌ Need to fetch blocks from repo_block table
- ❌ Need to include MST nodes and commits

### ❌ NOT YET IMPLEMENTED (Required for Full Federation)

**Integration & Wiring**
- ❌ Hook sequencer into repository write operations
- ❌ Broadcast commit events to firehose subscribers in real-time
- ❌ Live event streaming (currently only historical events work)

**PLC Integration**
- ❌ PLC DID registration (accounts currently use `did:web` only)
- ❌ DID document management with proper service endpoints
- ❌ Automatic DID registration on account creation

**Relay Client**
- ❌ Outbound connection to Bluesky relay
- ❌ Push events to relay in real-time
- ❌ Handle relay backpressure
- ❌ Reconnection logic

## Federation Configuration

Aurora Locus now includes configuration options for federation. These are configured via environment variables:

### Environment Variables

```bash
# Enable federation (default: false)
PDS_FEDERATION_ENABLED=false

# Bluesky relay URLs (comma-separated, default: https://bsky.network)
PDS_FEDERATION_RELAY_URLS=https://bsky.network

# Enable firehose WebSocket endpoint (default: false)
PDS_FEDERATION_FIREHOSE_ENABLED=false

# Allow relay to crawl repositories (default: false)
PDS_FEDERATION_CRAWL_ENABLED=false

# Public URL for this PDS (must be HTTPS and publicly accessible)
PDS_PUBLIC_URL=https://mypds.example.com

# Enable automatic event streaming to relay (default: false)
PDS_FEDERATION_AUTO_STREAM=false
```

## Prerequisites for Production Federation

To actually federate with Bluesky, you need:

### 1. Domain and Public Access
- **Domain name**: Get a public domain (e.g., `mypds.example.com`)
- **SSL/TLS**: HTTPS is **required** - obtain a certificate (Let's Encrypt recommended)
- **Public IP**: Server must be publicly accessible on port 443
- **DNS configuration**: Point your domain to your server's IP

### 2. Network Setup
```bash
# Example DNS records
A     mypds.example.com    -> 123.456.789.0
AAAA  mypds.example.com    -> 2001:db8::1
```

### 3. Firewall Configuration
- Open port 443 (HTTPS)
- Open port 80 (HTTP - for Let's Encrypt certificate validation)
- Configure your reverse proxy (nginx, caddy, traefik)

### 4. Reverse Proxy Example (Nginx)
```nginx
server {
    listen 443 ssl http2;
    server_name mypds.example.com;

    ssl_certificate /etc/letsencrypt/live/mypds.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/mypds.example.com/privkey.pem;

    location / {
        proxy_pass http://localhost:3000;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }

    # WebSocket support for firehose
    location /xrpc/com.atproto.sync.subscribeRepos {
        proxy_pass http://localhost:3000;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
    }
}
```

## Implementation Roadmap

To make Aurora Locus production-ready for Bluesky federation, the following components need to be implemented:

### Phase 1: Event Streaming (CRITICAL)
1. **Implement Firehose Endpoint**
   - WebSocket endpoint at `/xrpc/com.atproto.sync.subscribeRepos`
   - Real-time event streaming for repository changes
   - CBOR-encoded event frames
   - Cursor-based resumption

2. **Event Sequencer Integration**
   - Hook into repository write operations
   - Sequence all commits with monotonic sequence numbers
   - Store events in sequencer database
   - Emit events to firehose subscribers

### Phase 2: Repository Sync (CRITICAL)
3. **Implement Sync Endpoints**
   - `com.atproto.sync.getRepo` - Export full repository as CAR
   - `com.atproto.sync.getBlocks` - Fetch specific blocks by CID
   - `com.atproto.sync.getLatestCommit` - Get latest commit CID
   - `com.atproto.sync.listBlobs` - List blob CIDs for a repo

4. **CAR File Export**
   - Export repository as CAR (Content Addressable aRchive)
   - Include all blocks: commits, MST nodes, records
   - Support since parameter for incremental exports

### Phase 3: PLC Integration (CRITICAL)
5. **PLC DID Registration**
   - Register new DIDs with PLC Directory
   - Include PDS endpoint in DID document
   - Support did:plc rotation keys
   - Automatic DID document updates

6. **DID Document Management**
   - Serve `.well-known/atproto-did` file
   - Include proper service endpoints in DID doc:
     ```json
     {
       "id": "did:plc:xyz123",
       "service": [{
         "id": "#atproto_pds",
         "type": "AtprotoPersonalDataServer",
         "serviceEndpoint": "https://mypds.example.com"
       }]
     }
     ```

### Phase 4: Relay Integration
7. **Relay Client**
   - Outbound connection to Bluesky relay
   - Push events to relay in real-time
   - Handle relay backpressure
   - Reconnection logic

8. **Crawling Support**
   - Allow relay to discover and index repositories
   - Respond to sync requests from relay
   - Rate limiting for crawlers

## Testing Federation

Once implemented, test federation with these steps:

### 1. Local Testing
```bash
# Create account with did:plc
curl -X POST https://mypds.example.com/xrpc/com.atproto.server.createAccount \
  -H "Content-Type: application/json" \
  -d '{
    "handle": "alice.mypds.example.com",
    "password": "secure-password",
    "email": "alice@example.com"
  }'
```

### 2. Test Firehose
```bash
# Subscribe to firehose
websocat wss://mypds.example.com/xrpc/com.atproto.sync.subscribeRepos
```

### 3. Test Repository Export
```bash
# Export repository as CAR
curl https://mypds.example.com/xrpc/com.atproto.sync.getRepo?did=did:plc:xyz123 \
  > repo.car
```

### 4. Verify DID Document
```bash
# Check DID document includes PDS endpoint
curl https://plc.directory/did:plc:xyz123
```

### 5. Create Post and Verify Federation
```bash
# Create a post
curl -X POST https://mypds.example.com/xrpc/com.atproto.repo.createRecord \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "repo": "did:plc:xyz123",
    "collection": "app.bsky.feed.post",
    "record": {
      "text": "Hello federated Bluesky!",
      "createdAt": "2025-10-23T20:00:00Z"
    }
  }'

# Verify post appears on Bluesky
# Search for your handle or post on bsky.app
```

## Current Limitations

**Aurora Locus is currently suitable for:**
- Local development and testing
- Learning ATProto protocol
- Building client applications
- Testing repository operations

**Not yet suitable for:**
- Production use with Bluesky network
- Hosting real user accounts that federate
- Public PDS hosting

## Estimated Implementation Effort

To complete federation support:
- **Phase 1** (Event Streaming): 1-2 weeks
- **Phase 2** (Repository Sync): 1-2 weeks
- **Phase 3** (PLC Integration): 1 week
- **Phase 4** (Relay Integration): 1 week

**Total estimated effort**: 4-6 weeks of focused development

## Alternative: Use Bluesky's Official PDS

If you need immediate federation, consider:
- **Bluesky Social**: Create account at https://bsky.app (fully federated)
- **Self-hosted PDS**: Use Bluesky's official TypeScript PDS implementation

## Contributing

Want to help implement federation? See the todo list in the code:
1. Firehose WebSocket endpoint
2. Event sequencer integration
3. Sync endpoints (getRepo, getBlocks, etc.)
4. PLC DID registration
5. Relay client
6. CAR file export
7. DID document management

## References

- [ATProto Specifications](https://atproto.com/specs/atp)
- [PLC Directory](https://plc.directory)
- [Bluesky Relay](https://github.com/bluesky-social/atproto/tree/main/packages/bsky)
- [CAR Format](https://ipld.io/specs/transport/car/)
- [ATProto Data Repository](https://atproto.com/specs/repository)
