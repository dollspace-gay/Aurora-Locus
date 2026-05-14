# Blob storage GC sweep

Aurora-Locus ships an optional background sweep that
reconciles blob storage against the authoritative `blob`
metadata table and (optionally) deletes orphaned storage
entries — blobs present in storage with no corresponding DB
row.

This is reconciliation infrastructure for the rare cases where
Arc 4's `DeferredAction` queue's best-effort cleanup fails to
land. The sweep is **off by default** in v0.4. Operators opt
in via config after determining whether their deployment's
orphan rate justifies the additional background task.

---

## What the sweep does

Each run walks blob storage in paginated order, cross-
references every candidate against the `blob` table
(authoritative authorized-blob surface) and the
`temp_blob_metadata` table (authoritative in-flight upload
surface), and classifies each blob:

| Classification | Meaning | Action |
|----------------|---------|--------|
| `Authorized` | Present in `blob`. | Skip. |
| `InFlight` | Present in `temp_blob_metadata`. | Skip regardless of age. |
| `TooYoung` | Absent from both tables, but younger than the freshness threshold (1h default). | Skip; re-evaluate next run. |
| `ConfirmedOrphan` | Absent from both tables and older than the freshness threshold. | Delete (destructive mode) or log (dry-run). |

The classifier's precedence is `Authorized > InFlight > age`:
the tracking surfaces are authoritative; the freshness
threshold is belt-and-braces for the rare race where storage
list returns a CID whose `temp_blob_metadata` row hasn't yet
committed.

The sweep does **not** modify the `DeleteBlob` flow or the
`DeferredAction` queue. It sits alongside as reconciliation
infrastructure.

---

## When to enable the sweep

Most deployments do not need it. The Arc 4 `DeferredAction`
queue handles the common case: when a blob is deleted from
storage but the DB row deletion fails (or vice versa), the
queue retries until both halves succeed. The sweep is for the
rare deployments where:

- The `DeferredAction` queue's max retries are exhausted on
  some entries (e.g., extended storage backend outages).
- The PDS process was forcibly terminated mid-cleanup,
  leaving partial state the queue never replayed.
- Manual operator action created divergence between storage
  and DB.

Indicators that enabling the sweep may be valuable:

- Storage byte-count growing faster than `blob` row count
  growth over time.
- Periodic operator reports of "phantom blobs" — storage
  entries no account holds.
- Storage cost pressure where every orphaned blob is
  unnecessary spend.

If none of these apply, leave the sweep off.

---

## Enabling the sweep

Two configuration paths. Both require a PDS restart for the
change to take effect.

### File tier (recommended for permanent enablement)

In your `aurora-locus.yaml` (or equivalent config file
deserialized into `ServerConfig`):

```yaml
gc_sweep:
  enabled: true
  dry_run: true            # mandatory for shakedown; flip later
  interval_secs: 86400     # 24h cadence
  max_deletes_per_run: 10000
  freshness_threshold_secs: 3600
  page_size: 500
```

### Environment variables (recommended for testing or
operator-managed staging environments)

```bash
export PDS_GC_SWEEP_ENABLED=true
export PDS_GC_SWEEP_DRY_RUN=true
export PDS_GC_SWEEP_INTERVAL_SECS=86400
export PDS_GC_SWEEP_MAX_DELETES_PER_RUN=10000
export PDS_GC_SWEEP_FRESHNESS_THRESHOLD_SECS=3600
export PDS_GC_SWEEP_PAGE_SIZE=500
```

Zero `interval_secs` and `page_size` are rejected at startup.
Unparseable bool / numeric env values surface as validation
errors rather than silently defaulting.

Confirm the change took effect by looking for this line in
the PDS startup log:

```
GC sweep job scheduled  interval_secs=86400 dry_run=true max_deletes_per_run=10000
```

If `gc_sweep.enabled = false`, the startup log shows
`GC sweep job disabled (gc_sweep.enabled = false)` at debug
level instead.

---

## The dry-run shakedown

When first enabling the sweep, **always start with
`dry_run: true`**. The sweep classifies blobs and logs the
results without performing any deletes.

Run for at least 7 days of cadence (typically 7 sweep runs at
24h cadence). Inspect the structured log fields from each
sweep's "GC sweep complete" line:

```
pages_scanned=42 blobs_examined=20834 authorized=20809
in_flight=12 too_young=8 confirmed_orphans_found=5
orphans_deleted=0 orphans_skipped_safety_cap=0
duration_seconds=14.31
```

Verify three things over the shakedown window:

1. **Classification accuracy.** Confirmed orphans match
   actual orphans. Pick a few CIDs from the orphan logs and
   cross-reference manually:

   ```bash
   sqlite3 data/account.sqlite \
     "SELECT cid FROM blob WHERE cid = '<cid>';"
   sqlite3 data/account.sqlite \
     "SELECT cid FROM temp_blob_metadata WHERE cid = '<cid>';"
   ```

   Both should return zero rows for a true orphan.

2. **Orphan rate sanity.** The per-sweep
   `confirmed_orphans_found` count is consistent and
   expected. A sudden spike suggests something is wrong
   upstream (e.g., a deployment bug producing storage rows
   without DB anchors); investigate before promoting to
   destructive mode.

3. **Sweep duration.** `duration_seconds` is a small
   fraction of `interval_secs`. A sweep approaching the
   interval can't keep up with backend growth — see
   "Sweep duration approaching interval" below.

After the shakedown confirms accurate classification and
sensible orphan rates, flip `dry_run: false` and restart the
PDS. The sweep will now delete confirmed orphans subject to
the safety cap.

---

## CLI subcommand

For operator-initiated one-off sweeps (forensic, recovery, or
batch cleanup work):

```bash
aurora-locus gc-sweep [OPTIONS]
```

**Offline-only.** The CLI acquires the same PDS-liveness lock
that `serve` would, so it fast-fails if a PDS is running
against the same database:

```
Cannot run gc-sweep: PDS liveness lock is held by another
process. Stop the PDS before running gc-sweep, or enable the
scheduled `gc_sweep_job` via PDS_GC_SWEEP_ENABLED=true for
online sweeps.
```

For online sweeps, enable the scheduled background job above.
The CLI is for situations where stopping the PDS is
acceptable (post-incident recovery, scheduled maintenance
windows, batch reconciliation after a database migration).

### Options

| Flag | Effect |
|------|--------|
| `--dry-run` | Force `dry_run = true` regardless of config. Safety-direction only — there is no `--no-dry-run`; edit config + restart for destructive mode. |
| `--report-only` | Force `report_only = true`. Same loop behaviour as `--dry-run` in v0.4 (both classify-and-log); separate flag for operator-intent disambiguation in audit logs. |
| `--max-deletes <N>` | Override `gc_sweep.max_deletes_per_run` for this run. |
| `--threshold-secs <N>` | Override `gc_sweep.freshness_threshold_secs` for this run. |
| `--page-size <N>` | Override `gc_sweep.page_size` for this run. |

### Example: forensic dry-run with extended threshold

```bash
aurora-locus gc-sweep --report-only --threshold-secs 7200
```

Runs a classify-and-log sweep treating only blobs older than
2 hours as orphan candidates (vs the 1-hour default). Useful
when investigating whether a recent deployment introduced
orphans — the extended threshold rules out blobs from the
last hour's request volume.

### Output

```
GC sweep starting:
  dry_run:             true
  report_only:         true
  max_deletes_per_run: 10000
  freshness_threshold: 7200s
  page_size:           500

GC sweep complete:
  pages scanned:               42
  blobs examined:              20834
  authorized:                  20809
  in-flight:                   12
  too young:                   8
  confirmed orphans found:     5
  orphans deleted:             0
  orphans skipped (safety cap): 0
  duration:                    14.31s
```

---

## Metrics

The sweep emits three Prometheus metrics from
`src/metrics.rs`:

| Metric | Kind | Meaning |
|--------|------|---------|
| `gc_sweep_orphans_found_total` | Counter | Total blobs classified as confirmed orphans since process start. Counts dry-run classifications too. |
| `gc_sweep_orphans_deleted_total` | Counter | Total confirmed orphans actually deleted. Always ≤ `orphans_found`; difference indicates dry-run runs or safety-cap hits. |
| `gc_sweep_duration_seconds` | Histogram | Wall-clock duration of each sweep run. |

### Derivable signals

- **Cap-hit rate.** With `dry_run: false`,
  `gc_sweep_orphans_found_total - gc_sweep_orphans_deleted_total`
  > 0 means the safety cap is biting. If this difference
  grows over time, consider raising `max_deletes_per_run` or
  investigating the orphan-generation rate upstream.
- **Sweep duration vs interval.**
  `gc_sweep_duration_seconds`'s p99 approaching
  `interval_secs` indicates the sweep can't keep up with
  storage growth. Mitigations are listed under "Sweep
  duration approaching interval" below.
- **Dry-run vs destructive ratio.** While `dry_run: true`,
  `gc_sweep_orphans_deleted_total` stays at 0 and
  `gc_sweep_orphans_found_total` grows monotonically. Flipping
  to destructive mode shows up as `orphans_deleted` starting
  to climb from a fresh baseline.

---

## Troubleshooting

### Sweep not running

1. Confirm `gc_sweep.enabled: true` in config (file tier or
   env var).
2. Check the PDS startup log for `GC sweep job scheduled`. If
   absent, the config didn't load. Run
   `aurora-locus validate-config` to inspect.
3. Confirm the PDS has been restarted since the config change.
   The `gc_sweep` block is read once at startup; runtime
   changes have no effect until restart.

### Sweep running but no orphans found

The most common cause: the deployment has no orphans. The
sweep is working correctly; it's just reporting an empty
surface. Confirm by comparing storage count to DB count:

```bash
# Disk backend:
find /path/to/blob/storage -type f | wc -l

# S3 backend:
aws s3 ls s3://bucket/prefix/ --recursive --summarize \
  | tail -2

# DB row count:
sqlite3 data/account.sqlite "SELECT COUNT(*) FROM blob;"
# or
psql -d aurora -c "SELECT COUNT(*) FROM blob;"
```

If storage count ≈ DB count, there are no orphans and the
sweep correctly reports `confirmed_orphans_found: 0` every
run. Nothing to do.

### Sweep deleting more than expected

If `dry_run: false` and `orphans_deleted` is climbing faster
than the shakedown window suggested, **immediately**:

1. Flip `dry_run: true` and restart the PDS. This stops
   further destructive sweeps.
2. Inspect the next sweep's logs to verify classification.
3. Cross-reference a few CIDs the sweep classified as
   orphans against `blob` and `temp_blob_metadata` manually.
4. If genuine in-flight uploads are being classified as
   orphans, raise `freshness_threshold_secs` to widen the
   belt-and-braces window. Confirm there are no operational
   paths producing storage entries that bypass
   `temp_blob_metadata` (uncommon — only direct disk drops
   would).

Confirmed false-positive orphan deletes cannot be reversed
via the sweep; affected blobs must be re-uploaded.

### Sweep duration approaching interval

If `gc_sweep_duration_seconds` is approaching
`interval_secs`, the sweep is at risk of overlapping with
itself. The scheduled job's `tokio::time::interval` uses the
default `MissedTickBehavior` (`Burst`), so a long-running
sweep delays the next tick rather than queueing extra runs —
but if the gap closes entirely, the sweep starves out other
work on the runtime.

Mitigations, in order of preference:

1. **Increase `interval_secs`.** Less frequent sweeps. Most
   deployments don't need 24h cadence; 6-12h is often
   sufficient once the orphan baseline is established.
2. **Decrease `page_size`.** Smaller pages reduce
   per-page memory pressure and let the runtime interleave
   other tasks more responsively, though total roundtrip
   count goes up.
3. **Investigate orphan-generation upstream.** A sweep with
   tens of thousands of orphans per run on a deployment with
   few accounts suggests something is producing storage
   entries without DB anchors. Fix the upstream cause; the
   sweep duration shrinks naturally.

A stateful sweep mode (persistent cursor between runs, so a
single very-long sweep can span multiple intervals) is a v0.6
candidate if v0.4's stateless mode proves insufficient for
operational deployments.

### `validate-config` warnings

`aurora-locus validate-config` surfaces four warnings for
risky `gc_sweep` configurations. All are gated on
`gc_sweep.enabled = true` so off-by-default deployments stay
warning-free:

- **`dry_run: false`** — recommend a 7-day shakedown before
  destructive mode.
- **`dry_run: false` AND `max_deletes_per_run > 100000`** —
  blast-radius warning.
- **`freshness_threshold_secs < 600`** — in-flight false-
  positive risk.
- **`interval_secs < 3600`** — cadence vs throughput risk.

Address each warning before promoting the deployment.

---

## Configuration reference

| Field | Env var | Default | Allowed | Notes |
|-------|---------|---------|---------|-------|
| `enabled` | `PDS_GC_SWEEP_ENABLED` | `false` | `true` \| `false` | Off-by-default; opt in explicitly. |
| `interval_secs` | `PDS_GC_SWEEP_INTERVAL_SECS` | `86400` (24h) | `>0` | Time between scheduled runs. |
| `dry_run` | `PDS_GC_SWEEP_DRY_RUN` | `true` | `true` \| `false` | Classify-and-log vs delete. |
| `max_deletes_per_run` | `PDS_GC_SWEEP_MAX_DELETES_PER_RUN` | `10000` | `>0` | Safety cap; excess orphans logged and deferred. |
| `freshness_threshold_secs` | `PDS_GC_SWEEP_FRESHNESS_THRESHOLD_SECS` | `3600` (1h) | `>0` | Belt-and-braces age threshold. |
| `page_size` | `PDS_GC_SWEEP_PAGE_SIZE` | `500` | `>0` | Storage walk page size; benchmarked index-driven at 500. |

---

## Related

- **Arc 4 `DeferredAction` queue** — the primary cleanup
  mechanism. The sweep is reconciliation for cases the queue
  can't recover.
- **[`docs/operator/file-tier-config.md`](file-tier-config.md)**
  — general file-tier configuration reference. `gc_sweep`
  is file-tier + env-var only (not a runtime-settable key);
  changes require a PDS restart.
- **`aurora-locus validate-config`** — surfaces warnings for
  risky `gc_sweep` configurations at validate-time rather
  than waiting for the first sweep run.
