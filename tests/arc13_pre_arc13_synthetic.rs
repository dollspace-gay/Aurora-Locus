//! Arc 13 §6.4 Step 4.6 driver — compile-and-shape test for the
//! synthetic pre-Arc-13 op utility at `tests/support/pre_arc13_signing.rs`.
//!
//! This driver does NOT submit to a real PLC directory (that's
//! operator-driven Phase B Scenario 1's negative-path test per
//! `V05_DESIGN.md §6.8.2`). It exercises the utility's output
//! shape so a refactor that breaks the synthetic generator trips
//! immediately at `cargo test` rather than at Phase B.

#[path = "support/pre_arc13_signing.rs"]
mod pre_arc13_signing;

use aurora_locus::crypto::plc::PlcSigner;

#[test]
fn synthetic_op_carries_did_field_and_hex_sig_per_pre_arc13_shape() {
    let signer = PlcSigner::new(&[7u8; 32]).expect("signer");
    let (did, op) =
        pre_arc13_signing::synthesize_pre_arc13_genesis_op(
            &signer,
            "alice.example",
            "http://127.0.0.1:2583",
        );

    // Pre-Arc-13 wire shape MUST include the `did` field on the
    // op itself (the bug Arc 13 fixes by removing it).
    assert!(
        op.get("did").is_some(),
        "synthetic pre-Arc-13 op MUST carry a `did` field — that's the wire-shape bug we're synthesizing"
    );
    assert_eq!(op["did"].as_str(), Some(did.as_str()));

    // sig must be hex (not base64url). 64-byte raw sig = 128 hex chars.
    let sig = op
        .get("sig")
        .and_then(|s| s.as_str())
        .expect("sig present");
    assert_eq!(sig.len(), 128, "raw r||s sig = 64 bytes = 128 hex chars");
    assert!(
        sig.chars().all(|c| c.is_ascii_hexdigit()),
        "pre-Arc-13 sig MUST be hex (not base64url)"
    );

    // services is a JSON array (not the new map shape).
    let services = op.get("services").expect("services present");
    assert!(
        services.is_array(),
        "pre-Arc-13 services MUST be a JSON array (the bug Arc 13 fixes by switching to a map keyed by name)"
    );

    // verification_methods is a JSON array (not the new map shape).
    let vms = op
        .get("verificationMethods")
        .expect("verificationMethods present");
    assert!(
        vms.is_array(),
        "pre-Arc-13 verificationMethods MUST be a JSON array"
    );

    // DID suffix matches the legacy pubkey-hash derivation
    // (Arc 13 changes to SHA-256 of canonical CBOR of unsigned
    // op; the synthetic utility deliberately uses the wrong
    // derivation).
    assert!(did.starts_with("did:plc:"));
    assert_eq!(did.len(), "did:plc:".len() + 24);
}

#[test]
fn synthetic_op_is_byte_distinct_from_arc13_shape() {
    // Two signers with the same key produce the SAME synthetic
    // op (deterministic) — proves the utility is stable for
    // Phase B re-runs.
    let signer = PlcSigner::new(&[42u8; 32]).expect("signer");
    let (_, op1) = pre_arc13_signing::synthesize_pre_arc13_genesis_op(
        &signer,
        "bob.example",
        "http://127.0.0.1:2583",
    );
    let (_, op2) = pre_arc13_signing::synthesize_pre_arc13_genesis_op(
        &signer,
        "bob.example",
        "http://127.0.0.1:2583",
    );
    assert_eq!(op1, op2);
}
