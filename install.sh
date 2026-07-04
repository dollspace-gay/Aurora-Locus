#!/usr/bin/env bash
#
# Aurora Locus — installer.
#
# Composes a working `.env` for a fresh PDS deployment by DERIVING it from
# `.env.example` (the single source of truth). The installer never carries its
# own parallel list of config variables: it copies `.env.example` line-for-line,
# substituting values only for keys that already exist there. That structural
# choice is deliberate — it makes the whole class of "phantom env var" drift
# (vars the script wrote that the app never reads) impossible, and means a new
# field landing in `.env.example` is picked up here automatically with no edit.
#
# Value substitution follows three rules, all encoded in `.env.example` itself:
#   1. A `# Generate with:  <cmd>` comment above one or more KEY= lines marks
#      those as generated secrets — the installer runs <cmd> (an `openssl …`
#      invocation) and writes the output. This covers PDS_JWT_SECRET and the two
#      _K256_PRIVATE_KEY_HEX signing keys (raw 32-byte hex — the format
#      `PlcSigner` decodes; the previous installer emitted DER and never booted).
#   2. A small fixed set of deployment-identity keys is derived from the one
#      value the operator must supply, their public domain (PDS_HOSTNAME,
#      AURORA_DOMAIN, PDS_SERVICE_DID, PDS_PUBLIC_URL, PDS_SERVICE_HANDLE_DOMAINS).
#   3. Everything else is copied verbatim (working defaults + all documentation).
#
# Admin access is NOT an env var (the old `PDS_ADMIN_DIDS` never had a consumer).
# It is granted per-DID from the `admin_roles` table via the offline
# `grant-admin` subcommand; the installer prints that as a post-boot step.
#
# TLS is out of scope for the native path — run your own reverse proxy (nginx,
# standalone Caddy, or a cloud load balancer). The docker-compose path ships a
# Caddy sidecar with automatic Let's Encrypt (see docker-compose.yml / README).

set -euo pipefail

# ---- presentation -----------------------------------------------------------

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; BLUE='\033[0;34m'; NC='\033[0m'
info()    { echo -e "${BLUE}ℹ${NC} $1"; }
success() { echo -e "${GREEN}✓${NC} $1"; }
warn()    { echo -e "${YELLOW}⚠${NC} $1"; }
err()     { echo -e "${RED}✗${NC} $1" >&2; }
header() {
    echo -e "${BLUE}"
    echo "═══════════════════════════════════════════════════════════"
    echo "  $1"
    echo "═══════════════════════════════════════════════════════════"
    echo -e "${NC}"
}

# ---- locate the repo --------------------------------------------------------

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$SCRIPT_DIR"
ENV_EXAMPLE="$REPO_ROOT/.env.example"
ENV_OUT="$REPO_ROOT/.env"
SETUP_DB="$REPO_ROOT/scripts/setup-database.sh"

# ---- arguments --------------------------------------------------------------

DOMAIN=""
DATA_DIR="./data"
NON_INTERACTIVE=false
FORCE=false
ADMIN_DID=""

usage() {
    cat <<EOF
Aurora Locus installer — composes a working .env from .env.example.

Usage: ./install.sh [options]

  --domain DOMAIN       Public domain for this PDS (e.g. pds.example.com).
                        Derives PDS_HOSTNAME / AURORA_DOMAIN / PDS_SERVICE_DID /
                        PDS_PUBLIC_URL / PDS_SERVICE_HANDLE_DOMAINS. Omit for a
                        localhost dev install.
  --data-dir PATH       Data directory (default: ./data).
  --admin-did DID       DID to print grant-admin bootstrap instructions for.
  --non-interactive     Never prompt; use defaults / provided flags. Requires
                        --force to overwrite an existing .env.
  --force               Overwrite an existing .env (backs it up to .env.bak).
  -h, --help            Show this help.
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --domain)          DOMAIN="${2:?--domain needs a value}"; shift 2 ;;
        --data-dir)        DATA_DIR="${2:?--data-dir needs a value}"; shift 2 ;;
        --admin-did)       ADMIN_DID="${2:?--admin-did needs a value}"; shift 2 ;;
        --non-interactive) NON_INTERACTIVE=true; shift ;;
        --force)           FORCE=true; shift ;;
        -h|--help)         usage; exit 0 ;;
        *) err "Unknown option: $1"; echo; usage; exit 1 ;;
    esac
done

# ---- prerequisites ----------------------------------------------------------

header "Aurora Locus — Install"

if [[ ! -f "$ENV_EXAMPLE" ]]; then
    err ".env.example not found at $ENV_EXAMPLE"
    err "Run this script from a checkout of the Aurora Locus repository."
    exit 1
fi

if ! command -v openssl >/dev/null 2>&1; then
    err "openssl is required (used to generate secrets and signing keys) but was not found."
    exit 1
fi
success "openssl found"

if command -v cargo >/dev/null 2>&1; then
    success "cargo found — you can build/run natively (cargo run --release)"
elif command -v docker >/dev/null 2>&1; then
    success "docker found — you can run via docker compose up -d"
else
    warn "Neither cargo nor docker found. Install one before running the server:"
    warn "  • Rust toolchain (see rust-toolchain.toml for the pinned version), or"
    warn "  • Docker + Docker Compose (see docker-compose.yml)."
fi

# ---- gather operator input --------------------------------------------------

if [[ -z "$DOMAIN" && "$NON_INTERACTIVE" == false ]]; then
    echo
    info "Public domain for this PDS (leave blank for a localhost dev install)."
    info "Must be a domain you control with an A/AAAA record pointing here, so"
    info "did:web + federation + TLS can resolve it."
    read -r -p "Domain [localhost]: " DOMAIN
fi

if [[ -z "$DOMAIN" ]]; then
    DOMAIN="localhost"
    info "No domain supplied — configuring for localhost (development)."
else
    info "Configuring for domain: $DOMAIN"
fi

# ---- guard an existing .env -------------------------------------------------

if [[ -f "$ENV_OUT" ]]; then
    if [[ "$FORCE" == true ]]; then
        cp "$ENV_OUT" "$ENV_OUT.bak"
        warn "Existing .env backed up to .env.bak"
    elif [[ "$NON_INTERACTIVE" == true ]]; then
        err ".env already exists. Re-run with --force to overwrite (backs up to .env.bak)."
        exit 1
    else
        read -r -p ".env already exists. Overwrite? (backs up to .env.bak) [y/N]: " ans
        if [[ "${ans,,}" != "y" && "${ans,,}" != "yes" ]]; then
            err "Aborted — existing .env left untouched."
            exit 1
        fi
        cp "$ENV_OUT" "$ENV_OUT.bak"
        warn "Existing .env backed up to .env.bak"
    fi
fi

# ---- deployment-identity overrides (rule 2) --------------------------------
#
# The single unavoidable piece of domain knowledge: how one operator-supplied
# domain maps onto the identity-bearing config keys. Only used when a non-
# localhost domain is given; a localhost dev install keeps .env.example's
# defaults untouched. Every key here already exists in .env.example.

declare -A OVERRIDE=()

# The data directory is independent of the domain: always pin it so the .env
# and the directory setup-database.sh prepares agree (default ./data matches the
# template, so this is a no-op there).
OVERRIDE[PDS_DATA_DIRECTORY]="$DATA_DIR"

if [[ "$DOMAIN" != "localhost" ]]; then
    OVERRIDE[PDS_HOSTNAME]="$DOMAIN"
    OVERRIDE[AURORA_DOMAIN]="$DOMAIN"
    OVERRIDE[PDS_SERVICE_DID]="did:web:$DOMAIN"
    OVERRIDE[PDS_PUBLIC_URL]="https://$DOMAIN"
    OVERRIDE[PDS_SERVICE_HANDLE_DOMAINS]=".$DOMAIN"
fi

# ---- derive .env from .env.example (rules 1 & 3) ----------------------------

info "Composing .env from .env.example …"

gen_cmd=""          # pending `# Generate with:` command; applies to the
                    # consecutive KEY= lines that follow, reset by a blank
                    # line or a non-hint comment.
tmp_env="$(mktemp)"
generated_count=0
overridden_count=0

while IFS= read -r line || [[ -n "$line" ]]; do
    # Generator-hint comment → arm the generator for the following KEY= lines.
    if [[ "$line" =~ ^#[[:space:]]*Generate[[:space:]]with:[[:space:]]*(.+)$ ]]; then
        gen_cmd="${BASH_REMATCH[1]}"
        printf '%s\n' "$line" >>"$tmp_env"
        continue
    fi

    # Any other comment or a blank line ends the current generator run.
    if [[ "$line" =~ ^[[:space:]]*# ]] || [[ -z "${line//[[:space:]]/}" ]]; then
        gen_cmd=""
        printf '%s\n' "$line" >>"$tmp_env"
        continue
    fi

    # Uncommented KEY=value line?
    if [[ "$line" =~ ^([A-Za-z_][A-Za-z0-9_]*)=(.*)$ ]]; then
        key="${BASH_REMATCH[1]}"
        val="${BASH_REMATCH[2]}"

        if [[ -n "${OVERRIDE[$key]+x}" ]]; then
            val="${OVERRIDE[$key]}"
            overridden_count=$((overridden_count + 1))
        elif [[ -n "$gen_cmd" ]]; then
            # Only run recognised generators from the trusted template.
            if [[ "$gen_cmd" =~ ^openssl[[:space:]] ]]; then
                val="$(eval "$gen_cmd")"
                generated_count=$((generated_count + 1))
            else
                warn "Unrecognised generator '$gen_cmd' for $key — leaving blank."
                val=""
            fi
        fi

        printf '%s=%s\n' "$key" "$val" >>"$tmp_env"
        continue
    fi

    # Anything else (shouldn't happen) — copy verbatim.
    printf '%s\n' "$line" >>"$tmp_env"
done <"$ENV_EXAMPLE"

mv "$tmp_env" "$ENV_OUT"
chmod 600 "$ENV_OUT"
success ".env written ($generated_count secrets generated, $overridden_count identity fields set for $DOMAIN)"

# ---- data directory + schema ------------------------------------------------

echo
if [[ -x "$SETUP_DB" ]]; then
    info "Preparing the data directory via scripts/setup-database.sh …"
    "$SETUP_DB" --data-dir "$DATA_DIR" --non-interactive
else
    warn "scripts/setup-database.sh not found or not executable — creating $DATA_DIR directly."
    mkdir -p "$DATA_DIR"
fi

# ---- final instructions -----------------------------------------------------

header "Install complete"

echo "Configuration:"
echo "  .env           $ENV_OUT   (chmod 600)"
echo "  data directory $DATA_DIR"
echo "  domain         $DOMAIN"
echo
info "Verify the configuration before first boot:"
echo "  cargo run --release -- validate-config"
echo
info "Start the server (schema migrations run automatically on first boot):"
echo "  cargo run --release            # native"
echo "  # or"
echo "  docker compose up -d           # container + automatic-TLS Caddy sidecar"
echo
info "Grant your first admin (admin is table-based, not an env var):"
echo "  1. Create an account:  ./create-account.sh   (or the createAccount XRPC)"
if [[ -n "$ADMIN_DID" ]]; then
    echo "  2. Grant the role:     cargo run --release -- grant-admin $ADMIN_DID superadmin"
else
    echo "  2. Grant the role:     cargo run --release -- grant-admin <DID> superadmin"
fi
info "(grant-admin is offline and runs migrations itself, so it works before the"
info " first server boot; the DID must be an account that already exists.)"
echo
if [[ "$DOMAIN" != "localhost" ]]; then
    warn "Native install does not terminate TLS. Put a reverse proxy (nginx, Caddy,"
    warn "or a cloud load balancer) in front of :2583 for $DOMAIN, or use the"
    warn "docker-compose path which bundles automatic Let's Encrypt."
fi
