#!/bin/bash
# Aurora Locus PDS - Backup Script
# This script performs a complete backup of the PDS database and blob storage
# Usage: ./scripts/backup.sh [backup_directory]

set -e  # Exit on error
set -u  # Exit on undefined variable

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

# Configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
DEFAULT_BACKUP_DIR="${PROJECT_DIR}/backups"
TIMESTAMP=$(date +"%Y%m%d_%H%M%S")

# Get backup directory from argument or use default
BACKUP_DIR="${1:-$DEFAULT_BACKUP_DIR}"
BACKUP_PATH="${BACKUP_DIR}/backup_${TIMESTAMP}"

# Load environment variables if .env exists
if [ -f "${PROJECT_DIR}/.env" ]; then
    set -a
    source "${PROJECT_DIR}/.env"
    set +a
fi

# Default data directory from config or environment
DATA_DIR="${PDS_DATA_DIRECTORY:-${PROJECT_DIR}/data}"
ACCOUNT_DB="${PDS_ACCOUNT_DB_LOCATION:-${DATA_DIR}/account.sqlite}"
SEQUENCER_DB="${PDS_SEQUENCER_DB_LOCATION:-${DATA_DIR}/sequencer.sqlite}"
DID_CACHE_DB="${PDS_DID_CACHE_DB_LOCATION:-${DATA_DIR}/did_cache.sqlite}"
ACTOR_STORE_DIR="${PDS_ACTOR_STORE_DIRECTORY:-${DATA_DIR}/actors}"
BLOB_DIR="${PDS_BLOBSTORE_DISK_LOCATION:-${DATA_DIR}/blobs}"

# Compression settings
COMPRESSION="gzip"  # Options: gzip, bzip2, xz, none
COMPRESSION_LEVEL=6  # 1-9 for gzip (6 is default)

# Logging
LOG_FILE="${BACKUP_DIR}/backup_${TIMESTAMP}.log"

# Function to log messages
log() {
    local level="$1"
    shift
    local message="$@"
    local timestamp=$(date +"%Y-%m-%d %H:%M:%S")
    echo "[${timestamp}] [${level}] ${message}" | tee -a "$LOG_FILE"
}

log_info() {
    echo -e "${GREEN}[INFO]${NC} $@"
    log "INFO" "$@"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $@"
    log "WARN" "$@"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $@"
    log "ERROR" "$@"
}

# Function to check if a command exists
command_exists() {
    command -v "$1" >/dev/null 2>&1
}

# Function to get file size in human-readable format
get_size() {
    if [ -f "$1" ] || [ -d "$1" ]; then
        du -sh "$1" 2>/dev/null | cut -f1 || echo "Unknown"
    else
        echo "N/A"
    fi
}

# Function to backup a SQLite database
backup_sqlite_db() {
    local db_path="$1"
    local db_name="$2"
    local backup_file="${BACKUP_PATH}/databases/${db_name}.sqlite"

    if [ ! -f "$db_path" ]; then
        log_warn "Database not found: $db_path (skipping)"
        return 0
    fi

    log_info "Backing up database: $db_name"
    log_info "  Source: $db_path"
    log_info "  Size: $(get_size "$db_path")"

    # Create directory
    mkdir -p "$(dirname "$backup_file")"

    # Use sqlite3 .dump for consistent backup
    if command_exists sqlite3; then
        sqlite3 "$db_path" ".backup '${backup_file}'" 2>&1 | tee -a "$LOG_FILE"
        if [ $? -eq 0 ]; then
            log_info "  ✓ Database backed up successfully"

            # Compress if enabled
            if [ "$COMPRESSION" != "none" ]; then
                compress_file "$backup_file"
            fi
        else
            log_error "  ✗ Failed to backup database"
            return 1
        fi
    else
        # Fallback to simple copy if sqlite3 is not available
        log_warn "  sqlite3 command not found, using file copy"
        cp "$db_path" "$backup_file"

        if [ "$COMPRESSION" != "none" ]; then
            compress_file "$backup_file"
        fi
    fi

    return 0
}

# Function to compress a file
compress_file() {
    local file="$1"

    case "$COMPRESSION" in
        gzip)
            log_info "  Compressing with gzip (level $COMPRESSION_LEVEL)..."
            gzip -${COMPRESSION_LEVEL} "$file"
            log_info "  ✓ Compressed: $(get_size "${file}.gz")"
            ;;
        bzip2)
            log_info "  Compressing with bzip2..."
            bzip2 -${COMPRESSION_LEVEL} "$file"
            log_info "  ✓ Compressed: $(get_size "${file}.bz2")"
            ;;
        xz)
            log_info "  Compressing with xz..."
            xz -${COMPRESSION_LEVEL} "$file"
            log_info "  ✓ Compressed: $(get_size "${file}.xz")"
            ;;
        none)
            # No compression
            ;;
        *)
            log_warn "  Unknown compression: $COMPRESSION, skipping compression"
            ;;
    esac
}

# Function to backup blob storage
backup_blobs() {
    local blob_dir="$1"
    local backup_file="${BACKUP_PATH}/blobs.tar"

    if [ ! -d "$blob_dir" ]; then
        log_warn "Blob directory not found: $blob_dir (skipping)"
        return 0
    fi

    log_info "Backing up blob storage..."
    log_info "  Source: $blob_dir"
    log_info "  Size: $(get_size "$blob_dir")"

    # Create tar archive
    tar -cf "$backup_file" -C "$(dirname "$blob_dir")" "$(basename "$blob_dir")" 2>&1 | tee -a "$LOG_FILE"

    if [ $? -eq 0 ]; then
        log_info "  ✓ Blobs archived successfully"

        # Compress if enabled
        if [ "$COMPRESSION" != "none" ]; then
            compress_file "$backup_file"
        fi
    else
        log_error "  ✗ Failed to archive blobs"
        return 1
    fi

    return 0
}

# Function to backup actor store
backup_actor_store() {
    local actor_dir="$1"
    local backup_file="${BACKUP_PATH}/actors.tar"

    if [ ! -d "$actor_dir" ]; then
        log_warn "Actor store directory not found: $actor_dir (skipping)"
        return 0
    fi

    log_info "Backing up actor store..."
    log_info "  Source: $actor_dir"
    log_info "  Size: $(get_size "$actor_dir")"

    # Create tar archive
    tar -cf "$backup_file" -C "$(dirname "$actor_dir")" "$(basename "$actor_dir")" 2>&1 | tee -a "$LOG_FILE"

    if [ $? -eq 0 ]; then
        log_info "  ✓ Actor store archived successfully"

        # Compress if enabled
        if [ "$COMPRESSION" != "none" ]; then
            compress_file "$backup_file"
        fi
    else
        log_error "  ✗ Failed to archive actor store"
        return 1
    fi

    return 0
}

# Function to create backup manifest
create_manifest() {
    local manifest_file="${BACKUP_PATH}/manifest.json"

    log_info "Creating backup manifest..."

    cat > "$manifest_file" <<EOF
{
  "backup_timestamp": "$(date -u +"%Y-%m-%dT%H:%M:%SZ")",
  "backup_version": "1.0",
  "pds_version": "${PDS_VERSION:-unknown}",
  "hostname": "${PDS_HOSTNAME:-unknown}",
  "service_did": "${PDS_SERVICE_DID:-unknown}",
  "compression": "${COMPRESSION}",
  "databases": {
    "account": "$(basename "$ACCOUNT_DB")",
    "sequencer": "$(basename "$SEQUENCER_DB")",
    "did_cache": "$(basename "$DID_CACHE_DB")"
  },
  "data_directories": {
    "actor_store": "$(basename "$ACTOR_STORE_DIR")",
    "blob_store": "$(basename "$BLOB_DIR")"
  },
  "backup_size": "$(get_size "$BACKUP_PATH")"
}
EOF

    log_info "  ✓ Manifest created"
}

# Function to cleanup old backups
cleanup_old_backups() {
    local retain_days="${BACKUP_RETAIN_DAYS:-30}"

    log_info "Cleaning up backups older than ${retain_days} days..."

    find "$BACKUP_DIR" -maxdepth 1 -type d -name "backup_*" -mtime +${retain_days} -exec rm -rf {} \; 2>&1 | tee -a "$LOG_FILE"

    log_info "  ✓ Cleanup complete"
}

# Main backup function
main() {
    log_info "========================================="
    log_info "Aurora Locus PDS Backup"
    log_info "========================================="
    log_info "Timestamp: $(date)"
    log_info "Backup directory: $BACKUP_PATH"
    log_info ""

    # Create backup directory
    mkdir -p "$BACKUP_DIR"
    mkdir -p "$BACKUP_PATH"

    # Check if data directory exists
    if [ ! -d "$DATA_DIR" ]; then
        log_error "Data directory not found: $DATA_DIR"
        log_error "Please ensure PDS_DATA_DIRECTORY is set correctly"
        exit 1
    fi

    log_info "Data directory: $DATA_DIR"
    log_info ""

    # Backup databases
    log_info "Step 1: Backing up databases..."
    backup_sqlite_db "$ACCOUNT_DB" "account"
    backup_sqlite_db "$SEQUENCER_DB" "sequencer"
    backup_sqlite_db "$DID_CACHE_DB" "did_cache"
    log_info ""

    # Backup blob storage
    log_info "Step 2: Backing up blob storage..."
    backup_blobs "$BLOB_DIR"
    log_info ""

    # Backup actor store
    log_info "Step 3: Backing up actor store..."
    backup_actor_store "$ACTOR_STORE_DIR"
    log_info ""

    # Create manifest
    log_info "Step 4: Creating manifest..."
    create_manifest
    log_info ""

    # Cleanup old backups
    if [ -n "${BACKUP_RETAIN_DAYS:-}" ]; then
        log_info "Step 5: Cleaning up old backups..."
        cleanup_old_backups
        log_info ""
    fi

    # Summary
    log_info "========================================="
    log_info "Backup completed successfully!"
    log_info "========================================="
    log_info "Location: $BACKUP_PATH"
    log_info "Total size: $(get_size "$BACKUP_PATH")"
    log_info "Log file: $LOG_FILE"
    log_info ""
    log_info "To restore from this backup, run:"
    log_info "  ./scripts/restore.sh $BACKUP_PATH"
    log_info ""
}

# Run main function
main "$@"
