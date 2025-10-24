# Aurora Locus - Quick Start Guide

This guide will help you get Aurora Locus PDS up and running quickly.

## Installation Methods

### Method 1: Automated Installation (Recommended)

The easiest way to set up Aurora Locus is using the interactive installation script:

```bash
# Clone the repository
git clone <repository-url>
cd Aurora-Locus

# Run the installation script
./install.sh
```

The script will:
- Check dependencies
- Collect configuration (domain, email, passwords)
- Generate all cryptographic keys automatically
- Create OAuth keyset for admin authentication
- Set up `.env` configuration file
- Build the release binary
- Optionally create systemd service and nginx config

Follow the interactive prompts to configure your PDS!

### Method 2: Manual Installation

If you prefer manual setup:

#### Prerequisites

- Rust 1.75+ with Cargo
- SQLite 3.35+
- OpenSSL, jq, xxd

#### Generate Cryptographic Keys

```bash
# 1. Generate repository signing key (secp256k1)
openssl ecparam -name secp256k1 -genkey -noout -out repo_key.pem
openssl ec -in repo_key.pem -outform DER | xxd -p -c 256 > repo_key.hex

# 2. Generate PLC rotation key (secp256k1)
openssl ecparam -name secp256k1 -genkey -noout -out plc_key.pem
openssl ec -in plc_key.pem -outform DER | xxd -p -c 256 > plc_key.hex

# 3. Generate OAuth keyset (P-256/ES256)
./oauth-keyset-json.sh
```

#### Configure Environment

```bash
# Copy template
cp .env.example .env

# Edit .env and set:
# - PDS_HOSTNAME (your domain)
# - PDS_JWT_SECRET (random 64+ char string)
# - PDS_ADMIN_PASSWORD (secure password)
# - PDS_REPO_SIGNING_KEY_K256_PRIVATE_KEY_HEX (from repo_key.hex)
# - PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX (from plc_key.hex)
# - OAUTH_KEYSET_FILE=./oauth-keyset.json
```

#### Build and Run

```bash
# Create data directories
mkdir -p data/actors data/blobs data/tmp

# Build release binary
cargo build --release

# Run the server
./target/release/aurora-locus
```

## First-Time Setup

### 1. Create Admin Account

```bash
# Register your account
curl -X POST http://localhost:3000/xrpc/com.atproto.server.createAccount \
  -H "Content-Type: application/json" \
  -d '{
    "handle": "admin.yourdomain.com",
    "email": "admin@yourdomain.com",
    "password": "your-secure-password"
  }'

# Response will include your DID
# {"did": "did:plc:abc123...", ...}
```

### 2. Grant SuperAdmin Role

```bash
# Access the database
sqlite3 data/accounts.db

# Grant SuperAdmin role (replace YOUR_DID with actual DID from step 1)
INSERT INTO admin_role (did, role, granted_by, granted_at)
VALUES ('YOUR_DID', 'superadmin', 'system', datetime('now'));

# Exit
.exit
```

### 3. Update OAuth Admin DIDs

Edit `.env` and add your DID:

```bash
OAUTH_ADMIN_DIDS=did:plc:YOUR_DID_HERE
```

Restart the server for changes to take effect.

### 4. Test OAuth Admin Login

```bash
# Get OAuth authorization URL
curl http://localhost:3000/.well-known/oauth-authorization-server | jq .

# Access the admin OAuth flow
# Visit: http://localhost:3000/oauth/authorize?...
```

## Quick Test Commands

### Health Check
```bash
curl http://localhost:3000/health
```

### Server Info
```bash
curl http://localhost:3000/xrpc/com.atproto.server.describeServer | jq .
```

### Create a Post (as user)
```bash
# First, login
curl -X POST http://localhost:3000/xrpc/com.atproto.server.createSession \
  -H "Content-Type: application/json" \
  -d '{"identifier":"admin.yourdomain.com","password":"your-password"}' \
  | jq -r .accessJwt > token.txt

# Create a post
curl -X POST http://localhost:3000/xrpc/com.atproto.repo.createRecord \
  -H "Authorization: Bearer $(cat token.txt)" \
  -H "Content-Type: application/json" \
  -d '{
    "repo": "YOUR_DID",
    "collection": "app.bsky.feed.post",
    "record": {
      "$type": "app.bsky.feed.post",
      "text": "Hello from Aurora Locus!",
      "createdAt": "'$(date -u +%Y-%m-%dT%H:%M:%S.000Z)'"
    }
  }'
```

### Subscribe to Firehose
```bash
websocat ws://localhost:3000/xrpc/com.atproto.sync.subscribeRepos
```

## Production Deployment

### With Systemd

```bash
# The install.sh script creates this file at /tmp/aurora-locus.service
sudo cp /tmp/aurora-locus.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable aurora-locus
sudo systemctl start aurora-locus
sudo systemctl status aurora-locus
```

### With Nginx Reverse Proxy

```bash
# Get SSL certificate
sudo certbot --nginx -d yourdomain.com

# Install nginx config (created by install.sh)
sudo cp /tmp/aurora-locus-nginx.conf /etc/nginx/sites-available/aurora-locus
sudo ln -s /etc/nginx/sites-available/aurora-locus /etc/nginx/sites-enabled/
sudo nginx -t
sudo systemctl reload nginx
```

### With Docker

```bash
# Build image
docker build -t aurora-locus .

# Run container
docker run -d \
  -p 3000:3000 \
  -v $(pwd)/data:/app/data \
  -v $(pwd)/.env:/app/.env \
  -v $(pwd)/oauth-keyset.json:/app/oauth-keyset.json \
  --name aurora-locus \
  aurora-locus

# View logs
docker logs -f aurora-locus
```

## Configuration Reference

### Essential Settings

```bash
# Server
PDS_HOSTNAME=pds.example.com          # Your domain
PDS_PORT=3000                          # Server port
PDS_SERVICE_DID=did:web:pds.example.com

# Security
PDS_JWT_SECRET=<64-char-random-string>
PDS_ADMIN_PASSWORD=<secure-password>

# Cryptographic Keys (generated during install)
PDS_REPO_SIGNING_KEY_K256_PRIVATE_KEY_HEX=<hex-key>
PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX=<hex-key>

# OAuth
OAUTH_KEYSET_FILE=./oauth-keyset.json
OAUTH_ADMIN_DIDS=did:plc:your-admin-did

# Storage
PDS_DATA_DIRECTORY=./data
PDS_BLOBSTORE_DISK_LOCATION=./data/blobs
```

### Optional Settings

```bash
# Federation
FEDERATION_ENABLED=true
FEDERATION_RELAY_URLS=https://bsky.network

# Email
EMAIL_SMTP_URL=smtp://smtp.example.com:587
EMAIL_FROM_ADDRESS=noreply@pds.example.com

# Invite Codes
INVITE_REQUIRED=false

# Rate Limiting
RATE_LIMIT_ENABLED=true
RATE_LIMIT_GLOBAL_HOURLY=3000
```

## Monitoring

### View Logs

```bash
# Development
cargo run --release

# Production (systemd)
sudo journalctl -u aurora-locus -f

# Docker
docker logs -f aurora-locus
```

### Metrics Endpoint

```bash
# Prometheus-compatible metrics
curl http://localhost:3000/metrics
```

### Admin Stats

```bash
# Get server statistics (requires admin OAuth token)
curl http://localhost:3000/xrpc/com.atproto.admin.getStats \
  -H "Authorization: Bearer YOUR_ADMIN_TOKEN"
```

## Troubleshooting

### Server Won't Start

```bash
# Check dependencies
cargo --version
sqlite3 --version
openssl version

# Check port availability
lsof -i :3000

# Check logs
RUST_LOG=debug ./target/release/aurora-locus
```

### Database Errors

```bash
# Reset database (WARNING: deletes all data)
rm -rf data/
mkdir -p data/actors data/blobs data/tmp

# Migrations will run on next startup
```

### OAuth Issues

```bash
# Verify oauth-keyset.json exists and is valid
cat oauth-keyset.json | jq .

# Check OAuth admin DIDs in .env
grep OAUTH_ADMIN_DIDS .env

# Test OAuth metadata endpoint
curl http://localhost:3000/.well-known/oauth-authorization-server | jq .
```

### Federation Not Working

```bash
# Check federation is enabled
grep FEDERATION_ENABLED .env

# Check relay URL is correct
grep FEDERATION_RELAY_URLS .env

# Check sequencer is running
curl http://localhost:3000/xrpc/com.atproto.sync.subscribeRepos
```

## Next Steps

- **Configure DNS**: Point your domain to your server's IP
- **Set up SSL**: Use Let's Encrypt with certbot
- **Enable Federation**: Connect to Bluesky network
- **Create Users**: Start registering accounts
- **Monitor**: Set up logging and metrics collection

## Getting Help

- Read the full [README.md](README.md) for detailed information
- Check [ARCHITECTURE.md](ARCHITECTURE.md) for technical details
- Review the [migrations/](migrations/) for database schema
- Examine [.env.example](.env.example) for all configuration options

## Quick Reference

| Command | Description |
|---------|-------------|
| `./install.sh` | Interactive installation |
| `cargo build --release` | Build optimized binary |
| `./target/release/aurora-locus` | Run the server |
| `./oauth-keyset-json.sh` | Generate OAuth keys |
| `sqlite3 data/accounts.db` | Access database |
| `curl localhost:3000/health` | Health check |

---

**Production Ready**: Aurora Locus is ready for production deployment on the Bluesky network! 🎉
