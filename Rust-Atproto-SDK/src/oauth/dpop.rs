//! DPoP (Demonstrating Proof of Possession) implementation (RFC 9449)
//!
//! DPoP binds access tokens to specific HTTP requests by requiring the client
//! to prove possession of a private key. This prevents token theft and replay attacks.

use jsonwebtoken::{Algorithm, Header};
use rsa::{RsaPrivateKey, RsaPublicKey};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use parking_lot::RwLock;
use uuid::Uuid;

/// DPoP error types
#[derive(Debug, thiserror::Error)]
pub enum DPopError {
    #[error("RSA key generation failed: {0}")]
    KeyGeneration(String),

    #[error("JWT encoding failed: {0}")]
    JwtEncoding(#[from] jsonwebtoken::errors::Error),

    #[error("Public key encoding failed: {0}")]
    PublicKeyEncoding(String),
}

/// JWK (JSON Web Key) representation of RSA public key
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Jwk {
    /// Key type (always "RSA" for us)
    pub kty: String,

    /// Public exponent (base64url encoded)
    pub e: String,

    /// Modulus (base64url encoded)
    pub n: String,
}

/// DPoP JWT header
///
/// Documents the expected structure but not directly used since jsonwebtoken
/// doesn't support custom header fields (see generate_proof implementation).
/// The header is manually constructed at lines 201-205.
#[derive(Debug, Serialize, Deserialize)]
#[allow(dead_code)]
struct DPopHeader {
    /// Type (must be "dpop+jwt")
    pub typ: String,

    /// Algorithm (must be "RS256")
    pub alg: String,

    /// JSON Web Key (public key)
    pub jwk: Jwk,
}

/// DPoP JWT claims
#[derive(Debug, Serialize, Deserialize)]
struct DPopClaims {
    /// JWT ID (unique identifier for this token)
    pub jti: String,

    /// HTTP method (GET, POST, etc.)
    pub htm: String,

    /// HTTP URI (the target URL)
    pub htu: String,

    /// Issued at (Unix timestamp)
    pub iat: i64,

    /// Expiration (Unix timestamp, 60 seconds from iat)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exp: Option<i64>,
}

/// DPoP manager for generating and signing DPoP proofs
///
/// Manages an RSA key pair and generates DPoP JWTs that prove possession
/// of the private key for each HTTP request.
///
/// ## Example
///
/// ```
/// use atproto::oauth::DPopManager;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     // Generate a new key pair
///     let dpop = DPopManager::new()?;
///
///     // Generate a DPoP proof for a request
///     let proof = dpop.generate_proof("POST", "https://bsky.social/xrpc/com.atproto.server.createSession")?;
///
///     // Add proof to request header: DPoP: <proof>
///     println!("DPoP: {}", proof);
///
///     Ok(())
/// }
/// ```
pub struct DPopManager {
    /// RSA private key (2048-bit)
    private_key: Arc<RwLock<RsaPrivateKey>>,

    /// RSA public key
    /// Cached for potential future use (key export, verification).
    /// Currently only the JWK representation is used.
    #[allow(dead_code)]
    public_key: Arc<RwLock<RsaPublicKey>>,

    /// JWK representation of public key (cached)
    jwk: Arc<RwLock<Jwk>>,
}

impl DPopManager {
    /// Create a new DPoP manager with a fresh RSA key pair
    ///
    /// Generates a 2048-bit RSA key pair for signing DPoP proofs.
    ///
    /// ## Errors
    ///
    /// Returns `DPopError::KeyGeneration` if RSA key generation fails.
    pub fn new() -> Result<Self, DPopError> {
        let mut rng = rand::thread_rng();
        let bits = 2048;

        let private_key = RsaPrivateKey::new(&mut rng, bits)
            .map_err(|e| DPopError::KeyGeneration(e.to_string()))?;

        let public_key = RsaPublicKey::from(&private_key);
        let jwk = Self::public_key_to_jwk(&public_key)?;

        Ok(Self {
            private_key: Arc::new(RwLock::new(private_key)),
            public_key: Arc::new(RwLock::new(public_key)),
            jwk: Arc::new(RwLock::new(jwk)),
        })
    }

    /// Convert RSA public key to JWK format
    ///
    /// Extracts the modulus (n) and exponent (e) from the public key
    /// and encodes them as base64url strings.
    fn public_key_to_jwk(public_key: &RsaPublicKey) -> Result<Jwk, DPopError> {
        use rsa::traits::PublicKeyParts;
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

        // Get modulus and exponent
        let n = public_key.n().to_bytes_be();
        let e = public_key.e().to_bytes_be();

        // Encode as base64url
        let n_b64 = URL_SAFE_NO_PAD.encode(&n);
        let e_b64 = URL_SAFE_NO_PAD.encode(&e);

        Ok(Jwk {
            kty: "RSA".to_string(),
            n: n_b64,
            e: e_b64,
        })
    }

    /// Generate a DPoP proof JWT for an HTTP request
    ///
    /// Creates a JWT signed with the private key that proves possession
    /// for the specified HTTP method and URL.
    ///
    /// ## Arguments
    ///
    /// * `method` - HTTP method (GET, POST, etc.)
    /// * `url` - Target URL
    ///
    /// ## Returns
    ///
    /// A signed JWT string to be included in the DPoP header
    ///
    /// ## Example
    ///
    /// ```
    /// # use atproto::oauth::DPopManager;
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let dpop = DPopManager::new()?;
    /// let proof = dpop.generate_proof("POST", "https://bsky.social/xrpc/com.atproto.server.createSession")?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn generate_proof(&self, method: &str, url: &str) -> Result<String, DPopError> {
        let jwk = self.jwk.read().clone();
        let private_key = self.private_key.read();

        // Create header with JWK
        let mut header = Header::new(Algorithm::RS256);
        header.typ = Some("dpop+jwt".to_string());

        // Unfortunately jsonwebtoken doesn't support custom header fields,
        // so we'll construct the JWT manually

        // Create claims
        let now = chrono::Utc::now().timestamp();
        let claims = DPopClaims {
            jti: Uuid::new_v4().to_string(),
            htm: method.to_uppercase(),
            htu: url.to_string(),
            iat: now,
            exp: Some(now + 60), // 60 second expiration
        };

        // Encode the header and claims
        let header_json = serde_json::json!({
            "typ": "dpop+jwt",
            "alg": "RS256",
            "jwk": jwk
        });

        // Base64url encode header and claims
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
        let header_b64 = URL_SAFE_NO_PAD.encode(header_json.to_string());
        let claims_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_string(&claims).unwrap());

        // Create signing input
        let signing_input = format!("{}.{}", header_b64, claims_b64);

        // Sign with RSA-SHA256
        use sha2::{Sha256, Digest};
        use rsa::Pkcs1v15Sign;

        // Hash the signing input with SHA-256
        let mut hasher = Sha256::new();
        hasher.update(signing_input.as_bytes());
        let hash = hasher.finalize();

        // Sign the hash with RSA
        let signature = private_key
            .sign(Pkcs1v15Sign::new::<Sha256>(), &hash)
            .map_err(|e| DPopError::JwtEncoding(
                jsonwebtoken::errors::Error::from(jsonwebtoken::errors::ErrorKind::InvalidRsaKey(e.to_string()))
            ))?;
        let signature_b64 = URL_SAFE_NO_PAD.encode(&signature);

        // Construct final JWT
        Ok(format!("{}.{}", signing_input, signature_b64))
    }

    /// Get the JWK representation of the public key
    ///
    /// Used for client metadata and debugging.
    pub fn get_jwk(&self) -> Jwk {
        self.jwk.read().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dpop_manager_creation() {
        let dpop = DPopManager::new();
        assert!(dpop.is_ok());
    }

    #[test]
    fn test_generate_proof() {
        let dpop = DPopManager::new().unwrap();
        let proof = dpop.generate_proof("POST", "https://example.com/token");

        assert!(proof.is_ok());
        let proof_str = proof.unwrap();

        // JWT should have 3 parts separated by dots
        let parts: Vec<&str> = proof_str.split('.').collect();
        assert_eq!(parts.len(), 3, "JWT should have 3 parts (header.claims.signature)");
    }

    #[test]
    fn test_jwk_format() {
        let dpop = DPopManager::new().unwrap();
        let jwk = dpop.get_jwk();

        assert_eq!(jwk.kty, "RSA");
        assert!(!jwk.n.is_empty(), "Modulus should not be empty");
        assert!(!jwk.e.is_empty(), "Exponent should not be empty");
    }

    #[test]
    fn test_proof_contains_method_and_url() {
        let dpop = DPopManager::new().unwrap();
        let method = "POST";
        let url = "https://example.com/token";

        let proof = dpop.generate_proof(method, url).unwrap();

        // Decode the claims part (middle section)
        let parts: Vec<&str> = proof.split('.').collect();
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
        let claims_json = URL_SAFE_NO_PAD.decode(parts[1]).unwrap();
        let claims_str = String::from_utf8(claims_json).unwrap();

        assert!(claims_str.contains(method));
        assert!(claims_str.contains(url));
    }

    #[test]
    fn test_unique_jti_per_proof() {
        let dpop = DPopManager::new().unwrap();

        let proof1 = dpop.generate_proof("GET", "https://example.com").unwrap();
        let proof2 = dpop.generate_proof("GET", "https://example.com").unwrap();

        // Even with same method and URL, proofs should be different due to unique jti
        assert_ne!(proof1, proof2);
    }

    #[test]
    fn test_proof_expiration() {
        let dpop = DPopManager::new().unwrap();
        let proof = dpop.generate_proof("POST", "https://example.com/token").unwrap();

        // Decode claims
        let parts: Vec<&str> = proof.split('.').collect();
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
        let claims_json = URL_SAFE_NO_PAD.decode(parts[1]).unwrap();
        let claims: DPopClaims = serde_json::from_slice(&claims_json).unwrap();

        // Check that expiration is set and is 60 seconds after iat
        assert!(claims.exp.is_some());
        let exp = claims.exp.unwrap();
        assert_eq!(exp - claims.iat, 60);
    }
}
