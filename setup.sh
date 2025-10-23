#!/bin/bash
# Aurora Locus Database Setup Script
set -e

echo "=================================================================="
echo "  Aurora Locus PDS - Database Setup"
echo "=================================================================="
echo ""

# Remove existing data if requested
if [ -d "data" ]; then
    read -p "Data directory exists. Delete and recreate? (yes/no): " response
    if [ "$response" = "yes" ]; then
        echo "Removing existing data..."
        rm -rf data
    else
        echo "Setup cancelled"
        exit 1
    fi
fi

# Create directories
echo "Creating directories..."
mkdir -p data/actor-store
mkdir -p data/blobs/tmp

# Check for sqlx-cli
if ! command -v sqlx &> /dev/null; then
    echo "Installing sqlx-cli..."
    cargo install sqlx-cli --no-default-features --features sqlite
fi

# Run migrations
echo "Running database migrations..."
export DATABASE_URL="sqlite:data/account.sqlite"
sqlx database create
sqlx migrate run --source migrations

echo ""
echo "=================================================================="
echo "  Setup complete!"
echo "=================================================================="
echo ""
echo "Run the server with: cargo run --release"
echo "Create your first account at: http://localhost:2583"
echo ""
