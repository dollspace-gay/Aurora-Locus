#!/bin/bash

# Aurora Locus - Account Creation Script
# Creates a new account on a running PDS instance

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

print_success() { echo -e "${GREEN}✓ $1${NC}"; }
print_error() { echo -e "${RED}✗ $1${NC}"; }
print_warning() { echo -e "${YELLOW}⚠ $1${NC}"; }
print_info() { echo -e "${BLUE}ℹ $1${NC}"; }
print_header() {
    echo -e "${BLUE}"
    echo "═══════════════════════════════════════════════════════════"
    echo "  $1"
    echo "═══════════════════════════════════════════════════════════"
    echo -e "${NC}"
}

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

# Validate email
validate_email() {
    local email=$1
    if [[ $email =~ ^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$ ]]; then
        return 0
    else
        return 1
    fi
}

# Load PDS configuration
load_config() {
    if [ ! -f .env ]; then
        print_error ".env file not found"
        print_info "Make sure you're in the Aurora Locus directory"
        print_info "Run ./install.sh first to set up the PDS"
        exit 1
    fi

    # Load .env file (properly handle comments and special chars)
    while IFS='=' read -r key value; do
        # Skip empty lines and comments
        [[ -z "$key" || "$key" =~ ^#.* ]] && continue

        # Remove inline comments and trim whitespace
        value="${value%%#*}"
        value="${value%"${value##*[![:space:]]}"}"

        # Export the variable
        export "$key=$value"
    done < <(grep -E '^[A-Z_]+=.*' .env)

    # Prefer the deployment's canonical public URL when set. The config designates
    # PDS_SERVICE_PUBLIC_URL as the real endpoint and the hostname:port form as the
    # "internal :2583 leak"; its default is http://localhost:2583, so standard
    # installs are unchanged, while a real deployment (PDS_HOSTNAME set to a public
    # domain not served on :2583) now reaches the API instead of a dead port.
    if [ -n "${PDS_SERVICE_PUBLIC_URL:-}" ]; then
        PDS_URL="${PDS_SERVICE_PUBLIC_URL%/}"
    else
        PDS_URL="http://${PDS_HOSTNAME}:${PDS_PORT}"
    fi
}

# Check if server is running
check_server() {
    print_info "Checking if PDS is running..."

    if ! curl -s --connect-timeout 5 --max-time 10 "${PDS_URL}/health" > /dev/null 2>&1; then
        print_error "PDS server is not running at ${PDS_URL}"
        echo ""
        print_info "Start the server with:"
        echo "  ./target/release/aurora-locus"
        exit 1
    fi

    print_success "PDS is running at ${PDS_URL}"
}

# Create account
create_account() {
    print_header "Create New Account"

    # Get handle
    while true; do
        prompt HANDLE "Handle (without domain, e.g., 'alice')" ""
        if [[ $HANDLE =~ ^[a-z0-9-]+$ ]]; then
            FULL_HANDLE="${HANDLE}.${PDS_HOSTNAME}"
            break
        else
            print_error "Handle must contain only lowercase letters, numbers, and hyphens"
        fi
    done

    # Get email
    while true; do
        prompt EMAIL "Email address" ""
        if validate_email "$EMAIL"; then
            break
        else
            print_error "Invalid email address"
        fi
    done

    # Get password
    while true; do
        prompt PASSWORD "Password (min 8 characters)" "" "secret"
        if [ ${#PASSWORD} -ge 8 ]; then
            prompt PASSWORD_CONFIRM "Confirm password" "" "secret"
            if [ "$PASSWORD" = "$PASSWORD_CONFIRM" ]; then
                break
            else
                print_error "Passwords do not match"
            fi
        else
            print_error "Password must be at least 8 characters"
        fi
    done

    echo ""
    print_info "Creating account: $FULL_HANDLE"

    # Call API
    RESPONSE=$(curl -s -w "\n%{http_code}" --connect-timeout 5 --max-time 30 \
      -X POST "${PDS_URL}/xrpc/com.atproto.server.createAccount" \
      -H "Content-Type: application/json" \
      -d "{\"handle\":\"$FULL_HANDLE\",\"email\":\"$EMAIL\",\"password\":\"$PASSWORD\"}")

    HTTP_CODE=$(echo "$RESPONSE" | tail -n1)
    BODY=$(echo "$RESPONSE" | sed '$d')

    if [ "$HTTP_CODE" = "200" ]; then
        DID=$(echo "$BODY" | jq -r '.did // empty')

        echo ""
        print_success "Account created successfully!"
        echo ""
        echo "═══════════════════════════════════════════════════════════"
        echo "  Account Information"
        echo "═══════════════════════════════════════════════════════════"
        echo ""
        echo "  Handle:   $FULL_HANDLE"
        echo "  Email:    $EMAIL"
        echo "  DID:      $DID"
        echo "  PDS URL:  $PDS_URL"
        echo ""

        # Check if this should be an admin. Accept any case + the y/yes forms.
        prompt MAKE_ADMIN "Make this account an admin? (yes/no)" "no"
        MAKE_ADMIN=$(printf '%s' "$MAKE_ADMIN" | tr '[:upper:]' '[:lower:]')

        if [ "$MAKE_ADMIN" = "yes" ] || [ "$MAKE_ADMIN" = "y" ]; then
            echo ""
            print_info "To grant admin access, insert an admin_roles row:"
            echo ""
            echo "  sqlite3 \"\${PDS_DATA_DIRECTORY:-./data}/account.sqlite\""
            echo "  INSERT INTO admin_roles (did, role, granted_by, granted_at, revoked)"
            echo "    VALUES ('$DID', 'superadmin', 'system', datetime('now'), 0);"
            echo "  .exit"
            echo ""
            print_info "(Admin is resolved live per-request from the admin_roles"
            print_info " table — there is no PDS_ADMIN_DIDS env var. Restart not required.)"
            echo ""
        fi

        echo "═══════════════════════════════════════════════════════════"
        echo ""

    else
        print_error "Account creation failed"
        echo ""
        print_error "HTTP $HTTP_CODE"
        echo "$BODY" | jq -r '.message // .error // .' 2>/dev/null || echo "$BODY"
        exit 1
    fi
}

# Main
main() {
    clear
    print_header "Aurora Locus - Account Creation"
    echo ""

    load_config
    check_server

    echo ""
    create_account

    echo ""
    print_info "Create another account? Run this script again!"
    echo ""
}

main
