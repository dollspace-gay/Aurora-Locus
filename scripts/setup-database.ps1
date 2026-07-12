#!/usr/bin/env pwsh
#
# Aurora Locus — data-directory bootstrap (Windows / PowerShell).
#
# PowerShell parity of scripts/setup-database.sh. It prepares the on-disk data
# directory for a fresh PDS and nothing else: it does NOT create database files
# or apply schema, and it does NOT write a .env.
#
# Aurora owns exactly one migration authority — the embedded
# `sqlx::migrate!("./migrations")`, run the first time any process opens the
# account pool (server boot, or any offline subcommand such as `grant-admin` /
# `validate-config`, via `AppContext::new` -> `db::run_any_migrations`). Creating
# tables here in parallel (the previous version hand-forged `_sqlx_migrations`
# rows) risks checksum conflicts against that embedded set, so schema creation is
# left entirely to the app.
#
# Configuration has a single source of truth — `.env.example`, composed into
# `.env` by ..\install.sh. There is currently no install.ps1: on Windows run
# install.sh via Git Bash or WSL to generate the .env, then run this script (or
# let install.sh invoke it). This script only touches the filesystem layout under
# the data directory.
#
# Per-component paths (account.sqlite, sequencer.sqlite, did_cache.sqlite,
# actors\, blobs\) auto-derive from PDS_DATA_DIRECTORY and are created by the app
# on first use, so only the top-level directory is required here.

param(
    [switch]$Force,
    [switch]$NonInteractive,
    [string]$DataDir = ".\data"
)

$ErrorActionPreference = "Stop"

Write-Host "=================================================================="
Write-Host "  Aurora Locus - data-directory bootstrap"
Write-Host "=================================================================="
Write-Host ""

if (Test-Path $DataDir) {
    if ($Force) {
        Write-Host "WARNING: -Force: removing existing data directory $DataDir" -ForegroundColor Yellow
        Remove-Item -Path $DataDir -Recurse -Force
    } elseif ($NonInteractive) {
        Write-Host "Data directory $DataDir already exists - keeping it as-is." -ForegroundColor Cyan
        Write-Host "   (Aurora migrates the existing schema forward on next boot.)"
    } else {
        Write-Host "WARNING: Data directory already exists: $DataDir" -ForegroundColor Yellow
        Write-Host "   It may contain live data. Deleting is destructive."
        $response = Read-Host "Delete and recreate? (y/N)"
        if ($response -match '^(y|yes)$') {
            Remove-Item -Path $DataDir -Recurse -Force
            Write-Host "Existing data removed" -ForegroundColor Green
        } else {
            Write-Host "Keeping existing data directory." -ForegroundColor Cyan
        }
    }
}

New-Item -ItemType Directory -Path $DataDir -Force | Out-Null
Write-Host "Data directory ready: $DataDir" -ForegroundColor Green
Write-Host ""
Write-Host "Schema migrations are applied automatically by Aurora the first time it"
Write-Host "opens the database (server boot, or any offline subcommand). No separate"
Write-Host "migration step is required."
Write-Host ""
Write-Host "   account.sqlite / sequencer.sqlite / did_cache.sqlite / actors\ / blobs\"
Write-Host "   are created under this directory on first use."
