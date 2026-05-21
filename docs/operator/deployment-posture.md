# Aurora-Locus deployment posture (v0.5)

Operator guide pinning the v0.5 deployment posture and the forensic
recovery procedure that depends on it. Introduced by Arc 16e §9.5.4
Step 3.9 (round-4 F4 closure) — see
[`docs/V05_DESIGN.md`](../V05_DESIGN.md) §9.5.1.2 + §9.5.3.1.3 +
§9.5.5.8 for the design rationale.

## TL;DR

**v0.5 ships as systemd-only for forensic recovery purposes.**
Container deployments (k8s, docker, log shippers) run the binary
fine but lose the specific recovery guarantees that the
`apply_writes` Phase B forensic log depends on. Container deployment
forensic procedures are v0.6+ scope.

You can still run Aurora-Locus in docker today — the
[`README.md`](../../README.md) Docker section is unchanged. What
changes in v0.5 is that **the forensic-recovery procedure documented
below assumes a local journald** and operators on container
deployments lose the bounded-microsecond residual window of the
local-journald datagram socket.

## v0.5 posture

| Aspect | v0.5 posture | v0.6+ candidate |
|---|---|---|
| Process supervisor | `systemd` | k8s / docker / nomad / ... |
| Log destination | `journald` (Unix datagram socket) | network log shippers (fluent-bit / vector / promtail / ...) |
| Forensic recovery anchor | `journalctl _SYSTEMD_UNIT=aurora-locus.service` | TBD per shipper |
| Residual-window magnitude | microseconds (local datagram socket) | milliseconds-to-seconds (network round trips) |

The Phase B forensic log line
(`event="phase_b_starting"`, emitted by `RepositoryManager::
apply_writes` after `tx.begin()` and before any state mutation)
is the load-bearing recovery anchor for Option A failures (Phase A
committed; Phase B failed mid-flight). Drainage from Aurora-Locus's
stdout to journald's datagram socket is **OS-managed kernel
buffering** — the in-process `std::io::stdout().lock().flush()`
call drains the Rust-side stdio buffer but not that kernel-side
buffer. The window between flush-returns and kernel-delivers is
microseconds on local-journald deployments; network log shippers
widen it proportionally to milliseconds-to-seconds.

V0.5 explicitly accepts the local-journald window per
V05_DESIGN.md §9.5.5.8.

## Operator forensic procedure for Option A failures

When an `apply_writes.phase_b_failed` ERROR fires in alerts, the
recovery procedure is:

```bash
# 1. Pull the forensic anchor for the failure window.
journalctl _SYSTEMD_UNIT=aurora-locus.service \
  --since "<time of phase_b_failed alert>" \
  --until "<5 minutes before alert>" \
  | grep 'event="phase_b_starting"'
```

The matching `phase_b_starting` line carries the full `touch_set`
and the list of `record_uris` the batch operated on. From there,
the recovery operator walks the touched CIDs against
`blob_metadata` + `record_blob` to identify which rows committed
on Phase A and need manual reconciliation against Phase B's
intended end-state.

```bash
# 2. Sample expected log shape (round-4 F6: format-grep verification).
# tracing-subscriber 0.3.20 Full-format renders fields as
# field="value" for strings — the grep pattern above is verified
# against this format. Sample line:
#
# 2026-05-20T22:36:42.950367672Z  INFO aurora_locus::apply_writes:
#   phase_b_starting did="did:plc:alice"
#   record_uris=["at://did:plc:alice/app.bsky.feed.post/abc"]
#   touch_set=["bafyrei..."]
```

The Phase B test artifact (Step 4 / §9.5.4) records a real sample
line emitted by Aurora-Locus's actual subscriber config so the
grep pattern is verified end-to-end.

## What v0.5 does NOT pin

- **Container deployment forensic procedures.** k8s `kubectl logs`,
  docker `docker logs`, and log-shipper aggregators (fluent-bit,
  vector, promtail, etc.) all have their own log-buffering and
  durability shapes. V0.5 doesn't characterize the residual-window
  magnitude under network shipping; v0.6+ work is the canonical
  place for that.
- **Container deployment recovery procedures.** A v0.6+ deployment-
  posture revision will add per-shipper recovery procedures
  matching the journalctl shape above.

Aurora-Locus continues to ship a working Docker container — the
gap is purely in the operator forensic procedure, not in the
runtime binary.

## Cross-references

- [V05_DESIGN.md §9.5.1.2](../V05_DESIGN.md#L8890) — Arc 16e
  out-of-scope deferrals (container deployment + network log
  shipping → v0.6+).
- [V05_DESIGN.md §9.5.3.1.3](../V05_DESIGN.md#L8933) — forensic
  log discipline + journald-as-recovery-anchor rationale.
- [V05_DESIGN.md §9.5.5.8](../V05_DESIGN.md#L9712) — kernel-buffer
  residual-window acceptance posture.
- [src/actor_store/repository.rs](../../src/actor_store/repository.rs)
  `run_phase_b` — emits `phase_b_starting` / `phase_b_failed`.
