# Running aurora-locus with Docker

The installer (`deploy/install.sh`) asks a few questions, writes three
files and runs Docker Compose. This guide does the same by hand, for when
you want to see or control each step, keep the files in your own
configuration management, or run on a machine where the installer's
choices don't fit.

Every command below runs from `aurora-locus/deploy/docker/` in a clone of
this repository.

## What you need

- Linux with Docker and the Compose v2 plugin (`docker compose version`).
- Ports 80 and 443 free, for Caddy.
- A hostname for the PDS (`pds.example.com`) with DNS pointing at the
  server, and the handle domain's wildcard (`*.pds.example.com`) too.

## The stack

`compose.yaml` runs two containers on their own Docker network:

| Service | Does | Publishes |
|---|---|---|
| `caddy` | HTTPS, certificates, forwarding to locus | 80, 443, 443/udp |
| `locus` | The PDS | 3000 on `127.0.0.1` only, for the CLI over HTTP |

Caddy has a fixed address on the network (`172.30.53.2` by default), and
locus trusts forwarded client addresses from that one address only.
Connections through a published port arrive from the network's gateway, so
the gateway is never trusted.

## 1. `locus.env`

The PDS's environment. The minimum:

```sh
PDS_HOSTNAME=pds.example.com
PDS_SERVICE_HANDLE_DOMAINS=.pds.example.com
LOCUS_TRUSTED_PROXIES=172.30.53.2/32
LOCUS_PROXY_MODE=caddy
```

- `PDS_SERVICE_HANDLE_DOMAINS`: where handles go. `.pds.example.com` gives
  `alice.pds.example.com`; `.example.com` gives `alice.example.com`.
- `LOCUS_PROXY_MODE`: `caddy`, or `cloudflare` behind Cloudflare's proxy.
- Any other `PDS_*` or `LOCUS_*` variable can go here too. An existing
  reference `pds.env` works as it is: add the two `LOCUS_` lines.
  [Operations](design/operations.md#configuration) lists them all.

Keep it readable only by root (`chmod 600 locus.env`) once it holds
secrets such as `PDS_EMAIL_SMTP_URL`.

For blobs in S3 instead of the data volume, add these before the first
start:

```sh
PDS_BLOBSTORE_S3_BUCKET=my-pds-blobs
PDS_BLOBSTORE_S3_REGION=us-east-1
PDS_BLOBSTORE_S3_ENDPOINT=https://s3.example.com   # S3-compatible services only
PDS_BLOBSTORE_S3_FORCE_PATH_STYLE=true             # most S3-compatible services
PDS_BLOBSTORE_S3_ACCESS_KEY_ID=…
PDS_BLOBSTORE_S3_SECRET_ACCESS_KEY=…
```

## 2. `.env` (optional)

Compose reads this file for the stack's own settings. Every line is
optional:

```sh
LOCUS_LOCAL_PORT=3001                  # if 3000 is taken on this machine
LOCUS_SUBNET=172.31.99.0/24            # if 172.30.53.0/24 clashes with another network
LOCUS_GATEWAY=172.31.99.1
LOCUS_IP_RANGE=172.31.99.128/25        # the upper half: containers' dynamic addresses
LOCUS_CADDY_IP=172.31.99.2             # Caddy's address; LOCUS_TRUSTED_PROXIES must match
LOCUS_IMAGE=ghcr.io/dollspace-gay/aurora-locus:0.1.2   # pin an exact version
COMPOSE_FILE=compose.yaml:compose.build.yaml           # build from source (step 3)
CADDY_IMAGE=locus-caddy                # DNS-01 wildcards (see below)
DNS_API_TOKEN=…                        # the DNS provider's token, for DNS-01
```

If you change the network, change `LOCUS_TRUSTED_PROXIES` in `locus.env`
to `<LOCUS_CADDY_IP>/32`.

## 3. Get the image

```sh
docker compose pull locus
```

This pulls `ghcr.io/dollspace-gay/aurora-locus:0.1`, the published image.
`0.1` follows the 0.1.x patch releases; set `LOCUS_IMAGE` in `.env` to pin
an exact version (`ghcr.io/dollspace-gay/aurora-locus:0.1.2`) or to use
another registry.

**To build it from the source instead**, add `compose.build.yaml` to the
compose commands. Set it once in `.env` so every command below uses it:

```sh
COMPOSE_FILE=compose.yaml:compose.build.yaml
```

```sh
docker compose build locus
```

The first build takes a while (twenty minutes or so on a small server).
Later builds reuse cargo's caches and recompile only what changed.

## 4. The Caddyfile

Locus writes it. Pick one of three ways to get certificates.

**On demand** (simplest; fine for small and medium servers). Caddy gets one
certificate per name from Let's Encrypt, asking locus first which names are
allowed:

```sh
docker compose run --rm --no-deps --entrypoint aurora-locus locus \
  deploy caddyfile --strategy on-demand --email you@example.com > Caddyfile
```

**A DNS-01 wildcard** (for busy servers: Let's Encrypt limits new
certificates per domain per week). Needs Caddy with your DNS provider's
module and an API token:

```sh
docker build -t locus-caddy caddy
docker compose run --rm --no-deps --entrypoint aurora-locus locus \
  deploy caddyfile --strategy dns01 --email you@example.com \
  --hostname pds.example.com --wildcard pds.example.com --dns-provider cloudflare > Caddyfile
```

Then set `CADDY_IMAGE=locus-caddy` and `DNS_API_TOKEN` in `.env`. The
bundled providers are cloudflare, digitalocean, hetzner and duckdns.

**Behind Cloudflare's proxy** (orange cloud). Create an Origin CA
certificate in Cloudflare (SSL/TLS > Origin Server) covering the hostname
and the handle wildcard, set SSL/TLS to Full (strict), and save the
certificate and key as `certs/origin.pem` and `certs/origin.key`:

```sh
mkdir -p certs
chmod 0644 certs/origin.pem
chmod 0640 certs/origin.key
docker compose run --rm --no-deps --entrypoint aurora-locus locus \
  deploy caddyfile --strategy cloudflare --hostname pds.example.com \
  --wildcard pds.example.com --cert /certs/origin.pem --key /certs/origin.key > Caddyfile
```

and set `LOCUS_PROXY_MODE=cloudflare` in `locus.env`. In Cloudflare, also
turn on WebSockets and bypass the cache for `/xrpc/*`, `/oauth/*`,
`/admin/*` and `/blob/*`.

## 5. Start

```sh
docker compose up -d
docker compose ps
```

Both containers should be up, and locus healthy within half a minute.
`docker compose logs -f locus` shows what it's doing.

## 6. Setup

Locus starts in setup mode. Open `https://pds.example.com/setup` and enter
the one-time setup token:

```sh
docker compose exec locus cat /data/setup-token
```

The wizard asks for the first operator account, email, the network services
and the keys, and offers a key backup. Download it and keep it off the
server.

To set up without the wizard instead, put the answers in `locus.env`
(`LOCUS_SETUP_AUTO=true` and the values in
[Headless setup](design/operations.md#headless-setup)) before the first
start, or run `docker compose exec locus aurora-locus setup` for the same
questions in the terminal.

## Day to day

```sh
docker compose exec locus aurora-locus status          # nodes, accounts, jobs
docker compose exec locus aurora-locus --help          # every operator command
docker compose logs -f locus
```

The console is at `https://pds.example.com/admin`.

**Changing `locus.env`.** Run `docker compose up -d`, which recreates the
container with the new environment. `docker compose restart` keeps the old
one.

**Upgrading.** Pull the new image and recreate:

```sh
docker compose pull locus
docker compose up -d
```

If you pinned an exact version, change `LOCUS_IMAGE` first. Building from
source: `git pull`, `docker compose build locus`, `docker compose up -d`.

Database migrations run at start. A node refuses a database newer than it
knows, so an accidental downgrade stops instead of damaging data.

**Backups.** `docker compose exec locus aurora-locus backup create --wait`
writes one to `/data/backups` in the data volume; `backups.every_hours`
schedules them. Copy them off the server. [Operations](design/operations.md#backups-and-restore)
covers restore.

## Postgres on the same machine

`compose.postgres.yaml` adds a Postgres container. In `.env`:

```sh
POSTGRES_PASSWORD=<a long random password>
```

In `locus.env`, before the first start:

```sh
LOCUS_DATABASE_URL=postgres://locus:<the same password>@postgres:5432/locus
LOCUS_MASTER_KEY_SOURCE=env
LOCUS_MASTER_KEY=<openssl rand -hex 32>
```

A master key file is generated automatically only with SQLite, so with
Postgres you supply the key. Keep a copy of it somewhere safe: every stored
secret and signing key is encrypted under it.

Then use both files in every command:

```sh
docker compose -f compose.yaml -f compose.postgres.yaml up -d
docker compose -f compose.yaml -f compose.postgres.yaml exec locus aurora-locus status
```

To move an existing SQLite install to Postgres, use
`aurora-locus migrate storage` instead ([Migrations](design/operations.md#sqlite-to-postgres)).

## Your own reverse proxy instead of Caddy

Run only the locus service and point your proxy at `127.0.0.1:3000` (or
`LOCUS_LOCAL_PORT`):

```sh
docker compose up -d locus
```

Connections through the published port arrive from the network's gateway,
so here trust the gateway, and nothing else, with
`LOCUS_TRUSTED_PROXIES=172.30.53.1/32` and `LOCUS_PROXY_MODE=other`. The
port is published on `127.0.0.1` only, so only this machine reaches it.
`deploy/proxies/README.md` has nginx, Traefik and HAProxy examples; whatever
the proxy, keep the `Host` header, forward the client address, allow
WebSockets and don't cap request bodies below the blob upload limit.

## Many PDSes or several nodes

For several locus nodes behind one load balancer (Postgres, S3 and an
optional Redis shared by all), see [Running on Kubernetes](kubernetes.md);
the same settings apply to any orchestrator.
