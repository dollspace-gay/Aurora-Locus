# Aurora Locus Admin Panel - Access Guide

## Server URLs

### Admin Control Panel
- **URL**: `http://localhost:3000/admin/`
- **Login**: `http://localhost:3000/admin/login.html`
- **Public Access**: `http://129.222.126.193:3000/admin/`

### API Endpoints
- **Base**: `http://localhost:3000/xrpc/`
- **Health**: `http://localhost:3000/health`
- **Metrics**: `http://localhost:3000/metrics`

## Quick Start

### 1. Check if Server is Running
```bash
# Check if port 3000 is listening
netstat -an | findstr ":3000"

# Or check process
tasklist | findstr aurora-locus
```

### 2. Test Server Health
```bash
curl http://localhost:3000/health
```

### 3. Access Admin Panel
Open in browser:
```
http://localhost:3000/admin/
```

## Creating an Admin Account

### Method 1: Direct Database
```bash
cd "c:\Users\admin\RustSDK\Rust-Atproto-SDK\Aurora Locus"

# Open database
sqlite3 data/account.sqlite

# Create test account (password: admin123)
INSERT INTO accounts (did, handle, email, password_hash, created_at, status)
VALUES (
  'did:web:admin.localhost',
  'admin.localhost',
  'admin@localhost',
  '$2b$10$YourHashedPasswordHere',  -- Replace with actual hash
  datetime('now'),
  'active'
);

# Grant admin role
INSERT INTO admin_roles (did, role, granted_by, granted_at)
VALUES ('did:web:admin.localhost', 'admin', 'system', datetime('now'));

.exit
```

### Method 2: Via API
```bash
# Create account
curl -X POST http://localhost:3000/xrpc/com.atproto.server.createAccount \
  -H "Content-Type: application/json" \
  -d '{
    "handle": "admin.localhost",
    "email": "admin@localhost",
    "password": "admin123"
  }'

# Note the DID from response, then grant admin role in database
```

## Admin Panel Features

### Dashboard
- Server statistics
- User growth charts
- Activity metrics
- Real-time updates (30s refresh)

### Users
- Browse all accounts
- View user details
- Suspend/unsuspend users
- Search and filter

### Moderation Queue
- Pending moderation items
- Dismiss or takedown actions
- Reason categorization

### Reports
- User-submitted reports
- Status tracking
- Resolution actions

### Invite Codes
- Generate codes in bulk
- Usage tracking
- Disable functionality

### Settings
- Server configuration
- Registration settings
- Moderation policies

## Troubleshooting

### Port 3000 Not Accessible
```bash
# Check if something else is using port 3000
netstat -ano | findstr ":3000"

# Kill process if needed
taskkill /PID <PID> /F
```

### Server Won't Start
```bash
# Check logs
cd "c:\Users\admin\RustSDK\Rust-Atproto-SDK\Aurora Locus"
./target/release/aurora-locus.exe

# Clean database and restart
rm -rf data/*.sqlite*
./target/release/aurora-locus.exe
```

### Can't Login to Admin Panel
1. Verify account exists in `accounts` table
2. Check `admin_roles` table has entry for your DID
3. Try resetting password or creating new admin account
4. Check browser console for JavaScript errors

## Build Status

Current build started at: `$(date)`
Estimated completion: 3-5 minutes

Monitor build progress:
```bash
tail -f build.log
```

Check if binary exists:
```bash
ls -lh target/release/aurora-locus.exe
```

## Configuration

Server configured for:
- **IP**: 129.222.126.193 (public) / 0.0.0.0 (bind all)
- **Port**: 3000
- **DID**: did:web:129.222.126.193
- **Data**: ./data/
- **Static Files**: ./static/admin/

See `.env` file for full configuration.
