#!/usr/bin/env python3
"""Self-test for firehose-tap.py CBOR decoder + pretty-printer.

No live PDS required. Constructs synthetic binary payloads
mimicking the Aurora-Locus Arc 14 wire format (header + body as
two consecutive DAG-CBOR objects) and verifies:

  1. Two consecutive CBOR objects decode correctly from one
     binary blob.
  2. Canonical map ordering (RFC 8949 §4.2.1) is preserved.
  3. CIDs (tag 42) surface as `CidRef` objects, not raw bytes.
  4. The `blocks` field renders as hex, NOT base64.
  5. Delete-op `cid: null` (CBOR 0xf6) decodes to Python `None`.
  6. Error-frame header `{op: -1}` decodes correctly.
  7. Spec violation detection (single-object payload) is reported.

Run:

    phase-b/test-firehose-tap.py

Exits 0 on all-pass, 1 on any failure.
"""

from __future__ import annotations

import importlib.util
import os
import sys

try:
    import cbor2
except ImportError:
    sys.stderr.write("error: cbor2 not installed. pip install cbor2\n")
    sys.exit(2)


_TAP_PATH = os.path.join(os.path.dirname(__file__), "firehose-tap.py")
_spec = importlib.util.spec_from_file_location("_firehose_tap", _TAP_PATH)
assert _spec is not None and _spec.loader is not None
_tap = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_tap)


# ============================================================
# Test helpers
# ============================================================


def _encode_pair(header: dict, body: dict) -> bytes:
    """Encode header + body as two consecutive DAG-CBOR objects.

    Uses cbor2's canonical=True so map-key sort + shortest-int
    encoding matches DAG-CBOR. Note: cbor2 sorts maps by encoded
    key bytewise-lex, which for our test inputs equals AT Protocol
    byte-length-then-lex order (single-byte keys all have length 1).
    For multi-byte keys we use explicit insertion order matching
    the AT Protocol expectation.
    """
    h = cbor2.dumps(header, canonical=True)
    b = cbor2.dumps(body, canonical=True)
    return h + b


def _make_cid_tag(payload_hex: str) -> cbor2.CBORTag:
    """Build a tag-42 CBOR object holding identity-multibase + binary CID."""
    return cbor2.CBORTag(42, b"\x00" + bytes.fromhex(payload_hex))


# ============================================================
# Tests
# ============================================================


_results: list[tuple[str, bool, str]] = []


def _assert(name: str, cond: bool, detail: str = "") -> None:
    _results.append((name, cond, detail))


def test_two_consecutive_objects_decode() -> None:
    header = {"t": "#commit", "op": 1}
    body = {"seq": 42, "repo": "did:plc:test"}
    payload = _encode_pair(header, body)
    decoded = _tap.decode_dag_cbor_stream(payload)
    _assert(
        "two_consecutive_objects_decode",
        len(decoded) == 2
        and decoded[0] == header
        and decoded[1] == body,
        f"got {decoded!r}",
    )


def test_canonical_header_byte_prefix() -> None:
    # Per Arc 14 §7.6.1: #commit frame header bytes start with
    # 0xa2 (map-2) 0x61 (text-1) 0x74 ('t').
    header = {"t": "#commit", "op": 1}
    body: dict = {}
    payload = _encode_pair(header, body)
    _assert(
        "canonical_header_byte_prefix",
        payload[:3] == bytes([0xA2, 0x61, 0x74]),
        f"got {payload[:3].hex()}",
    )


def test_cid_tag_surfaces_as_cidref() -> None:
    header = {"t": "#commit", "op": 1}
    cid_hex = "01711220" + ("a" * 64)  # codec=dag-cbor, sha2-256, 32-byte digest
    body = {"commit": _make_cid_tag(cid_hex)}
    payload = _encode_pair(header, body)
    decoded = _tap.decode_dag_cbor_stream(payload)
    cid_value = decoded[1]["commit"]
    _assert(
        "cid_tag_surfaces_as_cidref",
        isinstance(cid_value, _tap.CidRef)
        and cid_value.raw.hex() == cid_hex,
        f"got {cid_value!r}",
    )


def test_blocks_field_renders_as_hex_not_base64() -> None:
    body = {"blocks": b"\xde\xad\xbe\xef"}
    rendered = _tap._render(body, indent=0)
    _assert(
        "blocks_field_renders_as_hex",
        "deadbeef" in rendered and "==" not in rendered,
        f"got {rendered!r}",
    )


def test_delete_op_cid_null_decodes_to_none() -> None:
    # Per Arc 14 §7.3.2: delete-op `cid` is CBOR null (0xf6),
    # not field-absent. cbor2 decodes null as Python None.
    body = {"ops": [{"action": "delete", "cid": None, "path": "p/r"}]}
    payload = cbor2.dumps(body, canonical=True)
    # Verify the encoded bytes contain 0xf6 directly after "cid".
    cid_null_marker = b"\x63cid\xf6"  # 0x63 = text-3
    _assert(
        "delete_op_cid_null_bytes_present",
        cid_null_marker in payload,
        f"payload hex: {payload.hex()}",
    )
    # And that decoding round-trips.
    decoded = cbor2.loads(payload)
    _assert(
        "delete_op_cid_decodes_to_python_none",
        decoded["ops"][0]["cid"] is None,
        f"got {decoded!r}",
    )


def test_error_frame_header_op_neg1() -> None:
    # Arc 14 §7.3.4 error frame: header is `{op: -1}` only.
    header = {"op": -1}
    body = {"error": "FutureCursor", "message": "test"}
    payload = _encode_pair(header, body)
    # 0xa1 = map-1; 0x62 = text-2; "op" = 0x6f 0x70; -1 = 0x20.
    _assert(
        "error_frame_header_op_neg1_bytes",
        payload[:5] == bytes([0xA1, 0x62, 0x6F, 0x70, 0x20]),
        f"got {payload[:5].hex()}",
    )
    decoded = _tap.decode_dag_cbor_stream(payload)
    _assert(
        "error_frame_decodes_to_op_neg1",
        decoded[0] == {"op": -1}
        and decoded[1] == {"error": "FutureCursor", "message": "test"},
        f"got {decoded!r}",
    )


def test_single_object_payload_detected() -> None:
    # If the server emits only ONE CBOR object (header without body
    # or body without header), the decoder returns a 1-element list
    # — print_frame() will flag this as spec-violating.
    single = cbor2.dumps({"t": "#commit", "op": 1}, canonical=True)
    decoded = _tap.decode_dag_cbor_stream(single)
    _assert(
        "single_object_payload_decoded_as_one_object",
        len(decoded) == 1,
        f"got {decoded!r}",
    )


def test_render_handles_nested_structures() -> None:
    # Smoke test that the pretty-printer doesn't crash on a
    # realistic-shape #commit body.
    body = {
        "seq": 42,
        "rebase": False,
        "tooBig": False,
        "repo": "did:plc:alice",
        "commit": _tap.CidRef(b"\x01\x71\x12\x20" + b"\x0a" * 32),
        "rev": "3l4abc",
        "since": _tap.CidRef(b"\x01\x71\x12\x20" + b"\x0b" * 32),
        "prevData": _tap.CidRef(b"\x01\x71\x12\x20" + b"\x0c" * 32),
        "blocks": b"\xde\xad\xbe\xef\xca\xfe",
        "ops": [
            {
                "action": "create",
                "path": "app.bsky.feed.post/abc",
                "cid": _tap.CidRef(b"\x01\x71\x12\x20" + b"\x0d" * 32),
            },
            {
                "action": "delete",
                "path": "app.bsky.feed.post/xyz",
                "cid": None,
                "prev": _tap.CidRef(b"\x01\x71\x12\x20" + b"\x0e" * 32),
            },
        ],
        "blobs": [],
        "time": "2026-05-18T00:00:00Z",
    }
    rendered = _tap._render(body, indent=0)
    _assert(
        "render_includes_cid_marker",
        "Cid(<" in rendered,
        f"got {rendered[:200]!r}",
    )
    _assert(
        "render_includes_blocks_hex",
        "deadbeefcafe" in rendered,
        f"got {rendered[:200]!r}",
    )
    _assert(
        "render_emits_null_for_delete_cid",
        ": null" in rendered,
        f"got {rendered[:400]!r}",
    )


# ============================================================
# Runner
# ============================================================


def main() -> int:
    tests = [
        test_two_consecutive_objects_decode,
        test_canonical_header_byte_prefix,
        test_cid_tag_surfaces_as_cidref,
        test_blocks_field_renders_as_hex_not_base64,
        test_delete_op_cid_null_decodes_to_none,
        test_error_frame_header_op_neg1,
        test_single_object_payload_detected,
        test_render_handles_nested_structures,
    ]
    for t in tests:
        try:
            t()
        except Exception as e:
            _results.append((t.__name__, False, f"raised {type(e).__name__}: {e}"))

    passed = sum(1 for _, ok, _ in _results if ok)
    failed = len(_results) - passed
    for name, ok, detail in _results:
        marker = "PASS" if ok else "FAIL"
        line = f"  [{marker}] {name}"
        if not ok and detail:
            line += f"    — {detail}"
        print(line)
    print(f"\n{passed}/{len(_results)} passed, {failed} failed")
    return 0 if failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
