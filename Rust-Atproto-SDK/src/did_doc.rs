//! DID Document types and resolution
//!
//! This module provides types and utilities for working with DID (Decentralized Identifier)
//! documents in ATProto. It supports both did:plc and did:web methods.
//!
//! ## DID Document Structure
//!
//! A DID document contains:
//! - Identity information (DID, handle)
//! - Service endpoints (PDS location)
//! - Verification methods (public keys)
//!
//! ## References
//!
//! - Spec: https://atproto.com/specs/did
//! - Identity Guide: https://atproto.com/guides/identity

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Error types for DID document operations
#[derive(Debug, Error, PartialEq)]
pub enum DidDocError {
    #[error("No PDS service endpoint found in DID document")]
    NoPdsEndpoint,

    #[error("Invalid service endpoint URL: {0}")]
    InvalidServiceEndpoint(String),

    #[error("Invalid DID document structure: {0}")]
    InvalidDocument(String),

    #[error("Deserialization error: {0}")]
    DeserializationError(String),
}

/// A DID Document
///
/// DID Documents are standardized JSON objects returned by the DID resolution process.
/// They include the URL of the user's PDS and cryptographic verification methods.
///
/// # Examples
///
/// ```
/// use atproto::did_doc::DidDocument;
///
/// let json = r##"{
///   "id": "did:plc:abc123",
///   "alsoKnownAs": ["at://alice.bsky.social"],
///   "service": [{
///     "id": "#atproto_pds",
///     "type": "AtprotoPersonalDataServer",
///     "serviceEndpoint": "https://pds.example.com"
///   }],
///   "verificationMethod": []
/// }"##;
///
/// let doc: DidDocument = serde_json::from_str(json).unwrap();
/// assert_eq!(doc.id, "did:plc:abc123");
/// assert_eq!(doc.get_pds_endpoint().unwrap(), "https://pds.example.com");
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DidDocument {
    /// JSON-LD context (optional)
    #[serde(rename = "@context", skip_serializing_if = "Option::is_none")]
    pub context: Option<serde_json::Value>,

    /// The DID identifier
    pub id: String,

    /// Also known as (handles with at:// scheme)
    #[serde(rename = "alsoKnownAs", default, skip_serializing_if = "Vec::is_empty")]
    pub also_known_as: Vec<String>,

    /// Service endpoints
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub service: Vec<Service>,

    /// Verification methods (public keys)
    #[serde(
        rename = "verificationMethod",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub verification_method: Vec<VerificationMethod>,
}

impl DidDocument {
    /// Parse a DID document from JSON
    ///
    /// # Errors
    ///
    /// Returns an error if the JSON is invalid or doesn't match the expected structure.
    pub fn from_json(json: &str) -> Result<Self, DidDocError> {
        serde_json::from_str(json)
            .map_err(|e| DidDocError::DeserializationError(e.to_string()))
    }

    /// Get the PDS (Personal Data Server) endpoint URL
    ///
    /// Searches the service array for an entry with:
    /// - `id` ending with `#atproto_pds`
    /// - `type` matching `AtprotoPersonalDataServer`
    ///
    /// # Errors
    ///
    /// Returns an error if no PDS endpoint is found or if the endpoint is invalid.
    pub fn get_pds_endpoint(&self) -> Result<String, DidDocError> {
        self.service
            .iter()
            .find(|s| {
                s.id.ends_with("#atproto_pds")
                    && s.service_type == "AtprotoPersonalDataServer"
            })
            .map(|s| s.service_endpoint.clone())
            .ok_or(DidDocError::NoPdsEndpoint)
    }

    /// Get the current handle for this DID
    ///
    /// Handles are found in the `alsoKnownAs` array with the `at://` scheme.
    pub fn get_handle(&self) -> Option<String> {
        self.also_known_as
            .iter()
            .find(|aka| aka.starts_with("at://"))
            .map(|aka| aka.strip_prefix("at://").unwrap_or(aka).to_string())
    }

    /// Get the verification method (public key) for ATProto
    ///
    /// Searches for a verification method with id ending in `#atproto`.
    pub fn get_signing_key(&self) -> Option<&VerificationMethod> {
        self.verification_method
            .iter()
            .find(|vm| vm.id.ends_with("#atproto"))
    }
}

/// A service endpoint in a DID document
///
/// Services identify where to find specific functionality for the DID.
/// In ATProto, the primary service is the PDS (Personal Data Server).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Service {
    /// Service identifier (e.g., "#atproto_pds")
    pub id: String,

    /// Service type (e.g., "AtprotoPersonalDataServer")
    #[serde(rename = "type")]
    pub service_type: String,

    /// Service endpoint URL
    #[serde(rename = "serviceEndpoint")]
    pub service_endpoint: String,
}

/// A verification method (public key) in a DID document
///
/// Verification methods are cryptographic public keys used to verify
/// signatures and authenticate the DID controller.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerificationMethod {
    /// Verification method identifier
    pub id: String,

    /// Key type (e.g., "Multikey")
    #[serde(rename = "type")]
    pub key_type: String,

    /// DID of the key controller
    pub controller: String,

    /// Public key in multibase format (optional)
    #[serde(
        rename = "publicKeyMultibase",
        skip_serializing_if = "Option::is_none"
    )]
    pub public_key_multibase: Option<String>,
}

/// Resolver for fetching and parsing DID documents
///
/// This handles resolution of both did:plc and did:web DIDs.
pub struct DidResolver {
    /// HTTP client for fetching DID documents
    client: reqwest::Client,

    /// PLC directory URL (for did:plc resolution)
    plc_directory: String,
}

impl DidResolver {
    /// Create a new DID resolver
    ///
    /// # Arguments
    ///
    /// * `plc_directory` - URL of the PLC directory server (e.g., "https://plc.directory")
    pub fn new(plc_directory: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            plc_directory,
        }
    }

    /// Create a resolver with the default PLC directory
    pub fn with_default_plc() -> Self {
        Self::new("https://plc.directory".to_string())
    }

    /// Resolve a DID to its document
    ///
    /// # Arguments
    ///
    /// * `did` - The DID to resolve (e.g., "did:plc:abc123" or "did:web:example.com")
    ///
    /// # Errors
    ///
    /// Returns an error if the DID cannot be resolved or parsed.
    pub async fn resolve(&self, did: &str) -> Result<DidDocument, DidDocError> {
        if did.starts_with("did:plc:") {
            self.resolve_plc(did).await
        } else if did.starts_with("did:web:") {
            self.resolve_web(did).await
        } else {
            Err(DidDocError::InvalidDocument(format!(
                "Unsupported DID method: {}",
                did
            )))
        }
    }

    /// Resolve a did:plc DID
    ///
    /// Fetches the DID document from the PLC directory.
    async fn resolve_plc(&self, did: &str) -> Result<DidDocument, DidDocError> {
        let url = format!("{}/{}", self.plc_directory, did);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| DidDocError::InvalidDocument(format!("HTTP error: {}", e)))?;

        if !response.status().is_success() {
            return Err(DidDocError::InvalidDocument(format!(
                "HTTP {} from PLC directory",
                response.status()
            )));
        }

        let json = response
            .text()
            .await
            .map_err(|e| DidDocError::InvalidDocument(format!("Failed to read response: {}", e)))?;

        DidDocument::from_json(&json)
    }

    /// Resolve a did:web DID
    ///
    /// Fetches the DID document from the web server at /.well-known/did.json
    async fn resolve_web(&self, did: &str) -> Result<DidDocument, DidDocError> {
        // Extract domain from did:web:example.com
        let domain = did
            .strip_prefix("did:web:")
            .ok_or_else(|| DidDocError::InvalidDocument("Invalid did:web format".to_string()))?;

        // did:web uses /.well-known/did.json
        let url = format!("https://{}/.well-known/did.json", domain);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| DidDocError::InvalidDocument(format!("HTTP error: {}", e)))?;

        if !response.status().is_success() {
            return Err(DidDocError::InvalidDocument(format!(
                "HTTP {} from did:web server",
                response.status()
            )));
        }

        let json = response
            .text()
            .await
            .map_err(|e| DidDocError::InvalidDocument(format!("Failed to read response: {}", e)))?;

        DidDocument::from_json(&json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE_DID_DOC: &str = r##"{
        "@context": [
            "https://www.w3.org/ns/did/v1",
            "https://w3id.org/security/multikey/v1",
            "https://w3id.org/security/suites/secp256k1-2019/v1"
        ],
        "id": "did:plc:ewvi7nxzyoun6zhxrhs64oiz",
        "alsoKnownAs": [
            "at://atproto.com"
        ],
        "service": [
            {
                "id": "#atproto_pds",
                "type": "AtprotoPersonalDataServer",
                "serviceEndpoint": "https://enoki.us-east.host.bsky.network"
            }
        ],
        "verificationMethod": [
            {
                "id": "did:plc:ewvi7nxzyoun6zhxrhs64oiz#atproto",
                "type": "Multikey",
                "controller": "did:plc:ewvi7nxzyoun6zhxrhs64oiz",
                "publicKeyMultibase": "zQ3shunBKsXixLxKtC5qeSG9E4J5RkGN57im31pcTzbNQnm5w"
            }
        ]
    }"##;

    #[test]
    fn test_did_document_parse() {
        let doc = DidDocument::from_json(EXAMPLE_DID_DOC).unwrap();
        assert_eq!(doc.id, "did:plc:ewvi7nxzyoun6zhxrhs64oiz");
        assert_eq!(doc.also_known_as.len(), 1);
        assert_eq!(doc.also_known_as[0], "at://atproto.com");
        assert_eq!(doc.service.len(), 1);
        assert_eq!(doc.verification_method.len(), 1);
    }

    #[test]
    fn test_get_pds_endpoint() {
        let doc = DidDocument::from_json(EXAMPLE_DID_DOC).unwrap();
        let endpoint = doc.get_pds_endpoint().unwrap();
        assert_eq!(endpoint, "https://enoki.us-east.host.bsky.network");
    }

    #[test]
    fn test_get_pds_endpoint_not_found() {
        let doc_json = r#"{
            "id": "did:plc:test",
            "service": []
        }"#;

        let doc = DidDocument::from_json(doc_json).unwrap();
        let result = doc.get_pds_endpoint();
        assert!(matches!(result, Err(DidDocError::NoPdsEndpoint)));
    }

    #[test]
    fn test_get_handle() {
        let doc = DidDocument::from_json(EXAMPLE_DID_DOC).unwrap();
        let handle = doc.get_handle().unwrap();
        assert_eq!(handle, "atproto.com");
    }

    #[test]
    fn test_get_handle_not_found() {
        let doc_json = r#"{
            "id": "did:plc:test",
            "alsoKnownAs": []
        }"#;

        let doc = DidDocument::from_json(doc_json).unwrap();
        assert!(doc.get_handle().is_none());
    }

    #[test]
    fn test_get_signing_key() {
        let doc = DidDocument::from_json(EXAMPLE_DID_DOC).unwrap();
        let key = doc.get_signing_key().unwrap();
        assert_eq!(key.id, "did:plc:ewvi7nxzyoun6zhxrhs64oiz#atproto");
        assert_eq!(key.key_type, "Multikey");
        assert_eq!(key.controller, "did:plc:ewvi7nxzyoun6zhxrhs64oiz");
        assert_eq!(
            key.public_key_multibase.as_ref().unwrap(),
            "zQ3shunBKsXixLxKtC5qeSG9E4J5RkGN57im31pcTzbNQnm5w"
        );
    }

    #[test]
    fn test_service_serialization() {
        let service = Service {
            id: "#atproto_pds".to_string(),
            service_type: "AtprotoPersonalDataServer".to_string(),
            service_endpoint: "https://pds.example.com".to_string(),
        };

        let json = serde_json::to_string(&service).unwrap();
        assert!(json.contains("\"id\":\"#atproto_pds\""));
        assert!(json.contains("\"type\":\"AtprotoPersonalDataServer\""));
        assert!(json.contains("\"serviceEndpoint\":\"https://pds.example.com\""));
    }

    #[test]
    fn test_verification_method_serialization() {
        let vm = VerificationMethod {
            id: "did:plc:test#atproto".to_string(),
            key_type: "Multikey".to_string(),
            controller: "did:plc:test".to_string(),
            public_key_multibase: Some("zQ3abc123".to_string()),
        };

        let json = serde_json::to_string(&vm).unwrap();
        assert!(json.contains("\"id\":\"did:plc:test#atproto\""));
        assert!(json.contains("\"type\":\"Multikey\""));
        assert!(json.contains("\"controller\":\"did:plc:test\""));
        assert!(json.contains("\"publicKeyMultibase\":\"zQ3abc123\""));
    }

    #[test]
    fn test_did_document_roundtrip() {
        let doc = DidDocument::from_json(EXAMPLE_DID_DOC).unwrap();
        let json = serde_json::to_string_pretty(&doc).unwrap();
        let doc2 = DidDocument::from_json(&json).unwrap();
        assert_eq!(doc.id, doc2.id);
        assert_eq!(doc.get_pds_endpoint().unwrap(), doc2.get_pds_endpoint().unwrap());
    }

    #[test]
    fn test_did_document_minimal() {
        let doc_json = r#"{
            "id": "did:plc:minimal"
        }"#;

        let doc = DidDocument::from_json(doc_json).unwrap();
        assert_eq!(doc.id, "did:plc:minimal");
        assert!(doc.also_known_as.is_empty());
        assert!(doc.service.is_empty());
        assert!(doc.verification_method.is_empty());
    }

    #[test]
    fn test_did_resolver_new() {
        let resolver = DidResolver::new("https://test.plc.directory".to_string());
        assert_eq!(resolver.plc_directory, "https://test.plc.directory");
    }

    #[test]
    fn test_did_resolver_default() {
        let resolver = DidResolver::default();
        assert_eq!(resolver.plc_directory, "https://plc.directory");
    }

    // Note: Network tests would require mocking or actual network access
    // These are omitted for now but could be added with proper test infrastructure
}
