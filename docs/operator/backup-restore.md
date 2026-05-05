# Backup & Restore

Operator guide for backing up and restoring the Aurora-Locus database.

This guide covers the SQLite default and the Postgres backend
(introduced in v0.2). The CLI wrappers are backend-aware: same
command, dispatches based on `PDS_DB_BACKEND` / `PDS_DB_URL`.

For point-in-time recovery via WAL archiving (Postgres only), see
[wal-archiving.md](wal-archiving.md).

---

## TL;DR

```bash
# Back up
aurora-locus backup --output /var/backups/aurora-$(date +%Y%m%d).sql --compress

# Restore (requires confirmation by default)
aurora-locus restore --input /var/backups/aurora-20260502.sql.gz
```

The wrappers read your `PDS_DB_*` env vars (or config file) and pick
the right tool: file copy for SQLite, `pg_dump` / `psql` for Postgres.

## Two backup approaches

Aurora-Locus's CLI wraps **logical backups** (`pg_dump` for Postgres,
file copy for SQLite). For most deployments, that's the right choice.
**Physical backups** (`pg_basebackup` for Postgres) are appropriate
for some scenarios but the CLI doesn't wrap them — operators who need
them invoke `pg_basebackup` directly.

| | Logical (`pg_dump`, file copy) | Physical (`pg_basebackup`) |
|---|---|---|
| Output format | SQL text (or compressed) | Raw cluster files |
| Restore target | Any compatible Postgres / SQLite version | Same Postgres major version |
| Restore time | Slower (replays SQL) | Faster (file copy) |
| Backup size | Smaller, gzip-friendly | Larger (~size of database) |
| Granularity | Full database | Full cluster |
| Best for | Smaller deployments, periodic snapshots, version migrations | Large deployments, fast restore needed |

If you're not sure which you need, start with logical (the wrapped
default). Most Aurora-Locus deployments at v0.2 scale fit comfortably
in logical backup workflows.

## SQLite (default)

The wrapper does a file copy of the configured `account_db` path,
optionally gzipping the output. The default backup is just the
account database; pass `--all` to back up the sequencer and DID-cache
databases too.

```bash
# Single-database backup
aurora-locus backup --output ~/backups/account.db

# Compressed
aurora-locus backup --output ~/backups/account.db.gz --compress

# All three databases
aurora-locus backup --output ~/backups/aurora --all --compress
```

Restoring overwrites the configured `account_db` file. The wrapper
creates a `.backup` snapshot of the existing file first, so a botched
restore can be undone manually.

```bash
aurora-locus restore --input ~/backups/account.db.gz
# Will prompt for confirmation; --yes to skip
```

## Postgres

The wrapper invokes `pg_dump` against the Postgres instance pointed
to by `PDS_DB_URL`, captures the SQL output, optionally gzips it,
and writes to `--output`.

```bash
# Set up
export PDS_DB_BACKEND=postgres
export PDS_DB_URL=postgres://aurora:secret@db.internal/aurora

# Plain SQL
aurora-locus backup --output /var/backups/aurora-$(date +%Y%m%d).sql

# Compressed (recommended for any non-trivial database)
aurora-locus backup --output /var/backups/aurora-$(date +%Y%m%d).sql.gz --compress
```

**Backup flags used internally**: `--no-owner --no-acl`. The dump is
portable across environments — no embedded ownership/grants that
might not exist on the restore target.

### Restoring

Restore reads the backup file and pipes it to `psql` against
`PDS_DB_URL`. The wrapper does **two pre-flight checks** that you
should pay attention to:

1. **Active-instance check**: tries to acquire the sequencer
   leader-election advisory lock. If the lock is held, another
   `aurora-locus` instance is live against this database — restoring
   while writes are in flight will produce an inconsistent state.
   The wrapper warns and asks for explicit confirmation.

2. **Backup-file check**: ensures the input file exists and reports
   whether it's gzip-compressed (auto-detected from extension).

After restore, the wrapper runs a **post-flight schema check**
(`SELECT COUNT(*) FROM actor`) and reports the row count. If the
schema isn't recognizable, the restore is reported as failed and
you'll need to investigate manually.

```bash
# Stop all aurora-locus instances first.
sudo systemctl stop aurora-locus    # if using systemd
# Or: kill the processes / scale deployment to 0.

# Then restore.
aurora-locus restore --input /var/backups/aurora-20260502.sql.gz
# Will prompt twice: pre-flight (if lock detected) and overwrite confirmation.

# Restart instances.
sudo systemctl start aurora-locus
```

### Multi-instance deployments

In multi-instance Postgres deployments, **back up from any instance**
— Postgres is the source of truth and all instances read the same
data. The advisory lock for sequencer leadership doesn't affect
read-only operations like `pg_dump`.

For **restores**, stop all instances first. Restoring while any
instance is live races the live writes against the restored state
and the result is unpredictable. The pre-flight check catches the
common case (one instance running locally) but operators should
explicitly confirm all instances are down before restoring.

A typical multi-instance restore sequence:

1. Scale the deployment to 0 instances (Kubernetes: `kubectl scale
   deploy aurora-locus --replicas=0`; systemd: stop all units; etc.).
2. Wait for any in-flight write transactions to finish (~10s after
   scale-to-zero is conservative).
3. Run `aurora-locus restore` from a single host.
4. Scale back up.

### Physical backups (out-of-scope for the wrapper)

For deployments that need fast restore (`pg_basebackup` restores in
the time it takes to copy files; `pg_dump` restores in the time it
takes to replay every INSERT), use `pg_basebackup` directly:

```bash
# On a host that can reach the Postgres primary:
pg_basebackup \
  --pgdata=/var/backups/aurora-base-$(date +%Y%m%d) \
  --format=tar \
  --gzip \
  --progress \
  --verbose \
  --host=db.internal \
  --username=aurora \
  --dbname=aurora
```

Restoring a `pg_basebackup` backup involves stopping Postgres,
replacing the data directory with the backup contents, and
restarting. See the
[Postgres documentation on continuous archiving](https://www.postgresql.org/docs/current/continuous-archiving.html)
for the full procedure — it's well beyond the scope of what
Aurora-Locus's wrappers handle.

## Retention

There's no one-size-fits-all retention policy. Some patterns we've
seen work in production:

- **Daily backups, 7-day retention**: small operational footprint;
  recovery is bounded to "lose at most 24h of data."
- **Hourly backups during business hours, 24h retention + daily
  archives, 30d retention**: better RPO during the working day;
  rolls into long-term archive each night.
- **Continuous WAL archiving + weekly base backups**: enables
  point-in-time recovery to any moment in the archive window. See
  [wal-archiving.md](wal-archiving.md).

Whatever cadence you pick, **test restores periodically**. A backup
you've never restored is a backup you don't actually have. A monthly
restore drill against a staging environment catches the
"oh, we forgot to back up the WAL archives" / "the backup script
was failing silently" / "the schema migration we ran in March
broke pg_dump compatibility" classes of issues before the real
disaster.

## Troubleshooting

### `pg_dump`: command not found

The wrapper shells out to the `pg_dump` and `psql` binaries.
Install them via your distro's `postgresql-client` package:

```bash
# Debian / Ubuntu
sudo apt install postgresql-client

# Alpine (e.g. inside a container)
apk add postgresql-client

# macOS
brew install libpq && brew link --force libpq
```

### Backup is unexpectedly large

`pg_dump` outputs a complete schema + data dump as text. For a
roughly 1GB database, expect a 1–2GB plain-text dump and a 100–300MB
gzip-compressed dump. If the backup is much larger than that,
something's accumulated unexpectedly (logs in a database table?
huge blob references?).

### Restore reports "Sequencer leader lock is held"

Another `aurora-locus` instance is running against the database. Stop
it before restoring. If you're certain no other instance is live,
the lock may be a stale entry from a recently-crashed process —
Postgres clears advisory locks when the connection drops, but a
network-partitioned process may still hold its connection until TCP
keepalive times out (default ~7200s on Linux). Either wait for the
keepalive to fire, or restart Postgres to clear all sessions.

### Post-flight schema check fails

`SELECT COUNT(*) FROM actor` after restore returned an error. The
backup file may be corrupted, or the restore may have hit an error
mid-stream. The wrapper uses `--single-transaction --set=ON_ERROR_STOP=on`
so a mid-restore error rolls back the whole thing — but the database
state should still be inspected manually. Check `psql`'s output
during the restore (re-run with `2>&1 | tee restore.log` to capture).
