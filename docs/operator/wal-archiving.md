# WAL Archiving & Point-in-Time Recovery

Operator guide for setting up Postgres write-ahead-log (WAL)
archiving against an Aurora-Locus database, enabling
point-in-time recovery (PITR).

This is **Postgres-only**. SQLite has no equivalent feature; SQLite
deployments rely on periodic [logical backups](backup-restore.md).

WAL archiving is configured at the **Postgres** layer, not at the
Aurora-Locus layer. Aurora-Locus's role is to stay out of the way:
the database backend is shared infrastructure, and PITR setup is a
Postgres operator concern that doesn't require any Aurora-Locus
configuration changes.

---

## What WAL archiving enables

A logical backup ([backup-restore.md](backup-restore.md)) gives you a
snapshot at the moment `pg_dump` ran. If your backup cadence is
nightly and a disaster strikes at 3pm, you lose ~15h of data.

WAL archiving lets you **continuously stream** Postgres's transaction
log to a durable archive. Combined with periodic base backups (weekly
or monthly), you can restore the database to **any moment in the
archive window**, down to second-level precision.

The trade-off:
- **Operational complexity**: configure archive_command, monitor
  archive health, manage archive storage.
- **Storage**: WAL volume is workload-dependent; for an
  Aurora-Locus instance with a few hundred users it's typically
  tens of MB per day, but heavier workloads scale linearly.
- **Recovery skill**: PITR is more involved than restoring a
  `pg_dump`. Practice it once before you need it.

If your RPO ("how much data can we lose?") tolerates 24h, logical
backups are simpler. If you need < 1h RPO or the ability to recover
to "just before the bad migration ran," WAL archiving is the
mechanism.

## Postgres configuration

Three Postgres settings turn WAL archiving on. Edit `postgresql.conf`
on the primary:

```ini
# Enable archive mode (requires restart, not just reload).
wal_level = replica
archive_mode = on

# Where to ship each WAL segment when Postgres rotates it.
# %p = source path, %f = WAL filename. Postgres invokes this for
# each rotated segment; success = exit 0, failure = nonzero.
archive_command = 'cp %p /var/archive/wal/%f'

# How often to force a WAL rotation even if the segment isn't full.
# Bounds your worst-case RPO: with 60s, you lose at most 60 seconds
# of writes if the primary dies between rotations.
archive_timeout = 60s
```

Restart Postgres for `archive_mode` to take effect. Subsequent
`archive_command` changes can be applied with `SELECT
pg_reload_conf();`.

Verify WAL archiving is working:

```sql
-- Should show 'on'
SHOW archive_mode;

-- Most recent successful archive (should be recent)
SELECT * FROM pg_stat_archiver;
```

If `failed_count` is increasing, your `archive_command` is broken —
check Postgres logs for the actual error. Common failure modes: the
target directory doesn't exist, doesn't have write permission for
the postgres user, or is full.

## Archive destinations

### Filesystem (development / single-host)

Simplest option. Postgres writes directly to a local or NFS-mounted
path:

```ini
archive_command = 'test ! -f /var/archive/wal/%f && cp %p /var/archive/wal/%f'
```

The `test ! -f` clause prevents overwriting an existing archive on
retry — important because Postgres retries failed `archive_command`
invocations indefinitely.

Filesystem archive is **not durable for production** unless the
filesystem itself is replicated (e.g. an NFS mount backed by an HA
storage system). A single-disk archive shares the failure domain of
the database; a disk failure that takes out Postgres takes out the
archive too.

### S3 / S3-compatible (production)

Most production deployments archive to S3 or an S3-compatible store
(MinIO, Backblaze B2, etc.) via the AWS CLI or a purpose-built tool
like `wal-g`.

**With AWS CLI:**

```ini
archive_command = 'aws s3 cp %p s3://aurora-wal-archive/$(hostname)/%f --quiet'
```

Configure AWS credentials for the postgres user (e.g.
`~postgres/.aws/credentials`). Tag the bucket with a lifecycle policy
that expires WAL segments older than your retention window.

**With wal-g** (recommended for production):

[wal-g](https://github.com/wal-g/wal-g) is a dedicated WAL archiver
that handles compression, encryption, and base-backup coordination
in one tool. Configure it per its docs and set:

```ini
archive_command = 'wal-g wal-push %p'
```

`wal-g backup-push` also produces base backups that pair with the
archived WAL for PITR. This is the cleanest production setup.

### Dedicated volume

If your deployment runs on a cloud provider with attached block
storage (EBS, GCP persistent disk, Azure managed disk), a dedicated
archive volume separate from the Postgres data volume gets you most
of the durability benefit of S3 with less operational overhead than
an external object store. Snapshot the archive volume on a schedule
appropriate for your retention.

## Performing a point-in-time recovery

Given continuous WAL archives + periodic base backups, you can
restore to any moment in the archive window. The procedure:

1. **Stop Postgres** on the recovery target host.
2. **Empty the data directory** (back up the corrupted state first
   if there's any chance you'll need it for forensics).
3. **Restore the most recent base backup** (taken before your target
   recovery time) into the data directory.
4. **Configure recovery** — create `recovery.signal` (Postgres 12+) and
   add to `postgresql.conf`:

   ```ini
   restore_command = 'cp /var/archive/wal/%f %p'
   recovery_target_time = '2026-05-02 14:30:00 UTC'
   recovery_target_action = 'promote'
   ```

   Adjust `restore_command` to match how you fetch from S3 / wal-g /
   wherever your archives live.

5. **Start Postgres**. It will replay WAL segments from the archive
   up to `recovery_target_time`, then promote.

6. **Verify** the database state matches what you expected for the
   target time. If you're recovering past a bad migration, confirm
   the schema looks right; if you're recovering past a data
   corruption event, sample the affected tables.

7. **Take a fresh base backup** immediately after recovery completes.
   The recovered cluster has a new "timeline" and your previous base
   backups are no longer compatible with the post-recovery WAL
   stream.

For multi-instance Aurora-Locus deployments, **do all of this on a
single Postgres host**, then point all aurora-locus instances at the
recovered database (they don't need any reconfiguration; the Postgres
URL is unchanged).

## Aurora-Locus-specific notes

### What gets archived

The `archive_command` ships **all** of Postgres's WAL — it doesn't
distinguish between Aurora-Locus tables, system tables, or any other
databases on the same cluster. If you run multiple databases on one
Postgres cluster, the WAL archive serves all of them; PITR recovers
all of them to the target timestamp.

Aurora-Locus typically runs against a dedicated Postgres database
(`aurora`); separating Aurora's data from other applications at the
database level is recommended even if you share a cluster.

### Recommended retention windows

| Use case | WAL retention | Base backup cadence |
|---|---|---|
| Personal / hobby instance | 24h | Daily logical backup, 7d retention |
| Small production (10s–100s of users) | 7d | Weekly base backup |
| Larger production | 30d | Weekly base backup, monthly archive |

These are starting points; tune based on your actual recovery
requirements and storage budget.

### Verifying archives are healthy

A WAL archive that silently stops working is worse than no archive,
because operators trust it exists. Set up monitoring for **two
signals**:

1. **Archive lag** (the difference between the most recently
   archived WAL position and the current WAL position). Should be
   bounded — typically a few seconds. Growing lag = `archive_command`
   is failing or slow.

2. **Archive failure count** (`pg_stat_archiver.failed_count`).
   Should be 0; any nonzero value means at least one segment failed
   to archive and Postgres is retrying. Alert on first nonzero, not
   on growth — the first failure is the warning sign.

```sql
SELECT
  archived_count,
  failed_count,
  last_archived_wal,
  last_archived_time,
  EXTRACT(EPOCH FROM (NOW() - last_archived_time)) AS lag_seconds
FROM pg_stat_archiver;
```

Wire this into your existing monitoring (Prometheus, Datadog,
whatever). Aurora-Locus's `tools.aurora.ops.getDatabaseStatus`
endpoint surfaces the basics for ad-hoc inspection but isn't a
substitute for standing alerts.

### Periodic restore drills

The same advice as logical backups: **test it before you need it**.
At least once per year, on a quiet day:

1. Pick a target time in the past 24h.
2. Restore to a separate host (or a separate Postgres cluster on
   the same host).
3. Sample the data and confirm it matches what you expect for the
   target time.
4. Document any surprises and fix them.

A drill that takes a quiet afternoon is the cheapest way to know
your PITR setup actually works.

## Further reading

- [Postgres continuous archiving documentation](https://www.postgresql.org/docs/current/continuous-archiving.html)
- [wal-g project](https://github.com/wal-g/wal-g)
- [Postgres recovery configuration reference](https://www.postgresql.org/docs/current/runtime-config-wal.html#RUNTIME-CONFIG-WAL-ARCHIVE-RECOVERY)
