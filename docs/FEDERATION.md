# Aurora Locus PDS - Federation Guide

## Table of Contents

1. [Overview](#overview)
2. [Cross-PDS Authentication](#cross-pds-authentication)
3. [PDS Discovery](#pds-discovery)
4. [Federated Search](#federated-search)
5. [Relay Support](#relay-support)
6. [Content Aggregation](#content-aggregation)
7. [Configuration](#configuration)
8. [Best Practices](#best-practices)
9. [Troubleshooting](#troubleshooting)

---

## Overview

Aurora Locus PDS is designed to participate in a federated network of PDS instances, enabling seamless interaction across the ATProto ecosystem.

### Federation Features

| Feature | Purpose | Use Case |
|---------|---------|----------|
| **Cross-PDS Authentication** | Verify users from other PDS instances | Remote follows, mentions, replies |
| **PDS Discovery** | Find other PDS instances in the network | Network exploration, migration |
| **Federated Search** | Search across all known PDS instances | Find users, posts, content globally |
| **Relay Support** | Real-time event streaming | Firehose aggregation, live updates |
| **Content Aggregation** | Combine timelines from multiple sources | Unified feed experience |

### Architecture

```
┌──────────────────────────────────────────────────────┐
│                ATProto Federation                     │
├──────────────────────────────────────────────────────┤
│                                                       │
│  ┌─────────┐      ┌─────────┐      ┌─────────┐     │
│  │  PDS 1  │◄────►│  Relay  │◄────►│  PDS 2  │     │
│  │(Aurora) │      │ Server  │      │(Bluesky)│     │
│  └─────────┘      └─────────┘      └─────────┘     │
│       ▲                │                 ▲           │
│       │                │                 │           │
│       │         ┌──────▼──────┐         │           │
│       └─────────│     PLC     │─────────┘           │
│                 │  Directory  │                      │
│                 └─────────────┘                      │
│                                                       │
└──────────────────────────────────────────────────────┘
```

---

## Cross-PDS Authentication

### Overview

Cross-PDS authentication allows users from any PDS instance to interact with your PDS by verifying their identity through DID resolution.

### How It Works

1. **User Request**: Remote user sends request with DID and auth token
2. **DID Resolution**: Resolve DID to find user's home PDS
3. **Token Verification**: Verify token with remote PDS
4. **Grant Access**: Allow authenticated cross-PDS interaction

### Configuration

```bash
# Enable federation
FEDERATION_ENABLED=true

# Your PDS identity
PDS_SERVICE_DID=did:plc:your-pds-did
PDS_SERVICE_URL=https://pds.example.com

# Trust settings
FEDERATION_TRUSTED_INSTANCES=pds.bsky.social,pds.trusted.com
```

### Code Example

```rust
use aurora_locus::federation::FederationAuthenticator;

// Verify remote user
let authenticator = FederationAuthenticator::new(identity_resolver);
let remote_user = authenticator
    .verify_remote_token("did:plc:remote123", "token")
    .await?;

println!("Verified user: @{}", remote_user.handle);
```

### Remote User Interactions

#### Follows

```
User@RemotePDS → Follow → User@YourPDS
   │
   ├─ DID Resolution
   ├─ Token Verification
   └─ Store Follow Relationship
```

#### Mentions

```
User@RemotePDS → Post mentioning @you@yourpds.com
   │
   ├─ Verify user identity
   ├─ Validate mention
   └─ Deliver notification
```

#### Replies

```
User@RemotePDS → Reply to your post
   │
   ├─ Authenticate user
   ├─ Verify original post ownership
   └─ Store reply
```

---

## PDS Discovery

### Discovery Methods

#### 1. Relay-Based Discovery

Relays maintain registries of active PDS instances:

```rust
use aurora_locus::federation::PdsDiscovery;

let discovery = PdsDiscovery::new(vec![
    "https://relay.bsky.app".to_string(),
]);

// Discover from relays
let instances = discovery.discover_from_relays().await?;
println!("Found {} PDS instances", instances.len());
```

#### 2. Well-Known Endpoint

Discover PDS via domain:

```rust
let instance = discovery
    .discover_via_wellknown("example.com")
    .await?;

println!("PDS: {} at {}", instance.did, instance.url);
```

#### 3. Manual Configuration

Add known PDS instances manually:

```rust
let instance = PdsInstance {
    did: "did:plc:abc123".to_string(),
    url: "https://pds.friend.com".to_string(),
    name: Some("Friend's PDS".to_string()),
    open_registrations: true,
    user_count: Some(50),
    last_seen: Some(chrono::Utc::now().timestamp()),
    features: vec!["firehose".to_string()],
};

discovery.add_instance(instance).await;
```

### Finding PDS Instances

```rust
// Find by DID
let instance = discovery.find_by_did("did:plc:abc123").await;

// Find instances accepting registrations
let open_instances = discovery.find_open_instances().await;

// Get all known instances
let all_instances = discovery.get_known_instances().await;
```

### Automatic Refresh

Keep instance list up-to-date:

```rust
// Refresh every hour
tokio::spawn(async move {
    let mut interval = tokio::time::interval(Duration::from_secs(3600));
    loop {
        interval.tick().await;
        if let Err(e) = discovery.refresh_instances().await {
            error!("Failed to refresh instances: {}", e);
        }
    }
});
```

---

## Federated Search

### Overview

Federated search enables searching across all known PDS instances in parallel, providing network-wide search capabilities.

### Actor Search

Search for users across the federation:

```rust
use aurora_locus::federation::FederatedSearch;

let search = FederatedSearch::new(discovery, 10, 30);

// Search actors
let actors = search.search_actors("alice", 50).await?;

for actor in actors {
    println!("@{} ({})", actor.handle, actor.did);
    println!("  {} followers", actor.followers_count);
}
```

### Post Search

Search for posts network-wide:

```rust
let posts = search.search_posts("atproto", 100).await?;

for post in posts {
    println!("Post by @{}", post.author.handle);
    println!("  {} likes, {} reposts", post.like_count, post.repost_count);
}
```

### Performance Characteristics

| Instances | Concurrent | Latency | Results |
|-----------|-----------|---------|---------|
| 10 | 10 | ~1s | Fast |
| 50 | 10 | ~5s | Good |
| 100 | 10 | ~10s | Acceptable |
| 100 | 50 | ~2s | Fast (higher load) |

### Configuration

```bash
# Enable federated search
FEDERATION_ENABLE_SEARCH=true

# Concurrent requests (balance speed vs load)
FEDERATION_MAX_CONCURRENT=10

# Timeout per request
FEDERATION_TIMEOUT=30
```

### Search API Endpoint

```http
GET /xrpc/app.bsky.actor.searchActorsTypehead?q=alice&federated=true
```

Response:
```json
{
  "actors": [
    {
      "did": "did:plc:abc123",
      "handle": "alice.bsky.social",
      "displayName": "Alice",
      "avatar": "https://...",
      "followersCount": 1000,
      "source": "pds.bsky.social"
    }
  ]
}
```

---

## Relay Support

### Overview

Relays provide real-time event distribution across the ATProto network, enabling:
- Live firehose of all network activity
- Content synchronization
- Repository backfill
- Network-wide notifications

### Relay Configuration

```bash
# Relay servers
FEDERATION_RELAY_SERVERS=https://relay.bsky.app,wss://relay.staging.bsky.dev

# Connection settings
RELAY_RECONNECT_INTERVAL=5
RELAY_BUFFER_SIZE=1000
RELAY_ENABLE_COMPRESSION=true
```

### Subscribing to Firehose

```rust
use aurora_locus::federation::{RelayClient, RelayConfig};

let config = RelayConfig {
    servers: vec![
        "https://relay.bsky.app".to_string(),
    ],
    ..Default::default()
};

let mut relay = RelayClient::new(config);

// Subscribe to firehose
let mut events = relay.subscribe_firehose().await?;

// Process events
while let Some(event) = events.recv().await {
    match event.event_type.as_str() {
        "commit" => {
            println!("New commit from {}: seq {}", event.did, event.seq);
        }
        "handle" => {
            println!("Handle update: {}", event.did);
        }
        "tombstone" => {
            println!("Account deleted: {}", event.did);
        }
        _ => {}
    }
}
```

### Publishing Events

```rust
let event = RelayEvent {
    event_type: "commit".to_string(),
    did: "did:plc:your123".to_string(),
    seq: 42,
    commit: Some(serde_json::json!({"cid": "bafy..."})),
    time: chrono::Utc::now().to_rfc3339(),
};

relay.publish_event(&event).await?;
```

### Repository Synchronization

Fetch complete repository from relay:

```rust
let repo_data = relay.fetch_repo("did:plc:abc123").await?;

// Parse CAR file
let repo = parse_car_file(&repo_data)?;
```

### Event Types

| Event | Description | Payload |
|-------|-------------|---------|
| `commit` | New repository commit | CID, operations, blocks |
| `handle` | Handle update | New handle |
| `migrate` | Account migration | New PDS |
| `tombstone` | Account deletion | - |
| `info` | Metadata update | Info message |

---

## Content Aggregation

### Overview

Content aggregation combines timelines and feeds from multiple PDS instances into a unified view.

### Aggregated Timeline

```rust
let search = FederatedSearch::new(discovery, 10, 30);

// DIDs to aggregate
let following_dids = vec![
    "did:plc:alice123".to_string(),
    "did:plc:bob456".to_string(),
    "did:plc:carol789".to_string(),
];

// Get aggregated timeline
let timeline = search
    .aggregate_timeline(following_dids, 100)
    .await?;

for post in timeline {
    println!("{}: {}", post.author.handle, extract_text(&post.record));
}
```

### Custom Feeds

Build custom feeds from multiple sources:

```rust
async fn build_trending_feed(
    search: &FederatedSearch,
    timeframe: Duration,
) -> Vec<PostResult> {
    // Search for trending topics
    let topics = vec!["atproto", "bluesky", "decentralization"];
    let mut all_posts = Vec::new();

    for topic in topics {
        let posts = search.search_posts(&topic, 50).await?;
        all_posts.extend(posts);
    }

    // Sort by engagement
    all_posts.sort_by(|a, b| {
        let score_a = a.like_count + a.repost_count * 2;
        let score_b = b.like_count + b.repost_count * 2;
        score_b.cmp(&score_a)
    });

    all_posts.truncate(100);
    all_posts
}
```

### Caching Strategy

```rust
// Cache aggregated results
let cache_key = format!("timeline:{}", user_did);
cache.set(&cache_key, &timeline, 300).await?; // 5 min TTL

// Check cache first
if let Some(cached) = cache.get(&cache_key).await? {
    return Ok(cached);
}

// Fetch if not cached
let timeline = search.aggregate_timeline(dids, limit).await?;
cache.set(&cache_key, &timeline, 300).await?;
```

---

## Configuration

### Environment Variables

```bash
# === Federation Core ===
FEDERATION_ENABLED=true
PDS_SERVICE_DID=did:plc:your-pds-did
PDS_SERVICE_URL=https://pds.example.com

# === Discovery ===
FEDERATION_RELAY_SERVERS=https://relay1.com,https://relay2.com
FEDERATION_KNOWN_INSTANCES=did:plc:abc123:https://pds1.com

# === Search ===
FEDERATION_ENABLE_SEARCH=true
FEDERATION_MAX_CONCURRENT=10
FEDERATION_TIMEOUT=30

# === Trust ===
FEDERATION_TRUSTED_INSTANCES=pds.bsky.social,pds.trusted.com
FEDERATION_BLOCK_INSTANCES=pds.spam.com,pds.bad.com

# === Relay ===
RELAY_RECONNECT_INTERVAL=5
RELAY_BUFFER_SIZE=1000
RELAY_ENABLE_COMPRESSION=true
```

### Code Configuration

```rust
use aurora_locus::federation::FederationConfig;

let config = FederationConfig {
    enabled: true,
    service_did: "did:plc:your123".to_string(),
    service_url: "https://pds.example.com".to_string(),
    relay_servers: vec![
        "https://relay.bsky.app".to_string(),
    ],
    enable_search: true,
    max_concurrent_requests: 10,
    request_timeout: 30,
    known_instances: vec![],
};
```

---

## Best Practices

### 1. Rate Limiting

Implement rate limits for federated requests:

```rust
// Limit to 10 federated searches per minute per user
let limiter = RateLimiter::new(10, 60);

if !limiter.check(user_did).await? {
    return Err(PdsError::RateLimitExceeded);
}
```

### 2. Caching

Cache federated results aggressively:

```rust
// Cache search results
const SEARCH_CACHE_TTL: u64 = 300; // 5 minutes

// Cache DID resolutions
const DID_CACHE_TTL: u64 = 3600; // 1 hour

// Cache remote profiles
const PROFILE_CACHE_TTL: u64 = 600; // 10 minutes
```

### 3. Error Handling

Handle federation errors gracefully:

```rust
match search.search_actors(query, limit).await {
    Ok(actors) => Ok(actors),
    Err(e) => {
        warn!("Federated search failed, falling back to local: {}", e);
        // Fall back to local search
        local_search(query, limit).await
    }
}
```

### 4. Monitoring

Track federation metrics:

```rust
metrics::record_federated_request("search_actors", success);
metrics::record_federation_latency(duration_ms);
metrics::record_relay_events(event_count);
```

### 5. Security

Verify remote content:

```rust
// Verify signatures on remote posts
if !verify_signature(&post, &author_did).await? {
    warn!("Invalid signature on remote post");
    return Err(PdsError::InvalidSignature);
}

// Check trust level
if !is_trusted_instance(&author_pds).await? {
    // Apply additional scrutiny
    moderate_remote_content(&post).await?;
}
```

---

## Troubleshooting

### Remote Authentication Failures

**Problem**: Can't verify remote user tokens

**Solutions**:
```bash
# Check DID resolution
curl https://plc.directory/did:plc:remote123

# Verify network connectivity
curl https://remote-pds.example.com/xrpc/com.atproto.server.getSession \
  -H "Authorization: Bearer token"

# Check logs
tail -f logs/aurora-locus.log | grep "federation"
```

### PDS Discovery Issues

**Problem**: Not finding PDS instances

**Solutions**:
```bash
# Verify relay connectivity
curl https://relay.bsky.app/xrpc/com.atproto.sync.listRepos

# Check DNS resolution
dig example.com TXT

# Manual instance addition
FEDERATION_KNOWN_INSTANCES=did:plc:abc:https://pds.com
```

### Slow Federated Search

**Problem**: Search taking too long

**Solutions**:
```rust
// Reduce concurrent requests
FEDERATION_MAX_CONCURRENT=5  // Lower concurrency

// Decrease timeout
FEDERATION_TIMEOUT=15  // Faster fail

// Implement search result caching
cache.set("search:query", &results, 600).await?;
```

### Relay Connection Drops

**Problem**: Relay disconnecting frequently

**Solutions**:
```bash
# Increase reconnect interval
RELAY_RECONNECT_INTERVAL=10

# Check network stability
ping -c 100 relay.bsky.app

# Monitor WebSocket health
tail -f logs/relay.log
```

---

## API Reference

### Federation Endpoints

```http
# Verify remote user
GET /xrpc/app.bsky.federation.verifyRemoteUser
  ?did=did:plc:remote123
  &token=bearer_token

# Federated search
GET /xrpc/app.bsky.actor.searchActors
  ?q=alice
  &federated=true
  &limit=50

# Aggregated timeline
GET /xrpc/app.bsky.feed.getTimeline
  ?federated=true
  &limit=100
```

### Response Examples

**Verified Remote User**:
```json
{
  "did": "did:plc:remote123",
  "handle": "alice.remote.com",
  "pdsEndpoint": "https://pds.remote.com",
  "verified": true
}
```

**Federated Search**:
```json
{
  "actors": [
    {
      "did": "did:plc:abc",
      "handle": "alice.bsky.social",
      "source": "pds.bsky.social"
    }
  ],
  "sources": {
    "pds.bsky.social": 10,
    "pds.example.com": 5
  }
}
```

---

## Examples

### Complete Federation Setup

```rust
use aurora_locus::federation::*;

// Initialize discovery
let discovery = Arc::new(PdsDiscovery::new(vec![
    "https://relay.bsky.app".to_string(),
]));

// Initialize authenticator
let authenticator = FederationAuthenticator::new(identity_resolver);

// Initialize search
let search = FederatedSearch::new(
    discovery.clone(),
    10,  // max concurrent
    30,  // timeout
);

// Initialize relay
let relay_config = RelayConfig {
    servers: vec!["https://relay.bsky.app".to_string()],
    ..Default::default()
};
let mut relay = RelayClient::new(relay_config);

// Subscribe to events
let mut events = relay.subscribe_firehose().await?;

// Handle events
tokio::spawn(async move {
    while let Some(event) = events.recv().await {
        process_relay_event(event).await;
    }
});
```

---

For more information:
- [ATProto Specification](https://atproto.com/)
- [PDS Deployment](DEPLOYMENT.md)
- [Scalability Guide](SCALABILITY.md)
