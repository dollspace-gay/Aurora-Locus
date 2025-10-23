# Aurora Locus PDS - Backup & Restore Scripts

This directory contains scripts for backing up and restoring Aurora Locus PDS data.

## Quick Start

### Backup (Linux/macOS)

```bash
# Simple backup (saves to ./backups)
./scripts/backup.sh

# Custom backup location
./scripts/backup.sh /mnt/external-drive/backups
```

### Backup (Windows)

```powershell
# Simple backup (saves to .\backups)
.\scripts\backup.ps1

# Custom backup location
.\scripts\backup.ps1 -BackupDir "D:\Backups"
```

### Restore (Linux/macOS)

```bash
# List available backups
ls -1t backups/

# Restore from backup
./scripts/restore.sh backups/backup_20250322_143022

# Force restore (skip confirmation)
./scripts/restore.sh backups/backup_20250322_143022 --force
```

### Restore (Windows)

```powershell
# List available backups
Get-ChildItem backups\ | Sort-Object LastWriteTime -Descending

# Restore from backup
.\scripts\restore.ps1 -BackupPath "backups\backup_20250322_143022"
```

## Files

| File | Platform | Description |
|------|----------|-------------|
| `backup.sh` | Linux/macOS | Bash backup script |
| `backup.ps1` | Windows | PowerShell backup script |
| `restore.sh` | Linux/macOS | Bash restore script |
| `restore.ps1` | Windows | PowerShell restore script (planned) |

## Environment Variables

Configure backup behavior using environment variables (in `.env` file or shell):

```bash
# Backup settings
BACKUP_RETAIN_DAYS=30          # Keep backups for 30 days
COMPRESSION=gzip               # Compression: gzip, bzip2, xz, none

# Data locations (override defaults)
PDS_DATA_DIRECTORY=./data
PDS_ACCOUNT_DB_LOCATION=./data/account.sqlite
PDS_SEQUENCER_DB_LOCATION=./data/sequencer.sqlite
PDS_DID_CACHE_DB_LOCATION=./data/did_cache.sqlite
PDS_ACTOR_STORE_DIRECTORY=./data/actors
PDS_BLOBSTORE_DISK_LOCATION=./data/blobs
```

## What Gets Backed Up

- **Databases**:
  - `account.sqlite` - User accounts, sessions, credentials
  - `sequencer.sqlite` - Event sequencing, commit history
  - `did_cache.sqlite` - Cached DID documents

- **Directories**:
  - `actors/` - Repository data, MST structures
  - `blobs/` - User-uploaded media files

## Backup Structure

```
backups/
└── backup_20250322_143022/
    ├── manifest.json              # Metadata about the backup
    ├── databases/
    │   ├── account.sqlite.gz      # Compressed account database
    │   ├── sequencer.sqlite.gz    # Compressed sequencer database
    │   └── did_cache.sqlite.gz    # Compressed DID cache
    ├── blobs.tar.gz               # Compressed blob storage
    └── actors.tar.gz              # Compressed actor store
```

## Advanced Usage

### Automated Backups (cron)

```bash
# Add to crontab (daily at 2 AM)
0 2 * * * /opt/aurora-locus/scripts/backup.sh /mnt/backup-drive

# Weekly on Sunday at 3 AM
0 3 * * 0 /opt/aurora-locus/scripts/backup.sh /mnt/weekly-backup
```

### Automated Backups (systemd)

See [../docs/BACKUP_RECOVERY.md](../docs/BACKUP_RECOVERY.md#systemd-timer-alternative)

### Remote Backups

```bash
# Backup and sync to remote server
./scripts/backup.sh && \
  rsync -avz backups/ user@remote-server:/backups/aurora-locus/

# Backup and upload to S3
./scripts/backup.sh && \
  aws s3 sync backups/ s3://my-bucket/aurora-locus-backups/
```

### Custom Compression

```bash
# Maximum compression (slower)
COMPRESSION=xz ./scripts/backup.sh

# No compression (faster, larger)
COMPRESSION=none ./scripts/backup.sh
```

## Troubleshooting

### "Database is locked"

Stop the PDS service before backing up:

```bash
systemctl stop aurora-locus
./scripts/backup.sh
systemctl start aurora-locus
```

### "Permission denied"

Make scripts executable:

```bash
chmod +x scripts/*.sh
```

### "Insufficient disk space"

Check available space:

```bash
df -h

# Reduce retention or use higher compression
BACKUP_RETAIN_DAYS=7 COMPRESSION=xz ./scripts/backup.sh
```

## Testing Backups

Verify a backup is valid:

```bash
# Check manifest
cat backups/backup_20250322_143022/manifest.json

# Test database decompression
gunzip -t backups/backup_20250322_143022/databases/*.gz

# Test archive extraction
tar -tzf backups/backup_20250322_143022/blobs.tar.gz > /dev/null
```

## Documentation

For complete backup and recovery documentation, see:
- [Backup & Recovery Guide](../docs/BACKUP_RECOVERY.md)

## Support

- GitHub Issues: https://github.com/aurora-locus/pds/issues
- Documentation: https://docs.aurora-locus.dev
