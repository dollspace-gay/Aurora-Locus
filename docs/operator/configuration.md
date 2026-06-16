# Configuration Reference

Aurora-Locus is configured exclusively through environment variables. This
document is the complete reference: every environment variable the server reads
at startup or at runtime, grouped by functional area. For "how to actually
stand it up," see [../getting-started.md](../getting-started.md).

**Validate before starting.** After setting environment variables, run:

```bash
aurora-locus validate-config
```

`validate-config` catches required-but-missing variables, conditional
dependencies (e.g. S3 selected without credentials), and the well-known
operator-error footguns (a `did:plc:` service DID on the cross-PDS service-JWT
path, or `PDS_LEXICON_DNS_NAMESERVER` set in production). Warnings emerge
before runtime failure.

**Conventions used in this reference:**

- **Required** — startup fails if the variable is absent.
- **Default** — the compiled-in default. `{PDS_HOSTNAME}` etc. denotes
  derivation from another variable.
- **Accepted values** — for enums, the literal accepted strings; for booleans,
  `true` or `1` enables, anything else (or absence) disables.

---

## 1. Service / Network

| Variable | Required / Default | Type | Purpose |
|---|---|---|---|
| `PDS_HOSTNAME` | Default: `localhost` | string | Server hostname; basis for derived defaults (service DID, public URL, default handle suffix). |
| `PDS_PORT` | Default: `2583` | u16 | TCP listen port. |
| `PDS_SERVICE_DID` | Default: `did:web:{PDS_HOSTNAME}` | string (`did:*`) | Service DID this PDS publishes as its identity. |
| `PDS_SERVICE_PUBLIC_URL` | Default: derived from hostname + port | URL | General public-URL override. Used wherever the server emits its own URL outside the federation-crawl context (DID document, OAuth metadata). Distinct from `PDS_PUBLIC_URL` (§17 Federation). |
| `PDS_VERSION` | Default: `0.1.0` | semver string | Server version string included in `describeServer` output. |

## 2. Cryptography & Keys

All three variables are **required at startup**. The two `*_K256_PRIVATE_KEY_HEX`
variables expect **exactly 32 bytes** of secp256k1 private-key material,
hex-encoded — exactly **64 hex characters**. Any other length fails at startup
with `Private key must be exactly 32 bytes (got N)`.

Generate a key pair:

```bash
openssl rand -hex 32 > repo_key.hex
openssl rand -hex 32 > plc_key.hex
```

(Any 32-byte random value is a valid secp256k1 private key with overwhelming
probability — the bad-value probability is roughly 1 in 2²⁵⁶.)

| Variable | Required | Type | Purpose |
|---|---|---|---|
| `PDS_JWT_SECRET` | **Required** | string ≥ 32 chars | HS256 JWT signing secret for admin tokens. |
| `PDS_REPO_SIGNING_KEY_K256_PRIVATE_KEY_HEX` | **Required** | hex, exactly 64 chars (32 bytes) | secp256k1 private key for signing repo MST commits. |
| `PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX` | **Required** | hex, exactly 64 chars (32 bytes) | secp256k1 private key for PLC rotation operations. |

## 3. Storage Paths

| Variable | Default | Type | Purpose |
|---|---|---|---|
| `PDS_DATA_DIRECTORY` | `./data` | path | Root data directory; basis for the per-component path defaults below. |
| `PDS_ACCOUNT_DB_LOCATION` | `{PDS_DATA_DIRECTORY}/account.sqlite` | path | Shared-DB account database file (SQLite backend only — Postgres uses `PDS_DB_URL`). |
| `PDS_SEQUENCER_DB_LOCATION` | `{PDS_DATA_DIRECTORY}/sequencer.sqlite` | path | Sequencer event log database (SQLite backend only). |
| `PDS_DID_CACHE_DB_LOCATION` | `{PDS_DATA_DIRECTORY}/did_cache.sqlite` | path | DID document / handle cache database (SQLite backend only). |
| `PDS_ACTOR_STORE_DIRECTORY` | `{PDS_DATA_DIRECTORY}/actors` | path | Per-actor repository storage. Always SQLite regardless of the shared-DB backend. |

## 4. Database & Backend Selection

Two variables select the shared-DB backend; `PDS_DB_URL` is **required when
`PDS_DB_BACKEND=postgres`** and optional under SQLite. Under SQLite, when
`PDS_DB_URL` is unset, the per-component paths in §3 resolve under
`{PDS_DATA_DIRECTORY}`.

| Variable | Default | Accepted values / Type | Purpose |
|---|---|---|---|
| `PDS_DB_BACKEND` | `sqlite` | `sqlite` / `postgres` / `postgresql` (case-insensitive) | Shared-DB backend selector. |
| `PDS_DB_URL` | None (SQLite); **required if Postgres** | `sqlite://path`, `postgres://user:pass@host:port/db`, or `postgresql://...` | Database connection URL. Wrong scheme for the selected backend fails at startup. |
| `PDS_DB_MAX_CONNECTIONS` | `25` | u32 > 0 | Connection pool max size. |
| `PDS_DB_MIN_CONNECTIONS` | `5` | u32 (0 ≤ min ≤ max) | Connection pool min size. |
| `PDS_DB_ACQUIRE_TIMEOUT_SECS` | `30` | u64 > 0 | Pool acquire timeout (seconds). |
| `PDS_DB_IDLE_TIMEOUT_SECS` | None | u64 (optional) | Pool idle timeout (seconds). |
| `PDS_DB_MAX_LIFETIME_SECS` | None | u64 (optional) | Pool connection max lifetime (seconds). |

## 5. Database — Postgres-Specific

Only consulted when `PDS_DB_BACKEND=postgres`.

| Variable | Default | Accepted values | Purpose |
|---|---|---|---|
| `PDS_SEQUENCER_LEADER_RETRY_MS` | `2000` | u64 (500-30000) | Standby retry interval for the sequencer leader-election loop. SQLite skips leader election entirely. |
| `PDS_DATABASE_PG_TRANSACTION_ISOLATION` | `read committed` | `read uncommitted` / `read committed` / `repeatable read` / `serializable` (case-insensitive) | Postgres connection-level transaction isolation pin. Default preserves the GC sweep's predicate-disjointness argument; raising the isolation level may trigger serialization failures on the sweep DELETE (currently not retried). `validate-config` warns when an active Postgres backend sets a non-default value. |

## 6. Distributed-State Substrate & Maintenance Pool

The substrate backs cross-instance DPoP JTI replay tracking, rate-limit
buckets, and OAuth flow state. With `PDS_DB_BACKEND=sqlite`, the substrate
still loads but offers no multi-instance benefit (operators see a startup
warning to that effect).

| Variable | Default | Accepted values | Purpose |
|---|---|---|---|
| `PDS_DISTRIBUTED_STATE_MODE` | `distributed` | `distributed` / `single_instance_inmemory` / `redis` | Substrate selector. `distributed` = Postgres-CAS (required for multi-instance HA). `single_instance_inmemory` = in-process only (auth state lost on restart; operator opt-in). `redis` = forward-compat slot; currently fails at startup with a clear error. |
| `PDS_MAINTENANCE_DB_MAX_CONNECTIONS` | `15` | u32 > 0 | Maintenance-pool max connections. Sized smaller than the main pool so total Postgres connection count stays predictable. |
| `PDS_MAINTENANCE_DB_MIN_CONNECTIONS` | `2` | u32 (0 ≤ min ≤ max) | Maintenance-pool min connections. |
| `PDS_MAINTENANCE_DB_ACQUIRE_TIMEOUT_SECS` | `10` | u64 > 0 | Maintenance-pool acquire timeout. Tighter than the main pool so DPoP / rate-limit paths fail fast under contention rather than block request threads. |

## 7. Blob Storage

Disk and S3 modes are **mutually exclusive**. Presence of
`PDS_BLOBSTORE_S3_BUCKET` triggers S3 mode; otherwise disk is used. When S3 is
selected, `PDS_BLOBSTORE_S3_ACCESS_KEY_ID` and
`PDS_BLOBSTORE_S3_SECRET_ACCESS_KEY` are both **required**.

| Variable | Default | Type | Purpose |
|---|---|---|---|
| `PDS_BLOB_UPLOAD_LIMIT` | `5242880` (5 MiB) | u64 bytes | Max blob upload size. |
| `PDS_BLOBSTORE_DISK_LOCATION` | `{PDS_DATA_DIRECTORY}/blobs` | path | Disk blob storage location (disk mode). |
| `PDS_BLOBSTORE_DISK_TMP_LOCATION` | `{PDS_DATA_DIRECTORY}/temp` | path | Disk staging directory (disk mode). |
| `PDS_BLOBSTORE_S3_BUCKET` | None | string | S3 bucket name. Presence triggers S3 mode. |
| `PDS_BLOBSTORE_S3_REGION` | `us-east-1` | string | AWS region. |
| `PDS_BLOBSTORE_S3_ENDPOINT` | None | URL | Custom S3 endpoint (MinIO, DigitalOcean Spaces, etc.). |
| `PDS_BLOBSTORE_S3_ACCESS_KEY_ID` | **Required if S3** | string | S3 access key. |
| `PDS_BLOBSTORE_S3_SECRET_ACCESS_KEY` | **Required if S3** | string | S3 secret key. |
| `PDS_BLOBSTORE_S3_PREFIX` | `blobs/` | string | S3 object-key prefix. |
| `PDS_BLOBSTORE_S3_FORCE_PATH_STYLE` | `false` | boolean | Path-style vs. virtual-hosted S3 addressing. |
| `PDS_BLOBSTORE_S3_UPLOAD_TIMEOUT_MS` | `20000` | u64 ms | S3 upload operation timeout. |

## 8. Blob Lifecycle

| Variable | Default | Type | Purpose |
|---|---|---|---|
| `PDS_BLOB_STAGE_TTL_SECONDS` | `86400` (24h) | u64 seconds | TTL for `temp_blob_metadata` rows before the staged-orphan reaper reclaims them. |
| `PDS_SERVICE_MAX_BLOB_FETCH_SIZE` | `50000000` (50 MiB) | u64 bytes | Per-blob memory cap for the origin-blob-fetch primitive. HEAD pre-check + streaming bound. |
| `PDS_SERVICE_BLOB_FETCH_TIMEOUT_SECONDS` | `30` | u64 seconds | Per-attempt timeout for origin-blob HTTP GET. Inner retry budget may issue multiple attempts within this. |
| `PDS_SERVICE_BLOB_FETCH_MAX_RETRIES` | `3` | u32 | Retry budget after the first attempt (total attempts ≤ 1 + this). 5xx / network / timeout retry; 4xx is durable (no retry). |

## 9. importRepo

| Variable | Default | Type | Purpose |
|---|---|---|---|
| `PDS_SERVICE_ACCEPTING_IMPORTS` | `true` | boolean | Master drain switch. When `false`, importRepo short-circuits with HTTP 503 inside the single-flight lock so in-flight imports finish but new ones are refused. |
| `PDS_SERVICE_MAX_IMPORT_SIZE` | None (unbounded) | u64 bytes (optional) | Streaming size cap for importRepo CAR bodies, enforced during decode. Returns HTTP 413 on overflow. `None` disables the cap (useful for dev workflows; set explicitly for production). |

## 10. Authentication / OAuth

Two prefix families in this section: `PDS_OAUTH_*` for routing configuration,
and `OAUTH_*` (no prefix) for the feature-flag set introduced during the
OAuth 2.1 migration.

| Variable | Default | Type | Purpose |
|---|---|---|---|
| `PDS_OAUTH_CLIENT_ID` | derived from `PDS_HOSTNAME` | URL | OAuth client metadata URL (e.g. `https://{hostname}/oauth/client-metadata.json`). |
| `PDS_OAUTH_REDIRECT_URI` | derived from `PDS_HOSTNAME` | URL | OAuth redirect callback (e.g. `https://{hostname}/admin-oauth/callback`). |
| `PDS_OAUTH_PDS_URL` | `https://bsky.social` | URL | PDS URL for OAuth login (e.g. `https://bsky.social` for upstream federation). |
| `OAUTH_ENABLED` | `false` | boolean | Master switch for the OAuth 2.1 authorization endpoints. |
| `OAUTH_ROLLOUT_PERCENTAGE` | `0` | u8 (0-100) | Gradual-rollout percentage (hash-based per DID). |
| `OAUTH_REQUIRE_DPOP` | `false` | boolean | Require DPoP token binding. Development: `false`. Production: set `true`. |
| `OAUTH_ENABLE_AUTHORIZE` | `false` | boolean | Enable `/oauth/authorize` endpoint. |
| `OAUTH_ENABLE_TOKEN` | `false` | boolean | Enable `/oauth/token` endpoint. |
| `OAUTH_ENABLE_DEVICE_MANAGEMENT` | `false` | boolean | Enable device-management endpoints. |
| `OAUTH_ALLOW_JWT_FALLBACK` | `true` | boolean | Accept JWT tokens alongside OAuth during the migration window. |

## 11. Identity / DID

| Variable | Default | Type | Purpose |
|---|---|---|---|
| `PDS_DID_PLC_URL` | `https://plc.directory` | URL | PLC directory URL for DID operations. |
| `PDS_SERVICE_HANDLE_DOMAINS` | `.{PDS_HOSTNAME}` | comma-separated strings | Service handle suffix domain(s); the first entry is used for handle construction during account creation. |
| `PDS_DID_CACHE_STALE_TTL` | `3600` (1h) | u64 seconds | DID document cache stale TTL; entries older than this trigger background refresh while still serving the cached value. |
| `PDS_DID_CACHE_MAX_TTL` | `86400` (24h) | u64 seconds | DID document cache hard expiry; entries older than this are evicted entirely. Must be ≥ stale TTL. |
| `PDS_IDENTITY_RECOVERY_DID_KEY` | None | `did:key:*` | Optional PDS-wide recovery key. When set, prepended to every new account's PLC rotation-key list after the per-account key. |

## 12. Email / SMTP

| Variable | Default | Type | Purpose |
|---|---|---|---|
| `PDS_EMAIL_SMTP_URL` | None | URL (`smtp://` or `smtps://`) | SMTP server URL. Presence enables email integration; absence disables email features (password reset, notifications). |
| `PDS_EMAIL_FROM_ADDRESS` | `noreply@{PDS_HOSTNAME}` | email | "From" address for outgoing SMTP messages. |

## 13. Invite System

| Variable | Default | Type | Purpose |
|---|---|---|---|
| `PDS_INVITE_REQUIRED` | `false` | boolean | Require invite codes for account creation. |
| `PDS_INVITE_INTERVAL` | `604800` (7d) | u64 seconds | Invite issuance interval. |
| `PDS_INVITE_EPOCH` | `2024-01-01T00:00:00Z` | RFC3339 timestamp | Invite epoch (start of the first interval). |

## 14. Rate Limiting

| Variable | Default | Type | Purpose |
|---|---|---|---|
| `PDS_RATE_LIMITS_ENABLED` | `true` | boolean | Master switch for the rate limiter. |
| `PDS_RATE_LIMIT_GLOBAL_REQUESTS_PER_MINUTE` | `3000` | u32 | Global request rate limit (requests per minute). |
| `PDS_RATE_LIMIT_EXEMPT_ADMIN_ASSETS` | `true` | boolean | Bypass the limiter for GET requests to admin-UI static assets. |
| `PDS_RATE_LIMIT_BUCKETS_RETENTION_DAYS` | `7` | u32 | Inactivity threshold for `rate_limit_buckets` reaper sweeps (whole days). |

## 15. Logging

| Variable | Default | Accepted values | Purpose |
|---|---|---|---|
| `RUST_LOG` | `aurora_locus=info,tower_http=info` | tracing filter | Rust logging level / module filter (e.g. `aurora_locus=debug,aurora_locus::federation=trace`). |
| `LOG_FORMAT` | `text` | `text` / `json` | Log output format. `text` is pretty-printed for development; `json` for production log aggregators. |

## 16. Validation

| Variable | Default | Accepted values | Purpose |
|---|---|---|---|
| `VALIDATION_MODE` | `optimistic` | `required` / `optimistic` / `none` (case-insensitive) | Record schema validation mode. `required` hard-rejects schema-violating writes before commit. `optimistic` absorbs violations into a tracking row but accepts the write. `none` disables validation. |

## 17. Federation

**Federation is on by default.** ATProto PDSes are federation peers; a fresh
deployment participates in the ATProto network on startup. Operators with
closed-network, single-tenant, development, or pre-go-live deployments opt
out by setting `PDS_FEDERATION_ENABLED=false`. An opted-out PDS is not a
participating ATProto peer — users on it can't be followed from elsewhere,
posts don't appear in cross-PDS feeds, and the firehose doesn't emit.

**The two PUBLIC_URL vars are easy to confuse:**

- **`PDS_PUBLIC_URL`** (this section) — federation-specific. Used when relays
  need the URL to crawl back to this PDS.
- **`PDS_SERVICE_PUBLIC_URL`** (§1 Service / Network) — general public-URL
  override used wherever the server emits its own URL outside the
  federation-crawl context (DID document, OAuth metadata).

Set both to the same value in most deployments; the distinction matters only
when the federation-crawl URL differs from the server's general public URL
(e.g. when fronted by a separate proxy for the federation surface).

| Variable | Default | Type | Purpose |
|---|---|---|---|
| `PDS_FEDERATION_ENABLED` | `true` | boolean | Master switch for federation with relays. Set `false` to disable for closed-network / single-tenant / development deployments. |
| `PDS_FEDERATION_RELAY_URLS` | `https://bsky.network` | comma-separated URLs | Relay server URLs. An empty list disables the relay loop entirely (federation may still be active for peer-PDS / entryway flows). |
| `PDS_FEDERATION_FIREHOSE_ENABLED` | `false` | boolean | Enable the WebSocket firehose endpoint (`com.atproto.sync.subscribeRepos`). |
| `PDS_FEDERATION_CRAWL_ENABLED` | `false` | boolean | Allow relays to crawl this PDS's repositories. |
| `PDS_FEDERATION_AUTO_STREAM` | `false` | boolean | Auto-publish events to configured relays without explicit operator action. |
| `PDS_PUBLIC_URL` | None | URL | Federation-specific public URL (must be internet-accessible if `PDS_FEDERATION_CRAWL_ENABLED=true`). |
| `PDS_FEDERATION_PEER_PDS` | None | CSV of `did@url` pairs | Trusted peer-PDS allowlist. Malformed entries fail at startup (all-or-nothing). Joins the trusted-issuer allowlist for cross-PDS auth. |
| `PDS_APPVIEW_URL` | None | URL | AppView URL for feed/profile proxying (e.g. `https://api.bsky.app`). |
| `PDS_REPO_BACKFILL_LIMIT_MS` | `86400000` (1 day) | u64 milliseconds | Sequencer backfill window for relay firehose replay. Caps how far back a connecting consumer can request events. |

## 18. Entryway

**All four variables are required together, or all absent.** Setting some but
not all fails at startup.

| Variable | Type | Purpose |
|---|---|---|
| `PDS_ENTRYWAY_URL` | URL | Entryway base URL (e.g. `https://entryway.example.com`). |
| `PDS_ENTRYWAY_ADMIN_TOKEN` | string | Admin Basic-auth token; pre-bound on `entryway_admin_client`. |
| `PDS_ENTRYWAY_JWT_PUBLIC_KEY_HEX` | hex, exactly 66 chars (33 bytes, SEC1-compressed secp256k1 public key) | Entryway's ES256K JWT-signing public key. |
| `PDS_ENTRYWAY_DID` | `did:*` | Entryway DID. Joins the trusted-issuer allowlist and becomes an accepted audience for `require_auth_forwarded` routes. |

## 19. GC Sweep

The background blob garbage-collection sweep is **enabled by default** but
runs in **dry-run mode** by default — it classifies orphans and logs them
but does not delete anything until an operator explicitly opts in by setting
`PDS_GC_SWEEP_DRY_RUN=false`.

| Variable | Default | Type | Purpose |
|---|---|---|---|
| `PDS_GC_SWEEP_ENABLED` | `true` | boolean | Master switch for the scheduled background sweep (both byte and row walkers). |
| `PDS_GC_SWEEP_ROW_SWEEP_ENABLED` | `true` | boolean | Row-walker (untethered `blob_metadata` rows) on/off toggle, sharing the same cadence as the byte walker. |
| `PDS_GC_SWEEP_INTERVAL_SECS` | `86400` (24h) | u64 seconds > 0 | Sweep run cadence (shared between byte and row walkers). |
| `PDS_GC_SWEEP_DRY_RUN` | `true` | boolean | Classify and log only; do not delete. Operators promote to destructive after observing the reported orphan rate. |
| `PDS_GC_SWEEP_MAX_DELETES_PER_RUN` | `10000` | usize | Safety cap on deletes per sweep run. |
| `PDS_GC_SWEEP_FRESHNESS_THRESHOLD_SECS` | `3600` (1h) | u64 seconds > 0 | Belt-and-suspenders freshness threshold. Blobs younger than this are never considered orphans, even when absent from `temp_blob_metadata`. |
| `PDS_GC_SWEEP_PAGE_SIZE` | `500` | usize > 0 | Storage pagination page size. |
| `PDS_GC_SWEEP_UNTETHERED_TTL_SECS` | `86400` (24h) | u64 seconds > 0 | Row-sweep TTL anchor. Untethered `blob_metadata` rows (temp_key still set) older than this are eligible for DELETE plus a bytes-delete. |

## 20. Lexicon

Dynamic lexicon loading is **off by default**. When enabled, the server
resolves unknown-NSID record collections against authoring repositories via
DNS-TXT authority resolution + HTTP fetch + two-layer cache.

**`PDS_LEXICON_DNS_NAMESERVER` must NOT be set in production.** It is a
test-harness affordance that silently retargets every lexicon DNS TXT lookup to
the operator-supplied nameserver with caching disabled, breaking live
federation resolution. `validate-config` emits an explicit warning when this
variable is present.

| Variable | Default | Type | Purpose |
|---|---|---|---|
| `PDS_LEXICON_ENABLED` | `false` | boolean | Master switch for dynamic lexicon loading. |
| `PDS_LEXICON_DID_AUTHORITY` | None | `did:*` | Optional authority-DID override. When set, the resolver bypasses DNS TXT + PLC and uses this DID as authority for every NSID (useful for testing and homogeneous-federation deployments). |
| `PDS_LEXICON_FETCH_FAILURE_BEHAVIOR` | `warn` | `hard_fail` / `hardfail` / `strict` (→ HardFail); `warn` / `optimistic` (→ Warn) | Behavior on lexicon fetch failure. `HardFail` propagates the error; `Warn` logs and falls back to Optimistic acceptance. |
| `PDS_LEXICON_FETCH_MAX_RETRIES` | `3` | u32 | HTTP retries on lexicon-record fetch. |
| `PDS_LEXICON_FETCH_TIMEOUT_SECS` | `30` | u64 seconds > 0 | Per-attempt timeout for HTTP lexicon-record fetch. |
| `PDS_LEXICON_CACHE_TTL_SECS` | `86400` (24h) | u64 seconds > 0 | In-memory lexicon cache TTL. Expired entries trigger background re-fetch while still serving the cached value. |
| `PDS_LEXICON_LAST_USED_PERSIST_THRESHOLD_SECS` | `60` | u64 seconds ≥ 0 | Throttle floor for on-disk `last_used_at` writes; prevents hot NSIDs from hammering the cache table. |
| `PDS_LEXICON_NAMESPACE_DENYLIST` | None | CSV of NSID prefixes | Denylisted collections; rejected with `NamespaceDenied` error. |
| `PDS_LEXICON_NAMESPACE_ALLOWLIST` | None | CSV of NSID prefixes | When non-empty: only matching collections route to lexicon fetch; non-matching fall through to Optimistic validation (exclusion, not rejection). |
| `PDS_LEXICON_VALIDATE_IMPORTS` | `true` | boolean | Apply lexicon validation to CAR-import records. Heterogeneous-federation deployments leave this on; homogeneous deployments may disable it to skip redundant work. |
| `PDS_LEXICON_DNS_NAMESERVER` | None | `ip:port` | Test-harness affordance — custom DNS nameserver for `_lexicon.<host>` TXT lookups. **Forbidden in production.** |

## 21. Runtime Settings & Recovery

The runtime-settings surface (`getRuntimeSetting` / `setRuntimeSetting`)
resolves keys through a four-tier hierarchy: recovery-mode env override →
runtime DB row → file-tier YAML → compiled-in default. These two variables
tune the recovery override and the file-tier YAML path.

| Variable | Default | Type | Purpose |
|---|---|---|---|
| `PDS_RUNTIME_FILE` | `{PDS_DATA_DIRECTORY}/runtime.yaml` | path | File-tier YAML path. Unknown keys warn-and-skip; invalid values warn-and-skip; malformed YAML produces a clear startup error with the file path. |
| `AURORA_RECOVERY_MODE` | `false` | boolean | Recovery-mode override. When `true`, bypasses the tier hierarchy for emergency operator action (read-only on `moderation-mode`). |

For the per-key value formats accepted by runtime settings, see
[file-tier-config.md](file-tier-config.md).

## 22. Debug

| Variable | Default | Accepted values | Purpose |
|---|---|---|---|
| `PDS_ENABLE_DEBUG_PAGES` | `false` | `true` / `1` to enable | Opt-in debug pages in the Aurora-Locus admin. **The debug pages render the bearer token visibly in the DOM — local-development only.** When disabled (default), the pages 404 on any non-localhost binding. |

## 23. Wire Deprecation & Moderation Retention

| Variable | Default | Type | Purpose |
|---|---|---|---|
| `PDS_V03_WIRE_SUNSET_DATE` | `deprecated` (sentinel for "unset") | HTTP-date string or `deprecated` | When set to a real HTTP-date string, the `Sunset:` header emits on responses that use the legacy wire shape. The sentinel value `deprecated` suppresses the `Sunset:` header while still serving legacy fields. |
| `PDS_MOD_EVENT_RETENTION_DAYS` | `7` | i64 days (positive) | Retention window for `mod_event_seq` rows. Operators running long-lived deployments raise this; typos or non-positive values fall back to the default rather than infinite retention or immediate purge. |

## 24. Backup

Backup configuration is documented separately. See
[backup-restore.md](backup-restore.md) for the `BACKUP_*` variable set and the
backup operator workflow.

---

## Cross-references

- **Install walkthrough**: [../getting-started.md](../getting-started.md)
- **Multi-instance deployment** (Postgres + leader election): [multi-instance-deployment.md](multi-instance-deployment.md)
- **WAL archiving + PITR** (Postgres backup): [wal-archiving.md](wal-archiving.md)
- **SuperAdmin bootstrap + admin auth**: [admin-auth.md](admin-auth.md)
- **Runtime-settings value formats**: [file-tier-config.md](file-tier-config.md)
- **Backup + restore**: [backup-restore.md](backup-restore.md)
