#!/usr/bin/env python3
"""Self-test for phase-b/mock-plc.py.

Exercises three scenarios end-to-end:
  1. Genesis-op accept (sig verifies; DID-suffix derivation matches).
  2. Update-op accept (snapshot mutator + prev-chain check).
  3. Malformed-JSON reject (400 InvalidRequest).
  4. Bonus: duplicate-op rejection (400 InvalidPrev).

Assumes the mock is already running on the same port the
script is configured for (default 2582). Exits 0 on all-pass,
non-zero with a diagnostic on failure.

Dependencies: same as mock-plc.py — `cryptography` only.

Usage:
  python3 phase-b/test-mock-plc.py            # default port 2582
  python3 phase-b/test-mock-plc.py --port 9999
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import sys
import urllib.request
from typing import Any
from urllib.error import HTTPError

from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.hazmat.primitives.asymmetric.utils import decode_dss_signature

# Re-import the helpers from the server script. We do this by
# loading the file directly so we don't need a package layout.
import importlib.util
import pathlib

_SCRIPT_DIR = pathlib.Path(__file__).parent
_MOCK_SPEC = importlib.util.spec_from_file_location(
    "mock_plc_module", _SCRIPT_DIR / "mock-plc.py"
)
assert _MOCK_SPEC is not None and _MOCK_SPEC.loader is not None
_MOCK = importlib.util.module_from_spec(_MOCK_SPEC)
_MOCK_SPEC.loader.exec_module(_MOCK)

dag_cbor_encode = _MOCK.dag_cbor_encode
cid_for_dag_cbor = _MOCK.cid_for_dag_cbor
base32_lower_no_pad = _MOCK.base32_lower_no_pad


# ============================================================
# base58btc encode (mirror of mock's decode, used here to
# encode the test's fresh pubkey into did:key form)
# ============================================================

_BASE58_ALPHABET = (
    "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
)


def base58_encode(data: bytes) -> str:
    n = int.from_bytes(data, "big") if data else 0
    out = ""
    while n > 0:
        n, r = divmod(n, 58)
        out = _BASE58_ALPHABET[r] + out
    # Each leading 0x00 byte → '1' prefix.
    for b in data:
        if b == 0x00:
            out = "1" + out
        else:
            break
    return out


def did_key_from_compressed_pubkey(pubkey_compressed: bytes) -> str:
    """did:key:z<base58btc(0xe7 0x01 || compressed_pubkey)>."""
    assert len(pubkey_compressed) == 33
    payload = b"\xe7\x01" + pubkey_compressed
    return f"did:key:z{base58_encode(payload)}"


# ============================================================
# Test fixture: generate a secp256k1 keypair + helpers to
# sign in the Arc 13 PLC wire shape.
# ============================================================


def make_signer() -> tuple[ec.EllipticCurvePrivateKey, bytes, str]:
    """Returns (private_key, compressed_pubkey_bytes, did_key)."""
    priv = ec.generate_private_key(ec.SECP256K1())
    pub_bytes = priv.public_key().public_bytes(
        encoding=__import__("cryptography").hazmat.primitives.serialization.Encoding.X962,
        format=__import__("cryptography").hazmat.primitives.serialization.PublicFormat.CompressedPoint,
    )
    return priv, pub_bytes, did_key_from_compressed_pubkey(pub_bytes)


def sign_op(priv: ec.EllipticCurvePrivateKey, op: dict[str, Any]) -> dict[str, Any]:
    """Sign `op` in-place per Arc 13 §6.3.1: canonical CBOR,
    SHA-256+ECDSA, raw r||s, base64url-no-pad."""
    unsigned = {k: v for k, v in op.items() if k != "sig"}
    cbor = dag_cbor_encode(unsigned)
    der_sig = priv.sign(cbor, ec.ECDSA(hashes.SHA256()))
    r, s = decode_dss_signature(der_sig)
    raw = r.to_bytes(32, "big") + s.to_bytes(32, "big")
    op["sig"] = base64.urlsafe_b64encode(raw).rstrip(b"=").decode("ascii")
    return op


def derive_did(unsigned_op: dict[str, Any]) -> str:
    """Arc 13 §6.3.1 DID-suffix derivation."""
    cbor = dag_cbor_encode(unsigned_op)
    suffix = base32_lower_no_pad(hashlib.sha256(cbor).digest())[:24]
    return f"did:plc:{suffix}"


# ============================================================
# HTTP helpers
# ============================================================


def post_json(url: str, body: dict[str, Any]) -> tuple[int, dict[str, Any]]:
    data = json.dumps(body).encode("utf-8")
    req = urllib.request.Request(
        url,
        data=data,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=10) as resp:
            return resp.status, json.loads(resp.read())
    except HTTPError as e:
        body_bytes = e.read() if e.fp else b""
        try:
            body_json = json.loads(body_bytes) if body_bytes else {}
        except json.JSONDecodeError:
            body_json = {"raw": body_bytes.decode("utf-8", "replace")}
        return e.code, body_json


def post_raw(url: str, raw: bytes, content_type: str = "application/json") -> tuple[int, dict[str, Any]]:
    req = urllib.request.Request(
        url, data=raw, headers={"Content-Type": content_type}, method="POST"
    )
    try:
        with urllib.request.urlopen(req, timeout=10) as resp:
            return resp.status, json.loads(resp.read())
    except HTTPError as e:
        body_bytes = e.read() if e.fp else b""
        try:
            body_json = json.loads(body_bytes) if body_bytes else {}
        except json.JSONDecodeError:
            body_json = {"raw": body_bytes.decode("utf-8", "replace")}
        return e.code, body_json


def get_json(url: str) -> tuple[int, Any]:
    try:
        with urllib.request.urlopen(url, timeout=10) as resp:
            return resp.status, json.loads(resp.read())
    except HTTPError as e:
        body_bytes = e.read() if e.fp else b""
        try:
            body_json = json.loads(body_bytes) if body_bytes else {}
        except json.JSONDecodeError:
            body_json = {"raw": body_bytes.decode("utf-8", "replace")}
        return e.code, body_json


# ============================================================
# Scenarios
# ============================================================


def check(cond: bool, label: str) -> None:
    if not cond:
        print(f"FAIL: {label}", file=sys.stderr)
        raise SystemExit(1)
    print(f"  ok  {label}")


def scenario_genesis_and_update(base_url: str) -> str:
    print("\n[Scenario 1+2] genesis-op + update-op via mutator")

    priv, _, did_key = make_signer()

    # Build genesis op (Aurora-Locus §6.3.1 shape).
    genesis_unsigned = {
        "type": "plc_operation",
        "rotationKeys": [did_key],
        "verificationMethods": {"atproto": did_key},
        "alsoKnownAs": ["at://alice.test"],
        "services": {
            "atproto_pds": {
                "type": "AtprotoPersonalDataServer",
                "endpoint": "http://127.0.0.1:2583",
            }
        },
        # No prev — Case II omits it for genesis.
    }
    did = derive_did(genesis_unsigned)
    print(f"  derived DID: {did}")

    genesis = sign_op(priv, dict(genesis_unsigned))
    status, body = post_json(f"{base_url}/{did}", genesis)
    check(status == 200, f"genesis POST returns 200 (got {status} {body})")
    genesis_cid = body["cid"]
    print(f"  genesis CID: {genesis_cid}")

    # Audit log shows one entry.
    status, audit = get_json(f"{base_url}/{did}/log/audit")
    check(status == 200 and isinstance(audit, list) and len(audit) == 1,
          f"audit log has 1 entry (got status={status} len={len(audit) if isinstance(audit, list) else '?'})")
    check(audit[0]["cid"] == genesis_cid, "audit log CID matches accepted CID")

    # Build update op via snapshot mutator (same fields, new alsoKnownAs).
    update_unsigned = {
        "type": "plc_operation",
        "rotationKeys": genesis_unsigned["rotationKeys"],
        "verificationMethods": genesis_unsigned["verificationMethods"],
        "alsoKnownAs": ["at://alice.updated.test"],
        "services": genesis_unsigned["services"],
        "prev": genesis_cid,
    }
    update = sign_op(priv, dict(update_unsigned))
    status, body = post_json(f"{base_url}/{did}", update)
    check(status == 200, f"update POST returns 200 (got {status} {body})")
    update_cid = body["cid"]
    print(f"  update CID: {update_cid}")

    # Audit log shows 2 entries with right order.
    status, audit = get_json(f"{base_url}/{did}/log/audit")
    check(status == 200 and len(audit) == 2, "audit log has 2 entries")
    check(audit[0]["cid"] == genesis_cid and audit[1]["cid"] == update_cid,
          "audit log oldest-first")

    return did


def scenario_malformed_reject(base_url: str) -> None:
    print("\n[Scenario 3] malformed JSON body rejected with InvalidRequest")
    # Use a synthetic DID so we don't depend on prior state.
    status, body = post_raw(
        f"{base_url}/did:plc:malformedtestaaaaaaaaaa",
        b"{not valid json}",
    )
    check(status == 400, f"malformed POST returns 400 (got {status})")
    check(body.get("error") == "InvalidRequest", f"error == InvalidRequest (got {body!r})")


def scenario_duplicate_reject(base_url: str, did_in_use: str) -> None:
    print("\n[Scenario 4 — bonus] duplicate-op submission rejected with InvalidPrev")
    # Re-fetch the audit log to get the current update_cid; then
    # build an op identical to the update we just did (same fields
    # and same prev) → same CBOR → same CID. The mock should
    # detect duplicate via CID-already-in-log AND prev-chain
    # mismatch (prev != current head); either way 400 InvalidPrev.
    status, audit = get_json(f"{base_url}/{did_in_use}/log/audit")
    check(status == 200, "audit fetch ok")
    last_entry = audit[-1]
    duplicate = dict(last_entry["operation"])
    status, body = post_json(f"{base_url}/{did_in_use}", duplicate)
    check(status == 400, f"duplicate POST returns 400 (got {status} {body})")
    check(body.get("error") == "InvalidPrev",
          f"error == InvalidPrev (got {body!r})")


def scenario_unknown_did_get(base_url: str) -> None:
    print("\n[Scenario 5 — bonus] GET /{unknown_did} returns 404 DidNotFound")
    status, body = get_json(f"{base_url}/did:plc:neverseenthisoneaaaaaaaa")
    check(status == 404, f"GET unknown returns 404 (got {status})")
    check(body.get("error") == "DidNotFound", "error == DidNotFound")


# ============================================================
# main
# ============================================================


def main() -> int:
    parser = argparse.ArgumentParser(description="Self-test for phase-b/mock-plc.py")
    parser.add_argument("--port", type=int, default=2582)
    args = parser.parse_args()
    base_url = f"http://127.0.0.1:{args.port}"

    # Sanity: server is up?
    try:
        status, _ = get_json(f"{base_url}/")
        check(status == 200, f"mock-plc reachable at {base_url} (got {status})")
    except Exception as exc:  # noqa: BLE001
        print(f"FAIL: mock-plc not reachable at {base_url}: {exc}", file=sys.stderr)
        return 1

    try:
        did = scenario_genesis_and_update(base_url)
        scenario_malformed_reject(base_url)
        scenario_duplicate_reject(base_url, did)
        scenario_unknown_did_get(base_url)
    except SystemExit:
        return 1

    print("\nALL PASS ✓")
    return 0


if __name__ == "__main__":
    sys.exit(main())
