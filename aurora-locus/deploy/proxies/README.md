# Other proxies

aurora-locus listens on plain HTTP (port 3000 by default) and expects a
proxy in front for TLS. Whatever the proxy:

- **Trust only the proxy.** Set `LOCUS_TRUSTED_PROXIES` (or the
  `http.trusted_proxies` setting) to the proxy's own address: `127.0.0.1/32`
  for one on the same machine (the default trusts loopback already).
  Forwarded headers from anyone else are ignored, so no one can claim
  another client's address for rate limits.
- **Forward the client and scheme.** Send `X-Forwarded-For` (appended, not
  replaced) and `X-Forwarded-Proto: https`.
- **Keep the Host.** Pass the request's `Host` through unchanged: locus
  routes by it (handles get only their well-known documents).
- **WebSockets.** `/xrpc/com.atproto.sync.subscribeRepos` is a WebSocket;
  allow upgrades and long-lived connections.
- **Body size.** Uploads go up to `blobs.upload_limit` and imports to
  `limits.import_size`; don't cap bodies below those.
- **Certificates.** Handles are subdomains, so either issue certificates
  on demand, asking `GET /tls-check?domain=<name>` first (200 means yes,
  anything else no), or use a wildcard for the handle domain.

## Caddy on the same machine

```
aurora-locus deploy caddyfile --email you@example.com \
  --upstream 127.0.0.1:3000 --ask-url http://127.0.0.1:3000/tls-check \
  > /etc/caddy/Caddyfile
systemctl reload caddy
```

`--strategy dns01 --hostname pds.example.com --wildcard example.com
--dns-provider cloudflare` gives a wildcard instead (Caddy needs the
provider's module; `DNS_API_TOKEN` in its environment).

## nginx

nginx can't issue certificates on demand, so use a wildcard for the PDS
hostname and the handle domain (for example certbot with a DNS plugin:
`certbot certonly --dns-cloudflare -d pds.example.com -d '*.example.com'`).

```nginx
map $http_upgrade $connection_upgrade {
    default upgrade;
    ''      close;
}

server {
    listen 443 ssl;
    listen [::]:443 ssl;
    http2 on;
    server_name pds.example.com *.example.com;

    ssl_certificate     /etc/letsencrypt/live/pds.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/pds.example.com/privkey.pem;

    client_max_body_size 300m;
    proxy_read_timeout 1h;

    location / {
        proxy_pass http://127.0.0.1:3000;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto https;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection $connection_upgrade;
        proxy_buffering off;
    }
}

server {
    listen 80;
    listen [::]:80;
    server_name pds.example.com *.example.com;
    return 301 https://$host$request_uri;
}
```

## Traefik (Docker labels)

Traefik has no on-demand issuance; use its DNS challenge for a wildcard.
On the locus container:

```yaml
labels:
  - traefik.enable=true
  - traefik.http.routers.locus.rule=Host(`pds.example.com`) || HostRegexp(`^.+\.example\.com$`)
  - traefik.http.routers.locus.entrypoints=websecure
  - traefik.http.routers.locus.tls.certresolver=dns
  - traefik.http.routers.locus.tls.domains[0].main=example.com
  - traefik.http.routers.locus.tls.domains[0].sans=*.example.com,pds.example.com
  - traefik.http.services.locus.loadbalancer.server.port=3000
```

with a `dns` certificate resolver using your DNS provider, and
`LOCUS_TRUSTED_PROXIES` set to Traefik's address on the shared network
(give Traefik a fixed address, as `deploy/docker/compose.yaml` does for
Caddy). Traefik appends `X-Forwarded-For` and sets `X-Forwarded-Proto`
itself.

## HAProxy

Terminate TLS with a wildcard certificate (cert and key in one PEM):

```
frontend https
    bind :443 ssl crt /etc/haproxy/certs/example.com.pem alpn h2,http/1.1
    bind :80
    http-request redirect scheme https unless { ssl_fc }
    option forwardfor
    http-request set-header X-Forwarded-Proto https
    timeout client 1h
    default_backend locus

backend locus
    timeout server 1h
    timeout tunnel 1h
    server locus 127.0.0.1:3000
```

## Cloudflare

In front of any of these, set `LOCUS_PROXY_MODE=cloudflare` (or choose it
in setup): locus then takes `CF-Connecting-IP` as the client, but only when
the hop before your proxy is one of Cloudflare's addresses (kept up to date
daily). Set Cloudflare's SSL/TLS mode to Full (strict), and have your proxy
trust Cloudflare's ranges for `X-Forwarded-For`
(`aurora-locus deploy caddyfile --strategy cloudflare` does this for
Caddy).
