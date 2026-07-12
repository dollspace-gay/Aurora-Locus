//! PLC (Public Ledger of Credentials) operation signing — Arc 13
//! spec-correct surface per V05_DESIGN.md §6.3.1.
//!
//! Wire-shape, per chainlink #61's ten divergences:
//!
//! - **No `did` field** on the op itself (was: pre-Arc-13 included
//!   a `did` field; PLC spec disallows it). DID-suffix is derived
//!   from the canonical CBOR of the unsigned genesis op
//!   ([`derive_did_suffix`]).
//! - **Canonical DAG-CBOR** is the signing input, NOT JSON
//!   ([`op_to_canonical_lex_value`] + `proto_blue::lex_cbor::encode`).
//! - **base64url-no-pad** sig encoding (was: hex). The `sig` field
//!   ends up as a base64url string in the serialized op.
//! - **`rotation_keys` / `also_known_as` / `verification_methods` /
//!   `services` are not optional** — they're always-present fields
//!   on the op. Only `prev` (None on genesis) and `sig` (None on
//!   unsigned form) are `Option<String>` with
//!   `#[serde(skip_serializing_if = "Option::is_none")]`.
//! - **`verification_methods` and `services` are maps**, not lists.
//!   `verification_methods: BTreeMap<String, String>` (name →
//!   did:key URI); `services: BTreeMap<String, ServiceEntry>`.
//! - **Signer's public key MUST be in `rotation_keys`** (chainlink
//!   #61 §1.4.5). This module doesn't enforce that invariant by
//!   itself; callers (post-Step-0.7 `generate_plc_did`) construct
//!   `rotation_keys` to satisfy it.
//!
//! Step 0.5 lands the struct + signing path + helpers. Step 0.7
//! refactors call sites to use the PDS-wide rotation key per
//! §6.3.2 key separation.

use crate::crypto::secp256k1::Secp256k1KeyPair;
use crate::error::{PdsError, PdsResult};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use k256::ecdsa::{signature::Signer, Signature};
use proto_blue::lex_cbor::{cid_for_lex, encode as lex_encode};
use proto_blue::lex_data::LexValue;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// PLC operation per spec — Arc 13 wire shape.
///
/// Field naming on the wire: camelCase (`rotationKeys`,
/// `verificationMethods`, `alsoKnownAs`) per `#[serde(rename_all)]`.
/// Field-absence convention: `sig` is omitted when `None`; `prev` is
/// ALWAYS serialized — a CID string for updates, explicit `null` for
/// genesis ops. The did:plc spec requires `prev` be present-as-null on a
/// creation ("the key should actually be part of the object, with value
/// null, not simply omitted"), in BOTH the JSON submitted to PLC and the
/// DAG-CBOR the DID suffix is digested over (§6.3.1). Omitting it makes
/// production `plc.directory` reject the op with 400 "Not a valid
/// operation".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlcOperation {
    /// Always `"plc_operation"` for non-tombstone ops.
    #[serde(rename = "type")]
    pub type_: String,
    /// Public-key did:key URIs that may sign update ops.
    pub rotation_keys: Vec<String>,
    /// Name → did:key URI for verification methods. The required
    /// entry for AT Protocol is `"atproto"` → per-actor signing
    /// key did:key.
    pub verification_methods: BTreeMap<String, String>,
    /// Alternate identifiers (e.g., `at://handle`).
    pub also_known_as: Vec<String>,
    /// Name → service entry. The required entry for AT Protocol is
    /// `"atproto_pds"` → `ServiceEntry { type_:
    /// "AtprotoPersonalDataServer", endpoint: <pds-url> }`.
    pub services: BTreeMap<String, ServiceEntry>,
    /// CID of the previous accepted op for this DID. `None` for genesis
    /// ops — serialized as an explicit `null`, never omitted (did:plc
    /// spec); `Some(cid_string)` for updates.
    pub prev: Option<String>,
    /// Base64url-no-pad ECDSA signature over canonical DAG-CBOR
    /// of the op with `sig: None`. `None` before signing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sig: Option<String>,
}

/// Service entry per spec. Field naming on the wire: camelCase
/// (no rename here — the two field names `type` (renamed from
/// `type_`) and `endpoint` are already canonical).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceEntry {
    #[serde(rename = "type")]
    pub type_: String,
    pub endpoint: String,
}

/// Arc 13 §6.3.5 — PLC tombstone op shape. Terminal-state op that
/// retires a DID. Carries only `type` ("plc_tombstone"), `prev`
/// (CID of the last accepted op), and `sig` (base64url ECDSA over
/// canonical CBOR of the unsigned tombstone).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlcTombstone {
    #[serde(rename = "type")]
    pub type_: String,
    pub prev: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sig: Option<String>,
}

impl PlcTombstone {
    /// Construct an unsigned tombstone targeting `prev_cid` (the
    /// CID of the last accepted op for the DID being tombstoned).
    pub fn new(prev_cid: String) -> Self {
        Self {
            type_: "plc_tombstone".to_string(),
            prev: prev_cid,
            sig: None,
        }
    }
}

/// §6.3.5 canonical-CBOR converter for tombstones — Case II
/// equivalent of [`op_to_canonical_lex_value`]. Omits `sig` when
/// `None`.
pub fn tombstone_to_canonical_lex_value(op: &PlcTombstone) -> LexValue {
    let mut m = BTreeMap::<String, LexValue>::new();
    m.insert("type".to_string(), LexValue::String(op.type_.clone()));
    m.insert("prev".to_string(), LexValue::String(op.prev.clone()));
    if let Some(sig) = &op.sig {
        m.insert("sig".to_string(), LexValue::String(sig.clone()));
    }
    LexValue::Map(m)
}

impl PlcSigner {
    /// §6.3.5 — sign a tombstone. Same flow as [`Self::sign_operation`]:
    /// clear sig → canonical CBOR → SHA-256+ECDSA → base64url-no-pad.
    pub fn sign_tombstone(&self, mut op: PlcTombstone) -> PdsResult<PlcTombstone> {
        op.sig = None;
        let lex = tombstone_to_canonical_lex_value(&op);
        let cbor_bytes = lex_encode(&lex).map_err(|e| {
            PdsError::Internal(format!("DAG-CBOR encode failed during tombstone sign: {}", e))
        })?;
        let signature: Signature = self.keypair.signing_key().sign(&cbor_bytes);
        // PLC spec: raw r||s 64-byte form, base64url-no-pad
        // (matches [`Self::sign_operation`]; do NOT DER-encode).
        let sig_b64 = URL_SAFE_NO_PAD.encode(signature.to_bytes());
        op.sig = Some(sig_b64);
        Ok(op)
    }
}

/// Builder for [`PlcOperation`]. No `did` setter (the new spec
/// disallows a `did` field on the op itself). All four collection
/// fields default to empty; the builder caller is responsible for
/// supplying values that satisfy the §6.3.1 invariants (signer's
/// key in `rotation_keys`, `verification_methods["atproto"]`
/// present for AT-Protocol use, etc.).
#[derive(Debug, Default)]
pub struct PlcOperationBuilder {
    rotation_keys: Vec<String>,
    verification_methods: BTreeMap<String, String>,
    also_known_as: Vec<String>,
    services: BTreeMap<String, ServiceEntry>,
    prev: Option<String>,
}

impl PlcOperationBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn rotation_keys(mut self, keys: Vec<String>) -> Self {
        self.rotation_keys = keys;
        self
    }

    pub fn verification_methods(mut self, methods: BTreeMap<String, String>) -> Self {
        self.verification_methods = methods;
        self
    }

    pub fn also_known_as(mut self, aka: Vec<String>) -> Self {
        self.also_known_as = aka;
        self
    }

    pub fn services(mut self, services: BTreeMap<String, ServiceEntry>) -> Self {
        self.services = services;
        self
    }

    pub fn prev(mut self, prev: String) -> Self {
        self.prev = Some(prev);
        self
    }

    /// Build the unsigned operation. Always succeeds — the
    /// spec-correctness invariants (signer-in-rotation-keys,
    /// services["atproto_pds"] present for non-tombstone ops, etc.)
    /// are caller responsibility; `validate_plc_operation` checks
    /// them post-build.
    pub fn build(self) -> PdsResult<PlcOperation> {
        Ok(PlcOperation {
            type_: "plc_operation".to_string(),
            rotation_keys: self.rotation_keys,
            verification_methods: self.verification_methods,
            also_known_as: self.also_known_as,
            services: self.services,
            prev: self.prev,
            sig: None,
        })
    }
}

/// §6.3.1 Case II canonical-CBOR converter. Builds a
/// [`LexValue::Map`] whose entries match the AT Protocol PLC op
/// wire shape. `sig` is omitted when `None`; `prev` is always present
/// — a CID string for updates, explicit CBOR `null` for genesis ops
/// (present-as-null per the did:plc spec, so the DID suffix digested
/// over this CBOR matches what `plc.directory` computes).
///
/// Map ordering input is informational — `proto_blue::lex_cbor::encode`
/// re-sorts map keys by byte-length then lexicographically per
/// DAG-CBOR strict mode.
pub fn op_to_canonical_lex_value(op: &PlcOperation) -> LexValue {
    let mut m = BTreeMap::<String, LexValue>::new();
    m.insert("type".to_string(), LexValue::String(op.type_.clone()));
    m.insert(
        "rotationKeys".to_string(),
        LexValue::Array(
            op.rotation_keys
                .iter()
                .map(|s| LexValue::String(s.clone()))
                .collect(),
        ),
    );
    let vm_lex: BTreeMap<String, LexValue> = op
        .verification_methods
        .iter()
        .map(|(k, v)| (k.clone(), LexValue::String(v.clone())))
        .collect();
    m.insert("verificationMethods".to_string(), LexValue::Map(vm_lex));
    m.insert(
        "alsoKnownAs".to_string(),
        LexValue::Array(
            op.also_known_as
                .iter()
                .map(|s| LexValue::String(s.clone()))
                .collect(),
        ),
    );
    let svc_lex: BTreeMap<String, LexValue> = op
        .services
        .iter()
        .map(|(k, v)| (k.clone(), service_entry_to_lex(v)))
        .collect();
    m.insert("services".to_string(), LexValue::Map(svc_lex));
    // `prev` is always present per the did:plc spec: a CID string for
    // updates, explicit CBOR null for genesis ops (present-as-null, not
    // omitted). This CBOR is what the DID suffix + op CID are digested
    // over, so it must match the JSON submitted to PLC.
    m.insert(
        "prev".to_string(),
        match &op.prev {
            Some(prev) => LexValue::String(prev.clone()),
            None => LexValue::Null,
        },
    );
    if let Some(sig) = &op.sig {
        m.insert("sig".to_string(), LexValue::String(sig.clone()));
    }
    LexValue::Map(m)
}

fn service_entry_to_lex(s: &ServiceEntry) -> LexValue {
    let mut m = BTreeMap::<String, LexValue>::new();
    m.insert("type".to_string(), LexValue::String(s.type_.clone()));
    m.insert(
        "endpoint".to_string(),
        LexValue::String(s.endpoint.clone()),
    );
    LexValue::Map(m)
}

/// §6.3.1 DID-suffix derivation. SHA-256 over the canonical DAG-CBOR
/// of the SIGNED genesis op (the `sig` field included), base32-lower
/// (no padding), first 24 chars. The full DID is
/// `format!("did:plc:{}", suffix)`.
///
/// Call site convention: pass the SIGNED genesis op (the output of
/// [`PlcSigner::sign_operation`]). Per the did:plc spec the DID is
/// derived from the signed operation — the same bytes `plc.directory`
/// recomputes from the submitted body to validate `POST /{did}`, so
/// deriving over the unsigned op produces a mismatching DID. `prev` is
/// digested as an explicit CBOR `null` on genesis (chainlink #430).
pub fn derive_did_suffix(op: &PlcOperation) -> PdsResult<String> {
    let lex = op_to_canonical_lex_value(op);
    let cbor = lex_encode(&lex)
        .map_err(|e| PdsError::Internal(format!("DAG-CBOR encode failed: {}", e)))?;
    let mut hasher = Sha256::new();
    hasher.update(&cbor);
    let hash = hasher.finalize();
    let b32 = base32::encode(
        base32::Alphabet::Rfc4648Lower { padding: false },
        &hash,
    );
    Ok(b32[..24].to_string())
}

/// §6.3.1 CID computation for a signed op (the `prev` field of the
/// next op should be this CID). Uses
/// `proto_blue::lex_cbor::cid_for_lex` which produces a CID over
/// the canonical DAG-CBOR encoding (SHA-256 + DAG-CBOR codec).
/// Returns the bafyrei… base32 multibase string PLC expects.
pub fn compute_op_cid(signed_op: &PlcOperation) -> PdsResult<String> {
    let lex = op_to_canonical_lex_value(signed_op);
    let cid = cid_for_lex(&lex)
        .map_err(|e| PdsError::Internal(format!("CID computation failed: {}", e)))?;
    Ok(cid.to_string())
}

/// PLC signer — wraps a `Secp256k1KeyPair`.
#[derive(Clone)]
pub struct PlcSigner {
    keypair: Secp256k1KeyPair,
}

impl PlcSigner {
    /// Construct from a 32-byte k256 private key.
    pub fn new(private_key: &[u8]) -> PdsResult<Self> {
        let keypair = Secp256k1KeyPair::from_bytes(private_key)?;
        Ok(Self { keypair })
    }

    /// Construct from a hex-encoded 32-byte k256 private key.
    pub fn from_hex(hex_key: &str) -> PdsResult<Self> {
        let key_bytes = hex::decode(hex_key)
            .map_err(|e| PdsError::Validation(format!("Invalid hex private key: {}", e)))?;
        Self::new(&key_bytes)
    }

    /// §6.3.1 spec-correct signing path:
    ///
    /// 1. Clear `sig` so the canonical-CBOR encoding is the
    ///    unsigned form.
    /// 2. Convert to canonical [`LexValue::Map`] omitting
    ///    `None`-valued entries.
    /// 3. Encode to DAG-CBOR via `proto_blue::lex_cbor::encode`.
    /// 4. ECDSA sign over the CBOR bytes (k256's `Signer::sign`
    ///    does SHA-256 internally — sig is over SHA-256(CBOR)).
    /// 5. Base64url-no-pad encode the signature (was hex
    ///    pre-Arc-13).
    /// 6. Set `sig` on the op and return.
    pub fn sign_operation(&self, mut op: PlcOperation) -> PdsResult<PlcOperation> {
        op.sig = None;
        let unsigned_lex = op_to_canonical_lex_value(&op);
        let cbor_bytes = lex_encode(&unsigned_lex).map_err(|e| {
            PdsError::Internal(format!("DAG-CBOR encode failed during sign: {}", e))
        })?;
        let signature: Signature = self.keypair.signing_key().sign(&cbor_bytes);
        let sig_b64 = URL_SAFE_NO_PAD.encode(signature.to_bytes());
        op.sig = Some(sig_b64);
        Ok(op)
    }

    /// Compressed public key bytes, hex-encoded (66 chars).
    pub fn public_key_hex(&self) -> String {
        self.keypair.public_key_hex()
    }

    /// Multibase-encoded public key (base58btc with `z` prefix).
    pub fn public_key_multibase(&self) -> String {
        self.keypair.public_key_multibase()
    }

    /// did:key URI for this signer's public key.
    pub fn public_key_did_key(&self) -> String {
        self.keypair.did()
    }

    /// Verifying key (public).
    pub fn verifying_key(&self) -> k256::ecdsa::VerifyingKey {
        self.keypair.verifying_key()
    }

    /// Arc 13 §6.4 Step 4.6 + round-4 F1 closure — raw ECDSA
    /// primitive accessor for the synthetic pre-Arc-13 test
    /// utility at `tests/support/pre_arc13_signing.rs`.
    ///
    /// The "test utility MUST NOT share code" prohibition (per
    /// §6.4 Step 4.6) applies to op-construction, serialization,
    /// canonicalization, and signature-encoding code — the four
    /// layers the wire-shape deviations target. **Below those
    /// layers, the raw ECDSA primitive is the same whether test
    /// or production calls it; reusing it preserves test-utility
    /// independence at the layer that matters.**
    ///
    /// `sign_raw` signs arbitrary bytes (k256 hashes them via
    /// SHA-256 internally per its Signer trait). Returns the
    /// raw 64-byte r||s form. Callers (the pre-Arc-13 utility)
    /// own all encoding/canonicalization decisions on either
    /// side of this call.
    ///
    /// Sole consumer is the pre-Arc-13 test scaffold at
    /// `tests/support/pre_arc13_signing.rs`; production uses
    /// `sign_operation` / `sign_tombstone`. The `--lib` build
    /// doesn't see integration tests, so the lint fires.
    #[allow(dead_code)]
    pub fn sign_raw(&self, msg: &[u8]) -> Signature {
        self.keypair.signing_key().sign(msg)
    }
}

/// Light shape check for a fully-built op. Pre-Arc-13 this was
/// `did`-checking; that field is gone. Now: type sanity + sig
/// presence + sig base64url-decodability.
pub fn validate_plc_operation(operation: &PlcOperation) -> PdsResult<()> {
    if operation.type_ != "plc_operation" {
        return Err(PdsError::Validation(format!(
            "Invalid operation type {:?}, expected 'plc_operation'",
            operation.type_
        )));
    }
    let sig = operation
        .sig
        .as_ref()
        .ok_or_else(|| PdsError::Validation("Operation must be signed".to_string()))?;
    URL_SAFE_NO_PAD
        .decode(sig)
        .map_err(|e| PdsError::Validation(format!("Signature must be valid base64url: {}", e)))?;
    Ok(())
}

/// Register a signed PLC op against the directory. The caller
/// supplies the DID separately (Arc 13's op has no `did` field;
/// the DID is the URL path component).
pub async fn register_plc_did(
    plc_url: &str,
    did: &str,
    operation: PlcOperation,
) -> PdsResult<String> {
    validate_plc_operation(&operation)?;
    let client = reqwest::Client::new();
    let endpoint = format!("{}/{}", plc_url.trim_end_matches('/'), did);
    let response = client
        .post(&endpoint)
        .json(&operation)
        .send()
        .await
        .map_err(|e| PdsError::Internal(format!("PLC registration request failed: {}", e)))?;
    if response.status().is_success() {
        Ok(did.to_string())
    } else {
        let status = response.status();
        let error_body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        Err(PdsError::Internal(format!(
            "PLC directory returned error {}: {}",
            status, error_body
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_op() -> PlcOperation {
        let mut vm = BTreeMap::new();
        vm.insert(
            "atproto".to_string(),
            "did:key:zQ3sample".to_string(),
        );
        let mut svc = BTreeMap::new();
        svc.insert(
            "atproto_pds".to_string(),
            ServiceEntry {
                type_: "AtprotoPersonalDataServer".to_string(),
                endpoint: "http://localhost:2583".to_string(),
            },
        );
        PlcOperationBuilder::new()
            .rotation_keys(vec!["did:key:zQ3rotation".to_string()])
            .verification_methods(vm)
            .also_known_as(vec!["at://alice.localhost".to_string()])
            .services(svc)
            .build()
            .unwrap()
    }

    #[test]
    fn test_plc_signer_creation() {
        let signer = PlcSigner::new(&[1u8; 32]);
        assert!(signer.is_ok());
    }

    #[test]
    fn test_plc_signer_invalid_key_length() {
        let signer = PlcSigner::new(&[1u8; 16]);
        assert!(signer.is_err());
    }

    #[test]
    fn test_plc_signer_from_hex() {
        let signer = PlcSigner::from_hex(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        );
        assert!(signer.is_ok());
    }

    #[test]
    fn test_builder_default_is_empty() {
        let op = PlcOperationBuilder::new().build().unwrap();
        assert_eq!(op.type_, "plc_operation");
        assert!(op.rotation_keys.is_empty());
        assert!(op.verification_methods.is_empty());
        assert!(op.also_known_as.is_empty());
        assert!(op.services.is_empty());
        assert!(op.prev.is_none());
        assert!(op.sig.is_none());
    }

    #[test]
    fn test_sign_operation_produces_base64url_sig() {
        let signer = PlcSigner::new(&[42u8; 32]).unwrap();
        let op = sample_op();
        let signed = signer.sign_operation(op).unwrap();
        let sig = signed.sig.expect("sig set");
        // base64url-no-pad alphabet
        assert!(
            sig.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "sig must be base64url-no-pad alphabet"
        );
        // Decodes cleanly.
        URL_SAFE_NO_PAD.decode(&sig).expect("sig decodes as base64url");
    }

    #[test]
    fn test_sign_operation_is_deterministic_for_same_input() {
        let signer = PlcSigner::new(&[42u8; 32]).unwrap();
        let op1 = sample_op();
        let op2 = sample_op();
        let s1 = signer.sign_operation(op1).unwrap();
        let s2 = signer.sign_operation(op2).unwrap();
        assert_eq!(s1.sig, s2.sig);
    }

    #[test]
    fn test_op_to_canonical_lex_value_genesis_prev_null_sig_omitted() {
        let op = sample_op();
        let lex = op_to_canonical_lex_value(&op);
        if let LexValue::Map(m) = &lex {
            // Genesis: `prev` present as explicit CBOR null (did:plc spec);
            // `sig` omitted (unsigned op).
            assert!(
                matches!(m.get("prev"), Some(LexValue::Null)),
                "genesis prev must be present as CBOR null, not omitted"
            );
            assert!(!m.contains_key("sig"));
            // Required fields present.
            assert!(m.contains_key("type"));
            assert!(m.contains_key("rotationKeys"));
            assert!(m.contains_key("verificationMethods"));
            assert!(m.contains_key("alsoKnownAs"));
            assert!(m.contains_key("services"));
        } else {
            panic!("expected LexValue::Map");
        }
    }

    #[test]
    fn test_op_to_canonical_lex_value_includes_present_prev_and_sig() {
        let mut op = sample_op();
        op.prev = Some("bafyreiPREV".to_string());
        op.sig = Some("AAAA".to_string());
        let lex = op_to_canonical_lex_value(&op);
        if let LexValue::Map(m) = &lex {
            assert!(matches!(m.get("prev"), Some(LexValue::String(s)) if s == "bafyreiPREV"));
            assert!(matches!(m.get("sig"), Some(LexValue::String(s)) if s == "AAAA"));
        } else {
            panic!("expected LexValue::Map");
        }
    }

    #[test]
    fn test_derive_did_suffix_shape() {
        let op = sample_op();
        let suffix = derive_did_suffix(&op).unwrap();
        assert_eq!(suffix.len(), 24, "PLC suffix is 24 base32 chars");
        assert!(
            suffix.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
            "PLC suffix is base32-lower (RFC 4648 alphabet, no padding)"
        );
    }

    #[test]
    fn test_derive_did_suffix_deterministic() {
        let op1 = sample_op();
        let op2 = sample_op();
        assert_eq!(
            derive_did_suffix(&op1).unwrap(),
            derive_did_suffix(&op2).unwrap()
        );
    }

    #[test]
    fn test_compute_op_cid_shape() {
        let signer = PlcSigner::new(&[42u8; 32]).unwrap();
        let signed = signer.sign_operation(sample_op()).unwrap();
        let cid = compute_op_cid(&signed).unwrap();
        assert!(
            cid.starts_with("bafyrei"),
            "CID should start with bafyrei (base32 multibase, SHA-256 + DAG-CBOR)"
        );
    }

    #[test]
    fn test_validate_signed_operation_passes() {
        let signer = PlcSigner::new(&[42u8; 32]).unwrap();
        let signed = signer.sign_operation(sample_op()).unwrap();
        assert!(validate_plc_operation(&signed).is_ok());
    }

    #[test]
    fn test_validate_unsigned_operation_fails() {
        let op = sample_op();
        assert!(validate_plc_operation(&op).is_err());
    }

    #[test]
    fn test_no_did_field_on_wire() {
        let signer = PlcSigner::new(&[42u8; 32]).unwrap();
        let signed = signer.sign_operation(sample_op()).unwrap();
        let json = serde_json::to_value(&signed).unwrap();
        assert!(
            json.get("did").is_none(),
            "PLC op MUST NOT have a `did` field per Arc 13 §6.3.1"
        );
    }

    #[test]
    fn test_json_genesis_prev_null_sig_omitted_via_serde() {
        let op = sample_op();
        let json = serde_json::to_value(&op).unwrap();
        // Genesis: `prev` serialized as explicit JSON null (present); `sig`
        // omitted (unsigned op).
        assert_eq!(
            json.get("prev"),
            Some(&serde_json::Value::Null),
            "JSON must emit prev: null on genesis, not omit it"
        );
        assert!(json.get("sig").is_none(), "JSON omits absent sig");
    }

    #[test]
    fn test_genesis_prev_null_in_both_json_and_cbor() {
        // Regression guard for the did:plc "prev present-as-null on genesis"
        // rule (spec §prev): production `plc.directory` rejects a genesis op
        // that omits `prev` with 400 "Not a valid operation". The submitted
        // JSON and the DAG-CBOR the DID suffix is digested over must BOTH
        // carry `prev` as an explicit null, or the op is rejected / the DID
        // fails to match PLC's recomputation.
        let signer = PlcSigner::new(&[42u8; 32]).unwrap();
        let signed = signer.sign_operation(sample_op()).unwrap();

        // JSON actually POSTed to PLC (`register_plc_did` uses `.json(&op)`).
        let json = serde_json::to_value(&signed).unwrap();
        assert_eq!(
            json.get("prev"),
            Some(&serde_json::Value::Null),
            "submitted genesis JSON must carry prev: null"
        );

        // DAG-CBOR the DID suffix / op CID are digested over.
        let lex = op_to_canonical_lex_value(&signed);
        if let LexValue::Map(m) = &lex {
            assert!(
                matches!(m.get("prev"), Some(LexValue::Null)),
                "genesis CBOR must carry prev as null so the DID matches PLC's digest"
            );
        } else {
            panic!("expected LexValue::Map");
        }
    }

    #[test]
    fn test_did_suffix_derived_from_signed_op_matches_plc_convention() {
        // did:plc derives the DID from the SIGNED genesis op (sig field
        // included) — the exact bytes plc.directory recomputes to validate
        // POST /{did}. Deriving over the unsigned op yields a different
        // suffix and a DID-identity rejection. Guard the distinction and pin
        // the digest to the signed CBOR (chainlink #430 follow-on).
        let signer = PlcSigner::new(&[42u8; 32]).unwrap();
        let unsigned = sample_op();
        let signed = signer.sign_operation(unsigned.clone()).unwrap();
        assert!(signed.sig.is_some(), "sign_operation must set sig");

        let from_signed = derive_did_suffix(&signed).unwrap();
        let from_unsigned = derive_did_suffix(&unsigned).unwrap();
        assert_ne!(
            from_signed, from_unsigned,
            "signed vs unsigned genesis must digest differently — the sig field \
             participates in the DID derivation, so the account-manager must \
             derive from the signed op"
        );

        // Pin `from_signed` to hash(canonical DAG-CBOR of the SIGNED op)
        // base32-lower[..24] — i.e. digested over the signed bytes, matching
        // what PLC computes from the submitted body.
        let cbor = lex_encode(&op_to_canonical_lex_value(&signed)).unwrap();
        let mut hasher = Sha256::new();
        hasher.update(&cbor);
        let hash = hasher.finalize();
        let expected = base32::encode(
            base32::Alphabet::Rfc4648Lower { padding: false },
            &hash,
        )[..24]
            .to_string();
        assert_eq!(
            from_signed, expected,
            "DID suffix must be the digest of the signed op's canonical CBOR"
        );
    }

    #[test]
    fn test_did_suffix_known_answer_vector_matches_strict_dag_cbor() {
        // Known-answer vector pinning proto-blue's DAG-CBOR to a spec-strict
        // encoding (chainlink #433). The suffix below was cross-verified
        // byte-for-byte against an independent, hand-rolled RFC-8949-strict
        // DAG-CBOR encoder (phase-b/mock-plc.py's `dag_cbor_encode`, which
        // sorts map keys by byte-length then lexicographically, encodes `null`
        // as 0xf6, and uses canonical uints): both hash the signed op below to
        // this exact suffix. If proto-blue's canonicalization ever drifts from
        // spec-strict form, this breaks — before a non-canonical DID can reach
        // production `plc.directory`. The vector is fixed by `sample_op()` +
        // the deterministic secp256k1 key `[42; 32]`.
        let signer = PlcSigner::new(&[42u8; 32]).unwrap();
        let signed = signer.sign_operation(sample_op()).unwrap();
        assert_eq!(
            derive_did_suffix(&signed).unwrap(),
            "j6ivb4xbpaj2fn3i5dseb3f6",
            "proto-blue DAG-CBOR diverged from the spec-strict encoding"
        );
    }

    #[test]
    fn test_public_key_extraction() {
        let signer = PlcSigner::new(&[42u8; 32]).unwrap();
        let pk = signer.public_key_hex();
        assert_eq!(pk.len(), 66, "33 bytes SEC1-compressed hex-encoded");
    }
}
