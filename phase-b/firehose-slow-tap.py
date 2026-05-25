#!/usr/bin/env python3
"""Aurora-Locus firehose slow-reader for Arc 14 Scenario 4 (ConsumerTooSlow).

Same as `firehose-tap.py` but with a non-zero default `--read-delay`,
named for operator clarity. Pauses between each `recv()` so the
server-side mpsc buffer (capacity 100, send timeout 5s — see
`src/api/firehose.rs::BUFFER_SIZE`/`SEND_TIMEOUT_MS`) overflows
once `trigger-commits.py` fires more events than the consumer can
drain.

# Scenario 4 (ConsumerTooSlow) procedure

Terminal 1 — start the slow consumer (wait 10s between reads):

    phase-b/firehose-slow-tap.py \\
        ws://localhost:3000/xrpc/com.atproto.sync.subscribeRepos

Terminal 2 — fire ≥150 commits in a tight loop:

    phase-b/trigger-commits.py --did "$DID" --jwt "$JWT" --count 150

# Expected (per Arc 14 §7.3.4)

Within seconds of Terminal 2 starting, the server's 100-deep buffer
fills (because Terminal 1 only drains one frame every 10s). The
send-timeout (5s) fires on the 101st frame; the server emits ONE
named error frame with `{op: -1, body: {error: "ConsumerTooSlow",
message: "..."}}` followed by WebSocket close code 1008.

# Output discipline

Identical to `firehose-tap.py` — same CBOR decoder, same
pretty-printer, same close-code interpretation. The only behavioral
difference is the default read-delay.

# Dependencies

  - Python 3.8+
  - `websockets` (`pip install websockets`)
  - `cbor2` (`pip install cbor2`)
"""

from __future__ import annotations

import argparse
import asyncio
import sys

# Re-use the tap implementation. Importing across hyphenated
# filenames requires a small import shim.
import importlib.util
import os

_TAP_PATH = os.path.join(os.path.dirname(__file__), "firehose-tap.py")
_spec = importlib.util.spec_from_file_location("_firehose_tap", _TAP_PATH)
if _spec is None or _spec.loader is None:
    sys.stderr.write(f"error: cannot load {_TAP_PATH}\n")
    sys.exit(2)
_firehose_tap = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_firehose_tap)


def _build_argparser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        prog="firehose-slow-tap.py",
        description=(
            "Aurora-Locus firehose tap with a deliberate read-delay, "
            "for Arc 14 Scenario 4 (ConsumerTooSlow). Defaults to a "
            "10s pause between recv()s; combined with ~150 commits "
            "from trigger-commits.py the server-side mpsc buffer "
            "(capacity 100) will overflow and emit ConsumerTooSlow + "
            "WS close 1008."
        ),
    )
    p.add_argument(
        "url",
        help=(
            "WebSocket URL, e.g. "
            "ws://localhost:3000/xrpc/com.atproto.sync.subscribeRepos"
        ),
    )
    p.add_argument(
        "--read-delay",
        type=float,
        default=10.0,
        metavar="SECONDS",
        help=(
            "Pause N seconds between recv() calls. Default 10s "
            "(higher than the server's 5s send-timeout, guaranteeing "
            "buffer-overflow once >100 events queue up)."
        ),
    )
    p.add_argument(
        "--max-frames",
        type=int,
        default=None,
        metavar="N",
        help="Stop after N frames (default: run until close).",
    )
    return p


def main() -> int:
    args = _build_argparser().parse_args()
    if args.read_delay <= 0:
        sys.stderr.write(
            "error: --read-delay must be > 0 for the slow-tap variant "
            "(use firehose-tap.py for no delay)\n"
        )
        return 2
    try:
        return asyncio.run(
            _firehose_tap.tap(args.url, args.read_delay, args.max_frames)
        )
    except KeyboardInterrupt:
        print("\ninterrupted", file=sys.stderr)
        return 130


if __name__ == "__main__":
    sys.exit(main())
