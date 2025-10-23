#!/bin/bash
# Aurora Locus - Server Startup Script

set -e

echo "===================================="
echo "   Aurora Locus PDS - Startup"
echo "===================================="
echo ""

# Check if binary exists
if [ ! -f "target/release/aurora-locus.exe" ] && [ ! -f "target/release/aurora-locus" ]; then
    echo "❌ Binary not found. Building..."
    cargo build --release
fi

# Create data directories
echo "📁 Creating data directories..."
mkdir -p data/blobs/tmp data/actors

# Start the server
echo ""
echo "🚀 Starting Aurora Locus PDS..."
echo "   Server will be accessible at:"
echo "   - http://129.222.126.193:3000"
echo "   - http://localhost:3000"
echo ""
echo "   Admin Panel:"
echo "   - http://129.222.126.193:3000/admin/"
echo "   - http://localhost:3000/admin/"
echo ""
echo "   Press Ctrl+C to stop the server"
echo ""

# Run the server
if [ -f "target/release/aurora-locus.exe" ]; then
    ./target/release/aurora-locus.exe
else
    ./target/release/aurora-locus
fi
