# Aurora Locus PDS - Backup & Recovery Guide

## Table of Contents

1. [Overview](#overview)
2. [Backup Strategy](#backup-strategy)
3. [Manual Backups](#manual-backups)
4. [Automated Backups](#automated-backups)
5. [Restore Procedures](#restore-procedures)
6. [Disaster Recovery](#disaster-recovery)
7. [Testing Backups](#testing-backups)
8. [Best Practices](#best-practices)
9. [Troubleshooting](#troubleshooting)

---

## Overview

Aurora Locus PDS includes comprehensive backup and recovery tools to protect your data:

- **SQLite databases**: Account, sequencer, and DID cache databases
- **Blob storage**: User-uploaded media and files
- **Actor store**: Repository data and commit history
- **Configuration**: Environment settings (manual backup recommended)

### What Gets Backed Up

| Component | Description | Location |
|-----------|-------------|----------|
| Account DB | User accounts, sessions, credentials | `data/account.sqlite` |
| Sequencer DB | Event sequencing, commit history | `data/sequencer.sqlite` |
| DID Cache DB | Cached DID documents and handles | `data/did_cache.sqlite` |
| Blob Storage | User-uploaded images, videos, files | `data/blobs/` |
| Actor Store | Repository data, MST structures | `data/actors/` |

---

## Backup Strategy

### Backup Types

1. **Full Backup**: Complete copy of all data (recommended for production)
2. **Incremental Backup**: Only changed files (not currently supported)
3. **Snapshot Backup**: Point-in-time copy using filesystem snapshots

### Recommended Schedule

| Environment | Frequency | Retention |
|-------------|-----------|-----------|
| Production | Daily (automated) | 30 days |
| Staging | Weekly | 14 days |
| Development | Manual/On-demand | 7 days |

### Storage Requirements

Estimate your backup storage needs:

```bash
# Check current data size
du -sh ./data

# Estimate with compression (typically 50-70% reduction)
# If data is 10GB, compressed backups will be ~3-5GB
```

---

## Manual Backups

### Linux/macOS

#### Basic Backup

```bash
# Run backup script (saves to ./backups by default)
./scripts/backup.sh

# Specify custom backup directory
./scripts/backup.sh /path/to/backups
```

#### With Custom Settings

```bash
# Set environment variables for custom behavior
export BACKUP_RETAIN_DAYS=60
export COMPRESSION=gzip

./scripts/backup.sh /mnt/backup-drive/aurora-locus
```

### Windows (PowerShell)

```powershell
# Run backup script
.\scripts\backup.ps1

# Specify custom backup directory
.\scripts\backup.ps1 -BackupDir "D:\Backups\AuroraLocus"
```

### Backup Output

The script creates a timestamped backup directory:

```
backups/
└── backup_20250322_143022/
    ├── manifest.json              # Backup metadata
    ├── databases/
    │   ├── account.sqlite.gz
    │   ├── sequencer.sqlite.gz
    │   └── did_cache.sqlite.gz
    ├── blobs.tar.gz              # Compressed blob storage
    └── actors.tar.gz             # Compressed actor store
```

### Verify Backup

```bash
# Check backup manifest
cat backups/backup_20250322_143022/manifest.json

# List backup contents
ls -lh backups/backup_20250322_143022/

# Test decompression (without overwriting)
gunzip -t backups/backup_20250322_143022/databases/account.sqlite.gz
```

---

## Automated Backups

### Configuration

Enable automated backups via environment variables:

```bash
# .env file
BACKUP_ENABLED=true
BACKUP_INTERVAL_HOURS=24        # Run every 24 hours
BACKUP_DIR=./backups
BACKUP_RETAIN_DAYS=30           # Keep backups for 30 days
BACKUP_COMPRESSION=gzip         # Options: gzip, bzip2, xz, none
```

### Running the Scheduler

The backup scheduler runs as part of the Aurora Locus service:

```rust
// In your application startup code
use aurora_locus::backup::{BackupConfig, BackupScheduler};

#[tokio::main]
async fn main() {
    let backup_config = BackupConfig::from_env();

    if backup_config.enabled {
        let scheduler = BackupScheduler::new(backup_config);
        tokio::spawn(async move {
            if let Err(e) = scheduler.start().await {
                eprintln!("Backup scheduler error: {}", e);
            }
        });
    }

    // ... rest of application
}
```

### Systemd Timer (Alternative)

For Linux systems, you can use systemd timers:

```ini
# /etc/systemd/system/aurora-locus-backup.service
[Unit]
Description=Aurora Locus PDS Backup
After=network.target

[Service]
Type=oneshot
User=aurora
WorkingDirectory=/opt/aurora-locus
ExecStart=/opt/aurora-locus/scripts/backup.sh
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
```

```ini
# /etc/systemd/system/aurora-locus-backup.timer
[Unit]
Description=Aurora Locus PDS Daily Backup
Requires=aurora-locus-backup.service

[Timer]
OnCalendar=daily
OnCalendar=02:00
Persistent=true

[Install]
WantedBy=timers.target
```

Enable the timer:

```bash
sudo systemctl daemon-reload
sudo systemctl enable aurora-locus-backup.timer
sudo systemctl start aurora-locus-backup.timer

# Check status
sudo systemctl status aurora-locus-backup.timer
```

### Windows Task Scheduler

1. Open Task Scheduler
2. Create Basic Task
3. Set trigger (e.g., Daily at 2:00 AM)
4. Action: Start a program
   - Program: `powershell.exe`
   - Arguments: `-ExecutionPolicy Bypass -File "C:\aurora-locus\scripts\backup.ps1"`
5. Finish and test

---

## Restore Procedures

### Before Restoring

⚠️ **IMPORTANT**: Restoring will replace current data. Always:

1. **Stop the PDS service** to prevent data corruption
2. **Verify the backup** is complete and not corrupted
3. **Create a pre-restore backup** of current data (automatic with restore script)

### Full Restore

#### Linux/macOS

```bash
# Stop the service (if running)
systemctl stop aurora-locus
# or
pkill aurora-locus

# Run restore script
./scripts/restore.sh backups/backup_20250322_143022

# Confirm when prompted (or use --force to skip prompt)
./scripts/restore.sh backups/backup_20250322_143022 --force

# Start the service
systemctl start aurora-locus
```

#### Windows (PowerShell)

```powershell
# Stop the service
Stop-Service AuroraLocus

# Run restore script
.\scripts\restore.ps1 -BackupPath ".\backups\backup_20250322_143022"

# Start the service
Start-Service AuroraLocus
```

### Partial Restore

To restore only specific components:

```bash
# Extract backup directory
BACKUP_DIR="backups/backup_20250322_143022"

# Restore only database
gunzip -c $BACKUP_DIR/databases/account.sqlite.gz > ./data/account.sqlite

# Restore only blobs
tar -xzf $BACKUP_DIR/blobs.tar.gz -C ./data/

# Restore only actors
tar -xzf $BACKUP_DIR/actors.tar.gz -C ./data/
```

### Restore to Different Server

1. **Copy backup to new server**:
   ```bash
   scp -r backups/backup_20250322_143022 user@newserver:/opt/aurora-locus/backups/
   ```

2. **Install Aurora Locus on new server**

3. **Run restore**:
   ```bash
   ./scripts/restore.sh backups/backup_20250322_143022 --force
   ```

4. **Update configuration**:
   - Update `.env` with new hostname, DIDs, etc.
   - Regenerate signing keys if necessary

5. **Start service**:
   ```bash
   systemctl start aurora-locus
   ```

---

## Disaster Recovery

### Recovery Time Objective (RTO)

Expected time to restore service:

| Scenario | RTO | Steps |
|----------|-----|-------|
| Single database corruption | 15 minutes | Restore specific database |
| Complete data loss | 1-2 hours | Full restore from backup |
| Server failure | 2-4 hours | New server + restore + DNS |

### Recovery Point Objective (RPO)

Maximum acceptable data loss:

- **Daily backups**: Up to 24 hours of data loss
- **Hourly backups**: Up to 1 hour of data loss
- **Real-time replication**: Minimal data loss

### Off-Site Backups

For production environments, maintain off-site backups:

```bash
# After local backup, sync to remote location
./scripts/backup.sh /mnt/local-backup

# Sync to S3
aws s3 sync /mnt/local-backup s3://aurora-locus-backups/

# Or rsync to remote server
rsync -avz /mnt/local-backup/ user@backup-server:/backups/aurora-locus/
```

### Backup Encryption

For sensitive data, encrypt backups:

```bash
# Encrypt backup with GPG
tar -czf - backups/backup_20250322_143022 | \
  gpg --symmetric --cipher-algo AES256 > backup_encrypted.tar.gz.gpg

# Decrypt when needed
gpg --decrypt backup_encrypted.tar.gz.gpg | tar -xzf -
```

---

## Testing Backups

### Regular Backup Tests

**Monthly**: Verify backup integrity

```bash
# Test database decompression
gunzip -t backups/backup_20250322_143022/databases/*.gz

# Test archive extraction (dry run)
tar -tzf backups/backup_20250322_143022/blobs.tar.gz > /dev/null
```

**Quarterly**: Full restore test

1. Set up test environment
2. Perform full restore
3. Start PDS and verify:
   - User accounts accessible
   - Repositories browsable
   - Blobs downloadable
   - Authentication working

### Automated Testing

```bash
#!/bin/bash
# test-backup.sh - Automated backup verification

BACKUP_DIR="$1"

echo "Testing backup: $BACKUP_DIR"

# Test manifest exists
if [ ! -f "$BACKUP_DIR/manifest.json" ]; then
    echo "❌ Manifest missing"
    exit 1
fi

# Test database files
for db in account sequencer did_cache; do
    if ! gunzip -t "$BACKUP_DIR/databases/${db}.sqlite.gz" 2>/dev/null; then
        echo "❌ Database $db corrupted"
        exit 1
    fi
done

# Test archives
for archive in blobs actors; do
    if ! tar -tzf "$BACKUP_DIR/${archive}.tar.gz" > /dev/null 2>&1; then
        echo "❌ Archive $archive corrupted"
        exit 1
    fi
done

echo "✅ Backup verification passed"
```

---

## Best Practices

### 1. **3-2-1 Backup Rule**

- **3** copies of data (1 primary + 2 backups)
- **2** different media types (local disk + cloud/tape)
- **1** off-site backup

### 2. **Backup Validation**

- Verify backups immediately after creation
- Test restore procedures regularly
- Monitor backup logs for errors

### 3. **Storage Management**

```bash
# Monitor backup disk usage
df -h /mnt/backup-drive

# List backups by size
du -sh backups/backup_* | sort -h

# Clean up old backups manually
find backups -name "backup_*" -mtime +60 -exec rm -rf {} \;
```

### 4. **Security**

- Encrypt backups containing sensitive data
- Restrict backup directory permissions:
  ```bash
  chmod 700 backups/
  chown aurora:aurora backups/
  ```
- Use separate credentials for backup storage

### 5. **Monitoring**

Set up alerts for:
- Backup failures
- Disk space running low
- Backups older than 48 hours

Example monitoring script:

```bash
#!/bin/bash
# Check last backup age

LAST_BACKUP=$(ls -1t backups/ | head -1)
BACKUP_TIME=$(stat -c %Y "backups/$LAST_BACKUP")
CURRENT_TIME=$(date +%s)
AGE_HOURS=$(( ($CURRENT_TIME - $BACKUP_TIME) / 3600 ))

if [ $AGE_HOURS -gt 48 ]; then
    echo "WARNING: Last backup is $AGE_HOURS hours old"
    # Send alert (email, Slack, PagerDuty, etc.)
fi
```

---

## Troubleshooting

### Backup Issues

#### Problem: "Database is locked"

**Solution**: Stop the PDS service before backup

```bash
systemctl stop aurora-locus
./scripts/backup.sh
systemctl start aurora-locus
```

Or use hot backup with SQLite:

```bash
sqlite3 data/account.sqlite ".backup backups/account_hot.sqlite"
```

#### Problem: "Insufficient disk space"

**Solution**:

1. Check available space: `df -h`
2. Clean old backups: Set `BACKUP_RETAIN_DAYS` lower
3. Use higher compression: `COMPRESSION=xz`
4. Move backups to larger drive

#### Problem: "Backup script not found"

**Solution**:

```bash
# Ensure scripts are executable
chmod +x scripts/*.sh

# Check script path
ls -l scripts/backup.sh
```

### Restore Issues

#### Problem: "Restore fails with corrupted data"

**Solution**:

1. Verify backup integrity:
   ```bash
   gunzip -t backups/backup_*/databases/*.gz
   ```

2. Try different backup:
   ```bash
   ls -1t backups/ | head -5  # List recent backups
   ```

3. Partial restore:
   ```bash
   # Restore only uncorrupted components
   ```

#### Problem: "Service won't start after restore"

**Solution**:

1. Check permissions:
   ```bash
   chown -R aurora:aurora data/
   chmod -R 755 data/
   ```

2. Check configuration:
   ```bash
   # Ensure .env matches restored data
   cat .env | grep PDS_
   ```

3. Check logs:
   ```bash
   journalctl -u aurora-locus -n 100
   ```

#### Problem: "Database version mismatch"

**Solution**:

If restoring to a different Aurora Locus version:

```bash
# Check backup version
cat backups/backup_*/manifest.json | jq .pds_version

# May need to run migrations
cargo run --bin migrate
```

---

## Support

For backup and recovery issues:

- **Documentation**: [https://docs.aurora-locus.dev](https://docs.aurora-locus.dev)
- **GitHub Issues**: [https://github.com/aurora-locus/pds/issues](https://github.com/aurora-locus/pds/issues)
- **Community Discord**: [https://discord.gg/aurora-locus](https://discord.gg/aurora-locus)

---

## Appendix A: Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `BACKUP_ENABLED` | `false` | Enable automated backups |
| `BACKUP_INTERVAL_HOURS` | `24` | Backup frequency in hours |
| `BACKUP_DIR` | `./backups` | Backup storage directory |
| `BACKUP_RETAIN_DAYS` | `30` | Days to retain old backups |
| `BACKUP_COMPRESSION` | `gzip` | Compression type (gzip/bzip2/xz/none) |
| `BACKUP_SCRIPT_PATH` | (auto) | Custom backup script path |

## Appendix B: Backup Checklist

### Daily
- [ ] Verify automated backup ran successfully
- [ ] Check disk space on backup drive

### Weekly
- [ ] Review backup logs for errors
- [ ] Verify at least 7 backups exist

### Monthly
- [ ] Test backup decompression
- [ ] Sync backups to off-site location
- [ ] Review and update retention policy

### Quarterly
- [ ] Perform full restore test
- [ ] Update disaster recovery documentation
- [ ] Review backup storage capacity
