//! Owned, server-side DID Document type.
//!
//! `proto-blue::common::did_doc::DidDocument` is `Deserialize`-only — it's
//! built for parsing DID documents fetched from the network. This PDS also
//! needs to **construct** DID documents (for `/.well-known/did.json` under
//! `did:web`), so we keep a local `Serialize + Deserialize` mirror with the
//! same wire shape plus the `@context` field that response serialisation
//! requires.
//!
//! For consumers that only parse DID docs, prefer `proto_blue::common::did_doc`.

use serde::{Deserialize, Serialize};

/// A W3C DID Document, both serializable and deserializable.
///
/// Wire format matches the `proto_blue::common::did_doc::DidDocument`
/// shape with the addition of an optional `@context` field used when
/// emitting documents over `/.well-known/did.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DidDocument {
    /// JSON-LD `@context`. Optional and omitted from output when `None`.
    #[serde(rename = "@context", skip_serializing_if = "Option::is_none")]
    pub context: Option<serde_json::Value>,

    /// The DID identifier (e.g. `did:plc:abc123` or `did:web:example.com`).
    pub id: String,

    /// `at://`-prefixed handles associated with this DID.
    #[serde(rename = "alsoKnownAs", default, skip_serializing_if = "Vec::is_empty")]
    pub also_known_as: Vec<String>,

    /// Service endpoints (PDS, labeler, etc).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub service: Vec<Service>,

    /// Verification methods (signing keys).
    #[serde(
        rename = "verificationMethod",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub verification_method: Vec<VerificationMethod>,
}

/// A verification method (public key) entry on a DID document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerificationMethod {
    /// Key identifier — e.g. `did:plc:abc#atproto`.
    pub id: String,

    /// Key type — `Multikey`, `EcdsaSecp256k1VerificationKey2019`, etc.
    #[serde(rename = "type")]
    pub key_type: String,

    /// Controller DID (typically the document's own `id`).
    pub controller: String,

    /// Public key in multibase encoding (`z` prefix for base58btc).
    #[serde(rename = "publicKeyMultibase", skip_serializing_if = "Option::is_none")]
    pub public_key_multibase: Option<String>,
}

/// A service endpoint entry on a DID document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Service {
    /// Service identifier — e.g. `did:plc:abc#atproto_pds`.
    pub id: String,

    /// Service type — e.g. `AtprotoPersonalDataServer`.
    #[serde(rename = "type")]
    pub service_type: String,

    /// Service endpoint URL.
    #[serde(rename = "serviceEndpoint")]
    pub service_endpoint: String,
}

impl DidDocument {
    /// Find the AT Protocol signing key (the verification method whose `id`
    /// ends with `#atproto`).
    ///
    /// Returns `None` when no such method exists. Callers needing any
    /// other key should iterate `verification_method` directly.
    pub fn get_signing_key(&self) -> Option<&VerificationMethod> {
        self.verification_method
            .iter()
            .find(|vm| vm.id.ends_with("#atproto"))
    }

    /// v0.9 Federation runtime-mutability arc §2.3 (#399) — the AT Protocol PDS
    /// service endpoint: the `serviceEndpoint` of the service whose type is
    /// `AtprotoPersonalDataServer`. Returns `None` when no such service exists.
    ///
    /// Mirrors [`get_signing_key`](Self::get_signing_key)'s inherent-method
    /// shape. PLC documents should never carry more than one
    /// `AtprotoPersonalDataServer` service; if they do, the first in document
    /// order wins (deterministic). The bulk did:plc update (Phase E2) uses this
    /// for the compare-and-skip short-circuit before publishing.
    pub fn get_service_endpoint(&self) -> Option<String> {
        self.service
            .iter()
            .find(|s| s.service_type == "AtprotoPersonalDataServer")
            .map(|s| s.service_endpoint.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // v0.9 Federation runtime-mutability arc §2.3 (#399) — get_service_endpoint.

    fn svc(service_type: &str, endpoint: &str) -> Service {
        Service {
            id: format!("did:plc:x#{service_type}"),
            service_type: service_type.to_string(),
            service_endpoint: endpoint.to_string(),
        }
    }

    fn doc_with(services: Vec<Service>) -> DidDocument {
        DidDocument {
            context: None,
            id: "did:plc:x".to_string(),
            also_known_as: vec![],
            service: services,
            verification_method: vec![],
        }
    }

    #[test]
    fn get_service_endpoint_returns_pds_endpoint() {
        let doc = doc_with(vec![svc("AtprotoPersonalDataServer", "https://pds.example.com")]);
        assert_eq!(doc.get_service_endpoint().as_deref(), Some("https://pds.example.com"));
    }

    #[test]
    fn get_service_endpoint_none_when_no_services() {
        assert!(doc_with(vec![]).get_service_endpoint().is_none());
    }

    #[test]
    fn get_service_endpoint_none_when_no_pds_service() {
        let doc = doc_with(vec![svc("SomeOtherService", "https://other.example.com")]);
        assert!(doc.get_service_endpoint().is_none());
    }

    #[test]
    fn get_service_endpoint_first_when_multiple_pds() {
        // PLC docs should never carry two; the extractor is deterministic (first).
        let doc = doc_with(vec![
            svc("AtprotoPersonalDataServer", "https://first.example.com"),
            svc("AtprotoPersonalDataServer", "https://second.example.com"),
        ]);
        assert_eq!(doc.get_service_endpoint().as_deref(), Some("https://first.example.com"));
    }

    #[test]
    fn round_trip_did_web_document() {
        let doc = DidDocument {
            context: Some(serde_json::json!(["https://www.w3.org/ns/did/v1"])),
            id: "did:web:example.com".to_string(),
            also_known_as: vec!["at://example.com".to_string()],
            service: vec![Service {
                id: "did:web:example.com#atproto_pds".to_string(),
                service_type: "AtprotoPersonalDataServer".to_string(),
                service_endpoint: "https://example.com".to_string(),
            }],
            verification_method: vec![VerificationMethod {
                id: "did:web:example.com#atproto".to_string(),
                key_type: "Multikey".to_string(),
                controller: "did:web:example.com".to_string(),
                public_key_multibase: Some("zQ3sh...".to_string()),
            }],
        };

        let json = serde_json::to_string(&doc).unwrap();
        let parsed: DidDocument = serde_json::from_str(&json).unwrap();
        assert_eq!(doc, parsed);

        // Camel-case keys per AT Protocol spec
        assert!(json.contains("\"alsoKnownAs\""));
        assert!(json.contains("\"verificationMethod\""));
        assert!(json.contains("\"serviceEndpoint\""));
        assert!(json.contains("\"publicKeyMultibase\""));
        assert!(json.contains("\"@context\""));
    }

    #[test]
    fn empty_collections_are_omitted() {
        let doc = DidDocument {
            context: None,
            id: "did:web:bare".to_string(),
            also_known_as: vec![],
            service: vec![],
            verification_method: vec![],
        };
        let json = serde_json::to_string(&doc).unwrap();
        assert!(!json.contains("@context"));
        assert!(!json.contains("alsoKnownAs"));
        assert!(!json.contains("service"));
        assert!(!json.contains("verificationMethod"));
    }
}
