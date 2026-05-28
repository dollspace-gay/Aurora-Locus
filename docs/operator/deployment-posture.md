# Aurora-Locus deployment posture

Operator guide pinning the forensic-recovery procedure for Option A
failures (`apply_writes` Phase A committed; Phase B failed mid-flight).

## TL;DR

**Docker, Kubernetes, and bare-metal systemd are all supported
runtimes.** You can deploy and run the binary in any of them today.

**What's pinned to systemd+journald is the *verified* forensic-
recovery procedure.** One fully-tested runbook exists for investigating
Option A failures: it queries `journalctl` against a local-journald-
attached service unit. Other configurations (Docker with
`--log-driver=journald`, k8s with a durable log sink, fluent-bit /
vector / promtail aggregators) can in principle achieve equivalent
recovery — they just need the procedure adapted to their query
surface. Those adaptations are not documented or verified here.

Net for an operator deploying Aurora-Locus:

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
on Phase A and need reconciliation against Phase B's intended
end-state.

Any sink that satisfies "durable + queryable + flushed line arrives
before crash" can host this procedure. The choices of sink and the
adapted query surface are deployment-specific; the binary doesn't
care.

## Verified path: systemd + local journald

This is the single configuration the project ships a tested runbook
for.

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
arrival). This is the **tightest window** any configuration
achieves, which is why it's the verified runbook.

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
# Sample expected log shape (format-grep verification).
# tracing-subscriber 0.3.20 Full-format renders fields as
# field="value" for strings; the grep pattern above is verified
# against this format. Sample line:
#
# 2026-05-20T22:36:42.950367672Z  INFO aurora_locus::apply_writes:
#   phase_b_starting did="did:plc:alice"
#   record_uris=["at://did:plc:alice/app.bsky.feed.post/abc"]
#   touch_set=["bafyrei..."]
```

The Phase B test artifact records a real captured `phase_b_starting`
line emitted by Aurora-Locus's actual subscriber config so the grep
pattern is verified end-to-end.

## Other configurations: capable in principle, unverified

These all work as runtimes. None of them are "broken" or
"unsupported"; the project just hasn't shipped a tested recovery
runbook for them.

### Docker with a durable log driver

Docker's `--log-driver=journald` routes container stdout straight
into the host's journald. With this driver, the same `journalctl
_SYSTEMD_UNIT=...` query works against the container's unit name
— the recovery procedure adapts trivially. Other drivers
(`json-file`, syslog over UDP, network shippers) still capture the
flushed line but the query surface differs and the adaptation
isn't verified here.

### Kubernetes with a log shipper

Pods running aurora-locus emit `phase_b_starting` to stdout per
normal; the cluster's log shipper (fluent-bit, vector, promtail,
etc.) collects it and lands it in whatever log store the platform
runs (Loki, Elasticsearch, CloudWatch, etc.). The recovery
requirement (durable + queryable + flushed line arrives before
crash) is satisfied as long as the shipper's pipeline doesn't
drop logs at high water, and the query surface is whatever the
log store exposes. Residual-window magnitude is shipper-specific
and not characterized here.

### Bare-metal non-systemd

Aurora-Locus run under any other supervisor (s6, runit, supervisord,
or no supervisor at all writing to a file) emits the line; the
operator's job is to ensure the chosen sink is durable + queryable.

## What this doc explicitly does NOT do

- **Verify a recovery procedure for non-journald log sinks.** Other
  sinks can host the procedure; the verified runbook only covers
  systemd + local journald. Per-shipper adaptations are future work.
- **Characterize residual-window magnitude for non-local sinks.**
  Local-journald is microseconds; network-shipped paths are
  ms-to-s. The microsecond window pin applies only to the verified
  path.

## Cross-references

- [src/actor_store/repository.rs](../../src/actor_store/repository.rs)
  `run_phase_b` — emits `phase_b_starting` / `phase_b_failed`.
