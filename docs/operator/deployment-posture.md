# Aurora-Locus deployment posture (v0.5)

Operator guide pinning the v0.5 forensic-recovery procedure for
Option A failures (`apply_writes` Phase A committed; Phase B failed
mid-flight). Introduced by Arc 16e §9.5.4 Step 3.9 (round-4 F4
closure) — see [`docs/V05_DESIGN.md`](../V05_DESIGN.md) §9.5.1.2 +
§9.5.3.1.3 + §9.5.5.8 for the design rationale.

## TL;DR

**Docker, k8s, and bare-metal systemd are all supported runtimes
for v0.5.** You can deploy and run the binary in any of them today.

**What's pinned to systemd+journald is the *verified* forensic-
recovery procedure.** v0.5 ships one fully-tested runbook for
investigating Option A failures: it queries `journalctl` against a
local-journald-attached service unit. Other configurations (Docker
with `--log-driver=journald`, k8s with a durable log sink, fluent-bit
/ vector / promtail aggregators) can in principle achieve equivalent
recovery — they just need the procedure adapted to their query
surface. v0.5 doesn't document or verify those adaptations; that's
v0.6+ scope.

Net for an operator deploying v0.5:

- Run it however your platform fits — Docker / k8s / systemd / nomad
  / etc. **all work as runtimes.**
- If you take the systemd + journald path, you get a tested
  recovery runbook out of the box (this doc).
- If you take another path, the binary still emits the right
  forensic log line; you're on your own to wire your log sink's
  query surface into an Option-A recovery procedure. The
  capability is there; the runbook isn't.

## The actual recovery requirement

For Option A failure recovery to work, **a single flushed log line
must reach a durable, queryable sink before the process crashes**.
That log line is `event="phase_b_starting"`, emitted by
`RepositoryManager::run_phase_b` after `tx.begin()` and before any
state mutation, with `std::io::stdout().lock().flush()` called
immediately to drain the Rust-side stdio buffer.

The recovery procedure is then: query the sink for the
`phase_b_starting` line in the failure window, pull its full
`touch_set` + `record_uris`, walk those CIDs against
`blob_metadata` + `record_blob` to identify which rows committed
on Phase A and need reconciliation against Phase B's intended end-
state.

Any sink that satisfies "durable + queryable + flushed line arrives
before crash" can host this procedure. The choices of sink and the
adapted query surface are deployment-specific; the binary doesn't
care.

## v0.5 verified path: systemd + local journald

This is the single configuration v0.5 ships a tested runbook for.

| Aspect | This configuration |
|---|---|
| Process supervisor | `systemd` |
| Log destination | local `journald` (Unix datagram socket) |
| Forensic-recovery query | `journalctl _SYSTEMD_UNIT=aurora-locus.service ...` |
| Residual-window magnitude | microseconds (local datagram socket) |

**Residual window analysis.** Drainage from Aurora-Locus's stdout
to journald's datagram socket is OS-managed kernel buffering. The
in-process `flush()` call drains the Rust-side stdio buffer but
not that kernel-side buffer. The window between flush-returns and
kernel-delivers is microseconds on this path (local Unix datagram
socket, no network round trip, journald persists to disk after
arrival). v0.5 explicitly accepts this microsecond window per
V05_DESIGN.md §9.5.5.8 — this is the **tightest window** any v0.5
configuration achieves, which is why it's the verified runbook.

Other sinks have wider windows (network shippers add ms-to-s round
trips, in-process buffered writers add their flush latency, etc.).
None of them break the recovery capability — they just have larger
residual windows that operators on those paths need to characterize
for their own SLAs.

### Operator runbook (verified path only)

When an `apply_writes.phase_b_failed` ERROR fires in alerts:

```bash
# 1. Pull the forensic anchor for the failure window.
journalctl _SYSTEMD_UNIT=aurora-locus.service \
  --since "<time of phase_b_failed alert>" \
  --until "<5 minutes before alert>" \
  | grep 'event="phase_b_starting"'
```

The matching `phase_b_starting` line carries the full `touch_set`
and the list of `record_uris` the batch operated on. From there,
walk the touched CIDs against `blob_metadata` + `record_blob` to
identify which rows committed on Phase A and need manual
reconciliation against Phase B's intended end-state.

```text
# Sample expected log shape (round-4 F6: format-grep verification).
# tracing-subscriber 0.3.20 Full-format renders fields as
# field="value" for strings; the grep pattern above is verified
# against this format. Sample line:
#
# 2026-05-20T22:36:42.950367672Z  INFO aurora_locus::apply_writes:
#   phase_b_starting did="did:plc:alice"
#   record_uris=["at://did:plc:alice/app.bsky.feed.post/abc"]
#   touch_set=["bafyrei..."]
```

The Phase B test artifact (Step 4 / §9.5.4) records a real
captured `phase_b_starting` line emitted by Aurora-Locus's actual
subscriber config so the grep pattern is verified end-to-end.

## Other configurations: capable in principle, unverified in v0.5

These all work as runtimes. None of them are "broken" or
"unsupported"; v0.5 just hasn't shipped a tested recovery runbook
for them.

### Docker with a durable log driver

Docker's `--log-driver=journald` routes container stdout straight
into the host's journald. With this driver, the same `journalctl
_SYSTEMD_UNIT=...` query works against the container's unit name
— the recovery procedure adapts trivially. Other drivers
(`json-file`, syslog over UDP, network shippers) still capture the
flushed line but the query surface differs and v0.5 hasn't
verified the adaptation.

### Kubernetes with a log shipper

Pods running aurora-locus emit `phase_b_starting` to stdout per
normal; the cluster's log shipper (fluent-bit, vector, promtail,
etc.) collects it and lands it in whatever log store the platform
runs (Loki, Elasticsearch, CloudWatch, etc.). The recovery
requirement (durable + queryable + flushed line arrives before
crash) is satisfied as long as the shipper's pipeline doesn't
drop logs at high water, and the query surface is whatever the
log store exposes. v0.5 doesn't characterize the residual-window
magnitude for any specific shipper; that's deployment-specific.

### Bare-metal non-systemd

Aurora-Locus run under any other supervisor (s6, runit, supervisord,
or no supervisor at all writing to a file) emits the line; the
operator's job is to ensure the chosen sink is durable + queryable.

## What v0.5 explicitly does NOT do

- **Verify a recovery procedure for non-journald log sinks.** Other
  sinks can host the procedure; v0.5 just doesn't ship the verified
  runbook. Doing so is v0.6+ scope.
- **Characterize residual-window magnitude for non-local sinks.**
  Local-journald is microseconds; network-shipped paths are
  ms-to-s. v0.5 pins the microsecond window for the verified path
  only.
- **Document per-shipper recovery procedures.** v0.6+ will add
  per-shipper adaptations matching the journalctl runbook above.

## Cross-references

- [V05_DESIGN.md §9.5.1.2](../V05_DESIGN.md) — Arc 16e
  out-of-scope deferrals (verification of non-journald recovery
  procedures → v0.6+).
- [V05_DESIGN.md §9.5.3.1.3](../V05_DESIGN.md) — forensic log
  discipline + flush-primitive rationale.
- [V05_DESIGN.md §9.5.5.8](../V05_DESIGN.md) — local-journald
  microsecond residual-window acceptance posture.
- [src/actor_store/repository.rs](../../src/actor_store/repository.rs)
  `run_phase_b` — emits `phase_b_starting` / `phase_b_failed`.
