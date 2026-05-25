//! Arc 13 §6.4 Step 4.6 + round-4 F1 closure — synthetic
//! pre-Arc-13 PLC op generator.
//!
//! Used by Phase B Scenario 1's negative-path test
//! (`V05_DESIGN.md §6.8.2`): builds a PLC genesis op in the
//! pre-Arc-13 wire shape (JSON-canonical-signed, hex sig, `did`
//! field present), submits to the mock PLC directory in mode (b)
//! per §6.4 Step 4.5, and confirms the mock REJECTS with
//! `HTTP 400 InvalidSignature`. That verifies the mock is
//! actually checking signatures correctly, closing recon §9.1.
//!
//! **Test-utility independence per §6.4 Step 4.6 v4.1 deltas**:
//! the prohibition on sharing code applies to op-construction,
//! serialization, canonicalization, and signature-encoding code.
//! Below those layers, the raw ECDSA primitive (`PlcSigner::sign_raw`)
//! is the same whether test or production calls it; reusing it
//! preserves test-utility independence at the layer that
//! matters. This file SHARES nothing else with
//! `src/crypto/plc.rs`'s Arc 13 wire-shape code.

use aurora_locus::crypto::plc::PlcSigner;
use serde_json::json;
use sha2::{Digest, Sha256};

/// A synthetic pre-Arc-13 genesis op + did suffix derivation.
/// Layout matches what Aurora-Locus emitted before Arc 13:
/// - `did` field present on the op itself.
/// - `rotation_keys`, `also_known_as`, `verification_methods`,
///   `services` may all be optional / wrong-shape.
/// - `sig` is hex-encoded (not base64url).
/// - Signing input is canonical JSON, not canonical CBOR.
///
/// Returns `(did, op_json)` where `op_json` is the full
/// signed-op JSON ready to POST to a PLC directory.
#[allow(dead_code)] // referenced by Phase B operator scripts + arc13_pre_arc13_synthetic driver
pub fn synthesize_pre_arc13_genesis_op(
    signer: &PlcSigner,
    handle_full: &str,
    service_endpoint: &str,
) -> (String, serde_json::Value) {
    // ---- Pre-Arc-13 op shape ----
    // The `did` field is included (modern PLC spec disallows it).
    // The signing key's hex pubkey is hashed to derive the did
    // suffix (modern: SHA-256 of canonical CBOR of unsigned op).
    let pubkey_hex = signer.public_key_hex();
    let mut hasher = Sha256::new();
    hasher.update(pubkey_hex.as_bytes());
    let pubkey_hash = hasher.finalize();
    let did_suffix = base32::encode(
        base32::Alphabet::Rfc4648Lower { padding: false },
        &pubkey_hash,
    )[..24]
        .to_string();
    let did = format!("did:plc:{}", did_suffix);

    // Pre-Arc-13 services as a JSON ARRAY (not the new map shape).
    let services = json!([{
        "id": "#atproto_pds",
        "type": "AtprotoPersonalDataServer",
        "serviceEndpoint": service_endpoint
    }]);

    // Pre-Arc-13 verification_methods as a JSON ARRAY.
    let verification_methods = json!([{
        "id": format!("{}#atproto", did),
        "type": "Multikey",
        "controller": did,
        "publicKeyMultibase": signer.public_key_multibase()
    }]);

    // Build the unsigned op as the same JSON object pre-Arc-13
    // serialized + signed.
    let unsigned = json!({
        "type": "plc_operation",
        "did": did,
        "rotationKeys": [signer.public_key_did_key()],
        "alsoKnownAs": [format!("at://{}", handle_full)],
        "verificationMethods": verification_methods,
        "services": services,
    });

    // Pre-Arc-13 canonical form: JSON serialize, SHA-256, sign.
    // serde_json's default to_vec uses key order matching the
    // struct/object insertion order — not strict-canonical, but
    // that's the bug we're synthesizing.
    let canonical_bytes = serde_json::to_vec(&unsigned).expect("json");
    let mut sig_hasher = Sha256::new();
    sig_hasher.update(&canonical_bytes);
    let prehash = sig_hasher.finalize();
    let sig = signer.sign_raw(&prehash);

    // Pre-Arc-13 sig encoding: HEX (not base64url).
    let sig_hex = hex::encode(sig.to_bytes());

    let mut signed = unsigned.clone();
    signed["sig"] = json!(sig_hex);

    (did, signed)
}
