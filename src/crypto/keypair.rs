//! P-256 KeyPair generation and management
//!
//! Provides utilities for generating P-256 (NIST P-256) keypairs and exporting
//! them in various formats (PEM, JWK, DID).

use crate::error::{PdsError, PdsResult};
use p256::{
    ecdsa::{SigningKey, VerifyingKey},
    pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding},
};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Output format for keypairs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyFormat {
    /// PEM format (PKCS#8 for private, SubjectPublicKeyInfo for public)
    Pem,
    /// JSON Web Key format
    Jwk,
    /// DID key format (did:key:...)
    Did,
}

impl FromStr for KeyFormat {
    type Err = PdsError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "pem" => Ok(KeyFormat::Pem),
            "jwk" => Ok(KeyFormat::Jwk),
            "did" => Ok(KeyFormat::Did),
            _ => Err(PdsError::Validation(format!(
                "Invalid key format: {}. Valid formats: pem, jwk, did",
                s
            ))),
        }
    }
}

/// JWK (JSON Web Key) representation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Jwk {
    /// Key type (always "EC" for elliptic curve)
    pub kty: String,
    /// Curve (always "P-256" for NIST P-256)
    pub crv: String,
    /// X coordinate (base64url-encoded)
    pub x: String,
    /// Y coordinate (base64url-encoded)
    pub y: String,
    /// Private key (base64url-encoded, only for private JWK)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub d: Option<String>,
}

/// P-256 KeyPair
pub struct KeyPair {
    signing_key: SigningKey,
}

impl KeyPair {
    /// Generate a new random P-256 keypair
    pub fn generate() -> Self {
        let signing_key = SigningKey::random(&mut OsRng);
        Self { signing_key }
    }

    /// Get the verifying (public) key
    pub fn verifying_key(&self) -> VerifyingKey {
        *self.signing_key.verifying_key()
    }

    /// Export private key in PEM format (PKCS#8)
    pub fn private_key_pem(&self) -> PdsResult<String> {
        self.signing_key
            .to_pkcs8_pem(LineEnding::LF)
            .map(|pem| pem.to_string())
            .map_err(|e| PdsError::Internal(format!("Failed to encode private key as PEM: {}", e)))
    }

    /// Export public key in PEM format (SubjectPublicKeyInfo)
    pub fn public_key_pem(&self) -> PdsResult<String> {
        self.verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .map_err(|e| PdsError::Internal(format!("Failed to encode public key as PEM: {}", e)))
    }

    /// Export private key in JWK format
    pub fn private_key_jwk(&self) -> PdsResult<Jwk> {
        let verifying_key = self.verifying_key();
        let point = verifying_key.to_encoded_point(false); // Uncompressed

        // Extract coordinates
        let x = point.x().ok_or_else(|| {
            PdsError::Internal("Failed to extract x coordinate".to_string())
        })?;
        let y = point.y().ok_or_else(|| {
            PdsError::Internal("Failed to extract y coordinate".to_string())
        })?;

        // Get private scalar (d)
        let d = self.signing_key.to_bytes();

        Ok(Jwk {
            kty: "EC".to_string(),
            crv: "P-256".to_string(),
            x: base64_url_encode(x),
            y: base64_url_encode(y),
            d: Some(base64_url_encode(&d)),
        })
    }

    /// Export public key in JWK format
    pub fn public_key_jwk(&self) -> PdsResult<Jwk> {
        let verifying_key = self.verifying_key();
        let point = verifying_key.to_encoded_point(false); // Uncompressed

        // Extract coordinates
        let x = point.x().ok_or_else(|| {
            PdsError::Internal("Failed to extract x coordinate".to_string())
        })?;
        let y = point.y().ok_or_else(|| {
            PdsError::Internal("Failed to extract y coordinate".to_string())
        })?;

        Ok(Jwk {
            kty: "EC".to_string(),
            crv: "P-256".to_string(),
            x: base64_url_encode(x),
            y: base64_url_encode(y),
            d: None,
        })
    }

    /// Export public key as did:key identifier
    ///
    /// Uses multicodec encoding with P-256 public key prefix (0x1200)
    pub fn public_key_did(&self) -> String {
        let verifying_key = self.verifying_key();
        let point = verifying_key.to_encoded_point(true); // Compressed (33 bytes)
        let compressed_bytes = point.as_bytes();

        // Multicodec prefix for P-256 public key is 0x1200
        let mut multicodec_bytes = vec![0x80, 0x24]; // varint encoding of 0x1200
        multicodec_bytes.extend_from_slice(compressed_bytes);

        // Encode as base58btc with multibase 'z' prefix
        let encoded = bs58::encode(&multicodec_bytes).into_string();
        format!("did:key:z{}", encoded)
    }

    /// Export keypair in specified format
    pub fn export(&self, format: KeyFormat, include_private: bool) -> PdsResult<String> {
        match format {
            KeyFormat::Pem => {
                if include_private {
                    self.private_key_pem()
                } else {
                    self.public_key_pem()
                }
            }
            KeyFormat::Jwk => {
                let jwk = if include_private {
                    self.private_key_jwk()?
                } else {
                    self.public_key_jwk()?
                };
                serde_json::to_string_pretty(&jwk).map_err(|e| {
                    PdsError::Internal(format!("Failed to serialize JWK: {}", e))
                })
            }
            KeyFormat::Did => {
                if include_private {
                    Err(PdsError::Validation(
                        "DID format only supports public keys".to_string(),
                    ))
                } else {
                    Ok(self.public_key_did())
                }
            }
        }
    }
}

/// Base64 URL-safe encoding (without padding)
fn base64_url_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_keypair() {
        let keypair = KeyPair::generate();
        let _verifying_key = keypair.verifying_key();
        // Should not panic
    }

    #[test]
    fn test_export_pem_private() {
        let keypair = KeyPair::generate();
        let pem = keypair.private_key_pem();
        assert!(pem.is_ok());
        let pem_str = pem.unwrap();
        assert!(pem_str.contains("-----BEGIN PRIVATE KEY-----"));
        assert!(pem_str.contains("-----END PRIVATE KEY-----"));
    }

    #[test]
    fn test_export_pem_public() {
        let keypair = KeyPair::generate();
        let pem = keypair.public_key_pem();
        assert!(pem.is_ok());
        let pem_str = pem.unwrap();
        assert!(pem_str.contains("-----BEGIN PUBLIC KEY-----"));
        assert!(pem_str.contains("-----END PUBLIC KEY-----"));
    }

    #[test]
    fn test_export_jwk_private() {
        let keypair = KeyPair::generate();
        let jwk = keypair.private_key_jwk();
        assert!(jwk.is_ok());
        let jwk_obj = jwk.unwrap();
        assert_eq!(jwk_obj.kty, "EC");
        assert_eq!(jwk_obj.crv, "P-256");
        assert!(jwk_obj.d.is_some());
        assert!(!jwk_obj.x.is_empty());
        assert!(!jwk_obj.y.is_empty());
    }

    #[test]
    fn test_export_jwk_public() {
        let keypair = KeyPair::generate();
        let jwk = keypair.public_key_jwk();
        assert!(jwk.is_ok());
        let jwk_obj = jwk.unwrap();
        assert_eq!(jwk_obj.kty, "EC");
        assert_eq!(jwk_obj.crv, "P-256");
        assert!(jwk_obj.d.is_none());
        assert!(!jwk_obj.x.is_empty());
        assert!(!jwk_obj.y.is_empty());
    }

    #[test]
    fn test_export_did() {
        let keypair = KeyPair::generate();
        let did = keypair.public_key_did();
        assert!(did.starts_with("did:key:z"));
    }

    #[test]
    fn test_key_format_from_str() {
        assert_eq!(KeyFormat::from_str("pem").unwrap(), KeyFormat::Pem);
        assert_eq!(KeyFormat::from_str("PEM").unwrap(), KeyFormat::Pem);
        assert_eq!(KeyFormat::from_str("jwk").unwrap(), KeyFormat::Jwk);
        assert_eq!(KeyFormat::from_str("JWK").unwrap(), KeyFormat::Jwk);
        assert_eq!(KeyFormat::from_str("did").unwrap(), KeyFormat::Did);
        assert_eq!(KeyFormat::from_str("DID").unwrap(), KeyFormat::Did);
        assert!(KeyFormat::from_str("invalid").is_err());
    }

    #[test]
    fn test_export_private_pem() {
        let keypair = KeyPair::generate();
        let result = keypair.export(KeyFormat::Pem, true);
        assert!(result.is_ok());
        assert!(result.unwrap().contains("PRIVATE KEY"));
    }

    #[test]
    fn test_export_public_pem() {
        let keypair = KeyPair::generate();
        let result = keypair.export(KeyFormat::Pem, false);
        assert!(result.is_ok());
        assert!(result.unwrap().contains("PUBLIC KEY"));
    }

    #[test]
    fn test_export_did_rejects_private() {
        let keypair = KeyPair::generate();
        let result = keypair.export(KeyFormat::Did, true);
        assert!(result.is_err());
    }
}
