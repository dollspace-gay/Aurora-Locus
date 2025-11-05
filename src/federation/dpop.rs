/// DPoP (Demonstrating Proof of Possession) Support
///
/// Implements ATProto DPoP specification for client-to-PDS authentication:
/// - Binds access tokens to specific client devices via public key cryptography
/// - Prevents token theft and replay attacks
/// - Requires clients to prove possession of private key on each request
///
/// DPoP Proof JWT Format:
/// - Header: typ="dpop+jwt", alg=ES256, jwk={client's public key}
/// - Claims: jti (nonce), htm (HTTP method), htu (HTTP URI), iat, exp
///
/// References:
/// - https://datatracker.ietf.org/doc/html/rfc9449
/// - https://atproto.com/specs/xrpc#dpop

use crate::error::{PdsError, PdsResult};
use chrono::Utc;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn};

/// DPoP proof JWT claims
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DPopClaims {
    /// JWT ID (unique nonce)
    pub jti: String,

    /// HTTP method (GET, POST, etc.)
    pub htm: String,

    /// HTTP URI (target endpoint)
    pub htu: String,

    /// Issued at (Unix timestamp)
    pub iat: i64,

    /// Expiration (Unix timestamp) - typically <60s
    pub exp: i64,
}

/// JWK (JSON Web Key) representation from DPoP proof header
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Jwk {
    pub kty: String,        // Key type (e.g., "EC")
    pub crv: String,        // Curve (e.g., "P-256")
    pub x: String,          // X coordinate (base64url)
    pub y: String,          // Y coordinate (base64url)
}

/// DPoP nonce generator and tracker
pub struct DPopNonceStore {
    /// Active nonces mapped to expiration time
    nonces: Arc<RwLock<HashMap<String, i64>>>,
}

impl DPopNonceStore {
    pub fn new() -> Self {
        Self {
            nonces: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Generate a new DPoP nonce
    pub async fn generate_nonce(&self) -> String {
        let nonce = uuid::Uuid::new_v4().to_string();
        let expires_at = Utc::now().timestamp() + 300; // 5 minutes

        let mut nonces = self.nonces.write().await;
        nonces.insert(nonce.clone(), expires_at);

        debug!("Generated DPoP nonce: {}", nonce);
        nonce
    }

    /// Check if nonce is valid and mark as used
    pub async fn check_and_consume_nonce(&self, nonce: &str) -> PdsResult<bool> {
        let mut nonces = self.nonces.write().await;

        if let Some(&expires_at) = nonces.get(nonce) {
            let now = Utc::now().timestamp();

            if now < expires_at {
                // Nonce is valid, consume it (remove from store)
                nonces.remove(nonce);
                debug!("DPoP nonce consumed: {}", nonce);
                Ok(true)
            } else {
                // Nonce expired
                nonces.remove(nonce);
                warn!("DPoP nonce expired: {}", nonce);
                Ok(false)
            }
        } else {
            // Nonce not found
            warn!("DPoP nonce not found: {}", nonce);
            Ok(false)
        }
    }

    /// Cleanup expired nonces
    pub async fn cleanup_expired(&self) -> PdsResult<usize> {
        let mut nonces = self.nonces.write().await;
        let now = Utc::now().timestamp();
        let initial_count = nonces.len();

        nonces.retain(|_, &mut expires_at| expires_at > now);

        let removed = initial_count - nonces.len();
        if removed > 0 {
            debug!("Cleaned up {} expired DPoP nonces", removed);
        }

        Ok(removed)
    }
}

/// DPoP verifier
pub struct DPopVerifier {
    nonce_store: Arc<DPopNonceStore>,
}

impl DPopVerifier {
    pub fn new(nonce_store: Arc<DPopNonceStore>) -> Self {
        Self { nonce_store }
    }

    /// Verify a DPoP proof JWT
    ///
    /// Steps:
    /// 1. Decode JWT header to extract JWK (public key)
    /// 2. Verify JWT signature using that public key
    /// 3. Validate claims (htm, htu, nonce, expiration)
    /// 4. Extract and return the public key thumbprint (for token binding)
    ///
    /// # Arguments
    /// * `dpop_proof` - The DPoP proof JWT
    /// * `http_method` - Expected HTTP method (e.g., "GET")
    /// * `http_uri` - Expected HTTP URI (e.g., "https://pds.example.com/xrpc/com.atproto.repo.getRecord")
    ///
    /// # Returns
    /// The JWK thumbprint (SHA-256 hash) if verification succeeds
    pub async fn verify_dpop_proof(
        &self,
        dpop_proof: &str,
        http_method: &str,
        http_uri: &str,
    ) -> PdsResult<String> {
        debug!("Verifying DPoP proof for {} {}", http_method, http_uri);

        // Decode header to extract JWK
        let header = decode_header(dpop_proof).map_err(|e| {
            warn!("Failed to decode DPoP proof header: {}", e);
            PdsError::Authentication("Invalid DPoP proof format".to_string())
        })?;

        // Check typ header
        if header.typ.as_deref() != Some("dpop+jwt") {
            warn!("Invalid DPoP proof typ: {:?}", header.typ);
            return Err(PdsError::Authentication(
                "Invalid DPoP proof type".to_string(),
            ));
        }

        // Extract JWK from header
        let jwk = header.jwk.ok_or_else(|| {
            warn!("DPoP proof missing JWK in header");
            PdsError::Authentication("DPoP proof missing JWK".to_string())
        })?;

        // Convert JWK to DecodingKey
        let jwk_json = serde_json::to_value(&jwk).map_err(|e| {
            PdsError::Internal(format!("Failed to serialize JWK: {}", e))
        })?;

        let decoding_key = jwk_to_decoding_key(&jwk_json)?;

        // Verify JWT signature
        let mut validation = Validation::new(Algorithm::ES256);
        validation.validate_exp = true;
        validation.leeway = 0; // Strict expiration

        let token_data = decode::<DPopClaims>(dpop_proof, &decoding_key, &validation).map_err(
            |e| {
                warn!("DPoP proof verification failed: {}", e);
                match e.kind() {
                    jsonwebtoken::errors::ErrorKind::ExpiredSignature => {
                        PdsError::Authentication("DPoP proof expired".to_string())
                    }
                    _ => PdsError::Authentication(format!("DPoP proof invalid: {}", e)),
                }
            },
        )?;

        let claims = token_data.claims;

        // Validate HTTP method
        if claims.htm.to_uppercase() != http_method.to_uppercase() {
            warn!(
                "DPoP proof HTTP method mismatch: expected {}, got {}",
                http_method, claims.htm
            );
            return Err(PdsError::Authentication(
                "DPoP proof HTTP method mismatch".to_string(),
            ));
        }

        // Validate HTTP URI (without query params)
        let expected_uri = http_uri.split('?').next().unwrap_or(http_uri);
        let proof_uri = claims.htu.split('?').next().unwrap_or(&claims.htu);

        if proof_uri != expected_uri {
            warn!(
                "DPoP proof HTTP URI mismatch: expected {}, got {}",
                expected_uri, proof_uri
            );
            return Err(PdsError::Authentication(
                "DPoP proof HTTP URI mismatch".to_string(),
            ));
        }

        // Validate and consume nonce
        if !self.nonce_store.check_and_consume_nonce(&claims.jti).await? {
            warn!("DPoP proof nonce invalid or already used: {}", claims.jti);
            return Err(PdsError::Authentication(
                "DPoP proof nonce invalid or expired".to_string(),
            ));
        }

        // Compute JWK thumbprint (SHA-256 hash for token binding)
        let thumbprint = compute_jwk_thumbprint(&jwk_json)?;

        debug!("✓ DPoP proof verified: thumbprint={}", thumbprint);

        Ok(thumbprint)
    }
}

/// Convert JWK to DecodingKey
///
/// Parses an EC P-256 public key from JWK format and converts it to a DecodingKey
/// for JWT signature verification.
///
/// # JWK Format (RFC 7517)
/// ```json
/// {
///   "kty": "EC",
///   "crv": "P-256",
///   "x": "base64url-encoded x-coordinate",
///   "y": "base64url-encoded y-coordinate"
/// }
/// ```
fn jwk_to_decoding_key(jwk: &Value) -> PdsResult<DecodingKey> {
    // Extract JWK parameters
    let kty = jwk["kty"].as_str().ok_or_else(|| {
        PdsError::Authentication("JWK missing 'kty' field".to_string())
    })?;

    let crv = jwk["crv"].as_str().ok_or_else(|| {
        PdsError::Authentication("JWK missing 'crv' field".to_string())
    })?;

    let x = jwk["x"].as_str().ok_or_else(|| {
        PdsError::Authentication("JWK missing 'x' field".to_string())
    })?;

    let y = jwk["y"].as_str().ok_or_else(|| {
        PdsError::Authentication("JWK missing 'y' field".to_string())
    })?;

    // Validate key type and curve
    if kty != "EC" {
        return Err(PdsError::Authentication(format!(
            "Unsupported JWK key type: {} (expected EC)",
            kty
        )));
    }

    if crv != "P-256" {
        return Err(PdsError::Authentication(format!(
            "Unsupported JWK curve: {} (expected P-256)",
            crv
        )));
    }

    // Decode base64url coordinates
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;

    let x_bytes = URL_SAFE_NO_PAD.decode(x).map_err(|e| {
        PdsError::Authentication(format!("Invalid JWK x coordinate: {}", e))
    })?;

    let y_bytes = URL_SAFE_NO_PAD.decode(y).map_err(|e| {
        PdsError::Authentication(format!("Invalid JWK y coordinate: {}", e))
    })?;

    // Validate coordinate lengths (P-256 uses 32-byte coordinates)
    if x_bytes.len() != 32 {
        return Err(PdsError::Authentication(format!(
            "Invalid JWK x coordinate length: {} (expected 32)",
            x_bytes.len()
        )));
    }

    if y_bytes.len() != 32 {
        return Err(PdsError::Authentication(format!(
            "Invalid JWK y coordinate length: {} (expected 32)",
            y_bytes.len()
        )));
    }

    // Construct uncompressed public key point (0x04 || x || y)
    // SEC 1 v2.0 Section 2.3.3: Uncompressed point encoding
    let mut public_key_bytes = Vec::with_capacity(65);
    public_key_bytes.push(0x04); // Uncompressed point marker
    public_key_bytes.extend_from_slice(&x_bytes);
    public_key_bytes.extend_from_slice(&y_bytes);

    // Parse as P-256 public key
    use p256::ecdsa::VerifyingKey;
    use p256::EncodedPoint;

    let encoded_point = EncodedPoint::from_bytes(&public_key_bytes).map_err(|e| {
        PdsError::Authentication(format!("Invalid EC point encoding: {}", e))
    })?;

    let verifying_key = VerifyingKey::from_encoded_point(&encoded_point).map_err(|e| {
        PdsError::Authentication(format!("Invalid P-256 public key: {}", e))
    })?;

    // Convert to SPKI (SubjectPublicKeyInfo) DER format
    // This is the standard format for public keys in X.509/PKCS#8
    use p256::pkcs8::EncodePublicKey;

    let public_key_der = verifying_key.to_public_key_der().map_err(|e| {
        PdsError::Internal(format!("Failed to encode public key to DER: {}", e))
    })?;

    // Convert DER to PEM format for jsonwebtoken
    let public_key_pem = format!(
        "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----",
        base64::engine::general_purpose::STANDARD.encode(public_key_der.as_bytes())
    );

    // Create DecodingKey from EC PEM
    DecodingKey::from_ec_pem(public_key_pem.as_bytes()).map_err(|e| {
        PdsError::Internal(format!("Failed to create DecodingKey from PEM: {}", e))
    })
}

/// Compute JWK thumbprint (SHA-256 hash)
///
/// The thumbprint is used for token binding - it uniquely identifies the client's key.
/// Reference: RFC 7638
fn compute_jwk_thumbprint(jwk: &Value) -> PdsResult<String> {
    // Extract required fields in canonical order: kty, crv, x, y (for EC keys)
    let kty = jwk["kty"].as_str().ok_or_else(|| {
        PdsError::Internal("JWK missing kty field".to_string())
    })?;

    let crv = jwk["crv"].as_str().ok_or_else(|| {
        PdsError::Internal("JWK missing crv field".to_string())
    })?;

    let x = jwk["x"].as_str().ok_or_else(|| {
        PdsError::Internal("JWK missing x field".to_string())
    })?;

    let y = jwk["y"].as_str().ok_or_else(|| {
        PdsError::Internal("JWK missing y field".to_string())
    })?;

    // Create canonical JSON (RFC 7638 requires specific ordering and no whitespace)
    let canonical = format!(
        r#"{{"crv":"{}","kty":"{}","x":"{}","y":"{}"}}"#,
        crv, kty, x, y
    );

    // Compute SHA-256 hash
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    let hash = hasher.finalize();

    // Encode as base64url (no padding)
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    Ok(URL_SAFE_NO_PAD.encode(hash))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dpop_claims_serialization() {
        let claims = DPopClaims {
            jti: "test-nonce-123".to_string(),
            htm: "POST".to_string(),
            htu: "https://pds.example.com/xrpc/com.atproto.repo.createRecord".to_string(),
            iat: Utc::now().timestamp(),
            exp: Utc::now().timestamp() + 60,
        };

        let json = serde_json::to_string(&claims).unwrap();
        let deserialized: DPopClaims = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.jti, "test-nonce-123");
        assert_eq!(deserialized.htm, "POST");
    }

    #[tokio::test]
    async fn test_dpop_nonce_store() {
        let store = DPopNonceStore::new();

        // Generate nonce
        let nonce = store.generate_nonce().await;
        assert!(!nonce.is_empty());

        // Check and consume nonce (should succeed)
        let valid = store.check_and_consume_nonce(&nonce).await.unwrap();
        assert!(valid);

        // Try to use same nonce again (should fail - already consumed)
        let valid = store.check_and_consume_nonce(&nonce).await.unwrap();
        assert!(!valid);

        // Try invalid nonce (should fail)
        let valid = store.check_and_consume_nonce("invalid-nonce").await.unwrap();
        assert!(!valid);
    }

    #[test]
    fn test_jwk_thumbprint() {
        let jwk = serde_json::json!({
            "kty": "EC",
            "crv": "P-256",
            "x": "WKn-ZIGevcwGIyyrzFoZNBdaq9_TsqzGl96oc0CWuis",
            "y": "y77t-RvAHRKTsSGdIYUfweuOvwrvDD-Q3Hv5J0fSKbE"
        });

        let thumbprint = compute_jwk_thumbprint(&jwk).unwrap();
        assert!(!thumbprint.is_empty());

        // Thumbprint should be deterministic
        let thumbprint2 = compute_jwk_thumbprint(&jwk).unwrap();
        assert_eq!(thumbprint, thumbprint2);
    }

    #[test]
    fn test_jwk_to_decoding_key_valid() {
        // Valid P-256 public key JWK
        let jwk = serde_json::json!({
            "kty": "EC",
            "crv": "P-256",
            "x": "WKn-ZIGevcwGIyyrzFoZNBdaq9_TsqzGl96oc0CWuis",
            "y": "y77t-RvAHRKTsSGdIYUfweuOvwrvDD-Q3Hv5J0fSKbE"
        });

        let result = jwk_to_decoding_key(&jwk);
        assert!(result.is_ok(), "Should successfully parse valid JWK");
    }

    #[test]
    fn test_jwk_to_decoding_key_missing_fields() {
        // Missing 'x' coordinate
        let jwk = serde_json::json!({
            "kty": "EC",
            "crv": "P-256",
            "y": "y77t-RvAHRKTsSGdIYUfweuOvwrvDD-Q3Hv5J0fSKbE"
        });

        let result = jwk_to_decoding_key(&jwk);
        assert!(result.is_err(), "Should fail on missing 'x' field");

        // Missing 'y' coordinate
        let jwk = serde_json::json!({
            "kty": "EC",
            "crv": "P-256",
            "x": "WKn-ZIGevcwGIyyrzFoZNBdaq9_TsqzGl96oc0CWuis"
        });

        let result = jwk_to_decoding_key(&jwk);
        assert!(result.is_err(), "Should fail on missing 'y' field");
    }

    #[test]
    fn test_jwk_to_decoding_key_unsupported_curve() {
        // Unsupported curve (P-384 instead of P-256)
        let jwk = serde_json::json!({
            "kty": "EC",
            "crv": "P-384",
            "x": "WKn-ZIGevcwGIyyrzFoZNBdaq9_TsqzGl96oc0CWuis",
            "y": "y77t-RvAHRKTsSGdIYUfweuOvwrvDD-Q3Hv5J0fSKbE"
        });

        let result = jwk_to_decoding_key(&jwk);
        assert!(result.is_err(), "Should fail on unsupported curve");
        assert!(
            result.unwrap_err().to_string().contains("P-384"),
            "Error should mention P-384"
        );
    }

    #[test]
    fn test_jwk_to_decoding_key_invalid_base64() {
        // Invalid base64url encoding in 'x' coordinate
        let jwk = serde_json::json!({
            "kty": "EC",
            "crv": "P-256",
            "x": "invalid!!!base64",
            "y": "y77t-RvAHRKTsSGdIYUfweuOvwrvDD-Q3Hv5J0fSKbE"
        });

        let result = jwk_to_decoding_key(&jwk);
        assert!(result.is_err(), "Should fail on invalid base64url");
    }

    #[test]
    fn test_jwk_to_decoding_key_wrong_key_type() {
        // Wrong key type (RSA instead of EC)
        let jwk = serde_json::json!({
            "kty": "RSA",
            "crv": "P-256",
            "x": "WKn-ZIGevcwGIyyrzFoZNBdaq9_TsqzGl96oc0CWuis",
            "y": "y77t-RvAHRKTsSGdIYUfweuOvwrvDD-Q3Hv5J0fSKbE"
        });

        let result = jwk_to_decoding_key(&jwk);
        assert!(result.is_err(), "Should fail on non-EC key type");
        assert!(
            result.unwrap_err().to_string().contains("RSA"),
            "Error should mention RSA"
        );
    }

    #[test]
    fn test_jwk_to_decoding_key_invalid_coordinate_length() {
        // Coordinate too short (only 16 bytes instead of 32)
        let jwk = serde_json::json!({
            "kty": "EC",
            "crv": "P-256",
            "x": "WKn-ZIGevcwGIw", // Only 10 base64url chars = ~7 bytes
            "y": "y77t-RvAHRKTsSGdIYUfweuOvwrvDD-Q3Hv5J0fSKbE"
        });

        let result = jwk_to_decoding_key(&jwk);
        assert!(result.is_err(), "Should fail on invalid coordinate length");
    }
}
