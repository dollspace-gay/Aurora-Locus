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
#      AURORA_DOMAIN, PDS_SERVICE_DID, PDS_SERVICE_PUBLIC_URL, PDS_SERVICE_HANDLE_DOMAINS).
#   3. Everything else is copied verbatim (working defaults + all documentation).
#
# Beyond the .env, the installer brings the NATIVE deploy path up to parity with
# the docker-compose path by handling the two prerequisites operators otherwise
# solve by hand:
#   - the Rust toolchain (bootstraps rustup if absent; the pinned toolchain then
#     auto-selects from rust-toolchain.toml on the first cargo invocation), and
#   - TLS termination (detects an installed reverse proxy — Caddy / nginx /
#     Apache — and writes a site config for the operator's domain, or offers to
#     install Caddy on a fresh host for automatic Let's Encrypt).
#
# Admin access is NOT an env var (the old `PDS_ADMIN_DIDS` never had a consumer).
# It is granted per-DID from the `admin_roles` table via the offline
# `grant-admin` subcommand; the installer prints that as a post-boot step.

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

# ---- privilege / platform helpers ------------------------------------------

# Run a command as root: directly if already root, via sudo if available,
# otherwise fail with a clear message. Used for package installs and writes
# under /etc.
run_root() {
    if [[ "${EUID:-$(id -u)}" -eq 0 ]]; then
        "$@"
    elif command -v sudo >/dev/null 2>&1; then
        sudo "$@"
    else
        err "This step needs root privileges (or sudo), which are not available."
        return 1
    fi
}

# Echo the distro family: debian | rhel | arch | unknown.
detect_distro() {
    if [[ -f /etc/debian_version ]]; then echo debian
    elif [[ -f /etc/redhat-release ]]; then echo rhel
    elif [[ -f /etc/arch-release ]]; then echo arch
    else echo unknown; fi
}

# True if systemd reports the given unit active.
svc_active() {
    command -v systemctl >/dev/null 2>&1 && systemctl is-active --quiet "$1" 2>/dev/null
}

# ---- reverse-proxy config renderers ----------------------------------------
#
# Pure functions: each echoes the site config it would write, with the
# operator's domain substituted, and touches nothing. `--emit-proxy-config`
# exposes them for preview/testing. The proxy's own runtime variables ($host,
# $scheme, …) are protected by a quoted heredoc and survive the substitution —
# only the __AURORA_DOMAIN__ token is replaced.

render_caddy_block() {
    cat <<CADDY
${DOMAIN} {
    reverse_proxy localhost:2583
}
CADDY
}

render_nginx_conf() {
    sed "s/__AURORA_DOMAIN__/${DOMAIN}/g" <<'NGINX'
server {
    listen 443 ssl http2;
    server_name __AURORA_DOMAIN__;

    # certbot-managed certs (adjust path if your certs live elsewhere)
    ssl_certificate     /etc/letsencrypt/live/__AURORA_DOMAIN__/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/__AURORA_DOMAIN__/privkey.pem;

    location / {
        proxy_pass http://127.0.0.1:2583;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_read_timeout 3600s;
    }
}

server {
    listen 80;
    server_name __AURORA_DOMAIN__;
    return 301 https://$host$request_uri;
}
NGINX
}

render_apache_conf() {
    sed "s/__AURORA_DOMAIN__/${DOMAIN}/g" <<'APACHE'
<VirtualHost *:443>
    ServerName __AURORA_DOMAIN__

    SSLEngine on
    SSLCertificateFile      /etc/letsencrypt/live/__AURORA_DOMAIN__/fullchain.pem
    SSLCertificateKeyFile   /etc/letsencrypt/live/__AURORA_DOMAIN__/privkey.pem

    ProxyPass        / http://127.0.0.1:2583/
    ProxyPassReverse / http://127.0.0.1:2583/
    ProxyPreserveHost On
    RequestHeader set X-Forwarded-Proto "https"
</VirtualHost>

<VirtualHost *:80>
    ServerName __AURORA_DOMAIN__
    Redirect permanent / https://__AURORA_DOMAIN__/
</VirtualHost>
APACHE
}

# ---- rustup bootstrap -------------------------------------------------------

# Echo the pinned channel from rust-toolchain.toml (e.g. "1.91"), or nothing if
# the file/line is absent. Tolerates single/double quotes, surrounding
# whitespace, trailing comments, and a missing or comment-only file.
read_pinned_channel() {
    local f="$REPO_ROOT/rust-toolchain.toml" line
    [[ -f "$f" ]] || { echo ""; return 0; }
    line="$(grep -E '^[[:space:]]*channel[[:space:]]*=' "$f" 2>/dev/null | head -1)"
    [[ -n "$line" ]] || { echo ""; return 0; }
    line="${line#*=}"        # drop up to the '='
    line="${line%%#*}"       # drop a trailing comment
    line="${line//\"/}"      # drop double quotes
    line="${line//\'/}"      # drop single quotes
    line="$(echo "$line" | tr -d '[:space:]')"
    echo "$line"
}

# Bug 1 fix: rustup-init ran with --default-toolchain none, so nothing was
# installed and no default is set — a later `cargo` in the workspace would only
# auto-install if cwd happens to be the workspace. Explicitly install the pinned
# channel and set it as the default so cargo works from any directory.
ensure_pinned_toolchain() {
    local ch; ch="$(read_pinned_channel)"
    if [[ -z "$ch" ]]; then
        warn "No pinned channel found in rust-toolchain.toml — leaving toolchain"
        warn "selection to rustup's on-demand install in the workspace."
        return 0
    fi
    info "Installing the pinned Rust toolchain ($ch) …"
    rustup toolchain install "$ch"
    rustup default "$ch"
    success "Active toolchain: $(rustup show active-toolchain 2>/dev/null || echo "$ch")"
}

# Bug 2 fix: rustup-init was run with --no-modify-path (no rc edits without
# consent), so ~/.cargo/bin is on install.sh's PATH (we source it) but NOT the
# operator's parent shell — the banner's `cargo` next-step would resolve to an
# older system cargo, or nothing. Offer to persist it (opt-in; auto-yes under
# --non-interactive). New shells then get cargo; the banner still prints how to
# fix the CURRENT shell.
offer_shell_rc_append() {
    local rc="$HOME/.bashrc"
    if [[ -f "$rc" ]] && grep -qF '.cargo/env' "$rc"; then
        info "~/.bashrc already sources ~/.cargo/env — new shells get cargo automatically."
        return 0
    fi
    local do_it=false
    if [[ "$NON_INTERACTIVE" == true ]]; then
        do_it=true
    else
        info "cargo is installed under ~/.cargo/bin, which is not yet on your shell's PATH."
        read -r -p "Add cargo to your PATH permanently? (appends to ~/.bashrc) [Y/n]: " ans
        [[ -z "$ans" || "${ans,,}" == "y" || "${ans,,}" == "yes" ]] && do_it=true
    fi
    if [[ "$do_it" == true ]]; then
        printf '\n# Added by aurora-locus install.sh — put rustup/cargo on PATH\n. "$HOME/.cargo/env"\n' >> "$rc"
        success "Appended cargo env to $rc (effective in new shells)."
    else
        info "Left ~/.bashrc unchanged."
    fi
}

bootstrap_rustup() {
    if command -v rustup >/dev/null 2>&1; then
        success "rustup found — toolchain resolves from rust-toolchain.toml"
        return 0
    fi
    if [[ "$SKIP_RUSTUP" == true ]]; then
        warn "rustup not found and --skip-rustup set — provide a Rust toolchain yourself before building."
        return 0
    fi

    local do_install=false
    if [[ "$NON_INTERACTIVE" == true ]]; then
        do_install=true
    else
        warn "rustup is not installed. Distro 'rust' packages are often too old to"
        warn "parse this workspace's lockfile; rustup is the supported toolchain."
        read -r -p "Install rustup now? [Y/n]: " ans
        [[ -z "$ans" || "${ans,,}" == "y" || "${ans,,}" == "yes" ]] && do_install=true
    fi

    if [[ "$do_install" != true ]]; then
        warn "Skipping rustup install — install a Rust toolchain before 'cargo run'."
        return 0
    fi

    info "Installing rustup (no rc edits yet; the pinned toolchain is installed next) …"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --default-toolchain none
    # Put rustup + cargo on install.sh's own PATH for the rest of this run.
    # shellcheck disable=SC1091
    [[ -f "$HOME/.cargo/env" ]] && source "$HOME/.cargo/env"
    if ! command -v rustup >/dev/null 2>&1; then
        err "rustup-init ran but rustup is not on PATH."
        err "Open a new shell (or 'source \$HOME/.cargo/env') and re-run install.sh."
        exit 1
    fi

    ensure_pinned_toolchain   # bug 1
    offer_shell_rc_append     # bug 2

    if ! command -v cargo >/dev/null 2>&1; then
        err "rustup installed but cargo is not on PATH."
        err "Open a new shell (or 'source \$HOME/.cargo/env') and re-run install.sh."
        exit 1
    fi
    RUSTUP_INSTALLED_THIS_RUN=true
    success "rustup + pinned toolchain installed — cargo available"
}

# ---- system C toolchain preflight -------------------------------------------

# Rust crates with C dependencies (openssl-sys, libc, ring, …) need a C compiler
# and the openssl development headers to build; without them the first
# `cargo run` dies with "linker `cc` not found" or an openssl-sys build error.
# Run this BEFORE bootstrap_rustup so rustup-init doesn't first emit its own
# "no default linker (cc) was found" warning. Idempotent: a present toolchain is
# detected and skipped.
ensure_c_toolchain() {
    local missing=()
    command -v cc         >/dev/null 2>&1 || missing+=("C compiler (cc)")
    if command -v pkg-config >/dev/null 2>&1; then
        pkg-config --exists openssl 2>/dev/null || missing+=("openssl development headers")
    else
        missing+=("pkg-config")
        missing+=("openssl development headers")   # can't probe without pkg-config
    fi

    if [[ ${#missing[@]} -eq 0 ]]; then
        success "C build toolchain found (cc, pkg-config, openssl headers)"
        return 0
    fi

    warn "System C toolchain is incomplete — missing: ${missing[*]}."
    warn "Rust crates like libc and openssl-sys need a C compiler and openssl dev headers."

    local do_install=false
    if [[ "$NON_INTERACTIVE" == true ]]; then
        do_install=true
    else
        read -r -p "Install the C build toolchain + openssl dev headers now? [Y/n]: " ans
        [[ -z "$ans" || "${ans,,}" == "y" || "${ans,,}" == "yes" ]] && do_install=true
    fi
    if [[ "$do_install" != true ]]; then
        err "Cannot build Aurora without a C compiler and openssl development headers."
        err "Install them for your distro and re-run install.sh."
        exit 1
    fi

    local distro; distro="$(detect_distro)"
    info "Installing the C build toolchain ($distro) …"
    case "$distro" in
        debian) run_root bash -c "apt-get update && apt-get install -y build-essential pkg-config libssl-dev" ;;
        rhel)   run_root dnf install -y gcc gcc-c++ make openssl-devel pkgconf-pkg-config ;;
        arch)   run_root pacman -S --needed --noconfirm base-devel openssl pkgconf ;;
        *)
            err "Automatic C-toolchain install is not supported on this distro."
            err "Install a C compiler, pkg-config, and the openssl development headers"
            err "(e.g. build-essential + pkg-config + libssl-dev on Debian/Ubuntu), then re-run."
            exit 1 ;;
    esac

    hash -r 2>/dev/null || true   # forget stale command lookups after the install
    if command -v cc >/dev/null 2>&1 && command -v pkg-config >/dev/null 2>&1 \
       && pkg-config --exists openssl 2>/dev/null; then
        success "C build toolchain installed"
    else
        err "The C-toolchain install did not satisfy all prerequisites (cc / pkg-config / openssl headers)."
        err "Install them manually for your distro and re-run install.sh."
        exit 1
    fi
}

# ---- proto-blue-codegen preflight -------------------------------------------

# kryphocron-lexicons' build.rs invokes the proto-blue-codegen binary as a
# subprocess (§5.2 fallback integration path); without it on PATH the workspace
# build fails inside that crate. Install it once cargo is available. The ~0.3.1
# constraint is the one that crate's build.rs error message prescribes.
# Idempotent: a present binary is detected and skipped.
ensure_proto_blue_codegen() {
    if command -v proto-blue-codegen >/dev/null 2>&1; then
        success "proto-blue-codegen found"
        return 0
    fi
    if ! command -v cargo >/dev/null 2>&1; then
        warn "cargo is not available — skipping proto-blue-codegen."
        warn "After you have a Rust toolchain, run: cargo install proto-blue-codegen --version '~0.3.1'"
        return 0
    fi
    info "Installing proto-blue-codegen (required by kryphocron-lexicons build.rs) …"
    cargo install proto-blue-codegen --version '~0.3.1'
    hash -r 2>/dev/null || true   # forget stale command lookups after the install
    if ! command -v proto-blue-codegen >/dev/null 2>&1; then
        err "proto-blue-codegen installed but the binary is not on PATH."
        err "Ensure ~/.cargo/bin is on PATH ('source \$HOME/.cargo/env') and re-run install.sh."
        exit 1
    fi
    success "proto-blue-codegen installed ($(proto-blue-codegen --version 2>/dev/null || echo 'version unknown'))"
}

# ---- Caddy install ----------------------------------------------------------

install_caddy() {
    local distro; distro="$(detect_distro)"
    info "Installing Caddy ($distro) …"
    case "$distro" in
        debian)
            run_root bash -c "
                apt-get install -y debian-keyring debian-archive-keyring apt-transport-https curl gnupg &&
                curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' | gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg &&
                curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' > /etc/apt/sources.list.d/caddy-stable.list &&
                apt-get update &&
                apt-get install -y caddy
            " ;;
        rhel)
            run_root bash -c "
                dnf install -y 'dnf-command(copr)' &&
                dnf copr enable -y @caddy/caddy &&
                dnf install -y caddy
            " ;;
        arch)
            run_root pacman -Sy --noconfirm caddy ;;
        *)
            err "Automatic Caddy install is not supported on this distro."
            err "Install Caddy manually (https://caddyserver.com/docs/install), then"
            err "re-run with --reverse-proxy=caddy."
            return 1 ;;
    esac
}

# ---- reverse-proxy detection + configuration --------------------------------

# Echo which proxy to configure: an active one wins; else the sole installed
# one; else "none" (nothing installed) or "multi:<list>" (several, none active).
detect_reverse_proxy() {
    if svc_active caddy;  then echo caddy;  return; fi
    if svc_active nginx;  then echo nginx;  return; fi
    if svc_active apache2 || svc_active httpd; then echo apache; return; fi

    local installed=()
    command -v caddy >/dev/null 2>&1 && installed+=(caddy)
    command -v nginx >/dev/null 2>&1 && installed+=(nginx)
    { command -v apache2 >/dev/null 2>&1 || command -v httpd >/dev/null 2>&1; } && installed+=(apache)

    case "${#installed[@]}" in
        0) echo none ;;
        1) echo "${installed[0]}" ;;
        *) echo "multi:${installed[*]}" ;;
    esac
}

configure_caddy() {
    if ! command -v caddy >/dev/null 2>&1; then
        install_caddy || { err "Caddy install failed."; exit 1; }
    fi
    local cf=/etc/caddy/Caddyfile block ts=""
    block="$(render_caddy_block)"

    if run_root test -s "$cf"; then
        if run_root grep -qF "${DOMAIN} {" "$cf" 2>/dev/null; then
            warn "Caddyfile already has a block for $DOMAIN — leaving it untouched."
            RP_SUMMARY="Caddy (existing $DOMAIN block kept)"
            return 0
        fi
        ts="$(date +%Y%m%d%H%M%S)"
        run_root cp "$cf" "$cf.bak.$ts"
        warn "Existing Caddyfile backed up to $cf.bak.$ts"
        printf '\n%s\n' "$block" | run_root tee -a "$cf" >/dev/null
    else
        run_root mkdir -p /etc/caddy
        printf '%s\n' "$block" | run_root tee "$cf" >/dev/null
    fi

    if ! run_root caddy validate --config "$cf" 2>/dev/null; then
        err "caddy validate failed for $cf."
        if [[ -n "$ts" ]]; then
            run_root cp "$cf.bak.$ts" "$cf"
            err "Restored the previous Caddyfile from backup."
        fi
        exit 1
    fi

    run_root systemctl daemon-reload || true
    run_root systemctl enable caddy || true
    run_root systemctl reload caddy || run_root systemctl restart caddy || true
    success "Caddy configured for $DOMAIN (automatic Let's Encrypt TLS)."
    RP_SUMMARY="Caddy (automatic Let's Encrypt)"
}

configure_nginx() {
    command -v nginx >/dev/null 2>&1 || { err "nginx not found on PATH."; exit 1; }
    local conf link=""
    if [[ -d /etc/nginx/sites-available ]]; then
        conf=/etc/nginx/sites-available/aurora-locus.conf
        link=/etc/nginx/sites-enabled/aurora-locus.conf
    else
        conf=/etc/nginx/conf.d/aurora-locus.conf
    fi

    render_nginx_conf | run_root tee "$conf" >/dev/null
    [[ -n "$link" ]] && run_root ln -sf "$conf" "$link"

    if ! run_root nginx -t 2>/dev/null; then
        err "nginx -t failed — removing the site config."
        run_root rm -f "$conf"
        [[ -n "$link" ]] && run_root rm -f "$link"
        exit 1
    fi
    run_root systemctl reload nginx || true
    success "nginx site written: $conf"
    RP_SUMMARY="nginx"
    run_root test -d "/etc/letsencrypt/live/$DOMAIN" || RP_CERTBOT_HINT=true
}

configure_apache() {
    local conf reload_svc
    if command -v apache2 >/dev/null 2>&1 || [[ "$(detect_distro)" == debian ]]; then
        conf=/etc/apache2/sites-available/aurora-locus.conf
        reload_svc=apache2
        local m
        for m in proxy proxy_http ssl headers; do run_root a2enmod "$m" >/dev/null 2>&1 || true; done
        render_apache_conf | run_root tee "$conf" >/dev/null
        run_root a2ensite aurora-locus.conf >/dev/null 2>&1 || true
        if ! run_root apache2ctl configtest 2>/dev/null; then
            err "apache2ctl configtest failed — removing the site config."
            run_root rm -f "$conf"; exit 1
        fi
    else
        conf=/etc/httpd/conf.d/aurora-locus.conf
        reload_svc=httpd
        render_apache_conf | run_root tee "$conf" >/dev/null
        if ! run_root httpd -t 2>/dev/null; then
            err "httpd -t failed — removing the site config."
            run_root rm -f "$conf"; exit 1
        fi
    fi
    run_root systemctl reload "$reload_svc" || true
    success "Apache site written: $conf"
    RP_SUMMARY="Apache"
    run_root test -d "/etc/letsencrypt/live/$DOMAIN" || RP_CERTBOT_HINT=true
}

# Orchestrate reverse-proxy setup based on RP_MODE + detection.
setup_reverse_proxy() {
    if [[ "$RP_MODE" == none ]]; then
        RP_SUMMARY="not configured (--reverse-proxy=none)"
        info "Skipping reverse-proxy setup — Aurora will listen on localhost:2583."
        info "Point your own proxy (Traefik, HAProxy, cloud LB, …) at that address."
        return 0
    fi
    if [[ "$DOMAIN" == localhost ]]; then
        RP_SUMMARY="not configured (localhost dev)"
        info "Localhost install — skipping reverse-proxy / TLS setup."
        return 0
    fi

    local choice="$RP_MODE"
    if [[ "$choice" == auto ]]; then
        choice="$(detect_reverse_proxy)"
        if [[ "$choice" == multi:* ]]; then
            local list="${choice#multi:}"
            warn "Multiple reverse proxies installed ($list) and none is active."
            if [[ "$NON_INTERACTIVE" == true ]]; then
                choice="${list%% *}"
                info "Non-interactive: choosing $choice."
            else
                read -r -p "Which should I configure? [caddy/nginx/apache]: " choice
            fi
        fi
    fi

    case "$choice" in
        caddy)  configure_caddy ;;
        nginx)  configure_nginx ;;
        apache) configure_apache ;;
        none)
            info "No reverse proxy detected. Caddy gives you automatic Let's Encrypt TLS on a fresh host."
            local do_it=false
            if [[ "$NON_INTERACTIVE" == true ]]; then
                do_it=true
            else
                read -r -p "Install and configure Caddy now? [Y/n]: " a
                [[ -z "$a" || "${a,,}" == "y" || "${a,,}" == "yes" ]] && do_it=true
            fi
            if [[ "$do_it" == true ]]; then
                configure_caddy
            else
                RP_SUMMARY="not configured"
                warn "No reverse proxy configured — Aurora will listen on localhost:2583 only."
            fi ;;
        *)
            err "Unknown reverse-proxy choice: '$choice' (expected caddy|nginx|apache|none)."
            exit 1 ;;
    esac
}

# ---- locate the repo --------------------------------------------------------

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$SCRIPT_DIR"
ENV_EXAMPLE="$REPO_ROOT/.env.example"
ENV_OUT="$REPO_ROOT/.env"
SETUP_DB="$REPO_ROOT/scripts/setup-database.sh"
SYSTEMD_UNIT="packaging/systemd/aurora-locus.service"

# ---- arguments --------------------------------------------------------------

DOMAIN=""
DATA_DIR=""          # empty = "not set"; prompted (interactive) or defaults to ./data
NON_INTERACTIVE=false
FORCE=false
ADMIN_DID=""
COMPOSE_ENV=true     # set false when the operator declines to overwrite an existing .env
RP_MODE="auto"       # auto | caddy | nginx | apache | none
SKIP_RUSTUP=false
EMIT_PROXY=""        # internal: render a proxy config to stdout and exit

# reverse-proxy result state (for the banner)
RP_SUMMARY="not configured"
RP_CERTBOT_HINT=false

# true once install.sh has installed rustup this run (drives the current-shell
# PATH hint in the banner)
RUSTUP_INSTALLED_THIS_RUN=false

usage() {
    cat <<EOF
Aurora Locus installer — composes .env from .env.example and brings up the
native deploy path (Rust toolchain + reverse-proxy TLS) to Docker parity.

Usage: ./install.sh [options]

  --domain DOMAIN         Public domain for this PDS (e.g. pds.example.com).
                          Derives the identity keys and drives reverse-proxy
                          config. Omit for a localhost dev install (no TLS).
  --data-dir PATH         Data directory (default: ./data).
  --admin-did DID         DID to print grant-admin bootstrap instructions for.
  --reverse-proxy MODE    auto (default) | caddy | nginx | apache | none.
                          auto detects an installed/active proxy, or offers to
                          install Caddy on a fresh host. none skips TLS setup.
  --no-caddy              Alias for --reverse-proxy=none.
  --skip-rustup           Do not offer to install rustup if it is missing.
  --non-interactive       Never prompt; accept installs, use defaults / flags.
                          Requires --force to overwrite an existing .env.
  --force                 Overwrite an existing .env (backs it up to .env.bak).
  --emit-proxy-config T   Print the reverse-proxy config (T = caddy|nginx|apache)
                          for --domain to stdout and exit (preview; no changes).
  -h, --help              Show this help.
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --domain)              DOMAIN="${2:?--domain needs a value}"; shift 2 ;;
        --data-dir)            DATA_DIR="${2:?--data-dir needs a value}"; shift 2 ;;
        --admin-did)           ADMIN_DID="${2:?--admin-did needs a value}"; shift 2 ;;
        --reverse-proxy)       RP_MODE="${2:?--reverse-proxy needs a value}"; shift 2 ;;
        --reverse-proxy=*)     RP_MODE="${1#*=}"; shift ;;
        --no-caddy)            RP_MODE="none"; shift ;;
        --skip-rustup)         SKIP_RUSTUP=true; shift ;;
        --non-interactive)     NON_INTERACTIVE=true; shift ;;
        --force)               FORCE=true; shift ;;
        --emit-proxy-config)   EMIT_PROXY="${2:?--emit-proxy-config needs a value}"; shift 2 ;;
        --emit-proxy-config=*) EMIT_PROXY="${1#*=}"; shift ;;
        -h|--help)             usage; exit 0 ;;
        *) err "Unknown option: $1"; echo; usage; exit 1 ;;
    esac
done

case "$RP_MODE" in
    auto|caddy|nginx|apache|none) : ;;
    *) err "Invalid --reverse-proxy '$RP_MODE' (expected auto|caddy|nginx|apache|none)."; exit 1 ;;
esac

# ---- preview short-circuit --------------------------------------------------
# `--emit-proxy-config` renders a config and exits before any prereqs or writes.

if [[ -n "$EMIT_PROXY" ]]; then
    [[ -z "$DOMAIN" ]] && DOMAIN="example.com"
    case "$EMIT_PROXY" in
        caddy)  render_caddy_block ;;
        nginx)  render_nginx_conf ;;
        apache) render_apache_conf ;;
        *) err "Unknown --emit-proxy-config type '$EMIT_PROXY' (expected caddy|nginx|apache)."; exit 1 ;;
    esac
    exit 0
fi

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

# System C toolchain (cc + pkg-config + openssl headers): needed to build crates
# with C dependencies. Done BEFORE rustup so rustup-init doesn't emit its own
# "no default linker (cc) was found" warning the operator would have to ignore.
ensure_c_toolchain

# Rust toolchain: bootstrap rustup if missing (unless --skip-rustup). The pinned
# toolchain in rust-toolchain.toml is fetched on the first cargo invocation.
bootstrap_rustup

# proto-blue-codegen: build-time codegen binary kryphocron-lexicons' build.rs
# invokes; install once cargo is available so the first `cargo build` succeeds.
ensure_proto_blue_codegen

if command -v docker >/dev/null 2>&1; then
    info "docker also present — the docker-compose path is available as an alternative."
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

# Data directory: prompt when interactive and not supplied via --data-dir; empty
# defaults to ./data. Mirrors the domain-prompt pattern.
if [[ -z "$DATA_DIR" && "$NON_INTERACTIVE" == false ]]; then
    echo
    info "Data directory for databases, blobs and the actor store."
    read -r -p "Data directory [./data]: " DATA_DIR
fi
if [[ -z "$DATA_DIR" ]]; then
    DATA_DIR="./data"
fi
info "Data directory: $DATA_DIR"

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
            COMPOSE_ENV=false
            info "Keeping existing .env — skipping .env composition step."
            # Prefer the existing .env's data directory so the rest of the
            # install (dir prep, final banner) reflects what the server will use.
            existing_dd="$(grep -E '^PDS_DATA_DIRECTORY=' "$ENV_OUT" | tail -1 | cut -d= -f2- || true)"
            if [[ -n "$existing_dd" ]]; then
                DATA_DIR="$existing_dd"
                info "Using PDS_DATA_DIRECTORY from the existing .env: $DATA_DIR"
            fi
        else
            cp "$ENV_OUT" "$ENV_OUT.bak"
            warn "Existing .env backed up to .env.bak"
        fi
    fi
fi

# ---- compose .env (skipped when keeping an existing .env) -------------------

if [[ "$COMPOSE_ENV" == true ]]; then

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
    OVERRIDE[PDS_SERVICE_PUBLIC_URL]="https://$DOMAIN"
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

else
    info "Existing .env left in place (composition skipped)."
fi

# ---- data directory + schema ------------------------------------------------

echo
if [[ -x "$SETUP_DB" ]]; then
    info "Preparing the data directory via scripts/setup-database.sh …"
    "$SETUP_DB" --data-dir "$DATA_DIR" --non-interactive
else
    warn "scripts/setup-database.sh not found or not executable — creating $DATA_DIR directly."
    mkdir -p "$DATA_DIR"
fi

# ---- reverse proxy / TLS ----------------------------------------------------

echo
setup_reverse_proxy

# ---- final instructions -----------------------------------------------------

header "Install complete"

echo "Configuration:"
if [[ "$COMPOSE_ENV" == true ]]; then
    echo "  .env           $ENV_OUT   (chmod 600)"
else
    echo "  .env           $ENV_OUT   (kept existing — not modified)"
fi
echo "  data directory $DATA_DIR"
echo "  domain         $DOMAIN"
echo "  reverse proxy  $RP_SUMMARY"
echo
if [[ "$RUSTUP_INSTALLED_THIS_RUN" == true ]]; then
    info "cargo was just installed. To use it in THIS shell, run:"
    echo "  source \"\$HOME/.cargo/env\""
    info "(new shells pick it up automatically if you let install.sh update ~/.bashrc.)"
    echo
fi
info "Verify the configuration before first boot:"
echo "  cargo run --release -- validate-config"
echo
info "Start the server (schema migrations run automatically on first boot):"
echo "  cargo run --release"
echo
info "Or run it as a systemd service (recommended for production):"
echo "  sudo cp $SYSTEMD_UNIT /etc/systemd/system/"
echo "  sudo systemctl daemon-reload"
echo "  sudo systemctl enable --now aurora-locus"
echo "  # (review the unit's User / WorkingDirectory / paths first)"
echo
if [[ "$RP_CERTBOT_HINT" == true ]]; then
    info "Provision a TLS certificate (nginx/Apache don't do it automatically):"
    echo "  sudo certbot --nginx -d $DOMAIN     # or --apache"
    echo
fi
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
if [[ "$RP_SUMMARY" == "not configured"* && "$DOMAIN" != "localhost" ]]; then
    warn "No reverse proxy was configured. Aurora listens on localhost:2583 —"
    warn "terminate TLS and forward to that address with your proxy of choice."
fi
