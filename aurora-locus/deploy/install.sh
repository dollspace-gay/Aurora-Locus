#!/usr/bin/env bash
# Installs aurora-locus on a fresh server with Docker and Caddy: asks for the
# hostname, the ACME contact email and how certificates are made, writes
# deploy/docker/locus.env and the Caddyfile, and starts the stack, so HTTPS
# works before setup. The setup wizard (or `aurora-locus setup`) asks the
# rest. Run it as root from the repository's aurora-locus/ directory:
#
#   sudo bash deploy/install.sh
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
docker_dir="$here/docker"
cd "$docker_dir"

say() { printf '%s\n' "$*"; }
ask() {
  local prompt="$1" default="${2:-}" reply
  if [[ -n "$default" ]]; then
    read -r -p "$prompt [$default]: " reply
    printf '%s' "${reply:-$default}"
  else
    read -r -p "$prompt: " reply
    printf '%s' "$reply"
  fi
}

if [[ $EUID -ne 0 ]]; then
  say "Run this as root (sudo bash deploy/install.sh)."
  exit 1
fi

if [[ -f locus.env ]]; then
  say "deploy/docker/locus.env exists: this server is already installed."
  say "To change how certificates are made, see: aurora-locus deploy caddyfile --help"
  exit 1
fi

# Docker.
if ! command -v docker >/dev/null 2>&1; then
  say "Installing Docker…"
  curl -fsSL https://get.docker.com | sh
fi
if ! docker compose version >/dev/null 2>&1; then
  say "Docker Compose v2 is needed (the docker compose plugin)."
  exit 1
fi

# Ports, before the long build: Caddy needs 80 and 443; locus's local port
# (3000 on 127.0.0.1, for the CLI) can move.
in_use() { [[ -n "$(ss -Hltn "sport = :$1" 2>/dev/null)" ]]; }
for port in 80 443; do
  if in_use "$port"; then
    say "Port $port is in use (sudo ss -ltnp 'sport = :$port' shows by what);"
    say "Caddy needs it. Stop that service and run this again."
    exit 1
  fi
done
local_port=3000
while in_use "$local_port"; do
  say "Port $local_port on this machine is in use."
  local_port="$(ask "Another local port for locus" $((local_port + 1)))"
done

# The questions.
hostname="$(ask "The PDS hostname (like pds.example.com)")"
hostname="${hostname,,}"
hostname="${hostname%.}"
if [[ ! "$hostname" =~ ^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$ ]]; then
  say "$hostname isn't a hostname."
  exit 1
fi
# Handles go under the hostname by default, as the reference's installer
# (alice.pds.example.com, or alice.example.com for a PDS at example.com).
domain="$(ask "The handle domain (handles are name.<domain>)" "$hostname")"
domain="${domain,,}"
domain="${domain#.}"
domain="${domain#\*.}"
domain="${domain%.}"
if [[ ! "$domain" =~ ^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$ ]]; then
  say "$domain isn't a domain."
  exit 1
fi
if [[ "$domain" == "$hostname" ]]; then
  cert_names="$hostname and *.$hostname"
else
  cert_names="$hostname, $domain and *.$domain"
fi
email="$(ask "An email address for certificate notices (Let's Encrypt)")"

say ""
say "How should certificates be made?"
say "  1) On demand, one per name (the default; fine for small and medium servers)"
say "  2) A wildcard by DNS-01 (*.$domain; for large servers; needs a DNS provider API token)"
say "  3) Behind Cloudflare's proxy (orange cloud), with a Cloudflare Origin CA certificate"
strategy="$(ask "1, 2 or 3" 1)"

subnet="$(ask "The Docker network for locus and Caddy" 172.30.53.0/24)"
base="${subnet%.*}"
caddy_ip="$base.2"

mkdir -p certs
caddy_image="caddy:2"
dns_token=""
case "$strategy" in
  1)
    args=(--strategy on-demand --email "$email")
    proxy_mode=caddy
    ;;
  2)
    say "DNS providers built in: cloudflare, digitalocean, hetzner, duckdns."
    provider="$(ask "Your DNS provider" cloudflare)"
    read -r -s -p "Its API token (with DNS edit rights): " dns_token; echo
    say "Building Caddy with the DNS provider modules…"
    docker build -t locus-caddy "$docker_dir/caddy"
    caddy_image="locus-caddy"
    args=(--strategy dns01 --email "$email" --hostname "$hostname" --wildcard "$domain" --dns-provider "$provider")
    proxy_mode=caddy
    ;;
  3)
    say "Create a Cloudflare Origin CA certificate for $cert_names"
    say "(SSL/TLS > Origin Server > Create Certificate), and set SSL/TLS to Full (strict)."
    say "Paste the certificate (PEM), then an empty line:"
    : > certs/origin.pem
    while IFS= read -r line && [[ -n "$line" ]]; do printf '%s\n' "$line" >> certs/origin.pem; done
    say "Paste the private key (PEM), then an empty line:"
    : > certs/origin.key
    while IFS= read -r line && [[ -n "$line" ]]; do printf '%s\n' "$line" >> certs/origin.key; done
    chmod 0644 certs/origin.pem
    chmod 0640 certs/origin.key
    if ! grep -q "BEGIN CERTIFICATE" certs/origin.pem || ! grep -q "PRIVATE KEY" certs/origin.key; then
      say "That doesn't look like a PEM certificate and key."
      exit 1
    fi
    args=(--strategy cloudflare --hostname "$hostname" --wildcard "$domain" --cert /certs/origin.pem --key /certs/origin.key)
    proxy_mode=cloudflare
    ;;
  *)
    say "Pick 1, 2 or 3."
    exit 1
    ;;
esac

# The environment the stack reads.
umask 077
cat > .env <<EOF
LOCUS_SUBNET=$subnet
LOCUS_GATEWAY=$base.1
LOCUS_IP_RANGE=$base.128/25
LOCUS_CADDY_IP=$caddy_ip
LOCUS_LOCAL_PORT=$local_port
CADDY_IMAGE=$caddy_image
DNS_API_TOKEN=$dns_token
EOF
cat > locus.env <<EOF
PDS_HOSTNAME=$hostname
PDS_SERVICE_HANDLE_DOMAINS=.$domain
LOCUS_TRUSTED_PROXIES=$caddy_ip/32
LOCUS_PROXY_MODE=$proxy_mode
# Blobs in an S3-compatible bucket instead of the data volume (set these
# before the first start; an install with blobs on disk moves them with
# \`aurora-locus migrate blobs\`):
# PDS_BLOBSTORE_S3_BUCKET=
# PDS_BLOBSTORE_S3_REGION=
# PDS_BLOBSTORE_S3_ENDPOINT=
# PDS_BLOBSTORE_S3_FORCE_PATH_STYLE=true
# PDS_BLOBSTORE_S3_ACCESS_KEY_ID=
# PDS_BLOBSTORE_S3_SECRET_ACCESS_KEY=
# LOCUS_BLOBSTORE_S3_GATED_BUCKET=
EOF
umask 022

say "Building aurora-locus…"
docker compose build locus
docker compose run --rm --no-deps --entrypoint aurora-locus locus deploy caddyfile "${args[@]}" > Caddyfile

say "Starting…"
docker compose up -d
for _ in $(seq 1 60); do
  if docker compose exec -T locus aurora-locus healthcheck >/dev/null 2>&1; then break; fi
  sleep 2
done

say ""
say "aurora-locus is running. Point $hostname's DNS at this server"
[[ "$strategy" == 3 ]] && say "(a proxied record at Cloudflare)"
say "and finish setup at:  https://$hostname/setup"
say ""
say "The one-time setup token is in the log and in the data volume:"
say "  docker compose -f $docker_dir/compose.yaml exec locus cat /data/setup-token"
