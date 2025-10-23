# Aurora Locus Database Setup Script
# Simple installer for Windows

param([switch]$Force)

$ErrorActionPreference = "Stop"

Write-Host "Aurora Locus PDS - Database Setup" -ForegroundColor Cyan
Write-Host ""

# Remove existing data if requested
if (Test-Path "data") {
    if ($Force) {
        Write-Host "Removing existing data..." -ForegroundColor Yellow
        Remove-Item -Path "data" -Recurse -Force
    } else {
        $response = Read-Host "Data directory exists. Delete and recreate? (yes/no)"
        if ($response -eq "yes") {
            Remove-Item -Path "data" -Recurse -Force
        } else {
            Write-Host "Setup cancelled" -ForegroundColor Red
            exit 1
        }
    }
}

# Create directories
Write-Host "Creating directories..." -ForegroundColor Cyan
New-Item -ItemType Directory -Path "data" -Force | Out-Null
New-Item -ItemType Directory -Path "data/actor-store" -Force | Out-Null
New-Item -ItemType Directory -Path "data/blobs" -Force | Out-Null
New-Item -ItemType Directory -Path "data/blobs/tmp" -Force | Out-Null

# Run migrations
Write-Host "Running database migrations..." -ForegroundColor Cyan
$env:DATABASE_URL = "sqlite:data/account.sqlite"
sqlx database create
sqlx migrate run --source migrations
Write-Host "Migrations complete!" -ForegroundColor Green

Write-Host ""
Write-Host "Setup complete!" -ForegroundColor Green
Write-Host "Run with: cargo run --release" -ForegroundColor Cyan
Write-Host ""
