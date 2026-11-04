//! DID documents locus emits: the service document, hosted did:web account
//! documents, and the `didDoc` returned with sessions.

use serde_json::{Value, json};

use crate::plc::Operation;

const CONTEXT: &[&str] = &[
    "https://www.w3.org/ns/did/v1",
    "https://w3id.org/security/multikey/v1",
    "https://w3id.org/security/suites/secp256k1-2019/v1",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationMethod {
    /// Fragment, e.g. `atproto`.
    pub fragment: String,
    /// A `did:key:z…` string.
    pub did_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceEntry {
    pub fragment: String,
    pub kind: String,
    pub endpoint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DidDocument {
    pub id: String,
    pub also_known_as: Vec<String>,
    pub verification_methods: Vec<VerificationMethod>,
    pub services: Vec<ServiceEntry>,
}

impl DidDocument {
    /// An account document: one `#atproto` key, one `#atproto_pds` service.
    #[must_use]
    pub fn account(did: &str, handle: &str, signing_did_key: &str, pds_endpoint: &str) -> Self {
        Self {
            id: did.to_owned(),
            also_known_as: vec![format!("at://{handle}")],
            verification_methods: vec![VerificationMethod {
                fragment: "atproto".into(),
                did_key: signing_did_key.to_owned(),
            }],
            services: vec![ServiceEntry {
                fragment: "atproto_pds".into(),
                kind: "AtprotoPersonalDataServer".into(),
                endpoint: pds_endpoint.to_owned(),
            }],
        }
    }

    /// The document a did:plc operation describes.
    #[must_use]
    pub fn from_plc(did: &str, op: &Operation) -> Self {
        Self {
            id: did.to_owned(),
            also_known_as: op.also_known_as.clone(),
            verification_methods: op
                .verification_methods
                .iter()
                .map(|(fragment, did_key)| VerificationMethod {
                    fragment: fragment.clone(),
                    did_key: did_key.clone(),
                })
                .collect(),
            services: op
                .services
                .iter()
                .map(|(fragment, s)| ServiceEntry {
                    fragment: fragment.clone(),
                    kind: s.kind.clone(),
                    endpoint: s.endpoint.clone(),
                })
                .collect(),
        }
    }

    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "@context": CONTEXT,
            "id": self.id,
            "alsoKnownAs": self.also_known_as,
            "verificationMethod": self.verification_methods.iter().map(|vm| json!({
                "id": format!("{}#{}", self.id, vm.fragment),
                "type": "Multikey",
                "controller": self.id,
                "publicKeyMultibase": vm.did_key.strip_prefix("did:key:").unwrap_or(&vm.did_key),
            })).collect::<Vec<_>>(),
            "service": self.services.iter().map(|s| json!({
                "id": format!("#{}", s.fragment),
                "type": s.kind,
                "serviceEndpoint": s.endpoint,
            })).collect::<Vec<_>>(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_document_shape() {
        let d = DidDocument::account(
            "did:plc:abc",
            "alice.example.com",
            "did:key:zQ3shtest",
            "https://pds.example.com",
        )
        .to_json();
        assert_eq!(d["id"], "did:plc:abc");
        assert_eq!(d["alsoKnownAs"][0], "at://alice.example.com");
        assert_eq!(d["verificationMethod"][0]["id"], "did:plc:abc#atproto");
        assert_eq!(
            d["verificationMethod"][0]["publicKeyMultibase"],
            "zQ3shtest"
        );
        assert_eq!(d["service"][0]["id"], "#atproto_pds");
        assert_eq!(
            d["service"][0]["serviceEndpoint"],
            "https://pds.example.com"
        );
    }
}
