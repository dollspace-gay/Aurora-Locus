# Federation Testing Guide

This guide explains how to set up a local test environment for federation development and testing.

## Table of Contents

- [Overview](#overview)
- [Quick Start](#quick-start)
- [Option 1: Two Local PDS Instances](#option-1-two-local-pds-instances)
- [Option 2: Local PDS + Bluesky Network](#option-2-local-pds--bluesky-network)
- [Option 3: Docker Compose Setup](#option-3-docker-compose-setup)
- [Testing Federation Features](#testing-federation-features)
- [Troubleshooting](#troubleshooting)

## Overview

To test federation, you need at least two PDS instances that can communicate with each other. This guide covers three testing approaches:

1. **Two local instances** - Complete isolation, fastest iteration
2. **Local + Bluesky** - Test with real network
3. **Docker Compose** - Reproducible environment

## Quick Start

### Prerequisites

- Rust 1.75+
- SQLite 3.35+
- Two terminal windows
- Basic networking knowledge

### Minimal Setup (5 minutes)

```bash
# Terminal 1: PDS Instance A
cd Aurora-Locus
cp .env.example .env.a

# Edit .env.a
sed -i 's/PDS_PORT=2583/PDS_PORT=2583/' .env.a
sed -i 's/PDS_FEDERATION_ENABLED=false/PDS_FEDERATION_ENABLED=true/' .env.a
sed -i 's|PDS_PUBLIC_URL=.*|PDS_PUBLIC_URL=http://localhost:2583|' .env.a

# Run PDS A
cargo run --release -- --env-file .env.a

# Terminal 2: PDS Instance B
cd Aurora-Locus
cp .env.example .env.b

# Edit .env.b
sed -i 's/PDS_PORT=2583/PDS_PORT=2584/' .env.b
sed -i 's/PDS_DATA_DIRECTORY=./data/PDS_DATA_DIRECTORY=./data-b/' .env.b
sed -i 's/PDS_FEDERATION_ENABLED=false/PDS_FEDERATION_ENABLED=true/' .env.b
sed -i 's|PDS_PUBLIC_URL=.*|PDS_PUBLIC_URL=http://localhost:2584|' .env.b

# Run PDS B
cargo run --release -- --env-file .env.b
```

Now you have two PDS instances running on ports 2583 and 2584!

## Option 1: Two Local PDS Instances

This is the recommended approach for development.

### Step 1: Create Separate Configurations

```bash
# Create configs directory
mkdir -p configs

# PDS A config
cat > configs/.env.a <<EOF
# Service Configuration
PDS_HOSTNAME=localhost
PDS_PORT=2583
PDS_SERVICE_DID=did:web:localhost:2583
PDS_VERSION=0.1.0

# Data Storage (separate directories!)
PDS_DATA_DIRECTORY=./data-a
PDS_ACCOUNT_DB_LOCATION=./data-a/account.sqlite
PDS_SEQUENCER_DB_LOCATION=./data-a/sequencer.sqlite
PDS_DID_CACHE_DB_LOCATION=./data-a/did_cache.sqlite
PDS_ACTOR_STORE_DIRECTORY=./data-a/actors
PDS_BLOBSTORE_DISK_LOCATION=./data-a/blobs
PDS_BLOBSTORE_DISK_TMP_LOCATION=./data-a/temp

# Authentication (generate separate keys!)
PDS_JWT_SECRET=test-secret-a-32-chars-minimum
PDS_ADMIN_PASSWORD=admin-password-a
PDS_REPO_SIGNING_KEY_K256_PRIVATE_KEY_HEX=generate-unique-key-a
PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX=generate-unique-key-a

# Identity
PDS_DID_PLC_URL=https://plc.directory
PDS_SERVICE_HANDLE_DOMAINS=.localhost

# Federation
PDS_FEDERATION_ENABLED=true
PDS_FEDERATION_RELAY_URLS=http://localhost:2584
PDS_FEDERATION_FIREHOSE_ENABLED=true
PDS_FEDERATION_CRAWL_ENABLED=true
PDS_PUBLIC_URL=http://localhost:2583
PDS_FEDERATION_AUTO_STREAM=true

# Logging
RUST_LOG=info,aurora_locus=debug
EOF

# PDS B config (similar but different ports/directories)
cat > configs/.env.b <<EOF
# Service Configuration
PDS_HOSTNAME=localhost
PDS_PORT=2584
PDS_SERVICE_DID=did:web:localhost:2584
PDS_VERSION=0.1.0

# Data Storage (separate directories!)
PDS_DATA_DIRECTORY=./data-b
PDS_ACCOUNT_DB_LOCATION=./data-b/account.sqlite
PDS_SEQUENCER_DB_LOCATION=./data-b/sequencer.sqlite
PDS_DID_CACHE_DB_LOCATION=./data-b/did_cache.sqlite
PDS_ACTOR_STORE_DIRECTORY=./data-b/actors
PDS_BLOBSTORE_DISK_LOCATION=./data-b/blobs
PDS_BLOBSTORE_DISK_TMP_LOCATION=./data-b/temp

# Authentication (generate separate keys!)
PDS_JWT_SECRET=test-secret-b-32-chars-minimum
PDS_ADMIN_PASSWORD=admin-password-b
PDS_REPO_SIGNING_KEY_K256_PRIVATE_KEY_HEX=generate-unique-key-b
PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX=generate-unique-key-b

# Identity
PDS_DID_PLC_URL=https://plc.directory
PDS_SERVICE_HANDLE_DOMAINS=.localhost

# Federation
PDS_FEDERATION_ENABLED=true
PDS_FEDERATION_RELAY_URLS=http://localhost:2583
PDS_FEDERATION_FIREHOSE_ENABLED=true
PDS_FEDERATION_CRAWL_ENABLED=true
PDS_PUBLIC_URL=http://localhost:2584
PDS_FEDERATION_AUTO_STREAM=true

# Logging
RUST_LOG=info,aurora_locus=debug
EOF
```

### Step 2: Generate Cryptographic Keys

```bash
# Generate keys for PDS A
openssl genrsa -out configs/repo_key_a.pem 2048
openssl rsa -in configs/repo_key_a.pem -outform DER | xxd -p -c 256 > configs/repo_key_a.hex

openssl genrsa -out configs/plc_key_a.pem 2048
openssl rsa -in configs/plc_key_a.pem -outform DER | xxd -p -c 256 > configs/plc_key_a.hex

# Generate keys for PDS B
openssl genrsa -out configs/repo_key_b.pem 2048
openssl rsa -in configs/repo_key_b.pem -outform DER | xxd -p -c 256 > configs/repo_key_b.hex

openssl genrsa -out configs/plc_key_b.pem 2048
openssl rsa -in configs/plc_key_b.pem -outform DER | xxd -p -c 256 > configs/plc_key_b.hex

# Update configs with generated keys
# (manually copy hex contents into .env.a and .env.b)
```

### Step 3: Initialize Databases

```bash
# Create data directories
mkdir -p data-a data-b

# Initialize PDS A database
sqlite3 data-a/account.sqlite < migrations/001_initial.sql

# Initialize PDS B database
sqlite3 data-b/account.sqlite < migrations/001_initial.sql
```

### Step 4: Run Both Instances

```bash
# Terminal 1: Run PDS A
PDS_CONFIG=configs/.env.a cargo run --release

# Terminal 2: Run PDS B
PDS_CONFIG=configs/.env.b cargo run --release
```

### Step 5: Verify Federation

```bash
# Check PDS A logs
curl http://localhost:2583/health
# Should show: "Federation enabled with 1 relay server(s)"

# Check PDS B logs
curl http://localhost:2584/health
# Should show: "Federation enabled with 1 relay server(s)"

# Test WebSocket firehose on PDS A
wscat -c ws://localhost:2583/xrpc/com.atproto.sync.subscribeRepos

# Create account on PDS A and watch events appear
```

## Option 2: Local PDS + Bluesky Network

Test your PDS with the real Bluesky network.

### Requirements

- Publicly accessible server (cloud VM, ngrok, etc.)
- Valid domain name
- HTTPS certificate

### Setup with ngrok (Development)

```bash
# Install ngrok
brew install ngrok  # macOS
# or download from https://ngrok.com/

# Start your PDS locally
cargo run --release

# In another terminal, create tunnel
ngrok http 2583

# Copy the HTTPS URL (e.g., https://abc123.ngrok.io)

# Update your .env
PDS_PUBLIC_URL=https://abc123.ngrok.io
PDS_FEDERATION_ENABLED=true
PDS_FEDERATION_RELAY_URLS=https://bsky.network
PDS_FEDERATION_FIREHOSE_ENABLED=true
PDS_FEDERATION_CRAWL_ENABLED=true
PDS_FEDERATION_AUTO_STREAM=true

# Restart PDS
cargo run --release
```

### Verify with Bluesky

```bash
# Check if your PDS is reachable
curl https://abc123.ngrok.io/xrpc/com.atproto.server.describeServer

# Create an account
curl -X POST https://abc123.ngrok.io/xrpc/com.atproto.server.createAccount \
  -H "Content-Type: application/json" \
  -d '{
    "handle": "test.abc123.ngrok.io",
    "email": "test@example.com",
    "password": "secure-password"
  }'

# Create a post
curl -X POST https://abc123.ngrok.io/xrpc/com.atproto.repo.createRecord \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "repo": "did:plc:...",
    "collection": "app.bsky.feed.post",
    "record": {
      "$type": "app.bsky.feed.post",
      "text": "Hello from Aurora Locus!",
      "createdAt": "'$(date -u +%Y-%m-%dT%H:%M:%S.000Z)'"
    }
  }'

# Watch for event in relay
# (may take 5-10 minutes for new PDSs)
```

## Option 3: Docker Compose Setup

Reproducible multi-instance setup.

### docker-compose.yml

```yaml
version: '3.8'

services:
  pds-a:
    build: .
    container_name: aurora-pds-a
    ports:
      - "2583:2583"
    environment:
      - PDS_PORT=2583
      - PDS_HOSTNAME=pds-a
      - PDS_SERVICE_DID=did:web:pds-a
      - PDS_DATA_DIRECTORY=/data
      - PDS_FEDERATION_ENABLED=true
      - PDS_FEDERATION_RELAY_URLS=http://pds-b:2584
      - PDS_FEDERATION_FIREHOSE_ENABLED=true
      - PDS_PUBLIC_URL=http://pds-a:2583
      - RUST_LOG=info,aurora_locus=debug
    volumes:
      - ./data-a:/data
    networks:
      - federation-net

  pds-b:
    build: .
    container_name: aurora-pds-b
    ports:
      - "2584:2584"
    environment:
      - PDS_PORT=2584
      - PDS_HOSTNAME=pds-b
      - PDS_SERVICE_DID=did:web:pds-b
      - PDS_DATA_DIRECTORY=/data
      - PDS_FEDERATION_ENABLED=true
      - PDS_FEDERATION_RELAY_URLS=http://pds-a:2583
      - PDS_FEDERATION_FIREHOSE_ENABLED=true
      - PDS_PUBLIC_URL=http://pds-b:2584
      - RUST_LOG=info,aurora_locus=debug
    volumes:
      - ./data-b:/data
    networks:
      - federation-net

networks:
  federation-net:
    driver: bridge
```

### Run with Docker Compose

```bash
# Build and start
docker-compose up --build

# In another terminal, test federation
curl http://localhost:2583/health
curl http://localhost:2584/health

# Clean up
docker-compose down -v
```

## Testing Federation Features

### Test 1: Event Publishing

```bash
# Create account on PDS A
TOKEN_A=$(curl -X POST http://localhost:2583/xrpc/com.atproto.server.createAccount \
  -H "Content-Type: application/json" \
  -d '{
    "handle": "alice.localhost",
    "email": "alice@example.com",
    "password": "test123"
  }' | jq -r '.accessJwt')

# Create post on PDS A
curl -X POST http://localhost:2583/xrpc/com.atproto.repo.createRecord \
  -H "Authorization: Bearer $TOKEN_A" \
  -H "Content-Type: application/json" \
  -d '{
    "repo": "did:plc:alice...",
    "collection": "app.bsky.feed.post",
    "record": {
      "$type": "app.bsky.feed.post",
      "text": "Hello federation!",
      "createdAt": "'$(date -u +%Y-%m-%dT%H:%M:%S.000Z)'"
    }
  }'

# Check PDS A logs for "Publishing event to relay"
docker logs aurora-pds-a | grep "Publishing"

# Subscribe to PDS B firehose - should see event from PDS A
# (once Phase 3 is implemented)
```

### Test 2: Repository Synchronization

```bash
# Export repository from PDS A
curl http://localhost:2583/xrpc/com.atproto.sync.getRepo?did=did:plc:alice... \
  -o alice-repo.car

# Verify CAR file
file alice-repo.car
# Should show: "alice-repo.car: data"

# List repositories on PDS A
curl http://localhost:2583/xrpc/com.atproto.sync.listRepos | jq
```

### Test 3: Firehose WebSocket

```bash
# Install wscat
npm install -g wscat

# Connect to PDS A firehose
wscat -c ws://localhost:2583/xrpc/com.atproto.sync.subscribeRepos

# In another terminal, create a post
# Should see event in wscat output
```

### Test 4: Cross-PDS Discovery (Phase 1+)

```bash
# Once Phase 1 is implemented:

# Trigger discovery on PDS A
curl -X POST http://localhost:2583/xrpc/com.aurora.federation.refreshDiscovery \
  -H "Authorization: Bearer $ADMIN_TOKEN"

# List discovered instances
curl http://localhost:2583/xrpc/com.aurora.federation.listInstances \
  -H "Authorization: Bearer $ADMIN_TOKEN" | jq

# Should show PDS B in the list
```

### Test 5: Federated Search (Phase 2+)

```bash
# Once Phase 2 is implemented:

# Create user on PDS B
TOKEN_B=$(curl -X POST http://localhost:2584/xrpc/com.atproto.server.createAccount \
  -H "Content-Type: application/json" \
  -d '{
    "handle": "bob.localhost",
    "email": "bob@example.com",
    "password": "test456"
  }' | jq -r '.accessJwt')

# Search for Bob from PDS A
curl "http://localhost:2583/xrpc/app.bsky.actor.searchActors?q=bob" \
  -H "Authorization: Bearer $TOKEN_A" | jq

# Should return Bob's profile from PDS B
```

## Troubleshooting

### Problem: "Address already in use"

**Solution:**
```bash
# Find process using port 2583
lsof -i :2583

# Kill it
kill -9 <PID>

# Or use different port in config
PDS_PORT=2585
```

### Problem: "Database locked"

**Solution:**
```bash
# Each instance needs separate database
# Check PDS_DATA_DIRECTORY in configs
# Ensure no SQLite processes holding lock

# Force close SQLite
fuser -k data-a/account.sqlite
```

### Problem: "Federation not connecting"

**Solution:**
```bash
# Check logs for connection errors
tail -f logs/aurora-locus.log | grep -i federation

# Verify relay URLs are correct
echo $PDS_FEDERATION_RELAY_URLS

# Test connectivity
curl http://localhost:2584/health

# Check firewall
sudo iptables -L -n
```

### Problem: "Events not appearing"

**Solution:**
```bash
# Verify auto-stream is enabled
grep AUTO_STREAM .env

# Check sequencer logs
tail -f logs/aurora-locus.log | grep sequencer

# Verify relay client is initialized
curl http://localhost:2583/metrics | grep relay_connection_status
```

### Problem: "WebSocket connection refused"

**Solution:**
```bash
# Verify firehose is enabled
grep FIREHOSE_ENABLED .env

# Check endpoint is reachable
curl -i http://localhost:2583/xrpc/com.atproto.sync.subscribeRepos

# Should return: "Upgrade: websocket"
```

## Automated Testing

### Integration Test Script

```bash
#!/bin/bash
# test-federation.sh

echo "Starting federation test..."

# Start PDS A
PDS_CONFIG=configs/.env.a cargo run --release &
PDS_A_PID=$!
sleep 5

# Start PDS B
PDS_CONFIG=configs/.env.b cargo run --release &
PDS_B_PID=$!
sleep 5

# Run tests
cargo test --test federation_integration_test

# Cleanup
kill $PDS_A_PID $PDS_B_PID

echo "Test complete!"
```

### CI/CD Integration

```yaml
# .github/workflows/federation-tests.yml
name: Federation Tests

on: [push, pull_request]

jobs:
  federation:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - uses: actions-rs/toolchain@v1
        with:
          toolchain: stable

      - name: Setup test environment
        run: |
          ./scripts/setup-federation-test.sh

      - name: Run federation tests
        run: |
          cargo test --test federation_integration_test
```

## Next Steps

Once your test environment is working:

1. **Implement Phase 1** - Add PDS discovery and admin API
2. **Implement Phase 2** - Add federated search
3. **Implement Phase 3** - Subscribe to relay firehose
4. **Implement Phase 4** - Enable cross-PDS authentication

See [bd issues](https://github.com/your-repo/issues) for implementation roadmap.

## Resources

- [ATProto Specification](https://atproto.com/specs/atp)
- [Bluesky Federation Docs](https://docs.bsky.app/docs/advanced-guides/federation)
- [Federation Implementation Plan](FEDERATION.md)

## Getting Help

- Open an issue on GitHub
- Check existing issues for similar problems
- Join the ATProto Discord community
