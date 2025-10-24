/// PLC (Public Ledger of Credentials) operation signing
///
/// Implements secp256k1-based signing for DID:PLC update operations

use crate::error::{PdsError, PdsResult};
use k256::{
    ecdsa::{signature::Signer, Signature, SigningKey},
    SecretKey,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// PLC Operation for DID updates
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlcOperation {
    /// Operation type (always "plc_operation" for updates)
    #[serde(rename = "type")]
    pub op_type: String,

    /// Previous operation CID (None for genesis)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prev: Option<String>,

    /// DID being updated
    pub did: String,

    /// Rotation keys (public keys that can sign operations)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rotation_keys: Option<Vec<String>>,

    /// Also known as (alternate identifiers, typically handles)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub also_known_as: Option<Vec<String>>,

    /// Verification methods (keys for authentication)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification_methods: Option<serde_json::Value>,

    /// Services (PDS endpoint, etc.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub services: Option<serde_json::Value>,

    /// Signature over the operation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sig: Option<String>,
}

/// Builder for PLC operations
#[derive(Debug, Default)]
pub struct PlcOperationBuilder {
    prev: Option<String>,
    did: Option<String>,
    rotation_keys: Option<Vec<String>>,
    also_known_as: Option<Vec<String>>,
    verification_methods: Option<serde_json::Value>,
    services: Option<serde_json::Value>,
}

impl PlcOperationBuilder {
    /// Create a new PLC operation builder
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the previous operation CID
    pub fn prev(mut self, prev: String) -> Self {
        self.prev = Some(prev);
        self
    }

    /// Set the DID
    pub fn did(mut self, did: String) -> Self {
        self.did = Some(did);
        self
    }

    /// Set rotation keys
    pub fn rotation_keys(mut self, keys: Vec<String>) -> Self {
        self.rotation_keys = Some(keys);
        self
    }

    /// Set also known as
    pub fn also_known_as(mut self, aka: Vec<String>) -> Self {
        self.also_known_as = Some(aka);
        self
    }

    /// Set verification methods
    pub fn verification_methods(mut self, methods: serde_json::Value) -> Self {
        self.verification_methods = Some(methods);
        self
    }

    /// Set services
    pub fn services(mut self, services: serde_json::Value) -> Self {
        self.services = Some(services);
        self
    }

    /// Build the operation (without signature)
    pub fn build(self) -> PdsResult<PlcOperation> {
        let did = self.did.ok_or_else(|| {
            PdsError::Validation("DID is required for PLC operation".to_string())
        })?;

        // Validate DID format
        if !did.starts_with("did:plc:") {
            return Err(PdsError::Validation(
                "Only did:plc identifiers are supported".to_string(),
            ));
        }

        Ok(PlcOperation {
            op_type: "plc_operation".to_string(),
            prev: self.prev,
            did,
            rotation_keys: self.rotation_keys,
            also_known_as: self.also_known_as,
            verification_methods: self.verification_methods,
            services: self.services,
            sig: None,
        })
    }
}

/// PLC Signer - handles signing of PLC operations
pub struct PlcSigner {
    signing_key: SigningKey,
}

impl PlcSigner {
    /// Create a new PLC signer from a private key (32 bytes)
    pub fn new(private_key: &[u8]) -> PdsResult<Self> {
        if private_key.len() != 32 {
            return Err(PdsError::Validation(
                "Private key must be exactly 32 bytes".to_string(),
            ));
        }

        let secret_key = SecretKey::from_slice(private_key).map_err(|e| {
            PdsError::Internal(format!("Invalid private key: {}", e))
        })?;

        let signing_key = SigningKey::from(secret_key);

        Ok(Self { signing_key })
    }

    /// Create a signer from hex-encoded private key
    pub fn from_hex(hex_key: &str) -> PdsResult<Self> {
        let key_bytes = hex::decode(hex_key).map_err(|e| {
            PdsError::Validation(format!("Invalid hex private key: {}", e))
        })?;

        Self::new(&key_bytes)
    }

    /// Sign raw bytes (for repository commits)
    ///
    /// Returns a 64-byte signature
    pub fn sign(&self, data: &[u8]) -> Vec<u8> {
        use k256::ecdsa::signature::Signer;
        let signature: k256::ecdsa::Signature = self.signing_key.sign(data);
        signature.to_bytes().to_vec()
    }

    /// Sign a PLC operation
    ///
    /// This creates a deterministic signature over the canonical JSON representation
    /// of the operation (without the sig field).
    pub fn sign_operation(&self, mut operation: PlcOperation) -> PdsResult<PlcOperation> {
        // Ensure sig field is not set
        operation.sig = None;

        // Serialize to canonical JSON (sorted keys)
        let canonical_json = serde_json::to_vec(&operation).map_err(|e| {
            PdsError::Internal(format!("Failed to serialize operation: {}", e))
        })?;

        // Hash the JSON
        let mut hasher = Sha256::new();
        hasher.update(&canonical_json);
        let hash = hasher.finalize();

        // Sign the hash
        let signature: Signature = self.signing_key.sign(&hash);

        // Encode signature as hex
        let sig_hex = hex::encode(signature.to_bytes());

        // Add signature to operation
        operation.sig = Some(sig_hex);

        Ok(operation)
    }

    /// Get the public key in compressed form (33 bytes, hex-encoded)
    pub fn public_key_hex(&self) -> String {
        let verifying_key = self.signing_key.verifying_key();
        let public_key_bytes = verifying_key.to_encoded_point(true); // Compressed form
        hex::encode(public_key_bytes.as_bytes())
    }

    /// Get the public key in multibase format (for DID documents)
    pub fn public_key_multibase(&self) -> String {
        let public_key_hex = self.public_key_hex();
        // Multibase prefix 'z' for base58btc, but we'll use a simpler approach
        // In production, you'd use proper multibase encoding
        format!("did:key:z{}", public_key_hex)
    }

    /// Get the verifying key (public key)
    ///
    /// Returns the ECDSA verifying key associated with this signer's private key
    pub fn verifying_key(&self) -> k256::ecdsa::VerifyingKey {
        *self.signing_key.verifying_key()
    }
}

/// Validate a PLC operation structure
pub fn validate_plc_operation(operation: &PlcOperation) -> PdsResult<()> {
    // Check type
    if operation.op_type != "plc_operation" {
        return Err(PdsError::Validation(
            "Invalid operation type, expected 'plc_operation'".to_string(),
        ));
    }

    // Check DID format
    if !operation.did.starts_with("did:plc:") {
        return Err(PdsError::Validation(
            "DID must be a did:plc identifier".to_string(),
        ));
    }

    // Check signature is present
    if operation.sig.is_none() {
        return Err(PdsError::Validation(
            "Operation must be signed".to_string(),
        ));
    }

    // Validate signature format (should be hex)
    if let Some(ref sig) = operation.sig {
        if hex::decode(sig).is_err() {
            return Err(PdsError::Validation(
                "Signature must be valid hex".to_string(),
            ));
        }
    }

    Ok(())
}

/// Register a PLC DID with the PLC Directory
///
/// Submits a signed PLC operation to the directory to create or update a DID
pub async fn register_plc_did(
    plc_url: &str,
    operation: PlcOperation,
) -> PdsResult<String> {
    // Validate operation before sending
    validate_plc_operation(&operation)?;

    // Create HTTP client
    let client = reqwest::Client::new();

    // For genesis operations (no prev), POST to base URL
    // For updates, POST to {plc_url}/{did}
    let endpoint = if operation.prev.is_none() {
        plc_url.to_string()
    } else {
        format!("{}/{}", plc_url, operation.did)
    };

    // Submit operation to PLC directory
    let response = client
        .post(&endpoint)
        .json(&operation)
        .send()
        .await
        .map_err(|e| PdsError::Internal(format!("PLC registration request failed: {}", e)))?;

    if response.status().is_success() {
        // Return the DID
        Ok(operation.did)
    } else {
        let status = response.status();
        let error_body = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
        Err(PdsError::Internal(format!(
            "PLC directory returned error {}: {}",
            status, error_body
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plc_signer_creation() {
        // Create a test private key (32 bytes)
        let private_key = [1u8; 32];
        let signer = PlcSigner::new(&private_key);
        assert!(signer.is_ok());
    }

    #[test]
    fn test_plc_signer_invalid_key_length() {
        // Wrong length private key
        let private_key = [1u8; 16];
        let signer = PlcSigner::new(&private_key);
        assert!(signer.is_err());
    }

    #[test]
    fn test_plc_signer_from_hex() {
        let hex_key = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let signer = PlcSigner::from_hex(hex_key);
        assert!(signer.is_ok());
    }

    #[test]
    fn test_plc_operation_builder() {
        let operation = PlcOperationBuilder::new()
            .did("did:plc:test123".to_string())
            .rotation_keys(vec!["key1".to_string()])
            .also_known_as(vec!["at://alice.bsky.social".to_string()])
            .build();

        assert!(operation.is_ok());
        let op = operation.unwrap();
        assert_eq!(op.did, "did:plc:test123");
        assert_eq!(op.op_type, "plc_operation");
        assert!(op.rotation_keys.is_some());
    }

    #[test]
    fn test_plc_operation_builder_requires_did() {
        let operation = PlcOperationBuilder::new()
            .rotation_keys(vec!["key1".to_string()])
            .build();

        assert!(operation.is_err());
    }

    #[test]
    fn test_plc_operation_builder_validates_did() {
        let operation = PlcOperationBuilder::new()
            .did("did:web:example.com".to_string())
            .build();

        assert!(operation.is_err());
    }

    #[test]
    fn test_sign_plc_operation() {
        let private_key = [42u8; 32];
        let signer = PlcSigner::new(&private_key).unwrap();

        let operation = PlcOperationBuilder::new()
            .did("did:plc:test123".to_string())
            .rotation_keys(vec!["key1".to_string()])
            .build()
            .unwrap();

        let signed = signer.sign_operation(operation);
        assert!(signed.is_ok());

        let signed_op = signed.unwrap();
        assert!(signed_op.sig.is_some());
        assert!(!signed_op.sig.unwrap().is_empty());
    }

    #[test]
    fn test_validate_plc_operation() {
        let private_key = [42u8; 32];
        let signer = PlcSigner::new(&private_key).unwrap();

        let operation = PlcOperationBuilder::new()
            .did("did:plc:test123".to_string())
            .build()
            .unwrap();

        let signed_op = signer.sign_operation(operation).unwrap();

        // Should pass validation
        assert!(validate_plc_operation(&signed_op).is_ok());
    }

    #[test]
    fn test_validate_unsigned_operation() {
        let operation = PlcOperationBuilder::new()
            .did("did:plc:test123".to_string())
            .build()
            .unwrap();

        // Should fail - no signature
        assert!(validate_plc_operation(&operation).is_err());
    }

    #[test]
    fn test_public_key_extraction() {
        let private_key = [42u8; 32];
        let signer = PlcSigner::new(&private_key).unwrap();

        let public_key = signer.public_key_hex();
        assert!(!public_key.is_empty());
        assert_eq!(public_key.len(), 66); // 33 bytes * 2 (hex)
    }

    #[test]
    fn test_deterministic_signing() {
        let private_key = [42u8; 32];
        let signer = PlcSigner::new(&private_key).unwrap();

        let operation1 = PlcOperationBuilder::new()
            .did("did:plc:test123".to_string())
            .rotation_keys(vec!["key1".to_string()])
            .build()
            .unwrap();

        let operation2 = operation1.clone();

        let signed1 = signer.sign_operation(operation1).unwrap();
        let signed2 = signer.sign_operation(operation2).unwrap();

        // Signatures should be identical for identical operations
        assert_eq!(signed1.sig, signed2.sig);
    }
}
