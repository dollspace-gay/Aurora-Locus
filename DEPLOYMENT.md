# Aurora Locus PDS - Deployment Guide

## Quick Start with Docker

### 1. Build and Run with Docker Compose

```bash
# Clone repository
git clone https://github.com/yourusername/aurora-locus
cd aurora-locus

# Create configuration
cp .env.example .env
# Edit .env with your settings

# Build and start
docker-compose up -d

# View logs
docker-compose logs -f

# Stop
docker-compose down
```

### 2. Environment Variables

Create a `.env` file:

```env
# Server Configuration
HOSTNAME=pds.example.com
PORT=3000
SERVICE_DID=did:web:pds.example.com

# Database
DATABASE_URL=sqlite:///data/aurora-locus.db

# Email (optional)
SMTP_HOST=smtp.example.com
SMTP_PORT=587
SMTP_USERNAME=noreply@example.com
SMTP_PASSWORD=your_password
SMTP_FROM=noreply@example.com

# Logging
RUST_LOG=info
```

## Manual Deployment

### Prerequisites

- Rust 1.75 or later
- SQLite 3.x
- SSL certificates (for production)

### 1. Build from Source

```bash
# Clone repository
git clone https://github.com/yourusername/aurora-locus
cd aurora-locus

# Build release binary
cargo build --release

# Binary location
./target/release/aurora-locus
```

### 2. Database Setup

```bash
# Migrations run automatically on startup
# Database created at location specified in DATABASE_URL
```

### 3. Run Server

```bash
# Set environment variables
export DATABASE_URL=sqlite:///path/to/database.db
export HOSTNAME=pds.example.com
export PORT=3000
export SERVICE_DID=did:web:pds.example.com

# Run server
./target/release/aurora-locus
```

### 4. Systemd Service (Linux)

Create `/etc/systemd/system/aurora-locus.service`:

```ini
[Unit]
Description=Aurora Locus ATProto PDS
After=network.target

[Service]
Type=simple
User=aurora
WorkingDirectory=/opt/aurora-locus
EnvironmentFile=/opt/aurora-locus/.env
ExecStart=/opt/aurora-locus/aurora-locus
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
```

Enable and start:

```bash
sudo systemctl enable aurora-locus
sudo systemctl start aurora-locus
sudo systemctl status aurora-locus
```

## Production Considerations

### 1. Reverse Proxy (Nginx)

```nginx
server {
    listen 443 ssl http2;
    server_name pds.example.com;

    ssl_certificate /path/to/cert.pem;
    ssl_certificate_key /path/to/key.pem;

    location / {
        proxy_pass http://127.0.0.1:3000;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }

    # WebSocket support (future)
    location /xrpc/com.atproto.sync.subscribeRepos {
        proxy_pass http://127.0.0.1:3000;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
    }
}
```

### 2. SSL/TLS

Use Let's Encrypt for free SSL certificates:

```bash
sudo apt-get install certbot python3-certbot-nginx
sudo certbot --nginx -d pds.example.com
```

### 3. Database Backups

```bash
# Backup script
#!/bin/bash
DATE=$(date +%Y%m%d_%H%M%S)
sqlite3 /data/aurora-locus.db ".backup /backups/aurora-locus-$DATE.db"

# Keep only last 7 days
find /backups -name "aurora-locus-*.db" -mtime +7 -delete
```

Add to crontab:
```
0 2 * * * /path/to/backup-script.sh
```

### 4. Monitoring

Health check endpoint:
```bash
curl http://localhost:3000/xrpc/_health
```

Check logs:
```bash
journalctl -u aurora-locus -f
```

### 5. Rate Limiting

Built-in rate limiting:
- Authenticated: 100 req/sec
- Unauthenticated: 10 req/sec
- Admin: 1000 req/sec

Configure in code or environment variables (future enhancement).

## Initial Setup

### 1. Create SuperAdmin Account

```bash
# Create first account via API
curl -X POST https://pds.example.com/xrpc/com.atproto.server.createAccount \
  -H "Content-Type: application/json" \
  -d '{
    "handle": "admin.pds.example.com",
    "email": "admin@example.com",
    "password": "secure_password"
  }'

# Grant superadmin role (requires database access initially)
sqlite3 /data/aurora-locus.db <<EOF
INSERT INTO admin_role (did, role, granted_by, granted_at)
VALUES ('did:plc:YOUR_ADMIN_DID', 'superadmin', 'system', datetime('now'));
EOF
```

### 2. Configure Invite Codes

```bash
# Create invite codes via admin API
curl -X POST https://pds.example.com/xrpc/com.atproto.admin.createInviteCode \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "uses": 10,
    "expiresDays": 30,
    "note": "Beta testers"
  }'
```

## Troubleshooting

### Database locked errors

```bash
# Enable WAL mode
sqlite3 /data/aurora-locus.db "PRAGMA journal_mode=WAL;"
```

### Port already in use

```bash
# Check what's using port 3000
sudo lsof -i :3000

# Change port in .env
PORT=3001
```

### Migration errors

```bash
# Check migration status
sqlite3 /data/aurora-locus.db <<EOF
SELECT * FROM _sqlx_migrations ORDER BY installed_on DESC;
EOF
```

## Scaling

### Horizontal Scaling

Aurora Locus is designed to run as a single instance per domain. For high availability:

1. Use database replication
2. Deploy multiple instances behind load balancer
3. Use shared storage for blobs
4. Configure session affinity

### Vertical Scaling

- CPU: 2+ cores recommended
- RAM: 2GB minimum, 4GB+ for production
- Disk: SSD recommended, size depends on user count
  - Estimate: 100MB per active user

## Security Checklist

- [ ] SSL/TLS configured and enforced
- [ ] Firewall configured (only ports 80, 443 exposed)
- [ ] Regular security updates applied
- [ ] Database backups automated
- [ ] Strong admin passwords
- [ ] Email verification enabled
- [ ] Rate limiting configured
- [ ] Log monitoring active
- [ ] Invite codes required (optional)

## Updates

### Rolling Update

```bash
# Build new version
git pull
cargo build --release

# Stop old version
sudo systemctl stop aurora-locus

# Replace binary
sudo cp target/release/aurora-locus /opt/aurora-locus/

# Start new version
sudo systemctl start aurora-locus

# Check logs
journalctl -u aurora-locus -n 100
```

### Docker Update

```bash
docker-compose down
git pull
docker-compose build
docker-compose up -d
```

## Support

- Documentation: https://github.com/yourusername/aurora-locus/docs
- Issues: https://github.com/yourusername/aurora-locus/issues
- Community: https://discord.gg/aurora-locus
