#!/bin/bash
# Aurora Locus Database Setup Script
# This script initializes a fresh database for Aurora Locus PDS

set -e

FORCE=false
DATA_DIR="./data"

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --force)
            FORCE=true
            shift
            ;;
        --data-dir)
            DATA_DIR="$2"
            shift 2
            ;;
        *)
            echo "Unknown option: $1"
            echo "Usage: $0 [--force] [--data-dir PATH]"
            exit 1
            ;;
    esac
done

echo "=================================================================="
echo "  Aurora Locus PDS - Database Setup"
echo "=================================================================="
echo ""

# Check if data directory exists
if [ -d "$DATA_DIR" ]; then
    if [ "$FORCE" = true ]; then
        echo "⚠️  Force mode enabled - removing existing data directory..."
        rm -rf "$DATA_DIR"
        echo "✓ Existing data removed"
    else
        echo "⚠️  Data directory already exists: $DATA_DIR"
        echo "   This may contain existing data. Options:"
        echo "   1. Use --force to delete and recreate"
        echo "   2. Manually delete the directory"
        echo "   3. Use a different directory with --data-dir parameter"
        echo ""
        read -p "Delete existing data and continue? (yes/no): " response
        if [ "$response" != "yes" ]; then
            echo "❌ Setup cancelled"
            exit 1
        fi
        rm -rf "$DATA_DIR"
        echo "✓ Existing data removed"
    fi
fi

# Create data directory structure
echo ""
echo "📁 Creating directory structure..."
mkdir -p "$DATA_DIR"
mkdir -p "$DATA_DIR/actor-store"
mkdir -p "$DATA_DIR/blobs"
mkdir -p "$DATA_DIR/blobs/tmp"
echo "✓ Directories created"

# Check if sqlx-cli is installed
echo ""
echo "🔍 Checking for sqlx-cli..."
if ! command -v sqlx &> /dev/null; then
    echo "⚠️  sqlx-cli not found"
    echo "   Installing sqlx-cli (this may take a few minutes)..."
    cargo install sqlx-cli --no-default-features --features sqlite
    echo "✓ sqlx-cli installed"
else
    echo "✓ sqlx-cli found"
fi

# Create database files
echo ""
echo "🗄️  Creating database files..."

ACCOUNT_DB="$DATA_DIR/account.sqlite"
SEQUENCER_DB="$DATA_DIR/sequencer.sqlite"
DID_CACHE_DB="$DATA_DIR/did-cache.sqlite"

# Set DATABASE_URL for migrations
export DATABASE_URL="sqlite:$ACCOUNT_DB"

echo "   Creating account database: $ACCOUNT_DB"

# Run migrations
echo ""
echo "📊 Running database migrations..."
sqlx database create
sqlx migrate run --source migrations
echo "✓ Migrations completed"

# Create .env file if it doesn't exist
echo ""
echo "⚙️  Setting up configuration..."

if [ ! -f ".env" ]; then
    echo "   Creating .env file..."

    # Generate random secrets
    JWT_SECRET=$(openssl rand -hex 32)
    ADMIN_PASSWORD=$(openssl rand -hex 16)
    SIGNING_KEY=$(openssl rand -hex 32)
    PLC_KEY=$(openssl rand -hex 32)

    cat > .env << EOF
# Aurora Locus PDS Configuration

# Service Configuration
PDS_HOSTNAME=localhost
PDS_PORT=2583
PDS_SERVICE_DID=did:web:localhost
PDS_VERSION=0.1.0

# Storage Configuration
PDS_DATA_DIRECTORY=./data
PDS_ACCOUNT_DB_LOCATION=./data/account.sqlite
PDS_SEQUENCER_DB_LOCATION=./data/sequencer.sqlite
PDS_DID_CACHE_DB_LOCATION=./data/did-cache.sqlite
PDS_ACTOR_STORE_DIRECTORY=./data/actor-store
PDS_BLOBSTORE_DISK_LOCATION=./data/blobs
PDS_BLOBSTORE_DISK_TMP_LOCATION=./data/blobs/tmp

# Authentication (CHANGE THESE IN PRODUCTION!)
PDS_JWT_SECRET=$JWT_SECRET
PDS_ADMIN_PASSWORD=$ADMIN_PASSWORD
PDS_REPO_SIGNING_KEY=$SIGNING_KEY
PDS_PLC_ROTATION_KEY=$PLC_KEY

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
EOF

    echo "✓ .env file created with random secrets"
else
    echo "   .env file already exists (keeping existing configuration)"
fi

# Success message
echo ""
echo "=================================================================="
echo "  ✅ Database setup complete!"
echo "=================================================================="
echo ""
echo "📁 Data directory: $DATA_DIR"
echo "🗄️  Account database: $ACCOUNT_DB"
echo "⚙️  Configuration file: .env"
echo ""
echo "🚀 Next steps:"
echo "   1. Review and customize the .env file"
echo "   2. Run the server: cargo run --release"
echo "   3. Create your first account at: http://localhost:2583"
echo ""
echo "⚠️  IMPORTANT: Change the secrets in .env before deploying to production!"
echo ""
