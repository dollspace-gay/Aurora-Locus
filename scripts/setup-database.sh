#!/usr/bin/env bash
#
# Aurora Locus — data-directory bootstrap.
#
# Prepares the on-disk data directory for a fresh PDS. It deliberately does NOT
# create database files or apply schema itself: Aurora owns exactly one
# migration authority — `sqlx::migrate!("./migrations")`, embedded in the binary
# and run the first time any process opens the account pool (server boot, or any
# offline subcommand such as `grant-admin` / `validate-config`, via
# `AppContext::new` → `db::run_any_migrations`). Creating tables here in parallel
# (the previous version hand-forged `_sqlx_migrations` rows) risks checksum
# conflicts against that embedded set, so schema creation is left entirely to the
# app.
#
# It also does NOT write a `.env`. Configuration has a single source of truth —
# `.env.example`, composed into `.env` by ../install.sh. This script only touches
# the filesystem layout under the data directory.
#
# Per-component paths (account.sqlite, sequencer.sqlite, did_cache.sqlite,
# actors/, blobs/) auto-derive from PDS_DATA_DIRECTORY and are created by the app
# on first use, so only the top-level directory is required here.

set -euo pipefail

DATA_DIR="./data"
FORCE=false
NON_INTERACTIVE=false

usage() {
    cat <<EOF
Aurora Locus — data-directory bootstrap.

Usage: $0 [--data-dir PATH] [--force] [--non-interactive]

  --data-dir PATH     Data directory to prepare (default: ./data).
  --force             Delete an existing data directory without asking.
  --non-interactive   Never prompt; keep an existing directory as-is (unless
                      --force). Intended for install.sh / CI.
  -h, --help          Show this help.
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --data-dir)        DATA_DIR="${2:?--data-dir needs a value}"; shift 2 ;;
        --force)           FORCE=true; shift ;;
        --non-interactive) NON_INTERACTIVE=true; shift ;;
        -h|--help)         usage; exit 0 ;;
        *) echo "Unknown option: $1" >&2; echo; usage; exit 1 ;;
    esac
done

echo "=================================================================="
echo "  Aurora Locus — data-directory bootstrap"
echo "=================================================================="
echo

if [[ -d "$DATA_DIR" ]]; then
    if [[ "$FORCE" == true ]]; then
        echo "⚠️  --force: removing existing data directory $DATA_DIR"
        rm -rf "$DATA_DIR"
    elif [[ "$NON_INTERACTIVE" == true ]]; then
        echo "ℹ️  Data directory $DATA_DIR already exists — keeping it as-is."
        echo "   (Aurora migrates the existing schema forward on next boot.)"
    else
        echo "⚠️  Data directory already exists: $DATA_DIR"
        echo "   It may contain live data. Deleting is destructive."
        read -r -p "Delete and recreate? [y/N]: " ans
        if [[ "${ans,,}" == "y" || "${ans,,}" == "yes" ]]; then
            rm -rf "$DATA_DIR"
            echo "✓ Existing data removed"
        else
            echo "ℹ️  Keeping existing data directory."
        fi
    fi
fi

mkdir -p "$DATA_DIR"
echo "✓ Data directory ready: $DATA_DIR"
echo
echo "ℹ️  Schema migrations are applied automatically by Aurora the first time it"
echo "   opens the database (server boot, or any offline subcommand). No separate"
echo "   migration step is required."
echo
echo "   account.sqlite / sequencer.sqlite / did_cache.sqlite / actors/ / blobs/"
echo "   are created under this directory on first use."
