#!/bin/bash

# Aurora Locus PDS Installation Script
# Interactive setup for a production-ready ATProto Personal Data Server
#
# This script will:
# - Collect configuration information
# - Generate cryptographic keys
# - Create OAuth keyset
# - Configure environment variables
# - Set up systemd service (optional)
# - Configure nginx reverse proxy (optional)

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Print functions
print_header() {
    echo -e "${BLUE}"
    echo "═══════════════════════════════════════════════════════════"
    echo "  $1"
    echo "═══════════════════════════════════════════════════════════"
    echo -e "${NC}"
}

print_success() {
    echo -e "${GREEN}✓ $1${NC}"
}

print_error() {
    echo -e "${RED}✗ $1${NC}"
}

print_warning() {
    echo -e "${YELLOW}⚠ $1${NC}"
}

print_info() {
    echo -e "${BLUE}ℹ $1${NC}"
}

# Check if running as root
check_root() {
    if [[ $EUID -eq 0 ]]; then
        print_error "This script should NOT be run as root"
        print_info "Run as a regular user. It will prompt for sudo when needed."
        exit 1
    fi
}

# Check dependencies
check_dependencies() {
    print_header "Checking Dependencies"

    local missing_deps=()

    for cmd in openssl jq xxd cargo sqlite3 curl; do
        if ! command -v $cmd &> /dev/null; then
            missing_deps+=("$cmd")
            print_error "Missing: $cmd"
        else
            print_success "Found: $cmd"
        fi
    done

    if [ ${#missing_deps[@]} -gt 0 ]; then
        echo ""
        print_error "Missing required dependencies: ${missing_deps[*]}"
        echo ""
        print_info "Install them with:"
        echo "  Ubuntu/Debian: sudo apt-get install openssl jq xxd build-essential sqlite3 curl"
        echo "  Fedora/RHEL:   sudo dnf install openssl jq vim-common gcc sqlite curl"
        echo "  macOS:         brew install openssl jq xxd sqlite curl"
        echo ""
        print_info "Install Rust from: https://rustup.rs/"
        exit 1
    fi

    echo ""
    print_success "All dependencies found!"
    echo ""
}

# Prompt for user input with default value
prompt() {
    local var_name=$1
    local prompt_text=$2
    local default_value=$3
    local secret=$4

    if [ -n "$default_value" ]; then
        prompt_text="$prompt_text [$default_value]"
    fi

    if [ "$secret" = "secret" ]; then
        read -s -p "$prompt_text: " value
        echo ""
    else
        read -p "$prompt_text: " value
    fi

    if [ -z "$value" ] && [ -n "$default_value" ]; then
        value=$default_value
    fi

    eval $var_name="'$value'"
}

# Generate random string
generate_random() {
    local length=$1
    openssl rand -base64 $length | tr -d "=+/\n" | tr -d '\n' | cut -c1-$length
}

# Validate domain name
validate_domain() {
    local domain=$1
    if [[ $domain =~ ^[a-zA-Z0-9]([a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(\.[a-zA-Z0-9]([a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*$ ]]; then
        return 0
    else
        return 1
    fi
}

# Validate email
validate_email() {
    local email=$1
    if [[ $email =~ ^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$ ]]; then
        return 0
    else
        return 1
    fi
}

# Generate repository signing key (secp256k1)
generate_repo_key() {
    print_info "Generating repository signing key (secp256k1)..."

    openssl ecparam -name secp256k1 -genkey -noout -out repo_key.pem
    openssl ec -in repo_key.pem -outform DER 2>/dev/null | xxd -p -c 256 > repo_key.hex

    REPO_KEY=$(cat repo_key.hex)
    rm repo_key.pem repo_key.hex

    print_success "Repository signing key generated"
}

# Generate PLC rotation key (secp256k1)
generate_plc_key() {
    print_info "Generating PLC rotation key (secp256k1)..."

    openssl ecparam -name secp256k1 -genkey -noout -out plc_key.pem
    openssl ec -in plc_key.pem -outform DER 2>/dev/null | xxd -p -c 256 > plc_key.hex

    PLC_KEY=$(cat plc_key.hex)
    rm plc_key.pem plc_key.hex

    print_success "PLC rotation key generated"
}

# Generate OAuth keyset (P-256 for ES256)
generate_oauth_keyset() {
    print_info "Generating OAuth keyset (P-256/ES256)..."

    # Generate P-256 key pair
    openssl ecparam -name prime256v1 -genkey -noout -out private-legacy.pem
    openssl pkcs8 -topk8 -nocrypt -in private-legacy.pem -out private-pkcs8.pem
    openssl ec -in private-legacy.pem -pubout -out public.pem 2>/dev/null

    # Read PEM files
    PRIVATE_KEY_PEM=$(cat private-pkcs8.pem)
    PUBLIC_KEY_PEM=$(cat public.pem)

    # Extract key components
    KEY_COMPONENTS_HEX=$(openssl ec -in private-legacy.pem -text -noout 2>/dev/null)

    PRIV_HEX=$(echo "$KEY_COMPONENTS_HEX" | grep priv -A 3 | tail -n +2 | tr -d ' \n:')
    PUB_HEX=$(echo "$KEY_COMPONENTS_HEX" | grep pub -A 5 | tail -n +2 | tr -d ' \n:')
    X_HEX=$(echo "$PUB_HEX" | cut -c 3-66)
    Y_HEX=$(echo "$PUB_HEX" | cut -c 67-130)

    # Convert to base64url
    D_B64URL=$(echo -n "$PRIV_HEX" | xxd -r -p | base64 | tr '/+' '_-' | tr -d '=')
    X_B64URL=$(echo -n "$X_HEX" | xxd -r -p | base64 | tr '/+' '_-' | tr -d '=')
    Y_B64URL=$(echo -n "$Y_HEX" | xxd -r -p | base64 | tr '/+' '_-' | tr -d '=')

    # Generate Key ID
    KID="$(date +%s)-$(openssl rand -hex 4)"

    # Create oauth-keyset.json
    jq -n \
      --arg kid "$KID" \
      --arg pkpem "$PRIVATE_KEY_PEM" \
      --arg pubpem "$PUBLIC_KEY_PEM" \
      --arg d "$D_B64URL" \
      --arg x "$X_B64URL" \
      --arg y "$Y_B64URL" \
      '{
        kid: $kid,
        privateKeyPem: $pkpem,
        publicKeyPem: $pubpem,
        jwk: {
          kid: $kid,
          kty: "EC",
          crv: "P-256",
          alg: "ES256",
          use: "sig",
          d: $d,
          x: $x,
          y: $y
        }
      }' > oauth-keyset.json

    # Cleanup
    rm private-legacy.pem private-pkcs8.pem public.pem

    print_success "OAuth keyset generated: oauth-keyset.json"
}

# Create .env file
create_env_file() {
    print_info "Creating .env configuration file..."

    cat > .env << EOF
# Aurora Locus PDS Configuration
# Generated on $(date)

# ============================================================================
# Server Configuration
# ============================================================================
PDS_HOSTNAME=$HOSTNAME
PDS_PORT=$PORT
PDS_SERVICE_DID=did:web:$HOSTNAME

# ============================================================================
# Security
# ============================================================================
PDS_JWT_SECRET=$JWT_SECRET

# ============================================================================
# Cryptographic Keys
# ============================================================================
# Repository signing key (secp256k1) - DO NOT SHARE
PDS_REPO_SIGNING_KEY_K256_PRIVATE_KEY_HEX=$REPO_KEY

# PLC rotation key (secp256k1) - DO NOT SHARE
PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX=$PLC_KEY

# ============================================================================
# OAuth Configuration
# ============================================================================
# OAuth keyset for admin authentication (P-256/ES256)
OAUTH_KEYSET_FILE=./oauth-keyset.json
OAUTH_CLIENT_ID=http://$HOSTNAME/oauth/client

# Admin DIDs allowed to use OAuth admin authentication
# Add your DID here after creating an account to get admin access
# Multiple DIDs can be comma-separated: did:plc:abc123,did:plc:def456
PDS_ADMIN_DIDS=$ADMIN_DID

# ============================================================================
# Storage
# ============================================================================
PDS_DATA_DIRECTORY=./data
PDS_ACTOR_STORE_DIRECTORY=./data/actors

# Blob storage configuration
# Options: disk or s3
PDS_BLOBSTORE_PROVIDER=disk
PDS_BLOBSTORE_DISK_LOCATION=./data/blobs
PDS_BLOBSTORE_DISK_TMP_LOCATION=./data/tmp

# S3 Configuration (uncomment and configure if using S3)
# PDS_BLOBSTORE_PROVIDER=s3
# PDS_BLOBSTORE_S3_BUCKET=my-pds-blobs
# PDS_BLOBSTORE_S3_REGION=us-east-1
# PDS_BLOBSTORE_S3_ACCESS_KEY_ID=
# PDS_BLOBSTORE_S3_SECRET_ACCESS_KEY=
# PDS_BLOBSTORE_S3_ENDPOINT=  # Optional: for S3-compatible services

# ============================================================================
# Database
# ============================================================================
PDS_ACCOUNT_DB_LOCATION=./data/account.sqlite

# ============================================================================
# Email Configuration (Optional)
# ============================================================================
EMAIL_SMTP_URL=
EMAIL_FROM_ADDRESS=noreply@$HOSTNAME

# ============================================================================
# Identity & Federation
# ============================================================================
# DID PLC Directory URL
DID_PLC_URL=https://plc.directory

# Federation settings
PDS_FEDERATION_ENABLED=$FEDERATION_ENABLED
PDS_FEDERATION_RELAY_URLS=$RELAY_URL
PDS_FEDERATION_FIREHOSE_ENABLED=true
PDS_FEDERATION_CRAWL_ENABLED=true
PDS_FEDERATION_AUTO_STREAM=true
PDS_PUBLIC_URL=$PDS_PUBLIC_URL

# ============================================================================
# Rate Limiting
# ============================================================================
RATE_LIMIT_ENABLED=true
RATE_LIMIT_GLOBAL_HOURLY=3000
RATE_LIMIT_GLOBAL_DAILY=10000
RATE_LIMIT_CREATE_SESSION_HOURLY=30
RATE_LIMIT_CREATE_SESSION_DAILY=300

# ============================================================================
# Invite Codes
# ============================================================================
INVITE_REQUIRED=$INVITE_REQUIRED
INVITE_INTERVAL=604800  # 1 week in seconds

# ============================================================================
# Logging
# ============================================================================
RUST_LOG=info,aurora_locus=debug

EOF

    print_success ".env file created"
}

# Create systemd service
create_systemd_service() {
    print_info "Creating systemd service file..."

    local service_file="/tmp/aurora-locus.service"

    cat > $service_file << EOF
[Unit]
Description=Aurora Locus ATProto PDS
After=network.target

[Service]
Type=simple
User=$USER
WorkingDirectory=$INSTALL_DIR
ExecStart=$INSTALL_DIR/target/release/aurora-locus
Restart=always
RestartSec=10

# Security hardening
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=$INSTALL_DIR/data

# Environment
Environment=RUST_LOG=info,aurora_locus=debug

[Install]
WantedBy=multi-user.target
EOF

    print_success "Systemd service file created: $service_file"
    echo ""
    print_info "To install the service, run:"
    echo "  sudo cp $service_file /etc/systemd/system/"
    echo "  sudo systemctl daemon-reload"
    echo "  sudo systemctl enable aurora-locus"
    echo "  sudo systemctl start aurora-locus"
    echo ""
}

# Create nginx configuration
create_nginx_config() {
    print_info "Creating nginx reverse proxy configuration..."

    local nginx_file="/tmp/aurora-locus-nginx.conf"

    cat > $nginx_file << EOF
# Aurora Locus PDS - Nginx Configuration
# Place this file in /etc/nginx/sites-available/aurora-locus
# Then: sudo ln -s /etc/nginx/sites-available/aurora-locus /etc/nginx/sites-enabled/

server {
    listen 80;
    server_name $HOSTNAME;

    # Redirect HTTP to HTTPS
    return 301 https://\$host\$request_uri;
}

server {
    listen 443 ssl http2;
    server_name $HOSTNAME;

    # SSL Configuration (update paths to your certificates)
    ssl_certificate /etc/letsencrypt/live/$HOSTNAME/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/$HOSTNAME/privkey.pem;

    # SSL Security Settings
    ssl_protocols TLSv1.2 TLSv1.3;
    ssl_ciphers HIGH:!aNULL:!MD5;
    ssl_prefer_server_ciphers on;
    ssl_session_cache shared:SSL:10m;
    ssl_session_timeout 10m;

    # Proxy settings
    location / {
        proxy_pass http://127.0.0.1:$PORT;
        proxy_http_version 1.1;

        # WebSocket support (for firehose)
        proxy_set_header Upgrade \$http_upgrade;
        proxy_set_header Connection "upgrade";

        # Headers
        proxy_set_header Host \$host;
        proxy_set_header X-Real-IP \$remote_addr;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;

        # Timeouts
        proxy_connect_timeout 60s;
        proxy_send_timeout 60s;
        proxy_read_timeout 60s;
    }

    # Security headers
    add_header X-Frame-Options "SAMEORIGIN" always;
    add_header X-Content-Type-Options "nosniff" always;
    add_header X-XSS-Protection "1; mode=block" always;
    add_header Referrer-Policy "strict-origin-when-cross-origin" always;

    # Logging
    access_log /var/log/nginx/aurora-locus-access.log;
    error_log /var/log/nginx/aurora-locus-error.log;
}
EOF

    print_success "Nginx configuration created: $nginx_file"
    echo ""
    print_info "To install the nginx config:"
    echo "  1. Get SSL certificates: sudo certbot --nginx -d $HOSTNAME"
    echo "  2. Copy config: sudo cp $nginx_file /etc/nginx/sites-available/aurora-locus"
    echo "  3. Enable site: sudo ln -s /etc/nginx/sites-available/aurora-locus /etc/nginx/sites-enabled/"
    echo "  4. Test config: sudo nginx -t"
    echo "  5. Reload nginx: sudo systemctl reload nginx"
    echo ""
}

# Main installation flow
main() {
    clear
    print_header "Aurora Locus PDS Installation"
    echo ""
    echo "This script will guide you through setting up a production-ready"
    echo "ATProto Personal Data Server (PDS) for the Bluesky network."
    echo ""
    read -p "Press Enter to continue..."
    echo ""

    # Check prerequisites
    check_root
    check_dependencies

    # Get installation directory
    print_header "Installation Directory"
    INSTALL_DIR=$(pwd)
    echo "Current directory: $INSTALL_DIR"
    prompt INSTALL_DIR "Install in this directory?" "$INSTALL_DIR"
    cd "$INSTALL_DIR"
    echo ""

    # Collect configuration
    print_header "Server Configuration"

    while true; do
        prompt HOSTNAME "PDS hostname (e.g., pds.example.com)" ""
        if validate_domain "$HOSTNAME"; then
            break
        else
            print_error "Invalid domain name. Please try again."
        fi
    done

    prompt PORT "Server port" "3000"
    echo ""

    # Admin DID configuration
    print_header "Admin DID Configuration"

    echo "Aurora Locus uses OAuth 2.0 with DID-based admin authentication."
    echo ""
    print_info "You can either:"
    echo "  1. Enter an admin DID now (if you already have an account DID)"
    echo "  2. Leave blank and add it later to .env after creating your account"
    echo ""

    prompt ADMIN_DID "Admin DID (leave blank to set later)" ""

    if [ -z "$ADMIN_DID" ]; then
        print_warning "No admin DID provided - you'll need to update PDS_ADMIN_DIDS in .env later"
        ADMIN_DID="__PLACEHOLDER_ADMIN_DID__"
    else
        # Basic validation - should start with did:
        if [[ ! $ADMIN_DID =~ ^did: ]]; then
            print_error "Invalid DID format. Should start with 'did:'"
            print_info "Example: did:plc:abc123xyz..."
            exit 1
        fi
        print_success "Admin DID will be configured: $ADMIN_DID"
    fi
    echo ""

    # Federation settings
    print_header "Federation Configuration"

    prompt FEDERATION_ENABLED "Enable federation with Bluesky network? (true/false)" "true"

    if [ "$FEDERATION_ENABLED" = "true" ]; then
        prompt RELAY_URL "Relay server URL" "https://bsky.network"

        # Set PDS_PUBLIC_URL based on hostname and port
        if [ -n "$HOSTNAME" ]; then
            if [ "$PORT" = "443" ]; then
                DEFAULT_PUBLIC_URL="https://$HOSTNAME"
            else
                DEFAULT_PUBLIC_URL="https://$HOSTNAME"
            fi
        else
            DEFAULT_PUBLIC_URL=""
        fi

        prompt PDS_PUBLIC_URL "Public URL for this PDS (must be accessible from internet)" "$DEFAULT_PUBLIC_URL"
    else
        RELAY_URL=""
        PDS_PUBLIC_URL=""
    fi
    echo ""

    # Invite codes
    print_header "Invite Code Configuration"

    prompt INVITE_REQUIRED "Require invite codes for registration? (true/false)" "false"
    echo ""

    # Generate cryptographic keys
    print_header "Generating Cryptographic Keys"

    print_info "Generating JWT secret..."
    JWT_SECRET=$(generate_random 64)
    print_success "JWT secret generated"

    generate_repo_key
    generate_plc_key
    generate_oauth_keyset
    echo ""

    # Create configuration files
    print_header "Creating Configuration Files"

    # Backup existing .env if it exists
    if [ -f .env ]; then
        print_warning "Existing .env file found - backing up to .env.backup"
        mv .env .env.backup
    fi

    create_env_file

    # Verify .env was created successfully
    if [ ! -f .env ]; then
        print_error ".env file was not created!"
        exit 1
    fi

    if ! grep -q "PDS_JWT_SECRET" .env; then
        print_error ".env file is missing PDS_JWT_SECRET"
        exit 1
    fi

    print_success ".env file created and verified"
    echo ""

    # Build the project
    print_header "Building Aurora Locus"

    print_info "This may take several minutes..."
    if cargo build --release 2>&1 | tee build.log | grep -q "Finished"; then
        print_success "Build completed successfully!"
        rm build.log
    else
        print_error "Build failed. Check build.log for details."
        exit 1
    fi
    echo ""

    # Create data directories
    print_header "Setting Up Data Directories"

    # Clean existing databases for fresh install
    if [ -d "data" ]; then
        if [ -f "data/account.sqlite" ] || [ -f "data/sequencer.sqlite" ] || [ -f "data/did_cache.sqlite" ]; then
            print_warning "Existing database files found"
            prompt CLEAN_DB "Delete existing databases for fresh install? (yes/no)" "yes"
            if [ "$CLEAN_DB" = "yes" ]; then
                rm -f data/*.sqlite data/*.sqlite-*
                print_success "Existing databases deleted"
            else
                print_warning "Keeping existing databases - may cause migration conflicts!"
            fi
        fi
    fi

    mkdir -p data/actors data/blobs data/tmp
    print_success "Data directories created"
    echo ""
    mkdir -p data/actors data/blobs data/tmp
    print_success "Data directories created"
    echo ""

    # Initialize database with inline SQL
    print_header "Initializing Database"

    if [ ! -f "data/account.sqlite" ]; then
        print_info "Creating database with core tables..."
        sqlite3 data/account.sqlite << 'EOSQL'
-- Core account table
CREATE TABLE IF NOT EXISTS account (
    did TEXT PRIMARY KEY NOT NULL,
    handle TEXT UNIQUE NOT NULL,
    email TEXT UNIQUE,
    password_hash TEXT NOT NULL,
    created_at DATETIME NOT NULL DEFAULT (datetime('now')),
    email_confirmed BOOLEAN NOT NULL DEFAULT 0,
    email_confirmed_at DATETIME,
    deactivated_at DATETIME,
    takedown_ref TEXT,
    status TEXT NOT NULL DEFAULT 'active'
);
CREATE INDEX IF NOT EXISTS account_handle_idx ON account(handle);
CREATE INDEX IF NOT EXISTS account_email_idx ON account(email) WHERE email IS NOT NULL;
CREATE INDEX IF NOT EXISTS account_status_idx ON account(status);

-- Actor table (for handle/DID mapping)
CREATE TABLE IF NOT EXISTS actor (
    did TEXT PRIMARY KEY NOT NULL,
    handle TEXT UNIQUE NOT NULL,
    created_at DATETIME NOT NULL DEFAULT (datetime('now')),
    indexed_at DATETIME NOT NULL DEFAULT (datetime('now')),
    takedown_ref TEXT,
    FOREIGN KEY (did) REFERENCES account(did) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS actor_handle_idx ON actor(handle);

-- Session table
CREATE TABLE IF NOT EXISTS session (
    id TEXT PRIMARY KEY NOT NULL,
    did TEXT NOT NULL,
    access_token TEXT UNIQUE NOT NULL,
    refresh_token TEXT UNIQUE NOT NULL,
    created_at DATETIME NOT NULL DEFAULT (datetime('now')),
    expires_at DATETIME NOT NULL,
    app_password_name TEXT,
    FOREIGN KEY (did) REFERENCES actor(did) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS session_did_idx ON session(did);
CREATE INDEX IF NOT EXISTS session_access_token_idx ON session(access_token);
CREATE INDEX IF NOT EXISTS session_refresh_token_idx ON session(refresh_token);
CREATE INDEX IF NOT EXISTS session_expires_at_idx ON session(expires_at);

-- Refresh token table
CREATE TABLE IF NOT EXISTS refresh_token (
    id TEXT PRIMARY KEY NOT NULL,
    did TEXT NOT NULL,
    token TEXT UNIQUE NOT NULL,
    created_at DATETIME NOT NULL DEFAULT (datetime('now')),
    expires_at DATETIME NOT NULL,
    used BOOLEAN NOT NULL DEFAULT 0,
    used_at DATETIME,
    next_id TEXT,
    FOREIGN KEY (did) REFERENCES actor(did) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS refresh_token_did_idx ON refresh_token(did);
CREATE INDEX IF NOT EXISTS refresh_token_token_idx ON refresh_token(token);
CREATE INDEX IF NOT EXISTS refresh_token_expires_at_idx ON refresh_token(expires_at);
CREATE INDEX IF NOT EXISTS refresh_token_used_idx ON refresh_token(used);

-- Email token table
CREATE TABLE IF NOT EXISTS email_token (
    token TEXT PRIMARY KEY NOT NULL,
    did TEXT NOT NULL,
    purpose TEXT NOT NULL,
    created_at DATETIME NOT NULL DEFAULT (datetime('now')),
    expires_at DATETIME NOT NULL,
    used BOOLEAN NOT NULL DEFAULT 0,
    FOREIGN KEY (did) REFERENCES actor(did) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS email_token_did_idx ON email_token(did);
CREATE INDEX IF NOT EXISTS email_token_purpose_idx ON email_token(purpose);
CREATE INDEX IF NOT EXISTS email_token_expires_at_idx ON email_token(expires_at);
CREATE INDEX IF NOT EXISTS email_token_used_idx ON email_token(used);

-- App password table
CREATE TABLE IF NOT EXISTS app_password (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    did TEXT NOT NULL,
    name TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    created_at DATETIME NOT NULL DEFAULT (datetime('now')),
    last_used_at DATETIME,
    privileged BOOLEAN NOT NULL DEFAULT 0,
    FOREIGN KEY (did) REFERENCES actor(did) ON DELETE CASCADE,
    UNIQUE(did, name)
);
CREATE INDEX IF NOT EXISTS app_password_did_idx ON app_password(did);
CREATE INDEX IF NOT EXISTS app_password_last_used_idx ON app_password(last_used_at);

-- Repository root table
CREATE TABLE IF NOT EXISTS repo_root (
    did TEXT PRIMARY KEY NOT NULL,
    cid TEXT NOT NULL,
    rev TEXT NOT NULL,
    indexed_at DATETIME NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (did) REFERENCES actor(did) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS repo_root_cid_idx ON repo_root(cid);
CREATE INDEX IF NOT EXISTS repo_root_indexed_at_idx ON repo_root(indexed_at);

-- Record table
CREATE TABLE IF NOT EXISTS record (
    uri TEXT PRIMARY KEY NOT NULL,
    cid TEXT NOT NULL,
    collection TEXT NOT NULL,
    rkey TEXT NOT NULL,
    repo_rev TEXT NOT NULL,
    indexed_at DATETIME NOT NULL DEFAULT (datetime('now')),
    takedown_ref TEXT,
    FOREIGN KEY (uri) REFERENCES record(uri) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS record_collection_idx ON record(collection);
CREATE INDEX IF NOT EXISTS record_rkey_idx ON record(rkey);
CREATE INDEX IF NOT EXISTS record_cid_idx ON record(cid);
CREATE INDEX IF NOT EXISTS record_indexed_at_idx ON record(indexed_at);
CREATE INDEX IF NOT EXISTS record_takedown_idx ON record(takedown_ref) WHERE takedown_ref IS NOT NULL;

-- Repo block table
CREATE TABLE IF NOT EXISTS repo_block (
    cid TEXT PRIMARY KEY NOT NULL,
    content BLOB NOT NULL,
    indexed_at DATETIME NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS repo_block_indexed_at_idx ON repo_block(indexed_at);

-- Repo sequence table
CREATE TABLE IF NOT EXISTS repo_seq (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    did TEXT NOT NULL,
    event_type TEXT NOT NULL,
    event TEXT NOT NULL,
    invalidated BOOLEAN NOT NULL DEFAULT 0,
    sequenced_at DATETIME NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (did) REFERENCES actor(did) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS repo_seq_did_idx ON repo_seq(did);
CREATE INDEX IF NOT EXISTS repo_seq_event_type_idx ON repo_seq(event_type);
CREATE INDEX IF NOT EXISTS repo_seq_sequenced_at_idx ON repo_seq(sequenced_at);
CREATE INDEX IF NOT EXISTS repo_seq_invalidated_idx ON repo_seq(invalidated);

-- Blob metadata table
CREATE TABLE IF NOT EXISTS blob_metadata (
    cid TEXT PRIMARY KEY NOT NULL,
    mime_type TEXT NOT NULL,
    size INTEGER NOT NULL,
    creator_did TEXT NOT NULL,
    created_at DATETIME NOT NULL DEFAULT (datetime('now')),
    width INTEGER,
    height INTEGER,
    thumbnail_cid TEXT,
    FOREIGN KEY (creator_did) REFERENCES actor(did) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS blob_metadata_creator_did_idx ON blob_metadata(creator_did);
CREATE INDEX IF NOT EXISTS blob_metadata_mime_type_idx ON blob_metadata(mime_type);
CREATE INDEX IF NOT EXISTS blob_metadata_created_at_idx ON blob_metadata(created_at);

-- Temporary blob metadata table
CREATE TABLE IF NOT EXISTS temp_blob_metadata (
    cid TEXT PRIMARY KEY NOT NULL,
    mime_type TEXT NOT NULL,
    size INTEGER NOT NULL,
    creator_did TEXT NOT NULL,
    created_at DATETIME NOT NULL DEFAULT (datetime('now')),
    width INTEGER,
    height INTEGER,
    FOREIGN KEY (creator_did) REFERENCES actor(did) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS temp_blob_metadata_creator_did_idx ON temp_blob_metadata(creator_did);
CREATE INDEX IF NOT EXISTS temp_blob_metadata_created_at_idx ON temp_blob_metadata(created_at);

-- Account moderation table
CREATE TABLE IF NOT EXISTS account_moderation (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    did TEXT NOT NULL,
    action TEXT NOT NULL,
    moderated_by TEXT NOT NULL,
    moderated_at DATETIME NOT NULL DEFAULT (datetime('now')),
    expires_at DATETIME,
    reason TEXT,
    notes TEXT,
    reversed BOOLEAN NOT NULL DEFAULT 0,
    reversed_at DATETIME,
    reversed_by TEXT,
    reversal_reason TEXT,
    report_id INTEGER,
    FOREIGN KEY (did) REFERENCES actor(did) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS account_moderation_did_idx ON account_moderation(did);
CREATE INDEX IF NOT EXISTS account_moderation_action_idx ON account_moderation(action);
CREATE INDEX IF NOT EXISTS account_moderation_moderated_at_idx ON account_moderation(moderated_at);
CREATE INDEX IF NOT EXISTS account_moderation_expires_at_idx ON account_moderation(expires_at) WHERE expires_at IS NOT NULL;
CREATE INDEX IF NOT EXISTS account_moderation_reversed_idx ON account_moderation(reversed);

-- Label table
CREATE TABLE IF NOT EXISTS label (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    uri TEXT NOT NULL,
    cid TEXT,
    val TEXT NOT NULL,
    neg BOOLEAN NOT NULL DEFAULT 0,
    src TEXT NOT NULL,
    created_at DATETIME NOT NULL DEFAULT (datetime('now')),
    created_by TEXT,
    expires_at DATETIME
);
CREATE INDEX IF NOT EXISTS label_uri_idx ON label(uri);
CREATE INDEX IF NOT EXISTS label_cid_idx ON label(cid) WHERE cid IS NOT NULL;
CREATE INDEX IF NOT EXISTS label_val_idx ON label(val);
CREATE INDEX IF NOT EXISTS label_src_idx ON label(src);
CREATE INDEX IF NOT EXISTS label_created_at_idx ON label(created_at);
CREATE INDEX IF NOT EXISTS label_expires_at_idx ON label(expires_at) WHERE expires_at IS NOT NULL;

-- Report table
CREATE TABLE IF NOT EXISTS report (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    subject_did TEXT,
    subject_uri TEXT,
    subject_cid TEXT,
    reason_type TEXT NOT NULL,
    reason TEXT,
    reported_by TEXT NOT NULL,
    reported_at DATETIME NOT NULL DEFAULT (datetime('now')),
    status TEXT NOT NULL DEFAULT 'open',
    resolved_by TEXT,
    resolved_at DATETIME,
    resolution_notes TEXT
);
CREATE INDEX IF NOT EXISTS report_subject_did_idx ON report(subject_did) WHERE subject_did IS NOT NULL;
CREATE INDEX IF NOT EXISTS report_subject_uri_idx ON report(subject_uri) WHERE subject_uri IS NOT NULL;
CREATE INDEX IF NOT EXISTS report_reason_type_idx ON report(reason_type);
CREATE INDEX IF NOT EXISTS report_reported_by_idx ON report(reported_by);
CREATE INDEX IF NOT EXISTS report_reported_at_idx ON report(reported_at);
CREATE INDEX IF NOT EXISTS report_status_idx ON report(status);

-- Admin roles table
CREATE TABLE IF NOT EXISTS admin_roles (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    did TEXT NOT NULL,
    role TEXT NOT NULL,
    granted_by TEXT NOT NULL,
    granted_at DATETIME NOT NULL,
    revoked BOOLEAN NOT NULL DEFAULT 0,
    revoked_at DATETIME,
    revoked_by TEXT,
    notes TEXT,
    FOREIGN KEY (did) REFERENCES actor(did) ON DELETE CASCADE,
    UNIQUE(did, role)
);
CREATE INDEX IF NOT EXISTS admin_roles_did_idx ON admin_roles(did);
CREATE INDEX IF NOT EXISTS admin_roles_role_idx ON admin_roles(role);
CREATE INDEX IF NOT EXISTS admin_roles_revoked_idx ON admin_roles(revoked);

-- Admin audit log table
CREATE TABLE IF NOT EXISTS admin_audit_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    admin_did TEXT NOT NULL,
    action TEXT NOT NULL,
    subject_did TEXT,
    subject_uri TEXT,
    details TEXT,
    timestamp DATETIME NOT NULL DEFAULT (datetime('now')),
    ip_address TEXT,
    FOREIGN KEY (admin_did) REFERENCES actor(did) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS admin_audit_log_admin_did_idx ON admin_audit_log(admin_did);
CREATE INDEX IF NOT EXISTS admin_audit_log_action_idx ON admin_audit_log(action);
CREATE INDEX IF NOT EXISTS admin_audit_log_timestamp_idx ON admin_audit_log(timestamp);
CREATE INDEX IF NOT EXISTS admin_audit_log_subject_did_idx ON admin_audit_log(subject_did) WHERE subject_did IS NOT NULL;

-- Lexicon failure table
CREATE TABLE IF NOT EXISTS lexicon_failure (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    collection TEXT NOT NULL,
    record_uri TEXT NOT NULL,
    validation_errors TEXT NOT NULL,
    detected_at DATETIME NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS lexicon_failure_collection_idx ON lexicon_failure(collection);
CREATE INDEX IF NOT EXISTS lexicon_failure_record_uri_idx ON lexicon_failure(record_uri);
CREATE INDEX IF NOT EXISTS lexicon_failure_detected_at_idx ON lexicon_failure(detected_at);

-- Invite code table
CREATE TABLE IF NOT EXISTS invite_code (
    code TEXT PRIMARY KEY NOT NULL,
    available_uses INTEGER NOT NULL DEFAULT 1,
    disabled BOOLEAN NOT NULL DEFAULT 0,
    for_account TEXT,
    created_by TEXT NOT NULL,
    created_at DATETIME NOT NULL DEFAULT (datetime('now')),
    expires_at DATETIME
);
CREATE INDEX IF NOT EXISTS invite_code_disabled_idx ON invite_code(disabled);
CREATE INDEX IF NOT EXISTS invite_code_for_account_idx ON invite_code(for_account) WHERE for_account IS NOT NULL;

-- Invite code use table
CREATE TABLE IF NOT EXISTS invite_code_use (
    code TEXT NOT NULL,
    used_by TEXT NOT NULL,
    used_at DATETIME NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (code) REFERENCES invite_code(code) ON DELETE CASCADE,
    PRIMARY KEY (code, used_by)
);
CREATE INDEX IF NOT EXISTS invite_code_use_code_idx ON invite_code_use(code);
CREATE INDEX IF NOT EXISTS invite_code_use_used_by_idx ON invite_code_use(used_by);
CREATE UNIQUE INDEX IF NOT EXISTS invite_code_use_unique_idx ON invite_code_use(code, used_by);

-- Sequencer config table
CREATE TABLE IF NOT EXISTS sequencer_config (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL,
    updated_at DATETIME NOT NULL DEFAULT (datetime('now'))
);

-- PLC keys table
CREATE TABLE IF NOT EXISTS plc_keys (
    did TEXT PRIMARY KEY NOT NULL,
    rotation_key_public TEXT NOT NULL,
    rotation_key_type TEXT NOT NULL,
    created_at DATETIME NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (did) REFERENCES actor(did) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS plc_keys_did_idx ON plc_keys(did);

-- NOTE: All tables created. sqlx will handle remaining migrations automatically.
EOSQL

        if [ "$ADMIN_DID" != "__PLACEHOLDER_ADMIN_DID__" ] && [ -n "$ADMIN_DID" ]; then
            # Use RFC3339 format for timestamp (e.g., 2025-10-24T18:41:11+00:00)
            TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%S+00:00")
            sqlite3 data/account.sqlite "INSERT INTO admin_roles (did, role, granted_by, granted_at, revoked) VALUES ('$ADMIN_DID', 'superadmin', 'installer', '$TIMESTAMP', 0);"
            print_success "Database initialized - Admin DID $ADMIN_DID added as superadmin"
        else
            print_success "Database initialized with core tables"
        fi
        print_info "OAuth tables (device, authorization_request, token, etc.) will be created automatically on first startup"
    else
        print_info "Database already exists - OAuth tables will be added automatically if missing"
    fi

    # Initialize DID cache database
    if [ ! -f "data/did_cache.sqlite" ]; then
        print_info "Creating DID cache database..."
        sqlite3 data/did_cache.sqlite << 'EOSQL'
-- DID document cache
CREATE TABLE IF NOT EXISTS did_doc (
    did TEXT PRIMARY KEY,
    doc TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    cached_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_did_doc_updated_at ON did_doc(updated_at);

-- Handle to DID mapping cache
CREATE TABLE IF NOT EXISTS did_handle (
    handle TEXT PRIMARY KEY,
    did TEXT NOT NULL,
    declared_at TEXT,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_did_handle_did ON did_handle(did);
CREATE INDEX IF NOT EXISTS idx_did_handle_updated_at ON did_handle(updated_at);

-- Migration tracking for DID cache database
CREATE TABLE IF NOT EXISTS _sqlx_migrations (
    version BIGINT PRIMARY KEY NOT NULL,
    description TEXT NOT NULL,
    installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    success BOOLEAN NOT NULL,
    checksum BLOB NOT NULL,
    execution_time BIGINT NOT NULL
);
INSERT OR IGNORE INTO _sqlx_migrations (version, description, installed_on, success, checksum, execution_time)
VALUES (20251122000000, 'did_cache_tables', CURRENT_TIMESTAMP, 1, X'00', 0);
EOSQL
        print_success "DID cache database initialized"
    else
        print_info "DID cache database already exists"
    fi

    echo ""

    print_header "Configuration Complete"
    print_success "All configuration files have been generated!"
    print_success "Database has been initialized"
    echo ""
    # Optional: systemd service
    print_header "System Integration (Optional)"

    prompt SETUP_SYSTEMD "Create systemd service file? (yes/no)" "yes"
    if [ "$SETUP_SYSTEMD" = "yes" ]; then
        create_systemd_service
    fi

    prompt SETUP_NGINX "Create nginx configuration? (yes/no)" "yes"
    if [ "$SETUP_NGINX" = "yes" ]; then
        create_nginx_config
    fi

    # Installation complete
    print_header "Installation Complete!"

    echo ""
    print_success "🎉 Aurora Locus PDS has been installed successfully!"
    echo ""

    print_header "Next Steps"
    echo ""

    print_info "STEP 1: Start the server"
    echo "  ./target/release/aurora-locus"
    echo ""
    print_info "  Or run in background:"
    echo "  nohup ./target/release/aurora-locus > pds.log 2>&1 &"
    echo ""

    print_info "STEP 2: Create your first account"
    echo "  curl -X POST http://localhost:$PORT/xrpc/com.atproto.server.createAccount \\"
    echo "    -H 'Content-Type: application/json' \\"
    echo "    -d '{\"handle\":\"you.$HOSTNAME\",\"email\":\"you@example.com\",\"password\":\"secure-password\"}'"
    echo ""
    print_warning "  SAVE THE DID from the response!"
    echo "  Example response: {\"did\": \"did:plc:abc123xyz...\", ...}"
    echo ""

    if [ "$ADMIN_DID" = "__PLACEHOLDER_ADMIN_DID__" ]; then
        print_info "STEP 3: Configure admin DID"
        echo "  Edit .env and replace:"
        echo "    PDS_ADMIN_DIDS=__PLACEHOLDER_ADMIN_DID__"
        echo "  With your actual DID:"
        echo "    PDS_ADMIN_DIDS=did:plc:abc123xyz..."
        echo ""
        print_warning "  Restart the server after updating .env!"
        echo ""
    else
        print_info "STEP 3: Admin DID already configured"
        echo "  Admin DID: $ADMIN_DID"
        echo "  ✓ Already set in .env"
        echo ""
    fi

    print_info "STEP 4: Grant admin role (optional - for database admin)"
    echo "  sqlite3 data/accounts.db"
    echo "  INSERT INTO admin_role (did, role, granted_by, granted_at, revoked)"
    echo "    VALUES ('YOUR_DID', 'superadmin', 'system', datetime('now'), 0);"
    echo "  .exit"
    echo ""
    print_info "  Note: If your DID is in PDS_ADMIN_DIDS, you automatically get admin"
    echo "  access via OAuth without needing a database role."
    echo ""

    print_info "STEP 5: Access OAuth admin panel"
    echo "  Visit: http://localhost:$PORT/oauth/authorize"
    echo "  Login with your handle and password"
    echo ""

    print_header "Testing Your PDS"
    echo ""
    print_info "Health check:"
    echo "  curl http://localhost:$PORT/health"
    echo ""
    print_info "Server info:"
    echo "  curl http://localhost:$PORT/xrpc/com.atproto.server.describeServer"
    echo ""

    if [ "$SETUP_SYSTEMD" = "yes" ]; then
        echo ""
        print_info "OPTIONAL: Install systemd service"
        echo "  sudo cp /tmp/aurora-locus.service /etc/systemd/system/"
        echo "  sudo systemctl daemon-reload"
        echo "  sudo systemctl enable aurora-locus"
        echo "  sudo systemctl start aurora-locus"
    fi

    if [ "$SETUP_NGINX" = "yes" ]; then
        echo ""
        print_info "OPTIONAL: Configure nginx reverse proxy"
        echo "  1. Get SSL certificate:"
        echo "     sudo certbot --nginx -d $HOSTNAME"
        echo "  2. Install config:"
        echo "     sudo cp /tmp/aurora-locus-nginx.conf /etc/nginx/sites-available/aurora-locus"
        echo "     sudo ln -s /etc/nginx/sites-available/aurora-locus /etc/nginx/sites-enabled/"
        echo "  3. Reload nginx:"
        echo "     sudo nginx -t && sudo systemctl reload nginx"
    fi

    echo ""
    print_header "Security Reminder"
    print_warning "Keep these files SECRET - they contain cryptographic keys:"
    echo "  - .env (JWT secret, signing keys)"
    echo "  - oauth-keyset.json (OAuth private key)"
    echo ""
    print_info "Generated files:"
    echo "  📄 .env                    - Configuration"
    echo "  🔐 oauth-keyset.json       - OAuth P-256 keyset"
    echo "  📁 data/                   - Data directory"
    echo "  🚀 target/release/aurora-locus - Server binary"
    echo ""

    print_success "Installation complete! 🎉"
    echo ""
    print_info "Your admin account will be: $FULL_HANDLE"
    print_info "Installation directory: $INSTALL_DIR"
    echo ""
}

# Run main installation
main
