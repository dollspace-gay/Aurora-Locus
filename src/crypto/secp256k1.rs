//! Secp256k1 KeyPair generation and management
//!
//! Provides utilities for generating secp256k1 keypairs and exporting
//! them in various formats. Used for ATProto signing operations.

use crate::error::{PdsError, PdsResult};
use k256::{
    ecdsa::{SigningKey, VerifyingKey},
    SecretKey,
};
use rand::rngs::OsRng;

/// Secp256k1 KeyPair for signing operations
#[derive(Clone)]
pub struct Secp256k1KeyPair {
    signing_key: SigningKey,
}

impl Secp256k1KeyPair {
    /// Generate a new random secp256k1 keypair
    pub fn generate() -> Self {
        let signing_key = SigningKey::random(&mut OsRng);
        Self { signing_key }
    }

    /// Import a keypair from raw private key bytes (32 bytes)
    pub fn from_bytes(private_key: &[u8]) -> PdsResult<Self> {
        if private_key.len() != 32 {
            return Err(PdsError::Validation(
                "Private key must be exactly 32 bytes".to_string(),
            ));
        }

        let secret_key = SecretKey::from_slice(private_key)
            .map_err(|e| PdsError::Internal(format!("Invalid private key: {}", e)))?;

        let signing_key = SigningKey::from(secret_key);
        Ok(Self { signing_key })
    }

    /// Export private key as raw bytes (32 bytes)
    pub fn to_bytes(&self) -> Vec<u8> {
        self.signing_key.to_bytes().to_vec()
    }

    /// Get the verifying (public) key
    pub fn verifying_key(&self) -> VerifyingKey {
        *self.signing_key.verifying_key()
    }

    /// Get the signing key reference
    pub fn signing_key(&self) -> &SigningKey {
        &self.signing_key
    }

    /// Export public key in compressed form (33 bytes)
    pub fn public_key_compressed(&self) -> Vec<u8> {
        let verifying_key = self.verifying_key();
        let point = verifying_key.to_encoded_point(true); // Compressed
        point.as_bytes().to_vec()
    }

    /// Export public key as did:key identifier
    ///
    /// Uses multicodec encoding with secp256k1 public key prefix (0xe7)
    pub fn did(&self) -> String {
        let compressed_bytes = self.public_key_compressed();

        // Multicodec prefix for secp256k1 public key is 0xe7
        // varint encoding: 0xe7 = 231, which fits in one byte with high bit set
        let mut multicodec_bytes = vec![0xe7, 0x01]; // varint encoding of 0xe7
        multicodec_bytes.extend_from_slice(&compressed_bytes);

        // Encode as base58btc with multibase 'z' prefix
        let encoded = bs58::encode(&multicodec_bytes).into_string();
        format!("did:key:z{}", encoded)
    }

    /// Export public key in multibase format (for DID documents)
    /// Returns base58btc encoding with 'z' prefix
    pub fn public_key_multibase(&self) -> String {
        let compressed_bytes = self.public_key_compressed();
        let encoded = bs58::encode(&compressed_bytes).into_string();
        format!("z{}", encoded)
    }

    /// Export public key as hex-encoded string (compressed, 66 chars)
    pub fn public_key_hex(&self) -> String {
        hex::encode(self.public_key_compressed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_keypair() {
        let keypair = Secp256k1KeyPair::generate();
        let _verifying_key = keypair.verifying_key();
        // Should not panic
    }

    #[test]
    fn test_keypair_roundtrip() {
        let keypair1 = Secp256k1KeyPair::generate();
        let bytes = keypair1.to_bytes();
        let keypair2 = Secp256k1KeyPair::from_bytes(&bytes).unwrap();

        // Public keys should match
        assert_eq!(
            keypair1.public_key_compressed(),
            keypair2.public_key_compressed()
        );
    }

    #[test]
    fn test_did_format() {
        let keypair = Secp256k1KeyPair::generate();
        let did = keypair.did();

        // Should start with did:key:z
        assert!(did.starts_with("did:key:z"));
        // Should be a reasonable length
        assert!(did.len() > 50);
    }

    #[test]
    fn test_public_key_compressed_length() {
        let keypair = Secp256k1KeyPair::generate();
        let compressed = keypair.public_key_compressed();

        // Compressed secp256k1 public key is 33 bytes
        assert_eq!(compressed.len(), 33);
    }

    #[test]
    fn test_public_key_hex_length() {
        let keypair = Secp256k1KeyPair::generate();
        let hex_key = keypair.public_key_hex();

        // 33 bytes * 2 = 66 hex characters
        assert_eq!(hex_key.len(), 66);
    }

    #[test]
    fn test_from_bytes_invalid_length() {
        let short_key = vec![0u8; 16];
        let result = Secp256k1KeyPair::from_bytes(&short_key);
        assert!(result.is_err());
    }

    #[test]
    fn test_deterministic_did() {
        // Same private key should produce same DID
        let private_key = [42u8; 32];
        let keypair1 = Secp256k1KeyPair::from_bytes(&private_key).unwrap();
        let keypair2 = Secp256k1KeyPair::from_bytes(&private_key).unwrap();

        assert_eq!(keypair1.did(), keypair2.did());
    }

    #[test]
    fn test_signing_key_access() {
        use k256::ecdsa::signature::Signer;

        let keypair = Secp256k1KeyPair::generate();
        let signing_key = keypair.signing_key();

        // Should be able to sign data with the signing key
        let data = b"test message";
        let signature: k256::ecdsa::Signature = signing_key.sign(data);

        // Signature should be 64 bytes (two 32-byte scalars r and s)
        assert_eq!(signature.to_bytes().len(), 64);
    }

    #[test]
    fn test_public_key_multibase() {
        let keypair = Secp256k1KeyPair::generate();
        let multibase = keypair.public_key_multibase();

        // Should start with 'z' prefix (base58btc multibase encoding)
        assert!(multibase.starts_with('z'));

        // Should be base58 encoded (no invalid characters)
        let encoded_part = &multibase[1..]; // Skip 'z' prefix
        assert!(encoded_part.chars().all(|c| {
            // Base58 alphabet
            "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz".contains(c)
        }));
    }
}
