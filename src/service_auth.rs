/// Service Authentication Module
///
/// Implements ATProto service-to-service authentication using JWT tokens.
/// This is critical for federation support, allowing PDS instances to authenticate
/// requests to each other.
///
/// ## JWT Token Structure
///
/// Service auth tokens contain:
/// - **iss**: Issuer DID (the actor making the request)
/// - **aud**: Audience DID (the service receiving the request)
/// - **exp**: Expiration timestamp (seconds since epoch)
/// - **lxm**: Optional lexicon method (for method-specific tokens)
/// - **jti**: Optional nonce for replay prevention
///
/// ## Token Generation Rules
///
/// - Signed using the actor's ATProto signing key (from DID document)
/// - Maximum expiration: 1 hour in future
/// - Method-less tokens: maximum 1 minute expiration
/// - Cannot be generated for "protected" methods
///
/// ## Token Verification
///
/// 1. Resolve issuer's DID document to get signing key
/// 2. Verify JWT signature using ES256K (secp256k1)
/// 3. Check audience matches this PDS's DID
/// 4. Check expiration hasn't passed
/// 5. Optionally track nonce to prevent replay attacks
///
/// ## References
///
/// - ATProto Service Auth: https://atproto.com/specs/xrpc#inter-service-authentication-temporary-specification
/// - JWT RFC: https://tools.ietf.org/html/rfc7519
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::Utc;
use k256::ecdsa::{signature::Signer, signature::Verifier, Signature, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::{
    error::{PdsError, PdsResult},
    identity::resolver::IdentityResolver,
};

/// Maximum expiration time for tokens with a lexicon method (1 hour)
const MAX_EXP_WITH_METHOD: i64 = 3600;

/// Maximum expiration time for tokens without a lexicon method (1 minute)
const MAX_EXP_WITHOUT_METHOD: i64 = 60;

/// Service auth JWT claims following ATProto specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceAuthClaims {
    /// Issuer DID (the actor making the request)
    pub iss: String,

    /// Audience DID (the service receiving the request)
    pub aud: String,

    /// Expiration timestamp (seconds since epoch)
    pub exp: i64,

    /// Optional lexicon method (e.g., "com.atproto.repo.createRecord")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lxm: Option<String>,

    /// Optional nonce/JWT ID for replay prevention
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jti: Option<String>,
}

impl ServiceAuthClaims {
    /// Validate the claims
    ///
    /// Checks:
    /// - Expiration hasn't passed
    /// - Expiration is within allowed limits
    pub fn validate(&self) -> PdsResult<()> {
        let now = Utc::now().timestamp();

        // Check expiration hasn't passed
        if self.exp <= now {
            return Err(PdsError::Authentication(
                "Service auth token has expired".to_string(),
            ));
        }

        // Check expiration is within limits
        let max_exp = if self.lxm.is_some() {
            MAX_EXP_WITH_METHOD
        } else {
            MAX_EXP_WITHOUT_METHOD
        };

        let exp_duration = self.exp - now;
        if exp_duration > max_exp {
            return Err(PdsError::Validation(format!(
                "Token expiration too far in future: {} seconds (max: {})",
                exp_duration, max_exp
            )));
        }

        Ok(())
    }
}

/// Generate a service auth JWT token
///
/// Creates a JWT token for service-to-service authentication following ATProto spec.
///
/// # Arguments
///
/// * `iss` - Issuer DID (the actor making the request)
/// * `aud` - Audience DID (the service receiving the request)
/// * `exp_seconds` - Optional expiration duration in seconds (default: 60s for no method, 3600s with method)
/// * `lxm` - Optional lexicon method for method-specific tokens
/// * `signing_key` - The secp256k1 private key bytes (32 bytes)
///
/// # Returns
///
/// A JWT token string that can be used in the Authorization header
///
/// # Errors
///
/// Returns an error if:
/// - Expiration is too far in the future
/// - Key is invalid
/// - JWT encoding fails
///
/// # Example
///
/// ```no_run
/// use aurora_locus::service_auth::create_service_jwt;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let signing_key = vec![0u8; 32]; // Your private key
/// let token = create_service_jwt(
///     "did:plc:issuer123",
///     "did:plc:audience456",
///     Some(300), // 5 minutes
///     Some("com.atproto.repo.createRecord"),
///     &signing_key,
/// )?;
/// # Ok(())
/// # }
/// ```
pub fn create_service_jwt(
    iss: &str,
    aud: &str,
    exp_seconds: Option<i64>,
    lxm: Option<&str>,
    signing_key: &[u8],
) -> PdsResult<String> {
    // Determine expiration
    let now = Utc::now().timestamp();
    let default_exp = if lxm.is_some() {
        MAX_EXP_WITH_METHOD
    } else {
        MAX_EXP_WITHOUT_METHOD
    };

    let exp_duration = exp_seconds.unwrap_or(default_exp);
    let exp = now + exp_duration;

    // Create claims
    let claims = ServiceAuthClaims {
        iss: iss.to_string(),
        aud: aud.to_string(),
        exp,
        lxm: lxm.map(|s| s.to_string()),
        jti: None, // Can be added if replay protection is needed
    };

    // Validate claims
    claims.validate()?;

    // Convert signing key bytes to k256 SigningKey
    let signing_key = SigningKey::from_slice(signing_key)
        .map_err(|e| PdsError::Authentication(format!("Invalid signing key: {}", e)))?;

    // Create JWT manually since jsonwebtoken doesn't support ES256K
    // JWT format: header.payload.signature (all base64url encoded)

    // Create header for ES256K
    let header = serde_json::json!({
        "alg": "ES256K",
        "typ": "JWT"
    });

    let header_json = serde_json::to_string(&header)
        .map_err(|e| PdsError::Jwt(format!("Failed to serialize header: {}", e)))?;
    let header_b64 = URL_SAFE_NO_PAD.encode(header_json.as_bytes());

    // Serialize claims
    let claims_json = serde_json::to_string(&claims)
        .map_err(|e| PdsError::Jwt(format!("Failed to serialize claims: {}", e)))?;
    let claims_b64 = URL_SAFE_NO_PAD.encode(claims_json.as_bytes());

    // Create signing input
    let signing_input = format!("{}.{}", header_b64, claims_b64);

    // Sign with secp256k1
    let signature: Signature = signing_key.sign(signing_input.as_bytes());

    // Encode signature (DER format)
    let signature_bytes = signature.to_der();
    let signature_b64 = URL_SAFE_NO_PAD.encode(signature_bytes.as_bytes());

    // Combine into final JWT
    let token = format!("{}.{}.{}", header_b64, claims_b64, signature_b64);

    Ok(token)
}

/// Verify a service auth JWT token
///
/// Verifies a service auth JWT by:
/// 1. Resolving the issuer's DID document
/// 2. Extracting the signing key
/// 3. Verifying the JWT signature
/// 4. Validating the claims (expiration, audience)
///
/// # Arguments
///
/// * `token` - The JWT token string (without "Bearer " prefix)
/// * `expected_aud` - The expected audience DID (this service's DID)
/// * `identity_resolver` - Identity resolver for fetching DID documents
///
/// # Returns
///
/// The validated claims if verification succeeds
///
/// # Errors
///
/// Returns an error if:
/// - Token format is invalid
/// - Issuer's DID document cannot be resolved
/// - Signing key cannot be extracted
/// - Signature verification fails
/// - Claims validation fails (expiration, audience mismatch)
///
/// # Example
///
/// ```no_run
/// use aurora_locus::service_auth::verify_service_jwt;
/// use aurora_locus::identity::resolver::IdentityResolver;
///
/// # async fn example(resolver: IdentityResolver) -> Result<(), Box<dyn std::error::Error>> {
/// let token = "eyJ..."; // JWT token from request
/// let claims = verify_service_jwt(
///     token,
///     "did:plc:myservice123",
///     &resolver,
/// ).await?;
///
/// println!("Authenticated request from: {}", claims.iss);
/// # Ok(())
/// # }
/// ```
#[allow(dead_code)] // Public API for future service-to-service auth
pub async fn verify_service_jwt(
    token: &str,
    expected_aud: &str,
    identity_resolver: &IdentityResolver,
) -> PdsResult<ServiceAuthClaims> {
    // Split JWT into parts
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(PdsError::Jwt(
            "Invalid JWT format: expected 3 parts".to_string(),
        ));
    }

    let (header_b64, claims_b64, signature_b64) = (parts[0], parts[1], parts[2]);

    // Decode header
    let header_bytes = URL_SAFE_NO_PAD
        .decode(header_b64)
        .map_err(|e| PdsError::Jwt(format!("Failed to decode header: {}", e)))?;
    let header: serde_json::Value = serde_json::from_slice(&header_bytes)
        .map_err(|e| PdsError::Jwt(format!("Failed to parse header: {}", e)))?;

    // Verify algorithm is ES256K
    let alg = header
        .get("alg")
        .and_then(|v| v.as_str())
        .ok_or_else(|| PdsError::Jwt("Missing algorithm in JWT header".to_string()))?;

    if alg != "ES256K" {
        return Err(PdsError::Jwt(format!("Unsupported algorithm: {}", alg)));
    }

    // Decode claims
    let claims_bytes = URL_SAFE_NO_PAD
        .decode(claims_b64)
        .map_err(|e| PdsError::Jwt(format!("Failed to decode claims: {}", e)))?;
    let claims: ServiceAuthClaims = serde_json::from_slice(&claims_bytes)
        .map_err(|e| PdsError::Jwt(format!("Failed to parse claims: {}", e)))?;

    // Validate audience matches expected
    if claims.aud != expected_aud {
        return Err(PdsError::Authentication(format!(
            "Invalid audience: expected {}, got {}",
            expected_aud, claims.aud
        )));
    }

    // Resolve issuer's DID document to get signing key
    let did_doc = identity_resolver
        .resolve_did(&claims.iss)
        .await
        .map_err(|e| PdsError::Authentication(format!("Failed to resolve issuer DID: {}", e)))?;

    // Extract signing key from DID document
    let verification_method = did_doc.get_signing_key().ok_or_else(|| {
        PdsError::Authentication(format!(
            "No signing key found in DID document for {}",
            claims.iss
        ))
    })?;

    // Decode multibase public key
    let public_key_bytes = verification_method
        .public_key_multibase
        .as_ref()
        .ok_or_else(|| {
            PdsError::Authentication("No public key in verification method".to_string())
        })?;

    // Parse multibase key (format: z<base58btc-encoded-key>)
    let public_key_decoded = decode_multibase_key(public_key_bytes)?;

    // Create verifying key
    let verifying_key = VerifyingKey::from_sec1_bytes(&public_key_decoded)
        .map_err(|e| PdsError::Authentication(format!("Invalid public key: {}", e)))?;

    // Decode signature
    let signature_bytes = URL_SAFE_NO_PAD
        .decode(signature_b64)
        .map_err(|e| PdsError::Jwt(format!("Failed to decode signature: {}", e)))?;
    let signature = Signature::from_der(&signature_bytes)
        .map_err(|e| PdsError::Authentication(format!("Invalid signature format: {}", e)))?;

    // Verify signature
    let signing_input = format!("{}.{}", header_b64, claims_b64);
    verifying_key
        .verify(signing_input.as_bytes(), &signature)
        .map_err(|e| PdsError::Authentication(format!("Signature verification failed: {}", e)))?;

    // Validate claims (expiration, etc.)
    claims.validate()?;

    Ok(claims)
}

/// Decode a multibase-encoded public key
///
/// ATProto uses multibase encoding (specifically base58btc) for public keys.
/// Format: z<base58btc-encoded-key>
///
/// For secp256k1 keys:
/// - Multicodec prefix: 0xe7 (secp256k1-pub)
/// - Key bytes: 33 bytes (compressed) or 65 bytes (uncompressed)
#[allow(dead_code)] // Used by verify_service_jwt and tests
fn decode_multibase_key(multibase_key: &str) -> PdsResult<Vec<u8>> {
    // Check for 'z' prefix (base58btc)
    if !multibase_key.starts_with('z') {
        return Err(PdsError::Authentication(
            "Invalid multibase key: must start with 'z' (base58btc)".to_string(),
        ));
    }

    // Decode base58btc (strip 'z' prefix)
    let encoded = &multibase_key[1..];
    let decoded = bs58::decode(encoded)
        .into_vec()
        .map_err(|e| PdsError::Authentication(format!("Failed to decode base58: {}", e)))?;

    // Check multicodec prefix for secp256k1-pub (0xe7)
    if decoded.is_empty() {
        return Err(PdsError::Authentication("Empty key data".to_string()));
    }

    // For secp256k1-pub, the multicodec is 0xe7 (231 in decimal)
    // However, the encoding uses varint, so we need to check for the varint-encoded value
    // 0xe7 in varint is: 0xe7, 0x01 (two bytes)
    if decoded[0] == 0xe7 && decoded.len() > 1 {
        // This is a secp256k1 key, skip the multicodec prefix (2 bytes for varint)
        let skip_bytes = if decoded[1] == 0x01 { 2 } else { 1 };
        Ok(decoded[skip_bytes..].to_vec())
    } else if decoded[0] == 0xe7 {
        // Single byte prefix
        Ok(decoded[1..].to_vec())
    } else {
        // Try without prefix (some implementations may not include it)
        Ok(decoded)
    }
}

/// Generate a random nonce for replay protection
///
/// Creates a cryptographically secure random 16-byte nonce encoded as hex.
/// This can be used as the `jti` (JWT ID) claim to prevent replay attacks.
///
/// # Returns
///
/// A 32-character hex string
///
/// # Example
///
/// ```
/// use aurora_locus::service_auth::generate_nonce;
///
/// let nonce = generate_nonce();
/// assert_eq!(nonce.len(), 32); // 16 bytes = 32 hex chars
/// ```
#[allow(dead_code)] // Public utility function for service auth
pub fn generate_nonce() -> String {
    use rand::Rng;
    let bytes: [u8; 16] = rand::thread_rng().gen();
    hex::encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use k256::ecdsa::SigningKey;

    #[test]
    fn test_service_auth_claims_validation() {
        let now = Utc::now().timestamp();

        // Valid claims with method
        let claims = ServiceAuthClaims {
            iss: "did:plc:issuer".to_string(),
            aud: "did:plc:audience".to_string(),
            exp: now + 300, // 5 minutes
            lxm: Some("com.atproto.repo.createRecord".to_string()),
            jti: None,
        };
        assert!(claims.validate().is_ok());

        // Valid claims without method
        let claims = ServiceAuthClaims {
            iss: "did:plc:issuer".to_string(),
            aud: "did:plc:audience".to_string(),
            exp: now + 30, // 30 seconds
            lxm: None,
            jti: None,
        };
        assert!(claims.validate().is_ok());

        // Expired claims
        let claims = ServiceAuthClaims {
            iss: "did:plc:issuer".to_string(),
            aud: "did:plc:audience".to_string(),
            exp: now - 60, // 1 minute ago
            lxm: None,
            jti: None,
        };
        assert!(claims.validate().is_err());

        // Expiration too far without method (> 1 minute)
        let claims = ServiceAuthClaims {
            iss: "did:plc:issuer".to_string(),
            aud: "did:plc:audience".to_string(),
            exp: now + 120, // 2 minutes
            lxm: None,
            jti: None,
        };
        assert!(claims.validate().is_err());

        // Expiration too far with method (> 1 hour)
        let claims = ServiceAuthClaims {
            iss: "did:plc:issuer".to_string(),
            aud: "did:plc:audience".to_string(),
            exp: now + 7200, // 2 hours
            lxm: Some("com.atproto.repo.createRecord".to_string()),
            jti: None,
        };
        assert!(claims.validate().is_err());
    }

    #[test]
    fn test_generate_nonce() {
        let nonce1 = generate_nonce();
        let nonce2 = generate_nonce();

        // Nonces should be 32 hex characters (16 bytes)
        assert_eq!(nonce1.len(), 32);
        assert_eq!(nonce2.len(), 32);

        // Nonces should be different
        assert_ne!(nonce1, nonce2);

        // Nonces should be valid hex
        assert!(hex::decode(&nonce1).is_ok());
        assert!(hex::decode(&nonce2).is_ok());
    }

    #[test]
    fn test_create_service_jwt_basic() {
        // Generate a test signing key
        let signing_key = SigningKey::random(&mut rand::thread_rng());
        let key_bytes = signing_key.to_bytes();

        // Create JWT with default expiration
        let result = create_service_jwt(
            "did:plc:issuer123",
            "did:plc:audience456",
            None,
            None,
            &key_bytes,
        );

        assert!(result.is_ok());
        let token = result.unwrap();

        // JWT should have 3 parts separated by dots
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);
    }

    #[test]
    fn test_create_service_jwt_with_method() {
        let signing_key = SigningKey::random(&mut rand::thread_rng());
        let key_bytes = signing_key.to_bytes();

        // Create JWT with lexicon method
        let result = create_service_jwt(
            "did:plc:issuer123",
            "did:plc:audience456",
            Some(300), // 5 minutes
            Some("com.atproto.repo.createRecord"),
            &key_bytes,
        );

        assert!(result.is_ok());
    }

    #[test]
    fn test_create_service_jwt_expiration_too_long() {
        let signing_key = SigningKey::random(&mut rand::thread_rng());
        let key_bytes = signing_key.to_bytes();

        // Try to create JWT with expiration too far in future (no method)
        let result = create_service_jwt(
            "did:plc:issuer123",
            "did:plc:audience456",
            Some(120), // 2 minutes without method (max is 1 minute)
            None,
            &key_bytes,
        );

        assert!(result.is_err());
    }

    #[test]
    fn test_decode_multibase_key_valid() {
        // Example multibase key (base58btc with 'z' prefix)
        // This is a synthetic example - in reality this would be a properly encoded key
        let key_bytes = vec![0x02; 33]; // Compressed secp256k1 public key (33 bytes)

        // Add multicodec prefix for secp256k1-pub (0xe7)
        let mut with_prefix = vec![0xe7];
        with_prefix.extend_from_slice(&key_bytes);

        // Encode as base58btc
        let encoded = bs58::encode(&with_prefix).into_string();
        let multibase = format!("z{}", encoded);

        let result = decode_multibase_key(&multibase);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), key_bytes);
    }

    #[test]
    fn test_decode_multibase_key_invalid_prefix() {
        // Invalid prefix (not 'z')
        let result = decode_multibase_key("a123456");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("must start with 'z'"));
    }

    #[test]
    fn test_decode_multibase_key_invalid_base58() {
        // Invalid base58 characters
        let result = decode_multibase_key("z0OIl"); // 0, O, I, l are not valid base58
        assert!(result.is_err());
    }
}
