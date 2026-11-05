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

# The questions.
hostname="$(ask "The PDS hostname (like pds.example.com)")"
hostname="${hostname,,}"
hostname="${hostname%.}"
if [[ ! "$hostname" =~ ^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$ ]]; then
  say "$hostname isn't a hostname."
  exit 1
fi
parent="${hostname#*.}"
email="$(ask "An email address for certificate notices (Let's Encrypt)")"

say ""
say "How should certificates be made?"
say "  1) On demand, one per name (the default; fine for small and medium servers)"
say "  2) A wildcard by DNS-01 (*.$parent; for large servers; needs a DNS provider API token)"
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
    wildcard="$(ask "The wildcard's domain (handles go under it)" "$parent")"
    read -r -s -p "Its API token (with DNS edit rights): " dns_token; echo
    say "Building Caddy with the DNS provider modules…"
    docker build -t locus-caddy "$docker_dir/caddy"
    caddy_image="locus-caddy"
    args=(--strategy dns01 --email "$email" --hostname "$hostname" --wildcard "$wildcard" --dns-provider "$provider")
    proxy_mode=caddy
    ;;
  3)
    say "Create a Cloudflare Origin CA certificate for $parent and *.$parent"
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
    args=(--strategy cloudflare --hostname "$hostname" --wildcard "$parent" --cert /certs/origin.pem --key /certs/origin.key)
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
CADDY_IMAGE=$caddy_image
DNS_API_TOKEN=$dns_token
EOF
cat > locus.env <<EOF
PDS_HOSTNAME=$hostname
PDS_SERVICE_HANDLE_DOMAINS=.$parent
LOCUS_TRUSTED_PROXIES=$caddy_ip/32
LOCUS_PROXY_MODE=$proxy_mode
EOF
umask 022
# Caddy's address on the network, to match LOCUS_TRUSTED_PROXIES.
sed -i "s/ipv4_address: 172.30.53.2/ipv4_address: $caddy_ip/" compose.yaml

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
