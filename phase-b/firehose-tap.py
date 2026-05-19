#!/usr/bin/env python3
"""Aurora-Locus firehose tap for Arc 14 Phase B verification.

Connects to `com.atproto.sync.subscribeRepos` and decodes each
binary WebSocket frame as the atproto subscription protocol
specifies: TWO consecutive DAG-CBOR objects (header + body)
packed into a single `Message::Binary` payload.

Per Arc 14 §7.3.1 wire format:
  - Header is `{t: "#<frame-type>", op: 1}` for data frames
    OR `{op: -1, error: "<name>" [, message: "..."]}` for
    error frames.
  - Body is a `LexValue::Map` whose canonical-CBOR shape varies
    by frame type (#commit, #sync, #identity, #account, #info).

Frame structure printed for operator inspection. CIDs (tag 42)
display as their string form; raw bytes (e.g. `blocks` CAR
payload) display as hex (NOT base64) so operators can compare
against expected byte sequences.

# Usage

    phase-b/firehose-tap.py ws://localhost:3000/xrpc/com.atproto.sync.subscribeRepos
    phase-b/firehose-tap.py 'ws://localhost:3000/xrpc/com.atproto.sync.subscribeRepos?cursor=42'

# Scenarios

  - **Scenario 1** (binary wire-format): connect, inspect first
    frame, verify header starts `{t: "#<x>", op: 1}` byte-wise
    (`0xa2 0x61 0x74` for #commit).
  - **Scenarios 2, 3a, 3c**: inspect inductive fields, cursor
    semantics — read whatever frames arrive.
  - **Scenario 3b** (FutureCursor): expect ONE error-frame
    payload (`{op: -1, error: "FutureCursor", ...}`) then WS
    close 1008.
  - **Scenario 4** (ConsumerTooSlow): use the slow-tap variant
    (`firehose-slow-tap.py --read-delay N`).

# Close codes (Arc 14 §7.3.4)

  - 1000 — normal close (client/server disconnect).
  - 1008 — policy violation (FutureCursor, ConsumerTooSlow).
  - 1011 — internal error (no named lexicon error per
    Sub-step 0.G).

# Dependencies

  - Python 3.8+
  - `websockets` (`pip install websockets`)
  - `cbor2` (`pip install cbor2`)
"""

from __future__ import annotations

import argparse
import asyncio
import io
import sys
from typing import Any

try:
    import cbor2
except ImportError:
    sys.stderr.write(
        "error: cbor2 not installed. Install: pip install cbor2\n"
    )
    sys.exit(2)

try:
    import websockets
except ImportError:
    sys.stderr.write(
        "error: websockets not installed. Install: pip install websockets\n"
    )
    sys.exit(2)


# ============================================================
# DAG-CBOR tag 42 = CID
# ============================================================

# cbor2 represents CBOR tags as cbor2.CBORTag(tag, value). For
# DAG-CBOR, tag 42 wraps a byte string whose first byte is 0x00
# (the multibase identity prefix for the binary CID form).


class CidRef:
    """Marker for tag-42 values, displayed as `Cid(<hex>)` in dumps.

    We do not decode the full CIDv1 structure here (codec + hash
    function + digest); operators verifying spec compliance can
    cross-reference the hex against the producer-side CID. The
    important assertion is "this slot holds a tag-42 value, not a
    string."
    """

    __slots__ = ("raw",)

    def __init__(self, raw: bytes) -> None:
        # Strip the leading 0x00 identity-multibase byte if present.
        # DAG-CBOR's tag-42 payload is `0x00 || <binary CID>`.
        if raw and raw[0] == 0x00:
            self.raw = raw[1:]
        else:
            self.raw = raw

    def __repr__(self) -> str:
        return f"Cid(<{len(self.raw)} bytes: {self.raw.hex()}>)"


def _tag_hook(decoder: Any, tag: cbor2.CBORTag) -> Any:
    # cbor2 6.x signature: tag_hook(decoder, CBORTag) -> Any.
    # Only fires for tags WITHOUT a built-in semantic decoder. Tag 42
    # may or may not be built-in depending on cbor2 version — we
    # override via semantic_decoders below to guarantee CidRef shape.
    if tag.tag == 42:
        value = tag.value
        if isinstance(value, bytes):
            return CidRef(value)
        return tag
    return tag


def _cid_semantic_decoder(value: Any, immutable: bool) -> Any:
    # cbor2 6.x semantic_decoders signature: (value, immutable) -> Any.
    # `value` is the ALREADY-decoded tag payload (not a decoder
    # instance). For DAG-CBOR tag 42 this is the binary CID payload
    # (multibase identity prefix + binary CIDv1).
    if isinstance(value, bytes):
        return CidRef(value)
    # If a future cbor2 version pre-parses the bytes into a typed
    # object, fall back to wrapping it as a generic tag for visibility.
    return cbor2.CBORTag(42, value)


def decode_dag_cbor_stream(payload: bytes) -> list[Any]:
    """Decode a sequence of CBOR objects from a single byte payload.

    The atproto subscription protocol packs header + body as TWO
    consecutive CBOR objects in one binary WebSocket message.
    cbor2's stream decoding pattern: call `loads` repeatedly on
    a `BytesIO` until the buffer is exhausted.

    Returns a list of decoded values (typically length 2: header,
    body).
    """
    buf = io.BytesIO(payload)
    decoded: list[Any] = []
    while buf.tell() < len(payload):
        try:
            value = cbor2.load(
                buf,
                tag_hook=_tag_hook,
                semantic_decoders={42: _cid_semantic_decoder},
            )
        except cbor2.CBORDecodeEOF:
            break
        decoded.append(value)
    return decoded


# ============================================================
# Pretty-printer
# ============================================================

_BYTES_HEX_KEYS = {"blocks"}  # fields rendered as hex, not repr-bytes


def _render(value: Any, indent: int = 0, key: str | None = None) -> str:
    """Render a decoded CBOR value as indented human-readable text.

    Arc 14 specifics:
      - CID values (`CidRef`) print as `Cid(<hex>)`.
      - Raw byte fields (e.g. `blocks`) print as hex with length.
      - Maps are sorted by key for deterministic operator review.
      - Booleans, ints, strings, nulls print directly.
    """
    pad = " " * indent

    if isinstance(value, CidRef):
        return f"Cid(<{len(value.raw)} bytes: {value.raw.hex()}>)"

    if value is None:
        return "null"

    if isinstance(value, bool):
        return "true" if value else "false"

    if isinstance(value, int):
        return str(value)

    if isinstance(value, str):
        # Show as quoted string; escape control chars.
        return repr(value)

    if isinstance(value, bytes):
        # Arc 14: `blocks` field MUST be raw bytes (no base64) — show as hex.
        if key in _BYTES_HEX_KEYS:
            return f"bytes({len(value)})<{value.hex()}>"
        # Other byte fields: show length + hex prefix.
        return f"bytes({len(value)})<{value[:32].hex()}{'...' if len(value) > 32 else ''}>"

    if isinstance(value, list):
        if not value:
            return "[]"
        inner = ",\n".join(
            f"{pad}  {_render(item, indent + 2, key=None)}" for item in value
        )
        return f"[\n{inner}\n{pad}]"

    if isinstance(value, dict):
        if not value:
            return "{}"
        # Sort for deterministic operator output.
        items = sorted(value.items(), key=lambda kv: str(kv[0]))
        inner = ",\n".join(
            f"{pad}  {repr(str(k))}: {_render(v, indent + 2, key=str(k))}"
            for k, v in items
        )
        return f"{{\n{inner}\n{pad}}}"

    if isinstance(value, cbor2.CBORTag):
        return f"CBORTag({value.tag}, {_render(value.value, indent, key=key)})"

    return repr(value)


def print_frame(payload: bytes, frame_index: int) -> None:
    """Decode + print a single binary WebSocket frame."""
    decoded = decode_dag_cbor_stream(payload)
    print(f"\n--- frame #{frame_index} ({len(payload)} bytes binary) ---")

    if not decoded:
        print("  (no decodable CBOR objects in payload)")
        return

    if len(decoded) < 2:
        print(
            "  ! WARNING: expected 2 consecutive CBOR objects (header+body), "
            f"got {len(decoded)}. Spec violation."
        )

    # First object = header per atproto subscription protocol.
    print(f"  header: {_render(decoded[0], indent=2)}")

    # Header inspection: report frame type + op code.
    if isinstance(decoded[0], dict):
        op = decoded[0].get("op")
        t = decoded[0].get("t")
        if op == 1 and isinstance(t, str):
            print(f"  ↳ data frame, type={t!r}")
        elif op == -1:
            print("  ↳ ERROR frame")
        else:
            print(f"  ! unexpected header shape: op={op!r}, t={t!r}")

    # Second object = body.
    if len(decoded) >= 2:
        print(f"  body:   {_render(decoded[1], indent=2)}")

    # Extra objects (shouldn't happen per spec).
    for i, extra in enumerate(decoded[2:], start=3):
        print(f"  ! extra object #{i}: {_render(extra, indent=2)}")

    # Raw header byte preamble — useful for §7.6.1 canonical-order
    # verification (expected first 3 bytes of #commit header:
    # `0xa2 0x61 0x74`).
    print(f"  raw header prefix: {payload[:8].hex()}")


# ============================================================
# Async WebSocket loop
# ============================================================


async def tap(url: str, read_delay: float, max_frames: int | None) -> int:
    """Connect, decode, print. Returns exit status.

    `read_delay`: seconds between `recv()` calls (used by slow-tap
    variant to force ConsumerTooSlow buffer overflow).
    `max_frames`: stop after N frames (None = run until close).
    """
    print(f"connecting to {url}", file=sys.stderr)
    try:
        async with websockets.connect(url, max_size=10 * 1024 * 1024) as ws:
            print("connected; reading frames (Ctrl-C to stop)", file=sys.stderr)
            frame_index = 0
            try:
                while True:
                    if max_frames is not None and frame_index >= max_frames:
                        print(
                            f"reached --max-frames={max_frames}; closing",
                            file=sys.stderr,
                        )
                        await ws.close(code=1000, reason="client done")
                        break
                    msg = await ws.recv()
                    if isinstance(msg, str):
                        # Spec violation: server emitted a TEXT frame.
                        print(
                            "\n! SPEC VIOLATION: server emitted Message::Text, "
                            "expected Message::Binary per atproto subscription "
                            "protocol (Arc 14 §7.3.1 / §7.6.1)."
                        )
                        print(f"  text payload: {msg!r}")
                        frame_index += 1
                        continue
                    frame_index += 1
                    print_frame(msg, frame_index)
                    if read_delay > 0:
                        # Deliberate slow-reader: pause to allow the
                        # server-side mpsc buffer to fill. Triggers
                        # ConsumerTooSlow + WS close 1008 once the
                        # backpressure buffer overflows (Arc 14 §7.3.4).
                        await asyncio.sleep(read_delay)
            except websockets.ConnectionClosed as cc:
                _print_close(cc.code, cc.reason)
                return 0
    except OSError as e:
        print(f"connection failed: {e}", file=sys.stderr)
        return 3
    except websockets.exceptions.InvalidStatusCode as e:
        print(f"server rejected upgrade: HTTP {e.status_code}", file=sys.stderr)
        return 4
    return 0


def _print_close(code: int, reason: str) -> None:
    """Map WS close codes to Arc 14 §7.3.4 semantics."""
    label = {
        1000: "normal",
        1008: "policy violation",
        1011: "internal error",
    }.get(code, "unknown")
    print(f"\n--- connection closed: code={code} ({label}), reason={reason!r} ---")
    if code == 1008:
        print(
            "  ↳ per Arc 14 §7.3.4, expected error frame "
            "(FutureCursor or ConsumerTooSlow) was sent just before close."
        )
    elif code == 1011:
        print(
            "  ↳ per Arc 14 §7.3.4 + Sub-step 0.G, no named lexicon error "
            "frame accompanies this — close-code is the signal."
        )
    elif code == 1000:
        print("  ↳ clean close.")


# ============================================================
# CLI
# ============================================================


def _build_argparser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        prog="firehose-tap.py",
        description=(
            "Aurora-Locus firehose tap for Arc 14 Phase B verification. "
            "Decodes binary DAG-CBOR frames (header + body) and prints "
            "them for operator inspection. `blocks` field shown as hex "
            "(NOT base64) per Arc 14 §7.3.1."
        ),
    )
    p.add_argument(
        "url",
        help=(
            "WebSocket URL, e.g. "
            "ws://localhost:3000/xrpc/com.atproto.sync.subscribeRepos "
            "[?cursor=N]"
        ),
    )
    p.add_argument(
        "--read-delay",
        type=float,
        default=0.0,
        metavar="SECONDS",
        help=(
            "Pause N seconds between recv() calls. Use to force "
            "ConsumerTooSlow buffer overflow (Scenario 4). Default 0 "
            "(no delay; live-tail)."
        ),
    )
    p.add_argument(
        "--max-frames",
        type=int,
        default=None,
        metavar="N",
        help="Stop after receiving N frames (default: run until close).",
    )
    return p


def main() -> int:
    args = _build_argparser().parse_args()
    try:
        return asyncio.run(tap(args.url, args.read_delay, args.max_frames))
    except KeyboardInterrupt:
        print("\ninterrupted", file=sys.stderr)
        return 130


if __name__ == "__main__":
    sys.exit(main())
