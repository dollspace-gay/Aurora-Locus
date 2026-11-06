//! did:plc operations: building, signing, verifying, and deriving the DID
//! from a genesis operation.
//!
//! An operation's signature covers the DAG-CBOR of the operation without
//! `sig`; `sig` is the base64url (unpadded) signature. An operation's CID is
//! the DAG-CBOR SHA-256 CID of the signed operation, and a DID is
//! `did:plc:` + the first 24 characters of the lowercase, unpadded base32
//! SHA-256 of the signed genesis operation.

use std::collections::BTreeMap;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use proto_blue_crypto::Signer;
use proto_blue_lex_data::{Cid, LexValue};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PlcError {
    #[error("invalid PLC operation: {0}")]
    Invalid(String),
    #[error("signing failed: {0}")]
    Sign(String),
    #[error("encoding failed: {0}")]
    Encode(String),
}

/// A service entry in a PLC document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    pub kind: String,
    pub endpoint: String,
}

/// A `plc_operation` before signing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operation {
    pub rotation_keys: Vec<String>,
    pub verification_methods: BTreeMap<String, String>,
    pub also_known_as: Vec<String>,
    pub services: BTreeMap<String, Service>,
    /// CID of the previous operation; None for a genesis operation.
    pub prev: Option<String>,
}

/// A signed operation or tombstone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignedOp {
    Operation { op: Operation, sig: String },
    Tombstone { prev: String, sig: String },
}

fn cbor(v: &LexValue) -> Result<Vec<u8>, PlcError> {
    proto_blue_lex_cbor::encode(v).map_err(|e| PlcError::Encode(e.to_string()))
}

fn s(v: &str) -> LexValue {
    LexValue::String(v.to_owned())
}

impl Operation {
    fn to_lex(&self) -> LexValue {
        let mut m = BTreeMap::new();
        m.insert("type".to_owned(), s("plc_operation"));
        m.insert(
            "rotationKeys".to_owned(),
            LexValue::Array(self.rotation_keys.iter().map(|k| s(k)).collect()),
        );
        m.insert(
            "verificationMethods".to_owned(),
            LexValue::Map(
                self.verification_methods
                    .iter()
                    .map(|(k, v)| (k.clone(), s(v)))
                    .collect(),
            ),
        );
        m.insert(
            "alsoKnownAs".to_owned(),
            LexValue::Array(self.also_known_as.iter().map(|k| s(k)).collect()),
        );
        m.insert(
            "services".to_owned(),
            LexValue::Map(
                self.services
                    .iter()
                    .map(|(k, svc)| {
                        let mut e = BTreeMap::new();
                        e.insert("type".to_owned(), s(&svc.kind));
                        e.insert("endpoint".to_owned(), s(&svc.endpoint));
                        (k.clone(), LexValue::Map(e))
                    })
                    .collect(),
            ),
        );
        m.insert(
            "prev".to_owned(),
            self.prev.as_deref().map_or(LexValue::Null, s),
        );
        LexValue::Map(m)
    }

    /// Checks the fields locus accepts in an operation it signs: rotation
    /// keys and verification methods are did:keys, services have a type and
    /// an http(s) endpoint, and `alsoKnownAs` entries are at:// URIs.
    pub fn validate(&self) -> Result<(), PlcError> {
        if self.rotation_keys.is_empty() || self.rotation_keys.len() > 10 {
            return Err(PlcError::Invalid(
                "1 to 10 rotation keys are required".into(),
            ));
        }
        let mut seen = std::collections::HashSet::new();
        for k in &self.rotation_keys {
            if proto_blue_crypto::parse_did_key(k).is_err() {
                return Err(PlcError::Invalid(format!(
                    "rotation key {k:?} is not a did:key"
                )));
            }
            if !seen.insert(k) {
                return Err(PlcError::Invalid(format!(
                    "rotation key {k:?} is listed twice"
                )));
            }
        }
        for (name, k) in &self.verification_methods {
            if proto_blue_crypto::parse_did_key(k).is_err() {
                return Err(PlcError::Invalid(format!(
                    "verification method {name:?} is not a did:key"
                )));
            }
        }
        for aka in &self.also_known_as {
            if !aka.starts_with("at://") || aka.len() <= 5 {
                return Err(PlcError::Invalid(format!(
                    "alsoKnownAs entry {aka:?} is not an at:// URI"
                )));
            }
        }
        for (name, svc) in &self.services {
            if svc.kind.is_empty() {
                return Err(PlcError::Invalid(format!("service {name:?} has no type")));
            }
            let ok = svc.endpoint.starts_with("https://") || svc.endpoint.starts_with("http://");
            if !ok {
                return Err(PlcError::Invalid(format!(
                    "service {name:?} endpoint is not an http(s) URL"
                )));
            }
        }
        Ok(())
    }

    /// Signs this operation with a rotation key.
    pub fn sign(&self, signer: &dyn Signer) -> Result<SignedOp, PlcError> {
        let sig = signer
            .sign(&self.signing_bytes()?)
            .map_err(|e| PlcError::Sign(e.to_string()))?;
        Ok(self.with_signature(&sig))
    }

    /// The bytes a rotation key signs (for a signer elsewhere, e.g. KMS).
    pub fn signing_bytes(&self) -> Result<Vec<u8>, PlcError> {
        cbor(&self.to_lex())
    }

    /// This operation with a signature over `signing_bytes`.
    #[must_use]
    pub fn with_signature(&self, sig: &[u8]) -> SignedOp {
        SignedOp::Operation {
            op: self.clone(),
            sig: URL_SAFE_NO_PAD.encode(sig),
        }
    }
}

impl Operation {
    /// This document with `handle` as its handle: the first `at://` entry of
    /// `alsoKnownAs` is replaced in place (or the handle goes first), as the
    /// reference's `updateHandleOp`.
    #[must_use]
    pub fn with_handle(&self, handle: &str) -> Self {
        let uri = format!("at://{handle}");
        let mut next = self.clone();
        match next
            .also_known_as
            .iter()
            .position(|a| a.starts_with("at://"))
        {
            Some(i) => next.also_known_as[i] = uri,
            None => next.also_known_as.insert(0, uri),
        }
        next
    }

    /// The handle the document names (its first `at://` entry).
    #[must_use]
    pub fn handle(&self) -> Option<&str> {
        self.also_known_as
            .iter()
            .find_map(|a| a.strip_prefix("at://"))
    }
}

/// What a DID's latest operation leaves: the document (None for a
/// tombstone) and the operation's CID, which the next operation names as
/// `prev`. Legacy `create` operations are read as the directory reads them.
pub fn last_op_state(v: &Value) -> Result<(Option<Operation>, String), PlcError> {
    let cid = Cid::for_cbor(&cbor(&json_to_lex(v)?)?).to_string_base32();
    let state = match v.get("type").and_then(Value::as_str) {
        Some("create") => {
            let field = |k: &str| {
                v.get(k)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| PlcError::Invalid(format!("legacy create op without {k}")))
            };
            let signing = field("signingKey")?;
            let recovery = field("recoveryKey")?;
            let handle = field("handle")?;
            let service = field("service")?;
            Some(Operation {
                rotation_keys: vec![recovery, signing.clone()],
                verification_methods: BTreeMap::from([("atproto".to_owned(), signing)]),
                also_known_as: vec![format!("at://{handle}")],
                services: BTreeMap::from([(
                    "atproto_pds".to_owned(),
                    Service {
                        kind: "AtprotoPersonalDataServer".into(),
                        endpoint: service,
                    },
                )]),
                prev: None,
            })
        }
        _ => match SignedOp::from_json(v)? {
            SignedOp::Operation { op, .. } => Some(op),
            SignedOp::Tombstone { .. } => None,
        },
    };
    Ok((state, cid))
}

/// JSON (as the directory serves operations) to the data model.
fn json_to_lex(v: &Value) -> Result<LexValue, PlcError> {
    Ok(match v {
        Value::Null => LexValue::Null,
        Value::Bool(b) => LexValue::Bool(*b),
        Value::Number(n) => LexValue::Integer(
            n.as_i64()
                .ok_or_else(|| PlcError::Invalid("non-integer number in an operation".into()))?,
        ),
        Value::String(s) => LexValue::String(s.clone()),
        Value::Array(a) => LexValue::Array(a.iter().map(json_to_lex).collect::<Result<_, _>>()?),
        Value::Object(m) => LexValue::Map(
            m.iter()
                .map(|(k, v)| Ok((k.clone(), json_to_lex(v)?)))
                .collect::<Result<_, PlcError>>()?,
        ),
    })
}

/// Signs a tombstone of the DID whose latest operation is `prev`.
pub fn sign_tombstone(prev: &str, signer: &dyn Signer) -> Result<SignedOp, PlcError> {
    let unsigned = tombstone_lex(prev, None);
    let sig = signer
        .sign(&cbor(&unsigned)?)
        .map_err(|e| PlcError::Sign(e.to_string()))?;
    Ok(SignedOp::Tombstone {
        prev: prev.to_owned(),
        sig: URL_SAFE_NO_PAD.encode(sig),
    })
}

/// The bytes a rotation key signs for a tombstone of `prev`'s DID.
pub fn tombstone_signing_bytes(prev: &str) -> Result<Vec<u8>, PlcError> {
    cbor(&tombstone_lex(prev, None))
}

/// A tombstone with a signature over `tombstone_signing_bytes`.
#[must_use]
pub fn tombstone_with_signature(prev: &str, sig: &[u8]) -> SignedOp {
    SignedOp::Tombstone {
        prev: prev.to_owned(),
        sig: URL_SAFE_NO_PAD.encode(sig),
    }
}

fn tombstone_lex(prev: &str, sig: Option<&str>) -> LexValue {
    let mut m = BTreeMap::new();
    m.insert("type".to_owned(), s("plc_tombstone"));
    m.insert("prev".to_owned(), s(prev));
    if let Some(sig) = sig {
        m.insert("sig".to_owned(), s(sig));
    }
    LexValue::Map(m)
}

impl SignedOp {
    fn unsigned_lex(&self) -> LexValue {
        match self {
            Self::Operation { op, .. } => op.to_lex(),
            Self::Tombstone { prev, .. } => tombstone_lex(prev, None),
        }
    }

    fn sig(&self) -> &str {
        match self {
            Self::Operation { sig, .. } | Self::Tombstone { sig, .. } => sig,
        }
    }

    /// The signed operation as a data-model value.
    #[must_use]
    pub fn to_lex(&self) -> LexValue {
        let mut v = self.unsigned_lex();
        if let LexValue::Map(m) = &mut v {
            m.insert("sig".to_owned(), s(self.sig()));
        }
        v
    }

    /// The operation's CID.
    pub fn cid(&self) -> Result<Cid, PlcError> {
        Ok(Cid::for_cbor(&cbor(&self.to_lex())?))
    }

    /// Whether the signature verifies against `did_key`.
    pub fn verify(&self, did_key: &str) -> bool {
        let Ok(bytes) = cbor(&self.unsigned_lex()) else {
            return false;
        };
        let Ok(sig) = URL_SAFE_NO_PAD.decode(self.sig()) else {
            return false;
        };
        // PLC accepts low-S and high-S signatures from older clients.
        proto_blue_crypto::verify_signature(did_key, &bytes, &sig, true).unwrap_or(false)
    }

    /// The DID this operation creates, if it's a genesis operation.
    pub fn genesis_did(&self) -> Result<String, PlcError> {
        match self {
            Self::Operation { op, .. } if op.prev.is_none() => {
                let hash = Sha256::digest(cbor(&self.to_lex())?);
                let mut b32 = base32_lower(&hash);
                b32.truncate(24);
                Ok(format!("did:plc:{b32}"))
            }
            _ => Err(PlcError::Invalid("not a genesis operation".into())),
        }
    }

    /// The JSON form the PLC directory accepts.
    #[must_use]
    pub fn to_json(&self) -> Value {
        match self {
            Self::Operation { op, sig } => json!({
                "type": "plc_operation",
                "rotationKeys": op.rotation_keys,
                "verificationMethods": op.verification_methods,
                "alsoKnownAs": op.also_known_as,
                "services": op.services.iter().map(|(k, svc)| {
                    (k.clone(), json!({"type": svc.kind, "endpoint": svc.endpoint}))
                }).collect::<serde_json::Map<_, _>>(),
                "prev": op.prev,
                "sig": sig,
            }),
            Self::Tombstone { prev, sig } => {
                json!({"type": "plc_tombstone", "prev": prev, "sig": sig})
            }
        }
    }

    /// Parses the JSON form (as in the directory's logs).
    pub fn from_json(v: &Value) -> Result<Self, PlcError> {
        let bad = |why: &str| PlcError::Invalid(why.to_owned());
        let str_of = |key: &str| v.get(key).and_then(Value::as_str).map(str::to_owned);
        let sig = str_of("sig").ok_or_else(|| bad("missing sig"))?;
        match v.get("type").and_then(Value::as_str) {
            Some("plc_tombstone") => Ok(Self::Tombstone {
                prev: str_of("prev").ok_or_else(|| bad("tombstone without prev"))?,
                sig,
            }),
            Some("plc_operation") => {
                let strings = |key: &str| -> Result<Vec<String>, PlcError> {
                    v.get(key)
                        .and_then(Value::as_array)
                        .ok_or_else(|| bad(&format!("missing {key}")))?
                        .iter()
                        .map(|x| {
                            x.as_str()
                                .map(str::to_owned)
                                .ok_or_else(|| bad(&format!("{key} holds a non-string")))
                        })
                        .collect()
                };
                let verification_methods = v
                    .get("verificationMethods")
                    .and_then(Value::as_object)
                    .ok_or_else(|| bad("missing verificationMethods"))?
                    .iter()
                    .map(|(k, x)| {
                        Ok((
                            k.clone(),
                            x.as_str().ok_or_else(|| bad("non-string key"))?.to_owned(),
                        ))
                    })
                    .collect::<Result<_, PlcError>>()?;
                let services = v
                    .get("services")
                    .and_then(Value::as_object)
                    .ok_or_else(|| bad("missing services"))?
                    .iter()
                    .map(|(k, x)| {
                        let kind = x
                            .get("type")
                            .and_then(Value::as_str)
                            .ok_or_else(|| bad("service without type"))?;
                        let endpoint = x
                            .get("endpoint")
                            .and_then(Value::as_str)
                            .ok_or_else(|| bad("service without endpoint"))?;
                        Ok((
                            k.clone(),
                            Service {
                                kind: kind.to_owned(),
                                endpoint: endpoint.to_owned(),
                            },
                        ))
                    })
                    .collect::<Result<_, PlcError>>()?;
                Ok(Self::Operation {
                    op: Operation {
                        rotation_keys: strings("rotationKeys")?,
                        verification_methods,
                        also_known_as: strings("alsoKnownAs")?,
                        services,
                        prev: match v.get("prev") {
                            Some(Value::String(p)) => Some(p.clone()),
                            Some(Value::Null) | None => None,
                            _ => return Err(bad("prev must be a string or null")),
                        },
                    },
                    sig,
                })
            }
            _ => Err(bad("unknown operation type")),
        }
    }
}

/// RFC 4648 base32, lowercase, no padding.
#[must_use]
pub fn base32_lower(data: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut out = String::with_capacity(data.len().div_ceil(5) * 8);
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for &byte in data {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proto_blue_crypto::{K256Keypair, Keypair};

    /// The first two operations of a public account, from plc.directory's
    /// audit log (the directory's own record of each op's CID included).
    const GENESIS: &str = r#"{"sig":"lza4at_jCtGo_TYgL5PC1ZNP7lhF4DV8H50LWHhvdHcB143x1wEwqZ43xvV36Pws6OOnJLJrkibEUFDFqkhIhg","prev":null,"type":"plc_operation","services":{"atproto_pds":{"type":"AtprotoPersonalDataServer","endpoint":"https://bsky.social"}},"alsoKnownAs":["at://atprotocol.bsky.social"],"rotationKeys":["did:key:zQ3shhCGUqDKjStzuDxPkTxN6ujddP4RkEKJJouJGRRkaLGbg","did:key:zQ3shpKnbdPx3g3CmPf5cRVTPe1HtSwVn5ish3wSnDPQCbLJK"],"verificationMethods":{"atproto":"did:key:zQ3shXjHeiBuRCKmM36cuYnm7YEMzhGnCmCyW92sRJ9pribSF"}}"#;
    const GENESIS_CID: &str = "bafyreibfvkh3n6odvdpwj54j4xxdsgnn4zo5utbyf7z7nfbyikhtygzjcq";
    const SECOND: &str = r#"{"sig":"lRDLz1RRcauzDos9LZ0Q5bi3YzJXbpgrUpZ51e__tdg89xYgHiWWtnKcrAJanBMkgW0uloD40TYWMVXyZWi4mw","prev":"bafyreibfvkh3n6odvdpwj54j4xxdsgnn4zo5utbyf7z7nfbyikhtygzjcq","type":"plc_operation","services":{"atproto_pds":{"type":"AtprotoPersonalDataServer","endpoint":"https://bsky.social"}},"alsoKnownAs":["at://atproto.com"],"rotationKeys":["did:key:zQ3shhCGUqDKjStzuDxPkTxN6ujddP4RkEKJJouJGRRkaLGbg","did:key:zQ3shpKnbdPx3g3CmPf5cRVTPe1HtSwVn5ish3wSnDPQCbLJK"],"verificationMethods":{"atproto":"did:key:zQ3shXjHeiBuRCKmM36cuYnm7YEMzhGnCmCyW92sRJ9pribSF"}}"#;
    const SECOND_CID: &str = "bafyreihljrd4zlm6egppxzwq52fgzdrh7hv2bkjnbgyjteqasmpjgqmryi";

    fn parse(text: &str) -> SignedOp {
        SignedOp::from_json(&serde_json::from_str(text).unwrap()).unwrap()
    }

    #[test]
    fn real_genesis_op_derives_its_did_and_cid() {
        let g = parse(GENESIS);
        assert_eq!(g.genesis_did().unwrap(), "did:plc:ewvi7nxzyoun6zhxrhs64oiz");
        assert_eq!(g.cid().unwrap().to_string_base32(), GENESIS_CID);
    }

    #[test]
    fn real_ops_verify_against_a_rotation_key_and_chain() {
        let g = parse(GENESIS);
        let second = parse(SECOND);
        let SignedOp::Operation { op, .. } = &g else {
            panic!()
        };
        assert!(
            op.rotation_keys.iter().any(|k| g.verify(k)),
            "genesis signed by a rotation key"
        );
        assert!(
            op.rotation_keys.iter().any(|k| second.verify(k)),
            "update signed by a rotation key"
        );
        assert!(
            !g.verify(&op.verification_methods["atproto"]),
            "not by the signing key"
        );
        assert_eq!(second.cid().unwrap().to_string_base32(), SECOND_CID);
        let SignedOp::Operation { op: op2, .. } = &second else {
            panic!()
        };
        assert_eq!(op2.prev.as_deref(), Some(GENESIS_CID));
        // JSON round trip is lossless.
        assert_eq!(SignedOp::from_json(&second.to_json()).unwrap(), second);
    }

    #[test]
    fn sign_verify_and_tamper() {
        let rotation = K256Keypair::generate();
        let signing = K256Keypair::generate();
        let op = Operation {
            rotation_keys: vec![rotation.did()],
            verification_methods: [("atproto".to_owned(), signing.did())].into(),
            also_known_as: vec!["at://alice.example.com".into()],
            services: [(
                "atproto_pds".to_owned(),
                Service {
                    kind: "AtprotoPersonalDataServer".into(),
                    endpoint: "https://pds.example.com".into(),
                },
            )]
            .into(),
            prev: None,
        };
        op.validate().unwrap();
        let signed = op.sign(&rotation).unwrap();
        assert!(signed.verify(&rotation.did()));
        let did = signed.genesis_did().unwrap();
        assert!(did.starts_with("did:plc:") && did.len() == 32);

        let SignedOp::Operation {
            op: mut changed,
            sig,
        } = signed.clone()
        else {
            panic!()
        };
        changed.also_known_as = vec!["at://mallory.example.com".into()];
        assert!(!SignedOp::Operation { op: changed, sig }.verify(&rotation.did()));

        let t = sign_tombstone(&signed.cid().unwrap().to_string_base32(), &rotation).unwrap();
        assert!(t.verify(&rotation.did()));
        assert!(t.genesis_did().is_err());
    }

    #[test]
    fn validation_rejects_bad_fields() {
        let k = K256Keypair::generate().did();
        let base = Operation {
            rotation_keys: vec![k.clone()],
            verification_methods: [("atproto".to_owned(), k.clone())].into(),
            also_known_as: vec!["at://a.example".into()],
            services: [(
                "atproto_pds".to_owned(),
                Service {
                    kind: "AtprotoPersonalDataServer".into(),
                    endpoint: "https://pds.example".into(),
                },
            )]
            .into(),
            prev: None,
        };
        let mut bad = base.clone();
        bad.rotation_keys = vec!["not-a-key".into()];
        assert!(bad.validate().is_err());
        let mut bad = base.clone();
        bad.also_known_as = vec!["https://a.example".into()];
        assert!(bad.validate().is_err());
        let mut bad = base.clone();
        bad.services.get_mut("atproto_pds").unwrap().endpoint = "ftp://x".into();
        assert!(bad.validate().is_err());
        let mut bad = base;
        bad.rotation_keys = vec![k.clone(), k];
        assert!(bad.validate().is_err());
    }

    #[test]
    fn the_last_op_gives_the_document_and_the_next_prev() {
        let (state, cid) = last_op_state(&serde_json::from_str(SECOND).unwrap()).unwrap();
        assert_eq!(cid, SECOND_CID, "CID computed from the directory's JSON");
        let op = state.unwrap();
        assert_eq!(op.handle(), Some("atproto.com"));
        let next = op.with_handle("new.example.net");
        assert_eq!(next.also_known_as, vec!["at://new.example.net"]);

        // Other alsoKnownAs entries keep their place; a missing handle goes first.
        let mut multi = op.clone();
        multi.also_known_as = vec!["https://elsewhere".into(), "at://old.test".into()];
        assert_eq!(
            multi.with_handle("h.test").also_known_as,
            vec!["https://elsewhere", "at://h.test"]
        );
        multi.also_known_as = vec!["https://elsewhere".into()];
        assert_eq!(
            multi.with_handle("h.test").also_known_as,
            vec!["at://h.test", "https://elsewhere"]
        );

        let tomb = json!({"type": "plc_tombstone", "prev": SECOND_CID, "sig": "x"});
        assert!(last_op_state(&tomb).unwrap().0.is_none());

        let legacy = json!({"type": "create", "signingKey": "did:key:zS", "recoveryKey": "did:key:zR",
            "handle": "old.bsky.social", "service": "https://bsky.social", "prev": null, "sig": "x"});
        let op = last_op_state(&legacy).unwrap().0.unwrap();
        assert_eq!(op.rotation_keys, vec!["did:key:zR", "did:key:zS"]);
        assert_eq!(op.handle(), Some("old.bsky.social"));
        assert_eq!(op.services["atproto_pds"].endpoint, "https://bsky.social");
    }

    #[test]
    fn base32_matches_rfc4648() {
        assert_eq!(base32_lower(b""), "");
        assert_eq!(base32_lower(b"f"), "my");
        assert_eq!(base32_lower(b"foobar"), "mzxw6ytboi");
    }
}
