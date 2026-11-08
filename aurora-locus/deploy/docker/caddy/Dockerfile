# locus-caddy: Caddy with the common DNS provider modules, for DNS-01
# wildcard certificates (the stock image has none). Every module listed takes
# one API token, which the generated Caddyfile reads from DNS_API_TOKEN:
#
#   docker build -t locus-caddy deploy/docker/caddy
#
# For a provider that needs more than a token, add its module here and edit
# the `dns` line of the Caddyfile.

FROM caddy:2-builder AS build
RUN xcaddy build \
    --with github.com/caddy-dns/cloudflare \
    --with github.com/caddy-dns/digitalocean \
    --with github.com/caddy-dns/hetzner \
    --with github.com/caddy-dns/duckdns

FROM caddy:2
COPY --from=build /usr/bin/caddy /usr/bin/caddy
