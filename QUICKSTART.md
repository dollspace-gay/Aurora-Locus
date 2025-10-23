# Aurora Locus - Quick Start Guide

## Server Information

**IP Address**: 129.222.126.193
**Port**: 3000

## Access URLs

### Main Server
- **Public**: http://129.222.126.193:3000
- **Local**: http://localhost:3000

### Admin Control Panel
- **Public**: http://129.222.126.193:3000/admin/
- **Local**: http://localhost:3000/admin/

### API Endpoints
- **XRPC Base**: http://129.222.126.193:3000/xrpc/
- **Metrics**: http://129.222.126.193:3000/metrics
- **Health**: http://129.222.126.193:3000/health

## Starting the Server

### Option 1: Using the startup script
```bash
cd "c:\Users\admin\RustSDK\Rust-Atproto-SDK\Aurora Locus"
./start_server.sh
```

### Option 2: Manual start
```bash
cd "c:\Users\admin\RustSDK\Rust-Atproto-SDK\Aurora Locus"
cargo run --release
```

## Creating an Admin Account

### Step 1: Register an account via API
```bash
curl -X POST http://localhost:3000/xrpc/com.atproto.server.createAccount \
  -H "Content-Type: application/json" \
  -d '{
    "handle": "admin.129.222.126.193",
    "email": "admin@localhost",
    "password": "admin123"
  }'
```

### Step 2: Grant admin role in database
```bash
# Connect to the account database
sqlite3 data/account.sqlite

# Find your DID
SELECT did, handle FROM account WHERE handle LIKE '%admin%';

# Grant admin role (replace <YOUR_DID> with the actual DID)
INSERT INTO admin_roles (did, role, granted_by, granted_at)
VALUES ('<YOUR_DID>', 'admin', 'system', datetime('now'));

# Exit sqlite
.exit
```

### Step 3: Login to Admin Panel
1. Open http://129.222.126.193:3000/admin/login.html
2. Enter credentials:
   - **Identifier**: admin.129.222.126.193 (or admin@localhost)
   - **Password**: admin123
3. Click "Sign In"

## Admin Panel Features

Once logged in, you'll have access to:

1. **Dashboard** - Server statistics and metrics
2. **Users** - Manage user accounts
3. **Moderation Queue** - Review flagged content
4. **Reports** - Handle user reports
5. **Invite Codes** - Generate and manage invites
6. **Settings** - Configure server parameters

## Testing the Server

### Health Check
```bash
curl http://localhost:3000/health
```

### Server Description
```bash
curl http://localhost:3000/xrpc/com.atproto.server.describeServer
```

### Metrics (Prometheus format)
```bash
curl http://localhost:3000/metrics
```

## Troubleshooting

### Server won't start
- Check if port 3000 is already in use
- Verify data directories exist: `mkdir -p data/blobs/tmp data/actors`
- Check logs for errors

### Can't access admin panel
- Verify server is running
- Check that static files exist in `static/admin/`
- Try accessing via localhost first: http://localhost:3000/admin/

### Login fails
- Ensure account exists in database
- Verify admin role is granted in `admin_roles` table
- Check JWT secret is configured in .env

## Configuration

Server configuration is in `.env` file:

```bash
# Key settings
PDS_HOSTNAME=0.0.0.0
PDS_PORT=3000
PDS_SERVICE_DID=did:web:129.222.126.193
PDS_INVITE_REQUIRED=false
```

## Logs

Server logs are output to console. For JSON formatting:
```bash
LOG_FORMAT=json cargo run --release
```

## Support

For issues or questions, see the full documentation in `docs/ADMIN_PANEL.md`
