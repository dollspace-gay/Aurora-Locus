# Aurora Locus PDS - Backup Script (PowerShell)
# This script performs a complete backup of the PDS database and blob storage
# Usage: .\scripts\backup.ps1 [-BackupDir <path>]

param(
    [Parameter(Position=0)]
    [string]$BackupDir = ""
)

# Configuration
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectDir = Split-Path -Parent $ScriptDir
$Timestamp = Get-Date -Format "yyyyMMdd_HHmmss"
$DefaultBackupDir = Join-Path $ProjectDir "backups"

# Get backup directory
if ([string]::IsNullOrEmpty($BackupDir)) {
    $BackupDir = $DefaultBackupDir
}
$BackupPath = Join-Path $BackupDir "backup_$Timestamp"

# Load environment variables from .env if exists
$EnvFile = Join-Path $ProjectDir ".env"
if (Test-Path $EnvFile) {
    Get-Content $EnvFile | ForEach-Object {
        if ($_ -match '^\s*([^#][^=]*?)\s*=\s*(.+?)\s*$') {
            $key = $matches[1]
            $value = $matches[2]
            [Environment]::SetEnvironmentVariable($key, $value, "Process")
        }
    }
}

# Default data directory from environment
$DataDir = if ($env:PDS_DATA_DIRECTORY) { $env:PDS_DATA_DIRECTORY } else { Join-Path $ProjectDir "data" }
$AccountDB = if ($env:PDS_ACCOUNT_DB_LOCATION) { $env:PDS_ACCOUNT_DB_LOCATION } else { Join-Path $DataDir "account.sqlite" }
$SequencerDB = if ($env:PDS_SEQUENCER_DB_LOCATION) { $env:PDS_SEQUENCER_DB_LOCATION } else { Join-Path $DataDir "sequencer.sqlite" }
$DidCacheDB = if ($env:PDS_DID_CACHE_DB_LOCATION) { $env:PDS_DID_CACHE_DB_LOCATION } else { Join-Path $DataDir "did_cache.sqlite" }
$ActorStoreDir = if ($env:PDS_ACTOR_STORE_DIRECTORY) { $env:PDS_ACTOR_STORE_DIRECTORY } else { Join-Path $DataDir "actors" }
$BlobDir = if ($env:PDS_BLOBSTORE_DISK_LOCATION) { $env:PDS_BLOBSTORE_DISK_LOCATION } else { Join-Path $DataDir "blobs" }

# Compression settings
$UseCompression = $true

# Logging
$LogFile = Join-Path $BackupDir "backup_$Timestamp.log"

# Function to log messages
function Write-Log {
    param(
        [string]$Level,
        [string]$Message
    )
    $TimeStamp = Get-Date -Format "yyyy-MM-dd HH:mm:ss"
    $LogMessage = "[$TimeStamp] [$Level] $Message"
    Write-Output $LogMessage
    Add-Content -Path $LogFile -Value $LogMessage
}

function Write-Info {
    param([string]$Message)
    Write-Host "[INFO] $Message" -ForegroundColor Green
    Write-Log "INFO" $Message
}

function Write-Warn {
    param([string]$Message)
    Write-Host "[WARN] $Message" -ForegroundColor Yellow
    Write-Log "WARN" $Message
}

function Write-Error2 {
    param([string]$Message)
    Write-Host "[ERROR] $Message" -ForegroundColor Red
    Write-Log "ERROR" $Message
}

# Function to get directory/file size
function Get-ItemSize {
    param([string]$Path)
    if (Test-Path $Path) {
        $size = (Get-ChildItem -Path $Path -Recurse -ErrorAction SilentlyContinue | Measure-Object -Property Length -Sum).Sum
        if ($size -gt 1GB) {
            return "{0:N2} GB" -f ($size / 1GB)
        } elseif ($size -gt 1MB) {
            return "{0:N2} MB" -f ($size / 1MB)
        } elseif ($size -gt 1KB) {
            return "{0:N2} KB" -f ($size / 1KB)
        } else {
            return "$size bytes"
        }
    }
    return "N/A"
}

# Function to backup SQLite database
function Backup-SqliteDatabase {
    param(
        [string]$DbPath,
        [string]$DbName
    )

    if (-not (Test-Path $DbPath)) {
        Write-Warn "Database not found: $DbPath (skipping)"
        return $true
    }

    Write-Info "Backing up database: $DbName"
    Write-Info "  Source: $DbPath"
    Write-Info "  Size: $(Get-ItemSize $DbPath)"

    $BackupFile = Join-Path $BackupPath "databases\$DbName.sqlite"
    $BackupDbDir = Split-Path -Parent $BackupFile
    New-Item -ItemType Directory -Force -Path $BackupDbDir | Out-Null

    try {
        # Simple file copy for Windows
        Copy-Item -Path $DbPath -Destination $BackupFile -Force

        Write-Info "  ✓ Database backed up successfully"

        # Compress if enabled
        if ($UseCompression) {
            Compress-Archive -Path $BackupFile -DestinationPath "$BackupFile.zip" -Force
            Remove-Item $BackupFile
            Write-Info "  ✓ Compressed: $(Get-ItemSize "$BackupFile.zip")"
        }

        return $true
    } catch {
        Write-Error2 "  ✗ Failed to backup database: $_"
        return $false
    }
}

# Function to backup directory
function Backup-Directory {
    param(
        [string]$SourceDir,
        [string]$ArchiveName
    )

    if (-not (Test-Path $SourceDir)) {
        Write-Warn "Directory not found: $SourceDir (skipping)"
        return $true
    }

    Write-Info "Backing up directory: $ArchiveName"
    Write-Info "  Source: $SourceDir"
    Write-Info "  Size: $(Get-ItemSize $SourceDir)"

    $ArchiveFile = Join-Path $BackupPath "$ArchiveName.zip"

    try {
        Compress-Archive -Path $SourceDir -DestinationPath $ArchiveFile -Force
        Write-Info "  ✓ Directory archived successfully"
        Write-Info "  Archive size: $(Get-ItemSize $ArchiveFile)"
        return $true
    } catch {
        Write-Error2 "  ✗ Failed to archive directory: $_"
        return $false
    }
}

# Function to create manifest
function New-BackupManifest {
    Write-Info "Creating backup manifest..."

    $Manifest = @{
        backup_timestamp = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
        backup_version = "1.0"
        pds_version = if ($env:PDS_VERSION) { $env:PDS_VERSION } else { "unknown" }
        hostname = if ($env:PDS_HOSTNAME) { $env:PDS_HOSTNAME } else { $env:COMPUTERNAME }
        service_did = if ($env:PDS_SERVICE_DID) { $env:PDS_SERVICE_DID } else { "unknown" }
        compression = if ($UseCompression) { "zip" } else { "none" }
        databases = @{
            account = "account.sqlite"
            sequencer = "sequencer.sqlite"
            did_cache = "did_cache.sqlite"
        }
        data_directories = @{
            actor_store = "actors"
            blob_store = "blobs"
        }
        backup_size = Get-ItemSize $BackupPath
    }

    $ManifestFile = Join-Path $BackupPath "manifest.json"
    $Manifest | ConvertTo-Json -Depth 10 | Set-Content -Path $ManifestFile

    Write-Info "  ✓ Manifest created"
}

# Function to cleanup old backups
function Remove-OldBackups {
    $RetainDays = if ($env:BACKUP_RETAIN_DAYS) { [int]$env:BACKUP_RETAIN_DAYS } else { 30 }

    Write-Info "Cleaning up backups older than $RetainDays days..."

    $CutoffDate = (Get-Date).AddDays(-$RetainDays)
    Get-ChildItem -Path $BackupDir -Directory -Filter "backup_*" | Where-Object {
        $_.LastWriteTime -lt $CutoffDate
    } | ForEach-Object {
        Write-Info "  Removing old backup: $($_.Name)"
        Remove-Item -Path $_.FullName -Recurse -Force
    }

    Write-Info "  ✓ Cleanup complete"
}

# Main function
function Main {
    Write-Info "========================================="
    Write-Info "Aurora Locus PDS Backup"
    Write-Info "========================================="
    Write-Info "Timestamp: $(Get-Date)"
    Write-Info "Backup directory: $BackupPath"
    Write-Info ""

    # Create backup directory
    New-Item -ItemType Directory -Force -Path $BackupDir | Out-Null
    New-Item -ItemType Directory -Force -Path $BackupPath | Out-Null

    # Check if data directory exists
    if (-not (Test-Path $DataDir)) {
        Write-Error2 "Data directory not found: $DataDir"
        Write-Error2 "Please ensure PDS_DATA_DIRECTORY is set correctly"
        exit 1
    }

    Write-Info "Data directory: $DataDir"
    Write-Info ""

    # Backup databases
    Write-Info "Step 1: Backing up databases..."
    Backup-SqliteDatabase $AccountDB "account"
    Backup-SqliteDatabase $SequencerDB "sequencer"
    Backup-SqliteDatabase $DidCacheDB "did_cache"
    Write-Info ""

    # Backup blob storage
    Write-Info "Step 2: Backing up blob storage..."
    Backup-Directory $BlobDir "blobs"
    Write-Info ""

    # Backup actor store
    Write-Info "Step 3: Backing up actor store..."
    Backup-Directory $ActorStoreDir "actors"
    Write-Info ""

    # Create manifest
    Write-Info "Step 4: Creating manifest..."
    New-BackupManifest
    Write-Info ""

    # Cleanup old backups
    if ($env:BACKUP_RETAIN_DAYS) {
        Write-Info "Step 5: Cleaning up old backups..."
        Remove-OldBackups
        Write-Info ""
    }

    # Summary
    Write-Info "========================================="
    Write-Info "Backup completed successfully!"
    Write-Info "========================================="
    Write-Info "Location: $BackupPath"
    Write-Info "Total size: $(Get-ItemSize $BackupPath)"
    Write-Info "Log file: $LogFile"
    Write-Info ""
    Write-Info "To restore from this backup, run:"
    Write-Info "  .\scripts\restore.ps1 -BackupPath `"$BackupPath`""
    Write-Info ""
}

# Run main function
Main
