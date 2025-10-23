#!/bin/bash
# Aurora Locus PDS - Restore Script
# This script restores a PDS backup
# Usage: ./scripts/restore.sh <backup_directory> [--force]

set -e  # Exit on error
set -u  # Exit on undefined variable

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
TIMESTAMP=$(date +"%Y%m%d_%H%M%S")

# Parse arguments
BACKUP_PATH=""
FORCE_RESTORE=false

while [[ $# -gt 0 ]]; do
    case $1 in
        --force)
            FORCE_RESTORE=true
            shift
            ;;
        *)
            if [ -z "$BACKUP_PATH" ]; then
                BACKUP_PATH="$1"
            else
                echo "Error: Unknown argument: $1"
                exit 1
            fi
            shift
            ;;
    esac
done

# Validate backup path
if [ -z "$BACKUP_PATH" ]; then
    echo "Usage: $0 <backup_directory> [--force]"
    echo ""
    echo "Options:"
    echo "  --force    Skip confirmation prompt"
    echo ""
    echo "Available backups:"
    if [ -d "${PROJECT_DIR}/backups" ]; then
        ls -1t "${PROJECT_DIR}/backups" | grep "^backup_" | head -5
    fi
    exit 1
fi

if [ ! -d "$BACKUP_PATH" ]; then
    echo "Error: Backup directory not found: $BACKUP_PATH"
    exit 1
fi

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

# Pre-restore backup directory
PRE_RESTORE_BACKUP="${DATA_DIR}/pre_restore_backup_${TIMESTAMP}"

# Logging
LOG_FILE="${BACKUP_PATH}/restore_${TIMESTAMP}.log"

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

log_step() {
    echo -e "${BLUE}[STEP]${NC} $@"
    log "STEP" "$@"
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

# Function to decompress a file
decompress_file() {
    local compressed_file="$1"
    local output_file="$2"

    if [ -f "${compressed_file}.gz" ]; then
        log_info "  Decompressing gzip file..."
        gunzip -c "${compressed_file}.gz" > "$output_file"
    elif [ -f "${compressed_file}.bz2" ]; then
        log_info "  Decompressing bzip2 file..."
        bunzip2 -c "${compressed_file}.bz2" > "$output_file"
    elif [ -f "${compressed_file}.xz" ]; then
        log_info "  Decompressing xz file..."
        unxz -c "${compressed_file}.xz" > "$output_file"
    elif [ -f "$compressed_file" ]; then
        # File is not compressed
        cp "$compressed_file" "$output_file"
    else
        log_error "  Backup file not found: $compressed_file"
        return 1
    fi

    return 0
}

# Function to restore a SQLite database
restore_sqlite_db() {
    local db_name="$1"
    local target_path="$2"
    local backup_file="${BACKUP_PATH}/databases/${db_name}.sqlite"

    log_info "Restoring database: $db_name"
    log_info "  Target: $target_path"

    # Create directory
    mkdir -p "$(dirname "$target_path")"

    # Decompress and restore
    if ! decompress_file "$backup_file" "$target_path"; then
        log_error "  ✗ Failed to restore database"
        return 1
    fi

    log_info "  ✓ Database restored successfully"
    log_info "  Size: $(get_size "$target_path")"

    return 0
}

# Function to restore blob storage
restore_blobs() {
    local blob_dir="$1"
    local backup_file="${BACKUP_PATH}/blobs.tar"

    log_info "Restoring blob storage..."
    log_info "  Target: $blob_dir"

    # Create parent directory
    mkdir -p "$(dirname "$blob_dir")"

    # Decompress tar if needed
    local tar_file="$backup_file"
    if [ -f "${backup_file}.gz" ]; then
        log_info "  Decompressing and extracting..."
        tar -xzf "${backup_file}.gz" -C "$(dirname "$blob_dir")" 2>&1 | tee -a "$LOG_FILE"
    elif [ -f "${backup_file}.bz2" ]; then
        tar -xjf "${backup_file}.bz2" -C "$(dirname "$blob_dir")" 2>&1 | tee -a "$LOG_FILE"
    elif [ -f "${backup_file}.xz" ]; then
        tar -xJf "${backup_file}.xz" -C "$(dirname "$blob_dir")" 2>&1 | tee -a "$LOG_FILE"
    elif [ -f "$backup_file" ]; then
        log_info "  Extracting..."
        tar -xf "$backup_file" -C "$(dirname "$blob_dir")" 2>&1 | tee -a "$LOG_FILE"
    else
        log_warn "  Blob backup not found (skipping)"
        return 0
    fi

    if [ $? -eq 0 ]; then
        log_info "  ✓ Blobs restored successfully"
        log_info "  Size: $(get_size "$blob_dir")"
    else
        log_error "  ✗ Failed to restore blobs"
        return 1
    fi

    return 0
}

# Function to restore actor store
restore_actor_store() {
    local actor_dir="$1"
    local backup_file="${BACKUP_PATH}/actors.tar"

    log_info "Restoring actor store..."
    log_info "  Target: $actor_dir"

    # Create parent directory
    mkdir -p "$(dirname "$actor_dir")"

    # Decompress tar if needed
    if [ -f "${backup_file}.gz" ]; then
        log_info "  Decompressing and extracting..."
        tar -xzf "${backup_file}.gz" -C "$(dirname "$actor_dir")" 2>&1 | tee -a "$LOG_FILE"
    elif [ -f "${backup_file}.bz2" ]; then
        tar -xjf "${backup_file}.bz2" -C "$(dirname "$actor_dir")" 2>&1 | tee -a "$LOG_FILE"
    elif [ -f "${backup_file}.xz" ]; then
        tar -xJf "${backup_file}.xz" -C "$(dirname "$actor_dir")" 2>&1 | tee -a "$LOG_FILE"
    elif [ -f "$backup_file" ]; then
        log_info "  Extracting..."
        tar -xf "$backup_file" -C "$(dirname "$actor_dir")" 2>&1 | tee -a "$LOG_FILE"
    else
        log_warn "  Actor store backup not found (skipping)"
        return 0
    fi

    if [ $? -eq 0 ]; then
        log_info "  ✓ Actor store restored successfully"
        log_info "  Size: $(get_size "$actor_dir")"
    else
        log_error "  ✗ Failed to restore actor store"
        return 1
    fi

    return 0
}

# Function to read manifest
read_manifest() {
    local manifest_file="${BACKUP_PATH}/manifest.json"

    if [ ! -f "$manifest_file" ]; then
        log_warn "Manifest file not found (backup may be from older version)"
        return 0
    fi

    log_info "Backup manifest:"
    if command_exists jq; then
        jq '.' "$manifest_file" | tee -a "$LOG_FILE"
    else
        cat "$manifest_file" | tee -a "$LOG_FILE"
    fi
    log_info ""
}

# Function to create pre-restore backup
create_pre_restore_backup() {
    log_step "Creating pre-restore backup of current data..."
    log_info "  Location: $PRE_RESTORE_BACKUP"

    mkdir -p "$PRE_RESTORE_BACKUP"

    # Backup existing databases
    for db_path in "$ACCOUNT_DB" "$SEQUENCER_DB" "$DID_CACHE_DB"; do
        if [ -f "$db_path" ]; then
            cp "$db_path" "$PRE_RESTORE_BACKUP/"
            log_info "  ✓ Backed up: $(basename "$db_path")"
        fi
    done

    # Backup existing directories
    if [ -d "$BLOB_DIR" ]; then
        tar -czf "${PRE_RESTORE_BACKUP}/blobs_backup.tar.gz" -C "$(dirname "$BLOB_DIR")" "$(basename "$BLOB_DIR")" 2>&1 | tee -a "$LOG_FILE"
        log_info "  ✓ Backed up: blobs"
    fi

    if [ -d "$ACTOR_STORE_DIR" ]; then
        tar -czf "${PRE_RESTORE_BACKUP}/actors_backup.tar.gz" -C "$(dirname "$ACTOR_STORE_DIR")" "$(basename "$ACTOR_STORE_DIR")" 2>&1 | tee -a "$LOG_FILE"
        log_info "  ✓ Backed up: actors"
    fi

    log_info "  ✓ Pre-restore backup created"
    log_info ""
}

# Function to confirm restore
confirm_restore() {
    if [ "$FORCE_RESTORE" = true ]; then
        return 0
    fi

    echo ""
    echo -e "${YELLOW}=========================================${NC}"
    echo -e "${YELLOW}WARNING: This will replace current data!${NC}"
    echo -e "${YELLOW}=========================================${NC}"
    echo ""
    echo "Backup to restore from:"
    echo "  $BACKUP_PATH"
    echo ""
    echo "Current data will be replaced:"
    echo "  Databases: $DATA_DIR"
    echo "  Blobs: $BLOB_DIR"
    echo "  Actors: $ACTOR_STORE_DIR"
    echo ""
    echo "A pre-restore backup will be created at:"
    echo "  $PRE_RESTORE_BACKUP"
    echo ""
    read -p "Are you sure you want to continue? (yes/no): " -r
    echo ""

    if [[ ! $REPLY =~ ^[Yy][Ee][Ss]$ ]]; then
        echo "Restore cancelled."
        exit 0
    fi
}

# Main restore function
main() {
    log_info "========================================="
    log_info "Aurora Locus PDS Restore"
    log_info "========================================="
    log_info "Timestamp: $(date)"
    log_info "Restore from: $BACKUP_PATH"
    log_info ""

    # Read manifest
    read_manifest

    # Confirm restore
    confirm_restore

    # Create pre-restore backup
    create_pre_restore_backup

    # Stop PDS if running (optional - commented out)
    # log_step "Stopping Aurora Locus PDS..."
    # systemctl stop aurora-locus || true
    # log_info ""

    # Create data directory
    mkdir -p "$DATA_DIR"

    # Restore databases
    log_step "Restoring databases..."
    restore_sqlite_db "account" "$ACCOUNT_DB"
    restore_sqlite_db "sequencer" "$SEQUENCER_DB"
    restore_sqlite_db "did_cache" "$DID_CACHE_DB"
    log_info ""

    # Restore blob storage
    log_step "Restoring blob storage..."
    restore_blobs "$BLOB_DIR"
    log_info ""

    # Restore actor store
    log_step "Restoring actor store..."
    restore_actor_store "$ACTOR_STORE_DIR"
    log_info ""

    # Start PDS if it was stopped (optional - commented out)
    # log_step "Starting Aurora Locus PDS..."
    # systemctl start aurora-locus || true
    # log_info ""

    # Summary
    log_info "========================================="
    log_info "Restore completed successfully!"
    log_info "========================================="
    log_info "Pre-restore backup: $PRE_RESTORE_BACKUP"
    log_info "Log file: $LOG_FILE"
    log_info ""
    log_info "Next steps:"
    log_info "  1. Verify the data integrity"
    log_info "  2. Start the PDS service"
    log_info "  3. Check the health endpoints"
    log_info ""
    log_info "If you need to rollback, run:"
    log_info "  ./scripts/restore.sh $PRE_RESTORE_BACKUP --force"
    log_info ""
}

# Run main function
main "$@"
