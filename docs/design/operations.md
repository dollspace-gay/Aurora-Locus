# Operating aurora-locus

This document covers how to configure, set up, deploy and run aurora-locus
("locus"), an atproto PDS written in Rust. It starts with a quick start using
the installer, then goes through each part in more detail.

Locus aims to be a drop-in replacement for Bluesky's reference PDS
(`@atproto/pds`). It reads the reference's `PDS_*` environment variables, so
an existing `pds.env` boots unchanged, and it can import a reference
installation in place. Where locus works differently, this document says so
and says why.

## Quick start

You need a fresh Linux server with ports 80 and 443 free, a hostname for the
PDS (for example `pds.example.com`) and a DNS record you can point at the
server.

Clone this repository on the server, then run the installer from its
`aurora-locus/` directory:

```sh
cd aurora-locus
sudo bash deploy/install.sh
```

The installer:

1. Installs Docker if it is missing, and checks for Docker Compose v2.
2. Checks the ports before the long build. It stops if 80 or 443 is in use
   (Caddy needs them). If 3000 is in use on this machine, it asks for another
   local port for locus (`LOCUS_LOCAL_PORT`).
3. Asks for:
   - the PDS hostname (`pds.example.com`);
   - the handle domain, defaulting to the hostname, so handles look like
     `alice.pds.example.com`, as with the reference's installer. Enter
     `example.com` instead for handles like `alice.example.com`;
   - an email address for certificate notices from Let's Encrypt;
   - how certificates are made: on demand (the default), a DNS-01 wildcard,
     or Cloudflare's proxy with an Origin CA certificate (see
     [Certificates](#certificates));
   - the Docker network for locus and Caddy (default `172.30.53.0/24`).
4. Writes `deploy/docker/locus.env`, `deploy/docker/.env` and the
   `Caddyfile`, builds the image, and starts the stack.

When it finishes, point the hostname's DNS at the server (a proxied record,
if you use Cloudflare) and open the setup wizard:

```
https://pds.example.com/setup
```

The wizard asks for a one-time setup token. It is in the log and in the data
volume:

```sh
docker compose -f deploy/docker/compose.yaml exec locus cat /data/setup-token
```

HTTPS works before the wizard opens, in every certificate mode, because the
installer collects the hostname up front and Caddy is allowed to get a
certificate for it while locus is in setup mode.

The installer refuses to run a second time once `locus.env` exists. To
change how certificates are made later, see
[Changing the certificate strategy](#changing-the-certificate-strategy).

## Configuration

### Two layers

Locus splits configuration into two layers:

| Layer | Stored in | Changed by | Takes effect |
|---|---|---|---|
| Bootstrap | `locus.toml` and environment variables | Editing the file or env, then restarting | On restart |
| Runtime settings | The database | The console, the CLI, the admin API | Live, on every node |

The bootstrap layer is small on purpose. It holds only what a node needs
before it can reach the database, or what can't safely change in a running
process: the database URL, the listen address, the data directory, the
master key source, the log format, the Redis URL, the role, the initial
hostname, dev mode, and direct TLS. Everything else is a runtime setting:
handle domains, AppView and other services, crawlers, moderation services,
SMTP, rate limits, body and blob limits, sign-up policy, invites, branding,
the private tier, proxy trust, metrics access, backups, log level, and more.

Runtime settings are validated before they are saved, audited, and applied
on every node without a restart. A request reads one snapshot of the
settings from start to end, so a change never applies halfway through it.
A few settings (the S3 bucket settings, for example) say "takes effect on
restart" in their description.

### Bootstrap settings

Bootstrap values come from, lowest precedence first: built-in defaults,
`locus.toml` (or the file named by `LOCUS_CONFIG`), the reference's `PDS_*`
variables, then `LOCUS_*` variables.

| Env variable | `locus.toml` key | Default | Meaning |
|---|---|---|---|
| `LOCUS_DATABASE_URL` | `database_url` | `sqlite://<data>/locus.sqlite` | `sqlite://…` or `postgres://…` |
| `LOCUS_LISTEN` | `listen` | `0.0.0.0:<port>` | Listen address |
| `LOCUS_PORT` (`PDS_PORT`) | — | `2583` (the image sets `3000`) | Port, when no listen address is given |
| `LOCUS_DATA_DIR` (`PDS_DATA_DIRECTORY`) | `data_dir` | `./data` | Data directory |
| `LOCUS_DATA_PERSISTENT` | `data_persistent` | `true` | Set `false` where the data directory doesn't survive restarts |
| `LOCUS_ADMIN_SOCKET` | `admin_socket` | `<data>/admin.sock` | The CLI's Unix socket |
| `LOCUS_MASTER_KEY_SOURCE` | `master_key_source` | `file` | `file`, `env` or `kms` |
| `LOCUS_MASTER_KEY_FILE` | `master_key_file` | `<data>/master.key` | The key file (`file` source) |
| `LOCUS_MASTER_KEY_SHARED_FILE` | `master_key_shared_file` | `false` | The key file is supplied by you and shared by every node |
| `LOCUS_MASTER_KEY` | — | — | The key, as hex (`env` source) |
| `LOCUS_MASTER_KEY_KMS_ID` | `master_key_kms_id` | — | The KMS key (`kms` source) |
| `LOCUS_NODE_ID` | `node_id` | generated once | A node's id |
| `LOCUS_LOG_FORMAT` | `log_format` | `pretty` (the image sets `json`) | `pretty` or `json` |
| `LOCUS_REDIS_URL` (`PDS_REDIS_SCRATCH_ADDRESS`) | `redis_url` | — | Optional Redis for rate limits and replay windows |
| `LOCUS_ROLE` | `role` | `pds` | `pds`, `shard` or `entryway` |
| `LOCUS_HOSTNAME` (`PDS_HOSTNAME`) | `hostname` | — | The initial public hostname |
| `LOCUS_DEV_MODE` (`PDS_DEV_MODE`) | `dev_mode` | `false` | Dev mode (local hostnames only, see below) |
| `LOCUS_DISABLE_SSRF_PROTECTION` (`PDS_DISABLE_SSRF_PROTECTION`) | `disable_ssrf_protection` | unset | As the reference |
| `LOCUS_TLS` and friends | `tls`, … | off | [Direct TLS](#direct-tls) |
| `LOCUS_SETUP_AUTO` | — | — | Complete setup headless ([Setup](#headless-setup)) |

A minimal `locus.toml` for a bare-metal SQLite install:

```toml
data_dir = "/var/lib/aurora-locus"
listen = "127.0.0.1:3000"
hostname = "pds.example.com"
log_format = "json"
```

**Hostname.** The hostname is bootstrap only as an initial value. Locus
needs it in setup mode so Caddy can get its certificate and the wizard can
load over HTTPS. After setup it lives in the database. Changing it is a
migration, not an edit (see [Changing the hostname](#changing-the-hostname)):
a later `PDS_HOSTNAME` that differs is refused, logged and shown as a
console warning.

**Dev mode.** `PDS_DEV_MODE` relaxes HTTPS checks and lets the PDS reach
loopback and private addresses, as in the reference. Unlike the reference,
locus honors it only on a local hostname (`localhost`, `*.localhost`, a
loopback IP); on a public one it is ignored with a warning, because it
turns off checks users rely on. `PDS_DISABLE_SSRF_PROTECTION` works as in
the reference, and the console warns on every page while it is on.

The role, Redis URL, dev mode, SSRF protection and master key source kind
must match on every node; the console warns while live nodes disagree.

### Runtime settings

The console's Settings page lists every runtime setting with its
description, its current value and whether it applies live. On the command
line:

```sh
aurora-locus settings list
aurora-locus settings get identity.handle_domains
aurora-locus settings set identity.handle_domains '[".example.com"]'
aurora-locus settings set accounts.invite_required true
```

`settings set` takes the value as JSON (`true`, `30`, `["a"]`), or plain
text. An endpoint setting with a plain `http://` URL (a LAN AppView, a
local PLC) needs `--allow-insecure-http`.

The settings most installs touch:

| Setting | Reference env | Meaning |
|---|---|---|
| `identity.handle_domains` | `PDS_SERVICE_HANDLE_DOMAINS` | Domains new handles can be on, each starting with a dot |
| `identity.plc_url` | `PDS_DID_PLC_URL` | The PLC directory |
| `identity.recovery_did_key` | `PDS_RECOVERY_DID_KEY` | An operator recovery key added to new did:plc accounts |
| `accounts.invite_required` | `PDS_INVITE_REQUIRED` | Require an invite code to sign up |
| `accounts.default_did_method` | — | DID method for new accounts that don't ask for one |
| `server.service_name` | `PDS_SERVICE_NAME` | Name on the landing page, OAuth pages and emails |
| `server.contact_email` | `PDS_CONTACT_EMAIL_ADDRESS` | Published in `describeServer` |
| `email.from_address` | `PDS_EMAIL_FROM_ADDRESS` | From address of account emails |
| `appview.url`, `appview.did` | `PDS_BSKY_APP_VIEW_URL`, `_DID` | The AppView requests are proxied to |
| `appview.private_url`, `appview.private_did` | `LOCUS_PRIVATE_APPVIEW_URL`, `_DID` | The private AppView, for the private tier |
| `moderation.service_url`, `_did` | `PDS_MOD_SERVICE_URL`, `_DID` | Moderation service (for example cairn) |
| `moderation.report_service_url`, `_did` | `PDS_REPORT_SERVICE_URL`, `_DID` | Where reports go |
| `sync.crawlers` | `PDS_CRAWLERS` | Relays asked to crawl this PDS |
| `blobs.upload_limit` | `PDS_BLOB_UPLOAD_LIMIT` | Largest blob upload (default 300 MiB) |
| `limits.import_size` | — | Largest repository import (default 1 GiB) |
| `blobs.s3_*` | `PDS_BLOBSTORE_S3_*` | S3-compatible blob storage (applies on restart) |
| `http.trusted_proxies` | `LOCUS_TRUSTED_PROXIES` | Proxies whose forwarded headers are trusted |
| `deploy.proxy_mode` | `LOCUS_PROXY_MODE` | `caddy`, `cloudflare`, `other` or `direct` |
| `rate_limits.enabled` | `PDS_RATE_LIMITS_ENABLED` | Apply rate limits |
| `metrics.enabled`, `metrics.allowed_ips` | `LOCUS_METRICS_*` | Access to `/metrics` (the bearer token is a secret: `keys set metrics-token`) |
| `backups.every_hours`, `backups.keep`, `backups.dir` | `LOCUS_BACKUPS_DIR` (dir) | Scheduled backups |
| `log.level` | `LOG_LEVEL` | Log level, applied live |

Secrets are not plain settings. The SMTP URL, the S3 secret key, the JWT
and DPoP secrets, the hCaptcha secret and similar values go into the
encrypted key store, never into the settings table.

### Environment values and precedence

Env and file values for runtime settings are **initial values**: they seed
the database on first boot, so a container can be configured without the
wizard. On later boots, a value that **changed** is applied again and
audited as a change made by "environment". So the reference's workflow
still works: edit `pds.env`, restart.

An unchanged env value never overrides a later console change; the console
shows which env values are shadowed. With several nodes, a new value is
applied once every live node reports it, and then exactly once, so a
rolling restart applies it when the last node comes up.

Some values are never applied on a later boot, because changing them
silently would break sessions or identity: the JWT and DPoP secrets, the
entryway keys and admin token, the hCaptcha secret, `PDS_HOSTNAME` and an
explicit `PDS_SERVICE_DID`. A changed value is refused and shown as a
console warning. Secrets change with `aurora-locus keys set` (see
[Keys](#keys)); the hostname changes through the hostname migration.

A changed PLC rotation key (`PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX` or
`PDS_PLC_ROTATION_KEY_KMS_KEY_ID`) is applied, as the reference reads it at
every boot, but stored as *pending*: it becomes current once a key backup
holds it (or you acknowledge going without one).

`PDS_ADMIN_PASSWORD`, when set, is always the password for the reference's
`admin:<password>` Basic auth, so `pdsadmin`-style scripts keep working.
Plain `http://` endpoint URLs in env (a LAN AppView, a local PLC) are
accepted, with a console warning. Reference variables with no locus meaning
are listed in the console.

In the Docker stack the environment is `deploy/docker/locus.env`. Docker
reads an `env_file` when it creates the container, so after editing it run
`docker compose up -d` (not `docker compose restart`, which keeps the old
environment).

### Exporting and importing settings

```sh
aurora-locus settings export > settings.json
aurora-locus settings import settings.json
```

The export is one JSON object of every setting. `import` sets every setting
in such an object, after asking (or with `--yes`). This suits backups and
infrastructure-as-code.

## Setup

### First run

Until setup completes, locus is in **setup mode**:

- Only the setup wizard, the setup API, health and `/tls-check` respond.
  Other XRPC methods answer 503.
- `/xrpc/_health` answers 200, so the container health checks pass during
  setup. `/readyz` reports not ready.
- `/tls-check` approves only the bootstrap hostname, so Caddy can get the
  PDS hostname's certificate and `https://<host>/setup` loads.
- Without a bootstrap hostname, the wizard is at
  `http://localhost:<port>/setup` (for example through an SSH tunnel).

The first node to start generates a **one-time setup token**. It prints the
token to its log and writes it to `<data>/setup-token` (mode 0600). The
wizard asks for it before showing anything, and it stops working once setup
completes. Setup mode listens on the normal address; the token is what
stops someone else from claiming an exposed instance. To replace a lost
token:

```sh
aurora-locus setup --new-token
```

### Three ways to set up

All three run the same steps, the same checks and the same final
transaction, so they end in the same state.

- **Web wizard** at `https://<host>/setup`.
- **Interactive CLI:** `aurora-locus setup`.
- **From a file:** `aurora-locus setup --config setup.toml`, for automation.
  `--accept-warnings` accepts the final checks' warnings without asking.

### Steps

1. **Setup token.**
2. **Hostname.**
3. **Storage.** The database and blob storage are fixed by bootstrap config;
   this step confirms and tests them. The small profile is SQLite with blobs
   on disk; the large profile is Postgres with S3.
4. **Proxy and TLS.** The bundled Caddy (the default), Cloudflare's proxy in
   front of Caddy, another reverse proxy, or direct TLS. This step also
   collects trusted proxy ranges.
5. **Handle domains**, with the subdomains already in use.
6. **First operator**: username and password (at least 12 characters). A
   second sign-in factor (an authenticator app code) is strongly suggested;
   the console's setup for it shows a QR code.
7. **Email.** An SMTP URL and a from address, then a test email whose code
   you enter. You can skip email, and the wizard says what won't work
   without it (email confirmation, password reset, email sign-in codes).
8. **Network services.** The AppView (default Bluesky's,
   `https://api.bsky.app`), crawlers (default `https://bsky.network`),
   moderation services (optional, for example cairn), and where reports go
   (default Bluesky's moderation service, as with the reference's
   installer).
9. **Private posts.** Turns on the private tier (built on kryphocron) and
   asks for the private AppView. It may be the same service as the AppView,
   but it must be entered: with it empty, the private tier stays off.
10. **Keys.** Generates the PDS's PLC rotation key (or takes it from env)
    and the service signing key, offers the operator recovery key, and
    offers an encrypted key backup to download.
11. **Review and finish.** Runs the checks and shows pass, warn or fail:
    DNS resolves to this host (in Cloudflare mode, to Cloudflare with the
    record proxied), TLS works, `/xrpc/_health` answers through the public
    URL, the proxy reaches `/tls-check`, the blob limit fits under the
    proxy's body cap, and so on. Failures that would break the PDS block
    finishing; warnings can be acknowledged.

### Headless setup

Set `LOCUS_SETUP_AUTO=true` and supply every required value in env, and the
node completes setup on its own at boot. This works with a reference
`pds.env` too. The values setup reads, on top of the env mapping above:

| Variable | Meaning |
|---|---|
| `PDS_HOSTNAME` | The hostname |
| `PDS_SERVICE_HANDLE_DOMAINS` | Handle domains |
| `LOCUS_SETUP_OPERATOR_USERNAME`, `LOCUS_SETUP_OPERATOR_PASSWORD` | The first operator |
| `PDS_ADMIN_PASSWORD` | The reference's admin password (instead of, or alongside, an operator) |
| `PDS_EMAIL_SMTP_URL`, `PDS_EMAIL_FROM_ADDRESS` | Email; without an SMTP URL, setup goes without email |
| `LOCUS_SETUP_PROXY` | `caddy`, `cloudflare`, `other` or `direct` |
| `LOCUS_SETUP_EXISTING_SUBDOMAINS` | Subdomains of the handle domain already in use |
| `LOCUS_SETUP_KEYS_BACKUP_ACKNOWLEDGED` | `true`: you have a copy of the master key or a key backup (required) |
| `LOCUS_SETUP_CLOUDFLARE_TOKEN` | Optional: a Cloudflare API token with zone read, to check the SSL mode |

The same checks run. Warnings are logged and accepted; a failure is logged
and leaves the node in setup mode. SMTP from env counts as configured once
a connect and authentication check passes; the console marks it "delivery
unverified" until a test email is confirmed.

### Hostnames and handle domains

Handles are DNS names, so every handle needs a certificate and a route to
the PDS. There are two common layouts:

- **Handles under the hostname** (`alice.pds.example.com`). This is the
  installer's default and the reference's. It needs nothing else on the
  domain.
- **Handles under the parent domain** (`alice.example.com`), when the
  hostname is a subdomain. Handles are shorter, and Cloudflare's free edge
  certificate covers one wildcard level, which `*.pds.example.com` exceeds.
  When setup runs from a file without handle domains, it picks this one.

With a parent-level wildcard, setup asks for the domain's existing
subdomains (`www`, `mail`, …), reserves them, and shows the DNS records to
create: your specific records stay (they override the wildcard), plus a
wildcard record pointing at the PDS. Only names `/tls-check` approves get a
certificate, and names other than the PDS hostname reach only the
well-known documents a handle needs. The PDS hostname itself is always
reserved as a handle.

After setup, every choice is a runtime setting, except the storage backend
(which has migration tools) and the bootstrap settings.

## Web UI

| Surface | Path | Users |
|---|---|---|
| Setup wizard | `/setup` | First run only |
| Operator console | `/admin` | Operators |
| Account portal | `/account` | Account holders |
| OAuth pages | `/oauth/*` | Account holders, through OAuth clients |

The pages are server-rendered HTML with all assets built into the binary.
They load nothing from a CDN, work without JavaScript for core actions, and
use a strict content security policy.

### Operators and roles

Operators are separate from atproto accounts. Each has a username, a
password, an optional authenticator app code, and a role:

| Role | Can do |
|---|---|
| `owner` | Everything, including operators and keys |
| `admin` | Everything except operators and keys: account deletion, credential changes (password, email, handle), settings, services, limits, jobs, storage, deployment |
| `moderator` | Takedowns and their reversal, invites, reports, blocking the private tier for an account, and the read-only views moderation needs |
| `viewer` | Read-only |

Console sessions use secure, `SameSite=Strict` cookies with an idle timeout
(`admin.session_idle_minutes`); every sign-in ends after 12 hours. Sensitive
actions (keys, operators, deletion) ask you to sign in again.

The reference's `admin:<password>` Basic auth works on exactly the
reference's admin surface (`com.atproto.admin.*`, the invite endpoints and
the admin-token sync reads), where it counts as the `admin` role. It never
reaches settings, keys, operators or backups. Turn it off with
`admin.basic_auth_enabled`, unless `PDS_ADMIN_PASSWORD` is set.

### Console, portal and branding

The console has pages for the dashboard, accounts (create with did:plc or
did:web, takedown, suspend, reset password, change handle or email,
delete), invites, moderation, identity and PLC keys, services, private
posts, scanning, email, branding, limits, proxy and TLS (with a
`/tls-check` tester and DNS checker), storage and backups, jobs, settings,
operators (owner only, including API tokens for the CLI over HTTP), the
audit log, the hostname migration, and recent logs.

The account portal (`portal.enabled`) lets people manage their own account:
handle, email, password, app passwords, connected OAuth apps, deactivation
and deletion, repository export, and a PLC recovery key. The OAuth pages
handle sign-in, consent and the account picker, following the reference's
flow.

The branding editor covers the reference's options (service name, logo,
colors, backgrounds, links) plus fonts, light and dark schemes, page text
and custom CSS, with drafts, previews and versioned rollback. Custom CSS is
parsed and re-serialized, never passed through, and there is no custom HTML
or JavaScript. Custom CSS and uploaded fonts **never** apply to the OAuth
sign-in, consent and account picker pages, so a theme can't disguise what a
user is approving.

## CLI

One binary, `aurora-locus`, is both the server and the CLI.

### How the CLI reaches a node

Online commands talk to a running node through its admin API:

- **Over the Unix socket** (default `<data>/admin.sock`, mode 0600). The node
  checks the caller's user id: only root and the service's own user are
  accepted, and they get the owner role, since local access already implies
  control of the keys on disk.
- **Over HTTP** with `--url` and `--token` (or `LOCUS_ADMIN_URL` and
  `LOCUS_ADMIN_TOKEN`), using an operator API token from the console.
  Use this where Unix sockets aren't available.

In the Docker stack, run the CLI inside the container:

```sh
cd deploy/docker
docker compose exec locus aurora-locus status
```

With the systemd unit, run it as the service user:

```sh
sudo -u locus aurora-locus status
```

**Offline commands** (`import reference-pds`, `keys rewrap`,
`backup restore`, `backup post-restore`, `migrate storage`) work on the
database directly and refuse to run while a node holds it. On SQLite every
running node holds an exclusive lock on `<data>/node.lock` (so one node per
data directory, on a local filesystem); on Postgres every node holds a
shared advisory lock. `migrate storage` and `import reference-pds` accept
`--force`, with a warning; `keys rewrap`, `backup restore` and
`backup post-restore` can never be forced.

### Commands

| Command | Purpose |
|---|---|
| `serve` | Run a node |
| `setup [--config <file>] [--accept-warnings] [--new-token]` | First-run setup |
| `status` | Version, nodes, accounts and jobs |
| `account list/show/create/delete/takedown/untakedown/suspend/unsuspend/reset-password/set-handle/set-email/private-tier/import-status` | Accounts |
| `invite create [--count] [--uses] / list / disable` | Invite codes |
| `settings get/set/list/export/import` | Runtime settings |
| `operator add [--role] / remove / list / reset-password / set-role` | Operators |
| `moderation status/takedown/restore` | Subject status of accounts, records and blobs |
| `crawl request` | Ask every relay to crawl now |
| `jobs list / run <name> / pause <name> [--resume]` | Background jobs |
| `kryphocron status / rotate-now / audit` | Private-tier controls |
| `keys …` | [Keys](#keys) |
| `backup create / list / restore / post-restore` | [Backups](#backups-and-restore) |
| `migrate blobs / hostname / storage` | [Migrations](#migrations) |
| `import reference-pds <dir> --env <pds.env>` | [Import a reference installation](#importing-a-reference-pds) |
| `repo rebuild <did>` | The reference's `rebuild-repo` |
| `repair repo <did>` / `repair identity <did>` | Rebuild the record index; reset a did:plc document to this PDS |
| `sequencer recover [--did] [--since]` | The reference's `sequencer-recovery` |
| `repair repos --from-sequencer` | The reference's `recovery-repair-repos` |
| `keys rotate-account <did>… [--file] [--recovery]` | The reference's `rotate-keys` scripts |
| `identity publish <did>… [--file]` | The reference's `publish-identity` scripts |
| `shard add/list/weight/drain` | Entryway role: the shards behind it |
| `call <nsid> [key=value…] [--body <json>]` | Any XRPC method through the admin API |
| `deploy caddyfile` | Print a Caddyfile |
| `healthcheck [--url]` | GET `/xrpc/_health`; exit 0 on 200 |
| `version` | Version information |

Every `aurora-locus <command> --help` lists its flags. The reference's
`pdsadmin` operations all have equivalents, so existing habits translate.

### Importing a reference PDS

```sh
aurora-locus import reference-pds /pds --env /pds/pds.env
```

This turns a **stopped** reference PDS (0.5.x) into a locus install with the
same hostname, DIDs, handles, repositories, blobs, sessions and firehose
position. It runs offline into an empty SQLite install, and refuses while
the reference still has its databases open. To end up on Postgres, run
`migrate storage` afterwards.

It brings over:

- settings, through the env mapping, keeping the reference's handle domain;
- the JWT and DPoP secrets, the PLC rotation key (as the current key) and
  the admin password, into the encrypted key store. The import creates the
  key store, so the master key is chosen here;
- accounts, app passwords, sessions, invites, OAuth state and preferences.
  Passwords keep working and are rehashed to Argon2id on the next login;
- each repository's signing key, the blocks its current tree reaches, the
  record index with takedowns, and blob metadata;
- the event log within locus's retention, with sequence numbers continuing
  from the reference's last, so relay cursors stay valid.

Blobs on disk stay where they are, referenced in place under the blob
directory's parent (they are copied only if the directory's name clashes
with one of locus's own prefixes). New uploads go there too, so the node
must be able to write to it; with the systemd unit, add it to
`ReadWritePaths`. Blobs in S3 are referenced in place.

Trusted proxies are set to the reference's set (loopback, link-local,
unique-local and private ranges) so client-IP handling doesn't change; the
console suggests narrowing it. Setup is marked complete, so
`aurora-locus serve` starts directly, and the console shows an import
report. The import doesn't create an operator: add one with
`aurora-locus operator add <name> --role owner` over the socket.

Back up the reference's data directory before importing.

## Deployment

### Docker and Caddy (the default)

`deploy/docker/compose.yaml` runs two services, `locus` and `caddy`, with
volumes for locus's data and Caddy's state.

- **Only Caddy publishes public ports** (80, 443 and 443/udp). Locus
  publishes port 3000 on `127.0.0.1` only, for the CLI over HTTP on the same
  machine (`LOCUS_LOCAL_PORT` in `.env` picks another host port). It never
  publishes on `0.0.0.0`, which would let clients skip Caddy.
- **Trusted proxy.** The stack has its own network with a fixed subnet
  (`172.30.53.0/24` by default), an explicit gateway (`.1`) and a dynamic
  range limited to the upper half (`.128/25`). Caddy has the static address
  `.2`, and the installer sets `LOCUS_TRUSTED_PROXIES=172.30.53.2/32`. Locus
  trusts that one address, not the subnet, because connections through a
  published port arrive from the bridge gateway address, which must not be
  trusted. Limiting the dynamic range means no other container can take
  Caddy's address by starting first.
- **Health checks** run `aurora-locus healthcheck` against `/xrpc/_health`
  (never `/readyz`, which is not ready during setup).
- **No auto-updater by default.** A watchtower service is included,
  commented out, as the reference offers one.

The image runs as a non-root user on a slim Debian base, with the `goat`
CLI included as in the reference's image. Builds keep cargo's caches
between runs and turn link-time optimization off, which roughly halves a
rebuild on a server. For a fully optimized image:

```sh
docker build -f deploy/docker/Dockerfile -t aurora-locus \
  --build-arg CARGO_PROFILE_RELEASE_LTO=thin \
  --build-arg CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 .
```

To set up the stack by hand instead of with the installer, see
[Running with Docker](../docker.md). In short: write
`deploy/docker/locus.env` (at least `PDS_HOSTNAME` and
`LOCUS_TRUSTED_PROXIES`), then:

```sh
cd deploy/docker
docker compose build locus
docker compose run --rm --no-deps --entrypoint aurora-locus locus \
  deploy caddyfile --email you@example.com > Caddyfile
docker compose up -d
```

`deploy/docker/compose.postgres.yaml` adds Postgres on the same machine.

### Kubernetes

`deploy/helm/aurora-locus` is a Helm chart with two profiles: `small`
(SQLite on one pod with a PersistentVolume) and `large` (Postgres and S3,
any number of pods). See [Running on Kubernetes](../kubernetes.md).

**Upgrading.** Pull the new source, rebuild and recreate:

```sh
docker compose build locus
docker compose up -d
```

Database migrations are built in and run at start. A node refuses to start
against a database schema newer than it knows, so an accidental downgrade
stops rather than damaging data.

### Certificates

Locus never edits Caddy's configuration at runtime. The Caddyfile is
written once, and a catch-all site with on-demand TLS means new handle
domains and handles need no Caddy change: Caddy asks locus's `/tls-check`
before it gets each certificate.

`/tls-check?domain=<name>` answers 200 only for:

- the PDS hostname (and, during a hostname migration, the new and old ones);
- a prepared new hostname, so its certificate can be issued before the
  migration starts;
- former PDS hostnames, indefinitely, so their `did:web` keeps resolving;
- hosted did:web hosts;
- hostnames of existing handles, including handles under a handle domain
  that has since been removed.

Anything else gets 404, so Caddy won't get a certificate and the TLS
handshake fails. In setup mode it approves only the bootstrap hostname.

Every generated Caddyfile sets `strict_sni_host on`, so Caddy refuses a
request whose `Host` differs from the TLS server name.

There are three strategies:

**On demand** (the default). One certificate per name from Let's Encrypt,
each approved by `/tls-check`. Simple, and fine for small and medium
servers. Let's Encrypt allows 50 new certificates per registered domain per
week, so a busy server, especially with handles under a busy parent domain,
should use a wildcard instead.

```
{
	email admin@example.com
	on_demand_tls {
		ask http://locus:3000/tls-check
	}
	servers {
		strict_sni_host on
	}
}

https:// {
	tls {
		on_demand
	}
	reverse_proxy locus:3000
}
```

**DNS-01 wildcard.** One wildcard certificate for the handle domain (and the
hostname), proved through your DNS provider's API; on-demand stays on for
names outside the wildcard. The stock Caddy image has no DNS provider
modules, so this uses `locus-caddy`, built from `deploy/docker/caddy` with
the cloudflare, digitalocean, hetzner and duckdns modules. The token goes in
`DNS_API_TOKEN`.

```sh
aurora-locus deploy caddyfile --strategy dns01 --email admin@example.com \
  --hostname pds.example.com --wildcard example.com --dns-provider cloudflare
```

**Cloudflare's proxy (orange cloud).** On-demand certificates can't work
behind Cloudflare's proxy: TLS-ALPN-01 never reaches the origin and HTTP-01
breaks under "Always Use HTTPS". So this mode uses a fixed **Cloudflare
Origin CA certificate** covering the hostname and the handle wildcard. The
installer asks you to paste it and its key, and stores them in
`deploy/docker/certs`. The Caddyfile uses `tls <cert> <key>`, no ACME, and
sets Caddy's `trusted_proxies` to Cloudflare's ranges.

```sh
aurora-locus deploy caddyfile --strategy cloudflare \
  --hostname pds.example.com --wildcard example.com \
  --cert /certs/origin.pem --key /certs/origin.key
```

In Cloudflare mode (`LOCUS_PROXY_MODE=cloudflare`), locus reads
`CF-Connecting-IP` only when the hop before the trusted proxy is one of
Cloudflare's ranges (bundled, refreshed daily). Also:

- Set SSL/TLS to **Full (strict)**. Setup probes the hostname through
  Cloudflare and fails on Flexible's signs; with an API token that can read
  the zone, it checks the mode directly.
- Cloudflare's free edge certificate covers one wildcard level, so prefer
  handles at `*.example.com` over `*.pds.example.com`.
- Enable WebSockets, for the firehose.
- Cloudflare caps request bodies (100 MB on Free and Pro); setup warns when
  the blob or import limit is above that.
- Bypass the cache for `/xrpc/*`, `/oauth/*`, `/admin/*` and `/blob/*`.

### Changing the certificate strategy

This is an operator step. Print the new Caddyfile, put it in place and
reload Caddy:

```sh
cd deploy/docker
docker compose exec -T locus aurora-locus deploy caddyfile --strategy dns01 \
  --email admin@example.com --hostname pds.example.com \
  --wildcard example.com --dns-provider cloudflare > Caddyfile
docker compose up -d
```

For DNS-01, also build `locus-caddy` and set `CADDY_IMAGE=locus-caddy` and
`DNS_API_TOKEN` in `deploy/docker/.env`.

### Other proxies

`deploy/proxies/README.md` has full examples for Caddy on the same machine,
nginx, Traefik and HAProxy. Whatever the proxy:

- **Trust only the proxy.** Set `LOCUS_TRUSTED_PROXIES` (or
  `http.trusted_proxies`) to the proxy's own address. Forwarded headers
  (`X-Forwarded-For`, `X-Forwarded-Proto`, `Forwarded`) are honored only
  from those addresses, and the client is the right-most untrusted address.
- **Forward the client and scheme**: append to `X-Forwarded-For`, set
  `X-Forwarded-Proto: https`.
- **Keep the `Host` header.** Locus routes by it.
- **Allow WebSockets** and long-lived connections, for the firehose.
- **Don't cap bodies** below `blobs.upload_limit` and `limits.import_size`.
- **Certificates**: on demand through `/tls-check` where the proxy supports
  it, otherwise a wildcard for the handle domain.

The default trusted-proxy list is **loopback only** (`127.0.0.0/8`, `::1`),
which already covers a proxy on the same machine. This is stricter than the
reference, which trusts every internal address (loopback, link-local,
unique-local and private ranges): on a shared private network, any neighbor
could then set the client IP that rate limits key on. Locus trusts only
what you name.

Caddy on the same machine:

```sh
aurora-locus deploy caddyfile --email you@example.com \
  --upstream 127.0.0.1:3000 --ask-url http://127.0.0.1:3000/tls-check \
  > /etc/caddy/Caddyfile
systemctl reload caddy
```

### Bare binary with systemd

`deploy/systemd/aurora-locus.service` runs locus as a hardened service
behind a proxy on the same machine, listening on `127.0.0.1:3000` with its
data in `/var/lib/aurora-locus`.

```sh
install -m 0755 aurora-locus /usr/local/bin/aurora-locus
useradd --system --home-dir /var/lib/aurora-locus --shell /usr/sbin/nologin locus
mkdir -p /etc/aurora-locus
echo 'PDS_HOSTNAME=pds.example.com' > /etc/aurora-locus/locus.env
cp deploy/systemd/aurora-locus.service /etc/systemd/system/
systemctl enable --now aurora-locus
```

The unit sets a strict sandbox (read-only system, no new privileges, a
system call filter, no capabilities). After `import reference-pds`, add the
reference's data directory to `ReadWritePaths`, since blobs stay there.

### Direct TLS

For no proxy at all, locus can terminate TLS itself with ACME certificates,
ordered on demand for the names `/tls-check` approves (the PDS hostname's at
start) and renewed after 60 days.

| Variable | Default | Meaning |
|---|---|---|
| `LOCUS_TLS` | `false` | Turn direct TLS on |
| `LOCUS_TLS_LISTEN` | `0.0.0.0:443` | HTTPS listener |
| `LOCUS_TLS_HTTP_LISTEN` | `0.0.0.0:80` | HTTP-01 challenges and redirects; `off` for TLS-ALPN-01 only |
| `LOCUS_ACME_DIRECTORY` | Let's Encrypt | The ACME directory |
| `LOCUS_ACME_EMAIL` | — | The ACME account contact |
| `LOCUS_TLS_CACHE` | `<data>/tls` | Certificates and the ACME account |
| `LOCUS_ACME_CA` | — | A CA certificate for a private ACME directory |

Binding ports below 1024 needs `CAP_NET_BIND_SERVICE`; the systemd unit has
the lines for it, commented out.

### Health

| Endpoint | Answers |
|---|---|
| `GET /xrpc/_health` | `{"version": …}`, as the reference; 503 if the database is unreachable. Answers in setup mode |
| `GET /healthz` | Liveness |
| `GET /readyz` | Readiness: database and blob store reachable, not in setup mode |

These answer on any hostname. Use `/xrpc/_health` for container health
checks and `/readyz` for load balancers once setup is done.

### Several nodes and the entryway

The large profile runs several nodes on one Postgres database with S3 blob
storage, using the same code paths as the small profile. Any node serves
any request, with no sticky sessions; writes to a repository are serialized
in the database, and background jobs run under leases, one node at a time.
Settings changes reach every node through Postgres `NOTIFY`, with a
5-second poll as a fallback. An optional Redis (`LOCUS_REDIS_URL`) shares
rate-limit counters and replay windows; without it, they live in the
database.

Every node must use the same master key: an operator-supplied shared key
file (`LOCUS_MASTER_KEY_SHARED_FILE=true`), `LOCUS_MASTER_KEY`, or KMS. A
key file is generated automatically only on a fresh single-node SQLite
install with a persistent data directory.

Locus can also run as a PDS behind an **entryway** (`LOCUS_ROLE=shard`),
the reference's arrangement where a front door owns accounts, sign-in,
OAuth and handles for many PDS instances, or as an entryway itself
(`LOCUS_ROLE=entryway`). A shard reads the reference's `PDS_ENTRYWAY_*`
variables. The entryway manages its shards with `aurora-locus shard`. Most
installs don't need either role.

## Operations

### Keys

Every stored secret and account signing key is encrypted under one
**master key** per install. Its source is a bootstrap setting: a file
(`<data>/master.key` by default, generated on first start of a single-node
SQLite install), hex in `LOCUS_MASTER_KEY`, or AWS KMS (or a compatible
service at `LOCUS_KMS_ENDPOINT`), where a data key is kept wrapped in the
database. The database records a check value, so a node started with
another install's key refuses to start, and every write of a secret checks
the key again.

**Back up the master key.** Losing it loses every stored secret.

```sh
aurora-locus keys show                       # held keys, last backup, PLC key listing
aurora-locus keys backup --out keys.backup   # encrypted with a passphrase
```

A key backup holds every stored secret, every account signing key and the
PLC rotation keys, encrypted under a passphrase. `--passphrase-file` reads
the passphrase from a file (or `-` for stdin) for scripts. `keys import
<backup>` reads one back: it adds PLC rotation keys this PDS lacks, and
restores account signing keys that the accounts' live DID documents list.

Changing keys:

| Command | What it does |
|---|---|
| `keys set plc-rotation-key --generate` (or `--file`, `--kms-key-id`) | A new PLC rotation key for the PDS. Stored pending; becomes current once a key backup holds it, or with `--acknowledge-no-backup`. The previous key keeps signing for the accounts that list it |
| `keys drop-plc-key <did:key>` | Forget a secondary PLC rotation key, after checking no account still needs it |
| `keys rotate-service-key` | A new key for the service's own DID document |
| `keys set jwt-secret --file <f>` | Replace the legacy session secret (signs every legacy session out) |
| `keys set dpop-secret --file <f>` | Replace the DPoP nonce secret (clients retry once) |
| `keys set hcaptcha-secret --file <f>` | The hCaptcha secret, with `accounts.hcaptcha_site_key` |
| `keys set rate-limit-bypass-key --file <f>` (or `--remove`) | The `x-ratelimit-bypass` key (`PDS_RATE_LIMIT_BYPASS_KEY` sets it on first boot) |
| `keys set metrics-token --file <f>` (or `--remove`) | The bearer token that reads `/metrics` from anywhere (`LOCUS_METRICS_TOKEN` on first boot) |
| `keys set backup-passphrase --file <f>` | The passphrase scheduled backups encrypt their key backup with |
| `keys set entryway-*` | Shard role: follow the entryway's key changes |
| `keys rewrap --to file --path <new> \| env --out <f> \| kms --kms-key-id <id>` | Change the master key source (offline) |

Secrets are read from a file or stdin, never the command line. Key changes
need the owner role and a confirmation (a typed `yes` over the socket; your
password and sign-in code over HTTP).

`keys rewrap` re-encrypts every stored secret under the new key in one
transaction, so nothing changes if it fails; the old key file is never
touched. Afterwards, point the master key setting at the new source,
restart, and make a new key backup.

### Backups and restore

**Small profile (SQLite).** Backups run online:

```sh
aurora-locus backup create --wait
aurora-locus backup list
```

A backup is a consistent database snapshot, every blob object the snapshot
names, a key backup and, last, a manifest, in a directory under
`backups.dir` (default `<data>/backups`). While it runs, blob deletion
holds off, so nothing the snapshot names is deleted before it is copied.
Schedule backups with `backups.every_hours` and keep `backups.keep` of them
(default 7); scheduled backups include a key backup once
`keys set backup-passphrase` has set a passphrase. Keep copies off the
server.

**Restore** runs offline, **only into an empty install**, and never merges
with newer state. Move a damaged install aside first.

```sh
aurora-locus backup restore /backups/<backup> --backend disk
```

`--backend s3` writes the objects to the S3 bucket named in the restored
settings (overridable by env). Restore refuses a master key that doesn't
match the backup, and a backup marked failed unless `--accept-failed`.
Every refusal happens before anything is written.

Relays may have seen events newer than the backup, so restore:

- starts the next seq above the backup's last, `--seq-floor <n>`, and what
  each crawler reports for this host plus `sync.restore_seq_margin`, so
  relays don't skip new events;
- re-announces every account (`#identity`, then `#account` with its
  restored status);
- with `--emit-sync`, writes a fresh commit and `#sync` per active account,
  so relays resync each repository (this needs `--seq-floor` or a crawler
  that answers);
- lists accounts whose live DID document doesn't list their restored
  signing key. Fix them with `keys import` of a later key backup, then
  `identity publish` or `repair identity`.

**Large profile (Postgres and S3).** Use Postgres point-in-time recovery
and versioning on every bucket, keeping noncurrent versions at least as
long as the recovery window plus `blobs.delete_grace_ms`. Make key backups
with `keys backup`. After every point-in-time recovery, and **before any
node starts**, run:

```sh
aurora-locus backup post-restore --seq-floor <n>
```

It checks the master key and every tracked blob object (restoring deleted
ones from earlier versions where it can, listing the rest), ends any blob
migration in progress, and then handles the firehose as `backup restore`
does. Nothing is written before every check passes.

### Migrations

#### Changing the hostname

The hostname is the service's DID (`did:web:<hostname>`), the OAuth issuer
and the address in every account's DID document, so changing it is a
tracked migration, not a setting. It runs from the console's hostname page
or the CLI:

```sh
aurora-locus migrate hostname new.example.com   # prepare and check
aurora-locus migrate hostname --status
aurora-locus migrate hostname --confirm         # start
```

1. **Prepare.** The new name is reserved and `/tls-check` approves it, so
   Caddy can get its certificate. Point its DNS at the server. The
   pre-checks confirm it reaches this install over HTTPS; `--cancel` clears
   it.
2. **Confirm.** The new hostname takes effect on every node. **Every OAuth
   session must sign in again**, because OAuth clients accept exactly one
   authorization server. Other tokens keep working through the window.
3. **The job** moves every did:plc account's PDS endpoint and every hosted
   did:web document, with `#identity` for each, and asks relays to crawl
   the new host. Accounts it can't move are listed for their owners.
4. **The window.** The old hostname keeps full service while any DID
   document names it, and for at least `identity.hostname_transition_days`
   (default 30); `--close` then ends the migration with stragglers listed.
   Afterwards the old name serves its `did.json` forever and redirects
   everything else.

This isn't reversible in practice. It is refused in the shard and entryway
roles.

#### Moving blobs to another backend

```sh
aurora-locus migrate blobs --to s3
aurora-locus migrate blobs --status
```

Set the `blobs.s3_*` settings (and the secret key, `PDS_BLOBSTORE_S3_SECRET_ACCESS_KEY`)
first. The node keeps serving while the job copies every object in passes
and verifies them, then pauses uploads for a few seconds (at most
`blobs.cutover_max_pause_ms`) for a final pass and the switch. Uploads
during the pause get a retryable 503. Every node then reloads onto the new
backend. `--cancel` ends the migration without switching. Objects missing
from the source hold the switch until you accept them with
`--accept-missing`. The old backend's objects are left for you to remove.
`--to disk [--disk-root <path>]` moves blobs back to disk.

The S3 settings work with AWS and S3-compatible services (for example
MinIO, R2 or B2) through `blobs.s3_endpoint` and `blobs.s3_force_path_style`.

#### SQLite to Postgres

```sh
aurora-locus migrate storage --to postgres --url postgres://locus@db.example.com/locus
```

Offline: stop the node first. It copies the install into an empty Postgres
database in one transaction, with every sequence continuing above what was
copied. A node-generated master key file can't serve several Postgres
nodes as it is: declare it shared (`LOCUS_MASTER_KEY_SHARED_FILE=true`, the
same file on every node) or `keys rewrap` to env or KMS first. It refuses
while a blob migration is running. Afterwards set `LOCUS_DATABASE_URL` and
start the nodes.

### Metrics

Prometheus metrics are at `/metrics`: XRPC requests and latency by method
and status, commit and import-verify time, sequencer lag, firehose
subscribers and drops, job runs, rate-limit refusals, blob bytes, database
pool connections, private-tier encodes, scanner calls and gated media
fetches.

Access is limited to `metrics.allowed_ips` (loopback by default), or any
address presenting the metrics token as a bearer token. The token is a
secret in the key store: `LOCUS_METRICS_TOKEN` sets it on first boot, and
`aurora-locus keys set metrics-token` changes it (`--remove` removes
it). Turn metrics off with `metrics.enabled`.

### Logs

Logs go to standard output, as `pretty` text or `json` (`LOCUS_LOG_FORMAT`;
the image uses JSON). Log lines carry request ids and DID and method
fields; authorization headers and private-post plaintext are never logged.
The level (`log.level`) changes live, from the console's Logs page or:

```sh
aurora-locus settings set log.level debug
```

In Docker, read the logs with `docker compose logs -f locus`; with systemd,
`journalctl -u aurora-locus -f`. Changes made through the console, the CLI
or env are also recorded in the audit log, which the console filters and
exports.

### Traces

Set `OTEL_EXPORTER_OTLP_ENDPOINT` (for example
`http://127.0.0.1:4318`) and a serving node exports a span per request
(method, path without the query, status) over OTLP/HTTP to that collector
or agent. `OTEL_SERVICE_NAME` names the service (default `aurora-locus`);
`OTEL_EXPORTER_OTLP_HEADERS` and the other standard `OTEL_EXPORTER_*`
variables apply. The endpoint is plain HTTP: run a collector next to the
node and let it forward over TLS. Spans are at `info`, so they follow
`log.level`. Without an endpoint there are no request spans and nothing is
exported.
