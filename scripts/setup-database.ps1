#!/usr/bin/env pwsh
# Aurora Locus Database Setup Script
# This script initializes a fresh database for Aurora Locus PDS

param(
    [switch]$Force,
    [string]$DataDir = ".\data"
)

$ErrorActionPreference = "Stop"

Write-Host "==================================================================" -ForegroundColor Cyan
Write-Host "  Aurora Locus PDS - Database Setup" -ForegroundColor Cyan
Write-Host "==================================================================" -ForegroundColor Cyan
Write-Host ""

# Check if data directory exists
if (Test-Path $DataDir) {
    if ($Force) {
        Write-Host "⚠️  Force mode enabled - removing existing data directory..." -ForegroundColor Yellow
        Remove-Item -Path $DataDir -Recurse -Force
        Write-Host "✓ Existing data removed" -ForegroundColor Green
    } else {
        Write-Host "⚠️  Data directory already exists: $DataDir" -ForegroundColor Yellow
        Write-Host "   This may contain existing data. Options:" -ForegroundColor Yellow
        Write-Host "   1. Use -Force to delete and recreate" -ForegroundColor Yellow
        Write-Host "   2. Manually delete the directory" -ForegroundColor Yellow
        Write-Host "   3. Use a different directory with -DataDir parameter" -ForegroundColor Yellow
        Write-Host ""
        $response = Read-Host "Delete existing data and continue? (yes/no)"
        if ($response -ne "yes") {
            Write-Host "❌ Setup cancelled" -ForegroundColor Red
            exit 1
        }
        Remove-Item -Path $DataDir -Recurse -Force
        Write-Host "✓ Existing data removed" -ForegroundColor Green
    }
}

# Create data directory structure
Write-Host ""
Write-Host "📁 Creating directory structure..." -ForegroundColor Cyan
New-Item -ItemType Directory -Path $DataDir -Force | Out-Null
New-Item -ItemType Directory -Path "$DataDir\actor-store" -Force | Out-Null
New-Item -ItemType Directory -Path "$DataDir\blobs" -Force | Out-Null
New-Item -ItemType Directory -Path "$DataDir\blobs\tmp" -Force | Out-Null
Write-Host "✓ Directories created" -ForegroundColor Green

# Check if sqlx-cli is installed
Write-Host ""
Write-Host "🔍 Checking for sqlx-cli..." -ForegroundColor Cyan
$sqlxInstalled = $null -ne (Get-Command sqlx -ErrorAction SilentlyContinue)

if (-not $sqlxInstalled) {
    Write-Host "⚠️  sqlx-cli not found" -ForegroundColor Yellow
    Write-Host "   Installing sqlx-cli (this may take a few minutes)..." -ForegroundColor Yellow
    cargo install sqlx-cli --no-default-features --features sqlite
    if ($LASTEXITCODE -ne 0) {
        Write-Host "❌ Failed to install sqlx-cli" -ForegroundColor Red
        exit 1
    }
    Write-Host "✓ sqlx-cli installed" -ForegroundColor Green
} else {
    Write-Host "✓ sqlx-cli found" -ForegroundColor Green
}

# Create database files
Write-Host ""
Write-Host "🗄️  Creating database files..." -ForegroundColor Cyan

# Create account database
$accountDb = "$DataDir\account.sqlite"
$sequencerDb = "$DataDir\sequencer.sqlite"
$didCacheDb = "$DataDir\did-cache.sqlite"

# Set DATABASE_URL for migrations
$env:DATABASE_URL = "sqlite:$accountDb"

Write-Host "   Creating account database: $accountDb" -ForegroundColor Gray

# Run migrations
Write-Host ""
Write-Host "📊 Running database migrations..." -ForegroundColor Cyan
sqlx database create
if ($LASTEXITCODE -ne 0) {
    Write-Host "❌ Failed to create database" -ForegroundColor Red
    exit 1
}

sqlx migrate run --source migrations
if ($LASTEXITCODE -ne 0) {
    Write-Host "❌ Failed to run migrations" -ForegroundColor Red
    exit 1
}
Write-Host "✓ Migrations completed" -ForegroundColor Green

# Create .env file if it doesn't exist
Write-Host ""
Write-Host "⚙️  Setting up configuration..." -ForegroundColor Cyan

if (-not (Test-Path ".env")) {
    Write-Host "   Creating .env file..." -ForegroundColor Gray

    # Generate random secrets
    $jwtSecret = -join ((48..57) + (65..90) + (97..122) | Get-Random -Count 64 | ForEach-Object {[char]$_})
    $adminPassword = -join ((48..57) + (65..90) + (97..122) | Get-Random -Count 32 | ForEach-Object {[char]$_})
    $signingKey = -join ((48..57) + (97..102) | Get-Random -Count 64 | ForEach-Object {[char]$_})
    $plcKey = -join ((48..57) + (97..102) | Get-Random -Count 64 | ForEach-Object {[char]$_})

    $envContent = @"
# Aurora Locus PDS Configuration

# Service Configuration
PDS_HOSTNAME=localhost
PDS_PORT=2583
PDS_SERVICE_DID=did:web:localhost
PDS_VERSION=0.1.0

# Storage Configuration
PDS_DATA_DIRECTORY=.\data
PDS_ACCOUNT_DB_LOCATION=.\data\account.sqlite
PDS_SEQUENCER_DB_LOCATION=.\data\sequencer.sqlite
PDS_DID_CACHE_DB_LOCATION=.\data\did-cache.sqlite
PDS_ACTOR_STORE_DIRECTORY=.\data\actor-store
PDS_BLOBSTORE_DISK_LOCATION=.\data\blobs
PDS_BLOBSTORE_DISK_TMP_LOCATION=.\data\blobs\tmp

# Authentication (CHANGE THESE IN PRODUCTION!)
PDS_JWT_SECRET=$jwtSecret
PDS_ADMIN_PASSWORD=$adminPassword
PDS_REPO_SIGNING_KEY=$signingKey
PDS_PLC_ROTATION_KEY=$plcKey

# Identity Configuration
PDS_DID_PLC_URL=https://plc.directory
PDS_SERVICE_HANDLE_DOMAINS=localhost

# Email Configuration (optional)
# PDS_EMAIL_SMTP_URL=smtp://localhost:1025
# PDS_EMAIL_FROM_ADDRESS=noreply@localhost

# Invite Configuration
PDS_INVITES_REQUIRED=false
PDS_INVITES_INTERVAL=604800
PDS_INVITES_EPOCH=2024-01-01T00:00:00Z

# Rate Limiting
PDS_RATE_LIMIT_ENABLED=true
PDS_RATE_LIMIT_GLOBAL_RPM=3000

# Logging
PDS_LOG_LEVEL=info

# Federation (optional - for Bluesky network integration)
PDS_FEDERATION_ENABLED=false
# PDS_FEDERATION_RELAY_URLS=https://bsky.network
PDS_FEDERATION_FIREHOSE_ENABLED=true
PDS_FEDERATION_CRAWL_ENABLED=true
# PDS_FEDERATION_PUBLIC_URL=https://your-pds.example.com
PDS_FEDERATION_AUTO_STREAM_EVENTS=false
"@

    Set-Content -Path ".env" -Value $envContent
    Write-Host "✓ .env file created with random secrets" -ForegroundColor Green
} else {
    Write-Host "   .env file already exists (keeping existing configuration)" -ForegroundColor Gray
}

# Success message
Write-Host ""
Write-Host "==================================================================" -ForegroundColor Green
Write-Host "  ✅ Database setup complete!" -ForegroundColor Green
Write-Host "==================================================================" -ForegroundColor Green
Write-Host ""
Write-Host "📁 Data directory: $DataDir" -ForegroundColor Cyan
Write-Host "🗄️  Account database: $accountDb" -ForegroundColor Cyan
Write-Host "⚙️  Configuration file: .env" -ForegroundColor Cyan
Write-Host ""
Write-Host "🚀 Next steps:" -ForegroundColor Yellow
Write-Host "   1. Review and customize the .env file" -ForegroundColor White
Write-Host "   2. Run the server: cargo run --release" -ForegroundColor White
Write-Host "   3. Create your first account at: http://localhost:2583" -ForegroundColor White
Write-Host ""
Write-Host "WARNING: Change the secrets in .env before deploying to production!" -ForegroundColor Yellow
Write-Host ""
