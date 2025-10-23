# Aurora Locus PDS - Setup Guide

## Quick Start (Fresh Install)

### Option 1: Automatic Setup (Recommended)

Run the setup script to automatically create a fresh database:

**Windows (PowerShell):**
```powershell
.\setup.ps1 -Force
```

**Linux/Mac:**
```bash
chmod +x setup.sh
./setup.sh --force
```

### Option 2: Manual Setup

1. **Remove existing data** (if any):
   ```bash
   rm -rf data
   ```

2. **Create directory structure**:
   ```bash
   mkdir -p data/actor-store data/blobs data/blobs/tmp
   ```

3. **Run database migrations**:
   ```bash
   # Install sqlx-cli if not already installed
   cargo install sqlx-cli --no-default-features --features sqlite

   # Create and migrate database
   DATABASE_URL=sqlite:data/account.sqlite sqlx database create
   DATABASE_URL=sqlite:data/account.sqlite sqlx migrate run --source migrations
   ```

4. **Configure environment** (optional):
   - Copy `.env.example` to `.env` if you want custom configuration
   - Or just use the defaults (works fine for local development)

5. **Run the server**:
   ```bash
   cargo run --release
   ```

## Database Reset

If you need to reset the database to a fresh state:

```bash
# Remove all data
rm -rf data

# Re-run migrations
DATABASE_URL=sqlite:data/account.sqlite sqlx database create
DATABASE_URL=sqlite:data/account.sqlite sqlx migrate run --source migrations
```

## Configuration

Aurora Locus can be configured via environment variables. See `.env.example` for all available options.

### Key Settings:

- `PDS_HOSTNAME` - Server hostname (default: localhost)
- `PDS_PORT` - Server port (default: 2583)
- `PDS_DATA_DIRECTORY` - Data storage location (default: ./data)
- `PDS_FEDERATION_ENABLED` - Enable Bluesky federation (default: false)
- `PDS_FEDERATION_RELAY_URLS` - Relay servers for federation

## First Run

After setup, the server will be available at:
```
http://localhost:2583
```

### Create Your First Account

You can create an account via the API:

```bash
curl -X POST http://localhost:2583/xrpc/com.atproto.server.createAccount \
  -H "Content-Type: application/json" \
  -d '{
    "handle": "alice.localhost",
    "email": "alice@example.com",
    "password": "your-secure-password"
  }'
```

## Verification

Check that the server is running:

```bash
curl http://localhost:2583/xrpc/com.atproto.server.describeServer
```

You should see server information including the service DID.

## Troubleshooting

### "Cannot find database"
- Run the migrations: `DATABASE_URL=sqlite:data/account.sqlite sqlx migrate run --source migrations`

### "Permission denied"
- Ensure the data directory is writable
- On Linux/Mac: `chmod -R 755 data`

### "Port already in use"
- Change the port: `PDS_PORT=3000 cargo run --release`

## Production Deployment

For production use:

1. **Change all secrets** in your `.env` file
2. **Set proper hostname**: `PDS_HOSTNAME=your-domain.com`
3. **Use HTTPS**: Configure a reverse proxy (nginx, Caddy, etc.)
4. **Enable federation**: Set `PDS_FEDERATION_ENABLED=true`
5. **Set public URL**: `PDS_FEDERATION_PUBLIC_URL=https://your-domain.com`
6. **Configure PLC**: Set `PDS_PLC_ROTATION_KEY` to a secure random value

## Next Steps

- Read the [Federation Guide](FEDERATION.md) for Bluesky integration
- Check [IMPLEMENTATION_STATUS.md](IMPLEMENTATION_STATUS.md) for feature status
- Join the ATProto community for support

---

**Aurora Locus** - A Rust-based ATProto Personal Data Server
