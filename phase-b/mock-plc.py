#!/usr/bin/env python3
"""Mock PLC directory for Aurora-Locus Phase B.

Implements the contract documented at
docs/operator/phase-b-setup.md:
  - Arc 12 §5.8.2 mock-contract (GET/POST endpoints,
    sig+prev validation, error semantics).
  - Arc 13 §6.4 Step 4.5 mode-(b) strict signature
    verification (full canonical-CBOR re-encode + ECDSA
    verify against rotation keys).
  - Arc 13 §6.4 Step 4.5 tombstone contract
    (plc_tombstone acceptance + terminal-state semantics).

In-memory state — per-process; restart wipes. Phase B
reset semantics is "restart the process."

Default port 2582; override via --port.
Default mode is (b) strict per Arc 13 Phase B; --no-strict
falls back to mode (a) trust-on-faith for Arc 12 / earlier
backward-compat.

Dependencies: Python 3.8+, `cryptography` package
(>=3.0 with SECP256K1 support). No other third-party
deps — base58, base32lower, multibase, CID, DAG-CBOR
encoding are all implemented inline.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import logging
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any
from urllib.parse import urlparse

from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.hazmat.primitives.asymmetric.utils import encode_dss_signature

# ============================================================
# base58btc (Bitcoin alphabet)
# ============================================================

_BASE58_ALPHABET = (
    "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
)
_BASE58_INDEX = {ch: i for i, ch in enumerate(_BASE58_ALPHABET)}


def base58_decode(s: str) -> bytes:
    n = 0
    for ch in s:
        try:
            n = n * 58 + _BASE58_INDEX[ch]
        except KeyError as exc:
            raise ValueError(f"invalid base58 char {ch!r}") from exc
    # Convert to bytes (big-endian).
    if n == 0:
        body = b""
    else:
        body = n.to_bytes((n.bit_length() + 7) // 8, "big")
    # Restore leading zeros (each '1' = 0x00).
    zeros = 0
    for ch in s:
        if ch == "1":
            zeros += 1
        else:
            break
    return b"\x00" * zeros + body


# ============================================================
# base32 lower (RFC 4648, no padding)
# ============================================================

_BASE32_ALPHABET = "abcdefghijklmnopqrstuvwxyz234567"


def base32_lower_no_pad(data: bytes) -> str:
    if not data:
        return ""
    bits = 0
    val = 0
    out = []
    for b in data:
        val = (val << 8) | b
        bits += 8
        while bits >= 5:
            bits -= 5
            out.append(_BASE32_ALPHABET[(val >> bits) & 0x1F])
    if bits > 0:
        out.append(_BASE32_ALPHABET[(val << (5 - bits)) & 0x1F])
    return "".join(out)


# ============================================================
# Canonical DAG-CBOR encoder
#
# Implements the subset Aurora-Locus PLC ops use:
#   - text strings (major 3)
#   - byte strings (major 2; not used by PLC but defensive)
#   - unsigned ints (major 0)
#   - arrays (major 4)
#   - maps (major 5; keys sorted by byte-length then
#     lexicographic per DAG-CBOR strict mode)
#   - null (major 7, 0xf6)
#   - bool (major 7, 0xf4/0xf5)
#
# Refuses anything else (floats, negative ints) — PLC ops
# don't use them and DAG-CBOR strict mode forbids floats.
# ============================================================


def _uint_head(major: int, n: int) -> bytes:
    if n < 0:
        raise ValueError("uint encoding requires non-negative n")
    base = major << 5
    if n < 24:
        return bytes([base | n])
    if n < 0x100:
        return bytes([base | 24, n])
    if n < 0x10000:
        return bytes([base | 25]) + n.to_bytes(2, "big")
    if n < 0x100000000:
        return bytes([base | 26]) + n.to_bytes(4, "big")
    if n < 0x10000000000000000:
        return bytes([base | 27]) + n.to_bytes(8, "big")
    raise ValueError("uint too large for CBOR")


def dag_cbor_encode(value: Any) -> bytes:
    if value is None:
        return b"\xf6"
    if value is True:
        return b"\xf5"
    if value is False:
        return b"\xf4"
    if isinstance(value, bool):
        # handled above; guard against subclasses
        raise ValueError("unreachable bool")
    if isinstance(value, int):
        if value < 0:
            raise ValueError(
                "negative integers not supported by Aurora-Locus PLC ops"
            )
        return _uint_head(0, value)
    if isinstance(value, str):
        body = value.encode("utf-8")
        return _uint_head(3, len(body)) + body
    if isinstance(value, (bytes, bytearray)):
        return _uint_head(2, len(value)) + bytes(value)
    if isinstance(value, list):
        out = bytearray(_uint_head(4, len(value)))
        for item in value:
            out.extend(dag_cbor_encode(item))
        return bytes(out)
    if isinstance(value, dict):
        # DAG-CBOR map-key sort: ALL keys must be text strings,
        # sorted by byte-length then lexicographic.
        items: list[tuple[bytes, str, Any]] = []
        for k, v in value.items():
            if not isinstance(k, str):
                raise ValueError(
                    f"DAG-CBOR map keys must be strings; got {type(k).__name__}"
                )
            items.append((k.encode("utf-8"), k, v))
        items.sort(key=lambda t: (len(t[0]), t[0]))
        out = bytearray(_uint_head(5, len(items)))
        for kb, k, v in items:
            out.extend(_uint_head(3, len(kb)))
            out.extend(kb)
            out.extend(dag_cbor_encode(v))
        return bytes(out)
    raise ValueError(f"unsupported value type {type(value).__name__}")


# ============================================================
# CID computation (CIDv1, dag-cbor codec, sha2-256 multihash,
# base32lower multibase with 'b' prefix)
# ============================================================


def cid_for_dag_cbor(cbor_bytes: bytes) -> str:
    h = hashlib.sha256(cbor_bytes).digest()
    multihash = b"\x12\x20" + h  # 0x12 = sha2-256, 0x20 = 32-byte len
    cid_bytes = b"\x01\x71" + multihash  # 0x01 = CIDv1, 0x71 = dag-cbor
    return "b" + base32_lower_no_pad(cid_bytes)


# ============================================================
# did:key → compressed secp256k1 pubkey bytes
# ============================================================


def did_key_to_pubkey_bytes(did_key: str) -> bytes:
    """Decode `did:key:z<multibase58>` → 33-byte compressed
    secp256k1 pubkey. Raises ValueError for non-secp256k1 or
    malformed inputs.

    Multicodec prefix for secp256k1-pub is `0xe7 0x01` (varint
    of 0xe7). For our use case the varint always fits in 2 bytes.
    """
    if not did_key.startswith("did:key:z"):
        raise ValueError(f"did:key must start with `did:key:z`; got {did_key!r}")
    raw = base58_decode(did_key[len("did:key:z") :])
    if len(raw) < 2 + 33:
        raise ValueError(f"did:key payload too short ({len(raw)} bytes)")
    if raw[0] != 0xE7 or raw[1] != 0x01:
        raise ValueError(
            f"did:key multicodec is not secp256k1-pub (0xe7 0x01); got {raw[:2].hex()}"
        )
    return raw[2:35]


# ============================================================
# Signature verification (secp256k1, raw r||s sig over
# SHA-256(message))
# ============================================================


def verify_sig_raw_rs(
    pubkey_compressed: bytes, sig_raw: bytes, message: bytes
) -> bool:
    """Verify a 64-byte raw r||s ECDSA signature over SHA-256
    of `message` against a compressed secp256k1 pubkey.

    Returns True iff the signature verifies. Catches
    `InvalidSignature` + any parse errors and returns False so
    callers don't have to deal with the exception zoo.
    """
    if len(sig_raw) != 64:
        return False
    try:
        r = int.from_bytes(sig_raw[:32], "big")
        s = int.from_bytes(sig_raw[32:], "big")
        if r == 0 or s == 0:
            return False
        der = encode_dss_signature(r, s)
        pub = ec.EllipticCurvePublicKey.from_encoded_point(
            ec.SECP256K1(), pubkey_compressed
        )
        # `verify(signature, data, signature_algorithm)`:
        # signature_algorithm wraps a hash algorithm; cryptography
        # will sha256(data) internally and compare.
        pub.verify(der, message, ec.ECDSA(hashes.SHA256()))
        return True
    except (InvalidSignature, ValueError, TypeError) as exc:
        logging.debug("sig verify failed: %s", exc)
        return False


# ============================================================
# In-memory mock state
# ============================================================


class DidState:
    """Per-DID accumulated log + cached current-doc shape."""

    __slots__ = ("entries", "tombstoned")

    def __init__(self) -> None:
        # Each entry: {"cid": str, "did": str, "operation": dict,
        # "nullified": False, "createdAt": iso8601}.
        self.entries: list[dict[str, Any]] = []
        # When True, subsequent ops referencing the last CID as
        # `prev` are rejected per Arc 13 §6.4 Step 4.5 terminal
        # semantics.
        self.tombstoned: bool = False

    def last(self) -> dict[str, Any] | None:
        for e in reversed(self.entries):
            if not e["nullified"]:
                return e
        return None

    def last_cid(self) -> str | None:
        e = self.last()
        return e["cid"] if e else None


# ============================================================
# HTTP handler
# ============================================================


class MockHandler(BaseHTTPRequestHandler):
    # Set by main().
    state: dict[str, DidState] = {}
    strict_mode: bool = True

    # ---------- helpers ----------

    def _send_json(self, status: int, body: dict[str, Any]) -> None:
        payload = json.dumps(body).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def _send_404(self, error: str = "DidNotFound", message: str = "") -> None:
        body = {"error": error}
        if message:
            body["message"] = message
        self._send_json(404, body)

    def _read_body(self) -> bytes:
        length = int(self.headers.get("Content-Length", "0"))
        return self.rfile.read(length) if length else b""

    def _path_did(self) -> str | None:
        """Path is `/{did}` or `/{did}/log/audit`. Return the
        did part or None for root."""
        path = urlparse(self.path).path
        if path in ("/", ""):
            return None
        return path.lstrip("/").split("/", 1)[0]

    def log_message(self, fmt: str, *args: Any) -> None:
        # Route stdlib BaseHTTPRequestHandler logging through
        # our logger for consistency.
        logging.info("%s - %s", self.address_string(), fmt % args)

    # ---------- routes ----------

    def do_GET(self) -> None:
        path = urlparse(self.path).path

        if path in ("/", ""):
            self._send_json(
                200,
                {
                    "service": "mock-plc",
                    "mode": "strict (b)" if self.strict_mode else "weak (a)",
                    "dids": len(self.state),
                },
            )
            return

        did = self._path_did()
        if did is None or not did.startswith("did:plc:"):
            self._send_404("DidNotFound", f"unknown path {path!r}")
            return

        state = self.state.get(did)
        if state is None or not state.entries:
            self._send_404("DidNotFound", did)
            return

        if path.endswith("/log/audit"):
            self._send_json(200, state.entries)
            return

        # GET /{did} → reconstructed DID doc. Per PLC spec, when
        # tombstoned, the directory returns the tombstone marker;
        # we surface that as a {tombstoned: true, prev: <cid>}
        # payload alongside the current shape.
        last = state.last()
        if last is None:
            self._send_404("DidNotFound", did)
            return
        op = last["operation"]
        op_type = op.get("type")
        if op_type == "plc_tombstone":
            self._send_json(
                200,
                {
                    "did": did,
                    "tombstoned": True,
                    "prev": op.get("prev"),
                },
            )
            return

        # Build a minimal DID-doc shape from the latest plc_operation.
        services_obj = op.get("services", {}) or {}
        services = [
            {
                "id": f"#{name}",
                "type": entry.get("type"),
                "serviceEndpoint": entry.get("endpoint"),
            }
            for name, entry in services_obj.items()
        ]
        vms_obj = op.get("verificationMethods", {}) or {}
        vms = [
            {
                "id": f"{did}#{name}",
                "type": "Multikey",
                "controller": did,
                "publicKeyMultibase": (
                    vm.split(":", 2)[2] if isinstance(vm, str) and vm.startswith("did:key:") else vm
                ),
            }
            for name, vm in vms_obj.items()
        ]
        self._send_json(
            200,
            {
                "@context": ["https://www.w3.org/ns/did/v1"],
                "id": did,
                "alsoKnownAs": op.get("alsoKnownAs", []),
                "verificationMethod": vms,
                "service": services,
            },
        )

    def do_POST(self) -> None:
        did = self._path_did()
        if did is None or not did.startswith("did:plc:"):
            self._send_json(
                400,
                {
                    "error": "InvalidRequest",
                    "message": f"POST path must be /{{did}}; got {self.path!r}",
                },
            )
            return

        raw = self._read_body()
        try:
            op = json.loads(raw)
            if not isinstance(op, dict):
                raise ValueError("body is not a JSON object")
        except (json.JSONDecodeError, ValueError) as exc:
            self._send_json(
                400,
                {"error": "InvalidRequest", "message": f"malformed request body: {exc}"},
            )
            return

        # Required: type, sig.
        for field in ("type", "sig"):
            if field not in op:
                self._send_json(
                    400,
                    {"error": "InvalidRequest", "message": f"missing field: {field}"},
                )
                return

        op_type = op["type"]
        if op_type not in ("plc_operation", "plc_tombstone"):
            self._send_json(
                400,
                {
                    "error": "InvalidRequest",
                    "message": f"unsupported op type {op_type!r}",
                },
            )
            return

        sig_b64 = op["sig"]
        if not isinstance(sig_b64, str):
            self._send_json(
                400,
                {"error": "InvalidRequest", "message": "sig must be a string"},
            )
            return

        # plc_operation also requires rotationKeys for the
        # genesis-op sig-verify and downstream chaining.
        if op_type == "plc_operation":
            if "rotationKeys" not in op:
                self._send_json(
                    400,
                    {"error": "InvalidRequest", "message": "missing field: rotationKeys"},
                )
                return
            if not isinstance(op["rotationKeys"], list) or not op["rotationKeys"]:
                self._send_json(
                    400,
                    {
                        "error": "InvalidRequest",
                        "message": "rotationKeys must be a non-empty array",
                    },
                )
                return
        else:  # plc_tombstone
            if "prev" not in op:
                self._send_json(
                    400,
                    {"error": "InvalidRequest", "message": "missing field: prev"},
                )
                return

        # Decode base64url-no-pad sig → raw 64-byte r||s.
        sig_b64_padded = sig_b64 + "=" * (-len(sig_b64) % 4)
        try:
            import base64
            sig_raw = base64.urlsafe_b64decode(sig_b64_padded.encode("ascii"))
        except Exception as exc:
            self._send_json(
                400,
                {
                    "error": "InvalidSignature",
                    "message": f"sig is not valid base64url-no-pad: {exc}",
                },
            )
            return

        state = self.state.setdefault(did, DidState())
        last = state.last()
        prev_cid = state.last_cid()

        # Reject update-on-unknown-DID per §5.8.2 error semantics.
        is_genesis = prev_cid is None
        if not is_genesis and state.tombstoned:
            self._send_json(
                400,
                {
                    "error": "InvalidPrev",
                    "message": (
                        f"{did} is tombstoned; subsequent ops are rejected "
                        "(terminal-state semantics per Arc 13 §6.4 Step 4.5)"
                    ),
                },
            )
            return

        # Update-on-unknown: no prior op present → DidNotFound.
        if is_genesis and op.get("prev") not in (None,):
            # Submitter included a prev on what we treat as
            # genesis (we have no prior ops). Treat as
            # update-before-genesis.
            self._send_json(400, {"error": "DidNotFound"})
            return
        if not is_genesis:
            # Check prev chaining.
            op_prev = op.get("prev")
            if op_prev != prev_cid:
                logging.info(
                    "InvalidPrev for %s: op.prev=%r expected=%r", did, op_prev, prev_cid
                )
                self._send_json(400, {"error": "InvalidPrev"})
                return

        # Mode (b) strict signature verification per Arc 13
        # §6.4 Step 4.5.
        if self.strict_mode:
            # Re-encode the unsigned form (sig cleared) per the
            # canonical DAG-CBOR rules + verify against each
            # candidate rotation key.
            unsigned = {k: v for k, v in op.items() if k != "sig"}
            try:
                cbor_bytes = dag_cbor_encode(unsigned)
            except ValueError as exc:
                self._send_json(
                    400,
                    {
                        "error": "InvalidRequest",
                        "message": f"op fails canonical CBOR encoding: {exc}",
                    },
                )
                return

            # Candidate rotation keys depend on op type.
            if op_type == "plc_operation":
                if is_genesis:
                    candidates = op.get("rotationKeys", [])
                else:
                    candidates = (last or {}).get("operation", {}).get("rotationKeys", [])
            else:
                # tombstone: verify against prior op's rotation keys
                # (genesis-tombstone is impossible — caller would
                # need a prior op to reference).
                if is_genesis:
                    self._send_json(
                        400,
                        {
                            "error": "DidNotFound",
                            "message": "tombstone requires a prior accepted op",
                        },
                    )
                    return
                candidates = (last or {}).get("operation", {}).get("rotationKeys", [])

            if not candidates:
                self._send_json(
                    400,
                    {
                        "error": "InvalidSignature",
                        "message": "no candidate rotation keys",
                    },
                )
                return

            sig_ok = False
            for did_key in candidates:
                try:
                    pub = did_key_to_pubkey_bytes(did_key)
                except ValueError as exc:
                    logging.debug("skip rotation key %r: %s", did_key, exc)
                    continue
                if verify_sig_raw_rs(pub, sig_raw, cbor_bytes):
                    sig_ok = True
                    break

            if not sig_ok:
                logging.info(
                    "InvalidSignature for %s op_type=%s: tried %d candidate(s)",
                    did,
                    op_type,
                    len(candidates),
                )
                self._send_json(400, {"error": "InvalidSignature"})
                return

        # Compute the CID of the SIGNED op (the form in the log).
        try:
            signed_cbor = dag_cbor_encode(op)
        except ValueError as exc:
            self._send_json(
                400,
                {
                    "error": "InvalidRequest",
                    "message": f"signed op fails canonical CBOR encoding: {exc}",
                },
            )
            return
        op_cid = cid_for_dag_cbor(signed_cbor)

        # Duplicate detection per §5.8.2: same CID as current head.
        if state.entries:
            for entry in state.entries:
                if entry["cid"] == op_cid and not entry["nullified"]:
                    logging.info("duplicate-op submission for %s cid=%s", did, op_cid)
                    self._send_json(400, {"error": "InvalidPrev"})
                    return

        # Genesis-op did derivation correctness check: the did
        # the caller is POSTing under MUST equal
        # `did:plc:<base32lower(sha256(canonical_unsigned_cbor))[:24]>`.
        # If it doesn't match, the genesis op wasn't actually
        # constructed for this DID — reject.
        if is_genesis and op_type == "plc_operation" and self.strict_mode:
            unsigned = {k: v for k, v in op.items() if k != "sig"}
            unsigned_cbor = dag_cbor_encode(unsigned)
            expected_suffix = base32_lower_no_pad(
                hashlib.sha256(unsigned_cbor).digest()
            )[:24]
            expected_did = f"did:plc:{expected_suffix}"
            if did != expected_did:
                logging.info(
                    "DID-suffix mismatch: posted=%s expected=%s", did, expected_did
                )
                self._send_json(
                    400,
                    {
                        "error": "InvalidSignature",
                        "message": (
                            f"genesis op did-suffix mismatch: posted under {did} "
                            f"but canonical CBOR hash suffix is {expected_suffix}"
                        ),
                    },
                )
                return

        # Accept + append.
        import datetime
        created_at = datetime.datetime.now(datetime.timezone.utc).isoformat()
        entry = {
            "cid": op_cid,
            "did": did,
            "operation": op,
            "nullified": False,
            "createdAt": created_at,
        }
        state.entries.append(entry)
        if op_type == "plc_tombstone":
            state.tombstoned = True
        logging.info(
            "accepted %s op for %s cid=%s (prev=%s)",
            op_type,
            did,
            op_cid,
            op.get("prev"),
        )
        self._send_json(200, {"cid": op_cid})


# ============================================================
# Entrypoint
# ============================================================


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Mock PLC directory for Aurora-Locus Phase B."
    )
    parser.add_argument(
        "--port",
        type=int,
        default=2582,
        help="HTTP port to listen on (default: 2582)",
    )
    parser.add_argument(
        "--no-strict",
        dest="strict",
        action="store_false",
        default=True,
        help=(
            "Disable strict signature verification (mode a — "
            "trust-on-faith). Arc 13 Phase B requires mode b (default)."
        ),
    )
    args = parser.parse_args()

    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(levelname)s %(message)s",
    )

    MockHandler.state = {}
    MockHandler.strict_mode = args.strict
    mode_label = "strict (b)" if args.strict else "weak (a)"
    logging.info("mock-plc starting on 127.0.0.1:%d (mode: %s)", args.port, mode_label)

    server = ThreadingHTTPServer(("127.0.0.1", args.port), MockHandler)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        logging.info("mock-plc shutting down")
        server.shutdown()
    return 0


if __name__ == "__main__":
    sys.exit(main())
