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
    identity::resolver::IdentityResolverApi,
};

/// Structured failure modes from `verify_service_jwt`. Each variant
/// carries enough context for callers (specifically the
/// `AdminAuthContext` extractor) to emit a per-cause log line per
/// §5.3.5 without scraping error message strings. `Display` is
/// non-leaky — it doesn't include the token, signing key, or any
/// scripted resolver internals.
///
/// Step 2 introduces this so the extractor can distinguish audience
/// mismatch from expired-token from signature-verification failure
/// without parsing message prefixes (the §5.4.2 "Acceptable" path
/// would have left a brittle `// TODO(v0.4)` string-match marker).
#[derive(Debug, Clone)]
pub enum ServiceAuthError {
    /// Token couldn't be split into three dot-separated segments, the
    /// header isn't valid base64url, or the header isn't valid JSON.
    NotJwtShaped(String),
    /// Header lacks an `alg` field, or `alg` isn't a string.
    MissingOrInvalidAlg,
    /// Header's `alg` is a string but isn't `ES256K`.
    UnsupportedAlg(String),
    /// Claims segment couldn't be decoded or parsed.
    InvalidClaims(String),
    /// `aud` claim doesn't byte-equal `expected_aud`.
    AudienceMismatch { expected: String, received: String },
    /// `identity_resolver.resolve_did(...)` returned an error.
    ResolverError(String),
    /// DID document had no `#atproto` verification method, or the
    /// public key was malformed / multibase decode failed.
    InvalidPublicKey(String),
    /// Signature segment couldn't be base64-decoded, or the DER
    /// envelope was malformed.
    InvalidSignatureFormat(String),
    /// ES256K signature did not verify against the resolved key.
    SignatureVerificationFailed,
    /// `exp` claim is in the past.
    Expired,
    /// `exp` claim is too far in the future for the token's lxm
    /// presence/absence, per `ServiceAuthClaims::validate`.
    InvalidExpirationWindow(String),
    /// `identity_resolver.resolve_did(...)` returned typed
    /// `PdsError::DidTombstoned` (PLC-410 per Arc 13 v4.2 /
    /// `src/identity/resolver.rs::fetch_plc_document`). Cluster 2
    /// Member 2.2 (#144): split out from `ResolverError` so the
    /// `From<ServiceAuthError> for PdsError` impl below routes this
    /// typed variant to `PdsError::DidTombstoned` (→ HTTP 400) rather
    /// than the catch-all `PdsError::Authentication` (→ HTTP 401).
    /// The detail is the DID string from the resolver's typed payload.
    DidTombstoned(String),
}

impl std::fmt::Display for ServiceAuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotJwtShaped(detail) => write!(f, "JWT format invalid: {}", detail),
            Self::MissingOrInvalidAlg => write!(f, "Missing or non-string alg in JWT header"),
            Self::UnsupportedAlg(alg) => write!(f, "Unsupported algorithm: {}", alg),
            Self::InvalidClaims(detail) => write!(f, "JWT claims invalid: {}", detail),
            Self::AudienceMismatch { expected, received } => write!(
                f,
                "Invalid audience: expected {}, got {}",
                expected, received
            ),
            Self::ResolverError(detail) => write!(f, "Failed to resolve issuer DID: {}", detail),
            Self::InvalidPublicKey(detail) => write!(f, "Invalid public key: {}", detail),
            Self::InvalidSignatureFormat(detail) => {
                write!(f, "Invalid signature format: {}", detail)
            }
            Self::SignatureVerificationFailed => write!(f, "Signature verification failed"),
            Self::Expired => write!(f, "Service auth token has expired"),
            Self::InvalidExpirationWindow(detail) => write!(f, "Invalid expiration: {}", detail),
            Self::DidTombstoned(did) => write!(f, "Issuer DID tombstoned: {}", did),
        }
    }
}

impl std::error::Error for ServiceAuthError {}

/// Lift to the project-wide error vocabulary so the extractor's
/// `PdsResult<...>` callsites can return `ServiceAuthError` cases as
/// 401 authentication failures while still preserving the structured
/// cause for logging at the dispatch site.
impl From<ServiceAuthError> for PdsError {
    fn from(e: ServiceAuthError) -> Self {
        match e {
            // Shape-level rejections are reported as Jwt parse errors
            // to mirror the pre-Step-2 behavior of this function.
            ServiceAuthError::NotJwtShaped(_)
            | ServiceAuthError::MissingOrInvalidAlg
            | ServiceAuthError::UnsupportedAlg(_)
            | ServiceAuthError::InvalidClaims(_)
            | ServiceAuthError::InvalidSignatureFormat(_) => PdsError::Jwt(e.to_string()),
            // Validation-window violations stay as Validation errors
            // (matching `ServiceAuthClaims::validate`).
            ServiceAuthError::InvalidExpirationWindow(_) => PdsError::Validation(e.to_string()),
            // Cluster 2 Member 2.2 (#144) — LOAD-BEARING. Route the
            // typed DidTombstoned variant to PdsError::DidTombstoned
            // (→ HTTP 400 `{"error": "DidTombstoned", ...}` via
            // src/error.rs:620-624) BEFORE the catch-all below.
            // Omitting this arm — or placing it after the catch-all —
            // silently defeats the entire #144 fix: the new variant
            // falls through to PdsError::Authentication → HTTP 401
            // opaque, the typed wire shape never reaches IntoResponse.
            // The verifier paths (federation/service_auth.rs and the
            // free-fn `verify_service_jwt` in this file) emit the
            // typed variant so the typed mapping has somewhere to land.
            ServiceAuthError::DidTombstoned(did) => PdsError::DidTombstoned(did),
            // Everything else is an authentication failure (401).
            _ => PdsError::Authentication(e.to_string()),
        }
    }
}

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
    // Build the signing input (header + claims), then sign it in-process with
    // the substrate-held key. The did:web holder-mediated path
    // (`mint_service_jwt_via_holder`) reuses the SAME builder + assembler so the
    // two emit an identical JWT format; only the signature source differs.
    let signing_input = service_jwt_signing_input(iss, aud, exp_seconds, lxm)?;

    let signing_key = SigningKey::from_slice(signing_key)
        .map_err(|e| PdsError::Authentication(format!("Invalid signing key: {}", e)))?;
    let signature: Signature = signing_key.sign(signing_input.as_bytes());
    Ok(assemble_service_jwt(&signing_input, signature.to_der().as_bytes()))
}

/// Build a service-auth JWT's **signing input** — `base64url(header) + "." +
/// base64url(claims)` — the exact bytes that get signed.
///
/// Shared by the in-process signer ([`create_service_jwt`], did:plc) and the
/// holder-mediated signer (`mint_service_jwt_via_holder`, did:web, Phase δ), so
/// both produce a byte-identical JWT format and only the *source* of the
/// signature differs. Enforces the same expiration window (`claims.validate`).
pub fn service_jwt_signing_input(
    iss: &str,
    aud: &str,
    exp_seconds: Option<i64>,
    lxm: Option<&str>,
) -> PdsResult<String> {
    let now = Utc::now().timestamp();
    let default_exp = if lxm.is_some() {
        MAX_EXP_WITH_METHOD
    } else {
        MAX_EXP_WITHOUT_METHOD
    };
    let exp = now + exp_seconds.unwrap_or(default_exp);

    let claims = ServiceAuthClaims {
        iss: iss.to_string(),
        aud: aud.to_string(),
        exp,
        lxm: lxm.map(|s| s.to_string()),
        jti: None, // Can be added if replay protection is needed
    };
    claims.validate()?;

    // JWT is hand-rolled because jsonwebtoken doesn't support ES256K.
    let header = serde_json::json!({ "alg": "ES256K", "typ": "JWT" });
    let header_json = serde_json::to_string(&header)
        .map_err(|e| PdsError::Jwt(format!("Failed to serialize header: {}", e)))?;
    let header_b64 = URL_SAFE_NO_PAD.encode(header_json.as_bytes());

    let claims_json = serde_json::to_string(&claims)
        .map_err(|e| PdsError::Jwt(format!("Failed to serialize claims: {}", e)))?;
    let claims_b64 = URL_SAFE_NO_PAD.encode(claims_json.as_bytes());

    Ok(format!("{}.{}", header_b64, claims_b64))
}

/// Assemble the final JWT string from a signing input and the DER-encoded
/// signature bytes: `signing_input + "." + base64url(signature_der)`.
pub fn assemble_service_jwt(signing_input: &str, signature_der: &[u8]) -> String {
    format!("{}.{}", signing_input, URL_SAFE_NO_PAD.encode(signature_der))
}

/// Re-encode a 64-byte compact `R‖S` ES256K signature (the shape a holder
/// returns over [`crate::holder_signing::HolderSigningChannel`]) into the DER
/// form the service-auth / entryway JWT wire format uses — `verify_service_jwt`
/// decodes signatures with `Signature::from_der`.
pub fn compact_es256k_to_der(compact: &[u8]) -> PdsResult<Vec<u8>> {
    let signature = Signature::from_slice(compact).map_err(|e| {
        PdsError::Jwt(format!("holder returned an invalid compact ES256K signature: {}", e))
    })?;
    Ok(signature.to_der().as_bytes().to_vec())
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
/// use aurora_locus::identity::resolver::IdentityResolverApi;
///
/// # async fn example(resolver: &dyn IdentityResolverApi) -> Result<(), Box<dyn std::error::Error>> {
/// let token = "eyJ..."; // JWT token from request
/// let claims = verify_service_jwt(
///     token,
///     "did:plc:myservice123",
///     resolver,
/// ).await?;
///
/// println!("Authenticated request from: {}", claims.iss);
/// # Ok(())
/// # }
/// ```
pub async fn verify_service_jwt(
    token: &str,
    expected_aud: &str,
    identity_resolver: &dyn IdentityResolverApi,
) -> Result<ServiceAuthClaims, ServiceAuthError> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(ServiceAuthError::NotJwtShaped(
            "expected 3 dot-separated segments".to_string(),
        ));
    }

    let (header_b64, claims_b64, signature_b64) = (parts[0], parts[1], parts[2]);

    let header_bytes = URL_SAFE_NO_PAD
        .decode(header_b64)
        .map_err(|e| ServiceAuthError::NotJwtShaped(format!("header decode failed: {}", e)))?;
    let header: serde_json::Value = serde_json::from_slice(&header_bytes)
        .map_err(|e| ServiceAuthError::NotJwtShaped(format!("header parse failed: {}", e)))?;

    let alg = header
        .get("alg")
        .and_then(|v| v.as_str())
        .ok_or(ServiceAuthError::MissingOrInvalidAlg)?;

    if alg != "ES256K" {
        return Err(ServiceAuthError::UnsupportedAlg(alg.to_string()));
    }

    let claims_bytes = URL_SAFE_NO_PAD
        .decode(claims_b64)
        .map_err(|e| ServiceAuthError::InvalidClaims(format!("decode failed: {}", e)))?;
    let claims: ServiceAuthClaims = serde_json::from_slice(&claims_bytes)
        .map_err(|e| ServiceAuthError::InvalidClaims(format!("parse failed: {}", e)))?;

    if claims.aud != expected_aud {
        return Err(ServiceAuthError::AudienceMismatch {
            expected: expected_aud.to_string(),
            received: claims.aud.clone(),
        });
    }

    // Cluster 2 Member 2.2 (#144): pattern-match the resolver error so
    // PdsError::DidTombstoned (PLC-410 → typed variant per Arc 13 v4.2
    // / fetch_plc_document) reaches the From<ServiceAuthError> for
    // PdsError impl via the typed DidTombstoned variant rather than
    // being stringified into ResolverError → catch-all → HTTP 401
    // opaque. Non-tombstone resolver errors preserve today's
    // ResolverError(e.to_string()) shape byte-identical (the surface
    // is tracing-only — not grep'd by any test/runbook/metric).
    let did_doc = identity_resolver
        .resolve_did(&claims.iss)
        .await
        .map_err(|e| match e {
            PdsError::DidTombstoned(did) => ServiceAuthError::DidTombstoned(did),
            other => ServiceAuthError::ResolverError(other.to_string()),
        })?;

    let verification_method = did_doc.get_signing_key().ok_or_else(|| {
        ServiceAuthError::InvalidPublicKey(format!(
            "no #atproto verification method on DID document for {}",
            claims.iss
        ))
    })?;

    let public_key_bytes = verification_method
        .public_key_multibase
        .as_ref()
        .ok_or_else(|| {
            ServiceAuthError::InvalidPublicKey(
                "verification method missing publicKeyMultibase".to_string(),
            )
        })?;

    let public_key_decoded = decode_multibase_key(public_key_bytes)
        .map_err(|e| ServiceAuthError::InvalidPublicKey(e.to_string()))?;

    let verifying_key = VerifyingKey::from_sec1_bytes(&public_key_decoded)
        .map_err(|e| ServiceAuthError::InvalidPublicKey(e.to_string()))?;

    let signature_bytes = URL_SAFE_NO_PAD.decode(signature_b64).map_err(|e| {
        ServiceAuthError::InvalidSignatureFormat(format!("base64 decode failed: {}", e))
    })?;
    let signature = Signature::from_der(&signature_bytes)
        .map_err(|e| ServiceAuthError::InvalidSignatureFormat(format!("DER parse failed: {}", e)))?;

    let signing_input = format!("{}.{}", header_b64, claims_b64);
    verifying_key
        .verify(signing_input.as_bytes(), &signature)
        .map_err(|_| ServiceAuthError::SignatureVerificationFailed)?;

    // Validate claims (expiration window). `claims.validate` returns
    // PdsError variants; lift them into ServiceAuthError so the
    // structured-cause contract holds end-to-end.
    match claims.validate() {
        Ok(()) => Ok(claims),
        Err(PdsError::Authentication(_)) => Err(ServiceAuthError::Expired),
        Err(PdsError::Validation(msg)) => Err(ServiceAuthError::InvalidExpirationWindow(msg)),
        // `ServiceAuthClaims::validate` doesn't construct any other
        // PdsError variant, but be defensive in case it grows one.
        Err(other) => Err(ServiceAuthError::InvalidExpirationWindow(other.to_string())),
    }
}

/// Decode a multibase-encoded public key
///
/// ATProto uses multibase encoding (specifically base58btc) for public keys.
/// Format: z<base58btc-encoded-key>
///
/// For secp256k1 keys:
/// - Multicodec prefix: 0xe7 (secp256k1-pub)
/// - Key bytes: 33 bytes (compressed) or 65 bytes (uncompressed)
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

    /// Cluster 2 Member 2.1 (chainlink #143). Per-account-key
    /// correctness: when `create_service_jwt` is given the bytes of
    /// per-account key K_did (the value
    /// `AccountManager::get_atproto_signing_key_bytes(did)` returns),
    /// the resulting JWT signature MUST verify against the public key
    /// derived from K_did — and MUST NOT verify against an
    /// unrelated key (e.g. the server-wide `repo_signing_key` the
    /// pre-#143 handler used). This is the invariant
    /// `src/api/server.rs::get_service_auth` depends on post-#143:
    /// the handler reads per-account bytes, calls create_service_jwt,
    /// and a receiving PDS resolving `iss = auth.did` fetches the
    /// per-account published verification method (derived on
    /// account-create from the same K_did's public half) and verifies
    /// the JWT successfully.
    ///
    /// True unit test: the expected verifying key is derived LOCALLY
    /// from the seeded private key — no DID resolution, no mock
    /// resolver, no PDS roundtrip. The "different key MUST NOT
    /// verify" half is what proves the test isn't trivially
    /// passing; it's the test that catches the pre-#143
    /// server-wide-key bug (in pre-fix code the JWT would carry the
    /// server-wide signature and verify against the server-wide
    /// verifying key, NOT K_did's — that's the bug).
    #[test]
    fn create_service_jwt_signature_verifies_against_per_account_key_only() {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};

        // K_did — the per-account atproto_signing_key bytes the
        // post-#143 get_service_auth handler would hand to
        // create_service_jwt via
        // ctx.account_manager.get_atproto_signing_key_bytes(&auth.did).
        let per_account_signing_key = SigningKey::random(&mut rand::thread_rng());
        let per_account_bytes = per_account_signing_key.to_bytes();
        let per_account_verifying_key = *per_account_signing_key.verifying_key();

        // Unrelated key — stands in for the pre-#143 server-wide
        // `repo_signing_key`. Verifies the test would fail to false-
        // green if the handler regressed back to a non-per-account
        // key.
        let unrelated_signing_key = SigningKey::random(&mut rand::thread_rng());
        let unrelated_verifying_key = *unrelated_signing_key.verifying_key();

        let token = create_service_jwt(
            "did:plc:issuer-per-account-test",
            "did:plc:audience-per-account-test",
            None,
            None,
            &per_account_bytes,
        )
        .expect("create_service_jwt with per-account bytes must succeed");

        // Split header.claims.sig and reconstruct the signing input
        // (header_b64 || "." || claims_b64) — same format
        // create_service_jwt emits (src/service_auth.rs::~290).
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3, "JWT must have header.claims.sig");
        let signing_input = format!("{}.{}", parts[0], parts[1]);

        let sig_bytes = URL_SAFE_NO_PAD
            .decode(parts[2])
            .expect("signature segment must be valid base64url-no-pad");
        // create_service_jwt emits DER-encoded signatures
        // (`signature.to_der()` at the sign site); decode with the
        // matching DER constructor, NOT from_slice (which expects
        // fixed-length r||s).
        let signature = Signature::from_der(&sig_bytes)
            .expect("signature segment must be valid k256 ECDSA DER");

        // Positive half: per-account verifying key MUST verify.
        // `Verifier::verify` matches `Signer::sign`'s ES256K shape
        // (internal sha256 + secp256k1 ECDSA) — symmetric to the
        // sign call at create_service_jwt's signing line.
        per_account_verifying_key
            .verify(signing_input.as_bytes(), &signature)
            .expect(
                "JWT signature MUST verify against the verifying key derived from \
                 per-account signing bytes — Cluster 2 Member 2.1 (#143) invariant",
            );

        // Negative half: unrelated verifying key MUST NOT verify.
        // This is the load-bearing check — it proves the JWT carries
        // the per-account signature specifically, not a signature
        // that would coincidentally verify against any key. If this
        // assertion ever passes, create_service_jwt is signing with
        // the wrong key or the test is trivially-passing (false-green
        // for the pre-#143 server-wide-key bug shape).
        assert!(
            unrelated_verifying_key
                .verify(signing_input.as_bytes(), &signature)
                .is_err(),
            "JWT signature MUST NOT verify against an unrelated key"
        );
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

    /// Step 0.6 smoke test — proves the test-friendly resolver is
    /// callable from this module's test scope and that its invocation
    /// counter is observable. Step 1's algorithm-confusion tests will
    /// rely on this counter reading zero on the rejection path; this
    /// test only proves the counter increments on a real call.
    #[tokio::test]
    async fn mock_identity_resolver_invocation_counter_increments_on_call() {
        use crate::identity::resolver::test_doubles::MockIdentityResolver;
        use crate::identity::resolver::IdentityResolverApi;

        let mock = MockIdentityResolver::new();
        assert_eq!(mock.resolve_did_calls(), 0);
        let _ = mock.resolve_did("did:plc:nonexistent").await;
        assert_eq!(mock.resolve_did_calls(), 1);
    }
}

#[cfg(test)]
mod verify_service_jwt_tests {
    //! Step 1 (§5.4.1): activation tests for `verify_service_jwt`.
    //!
    //! The algorithm-confusion tests are load-bearing. Per the design
    //! doc, if the resolver is reached on any of the malformed-alg
    //! cases below, the security boundary leaked and the design needs
    //! revision — counter assertions read zero are the contract.
    //!
    //! `verify_service_jwt` calls `resolver.resolve_did(...)` and then
    //! reads the signing key off the returned `DidDocument` directly
    //! (it does NOT call `resolver.get_signing_key(...)`). So
    //! `get_signing_key_calls()` is always 0 in these tests; the
    //! load-bearing counter is `resolve_did_calls()`.
    use super::*;
    use crate::identity::did_document::{DidDocument, VerificationMethod};
    use crate::identity::resolver::test_doubles::MockIdentityResolver;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use hmac::{Hmac, Mac};
    use k256::ecdsa::{SigningKey, VerifyingKey};
    use sha2::Sha256;

    type HmacSha256 = Hmac<Sha256>;

    const TEST_AUD: &str = "did:plc:audience";
    const TEST_ISS: &str = "did:plc:test1234";

    /// Encode a verifying key as multibase per ATProto: `z` prefix,
    /// base58btc encoding of `[0xe7, 0x01]` multicodec varint
    /// (secp256k1-pub) + compressed SEC1 public-key bytes.
    fn multibase_encode(verifying_key: &VerifyingKey) -> String {
        let sec1 = verifying_key.to_encoded_point(true); // compressed
        let mut buf = vec![0xe7_u8, 0x01_u8];
        buf.extend_from_slice(sec1.as_bytes());
        format!("z{}", bs58::encode(&buf).into_string())
    }

    /// Build a synthetic DID document whose `#atproto` verification
    /// method carries the given verifying key in multibase form.
    fn did_doc_with_key(did: &str, verifying_key: &VerifyingKey) -> DidDocument {
        DidDocument {
            context: None,
            id: did.to_string(),
            also_known_as: vec![],
            service: vec![],
            verification_method: vec![VerificationMethod {
                id: format!("{}#atproto", did),
                key_type: "Multikey".to_string(),
                controller: did.to_string(),
                public_key_multibase: Some(multibase_encode(verifying_key)),
            }],
        }
    }

    /// Manually assemble a JWT from raw header + claims JSON strings
    /// and a raw signature byte slice. Used for malformed-header
    /// negative tests where `create_service_jwt` would refuse to emit
    /// the shape we want to attack.
    fn manual_jwt(header_json: &str, claims_json: &str, signature: &[u8]) -> String {
        let header_b64 = URL_SAFE_NO_PAD.encode(header_json.as_bytes());
        let claims_b64 = URL_SAFE_NO_PAD.encode(claims_json.as_bytes());
        let sig_b64 = URL_SAFE_NO_PAD.encode(signature);
        format!("{}.{}.{}", header_b64, claims_b64, sig_b64)
    }

    fn well_formed_claims_json(iss: &str, aud: &str, exp: i64) -> String {
        serde_json::to_string(&ServiceAuthClaims {
            iss: iss.to_string(),
            aud: aud.to_string(),
            exp,
            lxm: None,
            jti: None,
        })
        .unwrap()
    }

    /// Round-trip happy path: generate keypair, script the resolver
    /// with a matching DID document, sign a real JWT, verify, assert
    /// claims round-trip and the resolver was actually consulted.
    #[tokio::test]
    async fn round_trip_passes_with_matching_signing_key() {
        let signing_key = SigningKey::random(&mut rand::thread_rng());
        let verifying_key = *signing_key.verifying_key();
        let key_bytes = signing_key.to_bytes();

        let token = create_service_jwt(TEST_ISS, TEST_AUD, Some(60), None, &key_bytes)
            .expect("create_service_jwt happy path");

        let mock = MockIdentityResolver::new();
        mock.script_did(TEST_ISS, did_doc_with_key(TEST_ISS, &verifying_key));

        let claims = verify_service_jwt(&token, TEST_AUD, &mock)
            .await
            .expect("happy path verifies");

        assert_eq!(claims.iss, TEST_ISS);
        assert_eq!(claims.aud, TEST_AUD);
        assert!(
            mock.resolve_did_calls() >= 1,
            "happy path must reach resolver — otherwise verification short-circuited"
        );
        // `verify_service_jwt` reads the key off the DidDocument
        // directly rather than calling resolver.get_signing_key();
        // pin that fact as a contract.
        assert_eq!(mock.get_signing_key_calls(), 0);
    }

    // ---------- Algorithm-confusion negative tests (Q8 hypothesis) ----------
    //
    // Asserts: Err result + BOTH counters at zero. Failure here means
    // the algorithm boundary leaked — STOP and report per §5.4.1.

    #[tokio::test]
    async fn rejects_alg_none_without_reaching_resolver() {
        let header = r#"{"alg":"none","typ":"JWT"}"#;
        let claims = well_formed_claims_json(TEST_ISS, TEST_AUD, future_exp());
        let token = manual_jwt(header, &claims, b"");

        let mock = MockIdentityResolver::new();
        let result = verify_service_jwt(&token, TEST_AUD, &mock).await;

        assert!(result.is_err(), "alg:none must be rejected");
        assert_eq!(
            mock.resolve_did_calls(),
            0,
            "alg-rejection path called resolve_did — algorithm boundary leaked"
        );
        assert_eq!(
            mock.get_signing_key_calls(),
            0,
            "alg-rejection path called get_signing_key — algorithm boundary leaked"
        );
    }

    #[tokio::test]
    async fn rejects_hs256_signed_with_es256k_pubkey_as_secret_without_reaching_resolver() {
        // Classic algorithm-confusion attack: an attacker who knows
        // the issuer's public key forges an HS256 token using that
        // public key as the HMAC secret. If the verifier dispatched
        // verification by the header's alg, this would succeed.
        let signing_key = SigningKey::random(&mut rand::thread_rng());
        let verifying_key = *signing_key.verifying_key();
        let public_key_bytes = verifying_key.to_encoded_point(true).as_bytes().to_vec();

        let header = r#"{"alg":"HS256","typ":"JWT"}"#;
        let claims = well_formed_claims_json(TEST_ISS, TEST_AUD, future_exp());

        let header_b64 = URL_SAFE_NO_PAD.encode(header.as_bytes());
        let claims_b64 = URL_SAFE_NO_PAD.encode(claims.as_bytes());
        let signing_input = format!("{}.{}", header_b64, claims_b64);

        let mut mac = HmacSha256::new_from_slice(&public_key_bytes)
            .expect("HMAC accepts arbitrary key length");
        mac.update(signing_input.as_bytes());
        let sig = mac.finalize().into_bytes();
        let sig_b64 = URL_SAFE_NO_PAD.encode(sig);
        let token = format!("{}.{}.{}", header_b64, claims_b64, sig_b64);

        // Script the resolver with the matching DID document so that
        // IF the alg boundary leaked, the attacker's token would
        // actually verify against the (HS256-shaped) signature. The
        // counter assertions catch the leak even if downstream code
        // were to spuriously accept.
        let mock = MockIdentityResolver::new();
        mock.script_did(TEST_ISS, did_doc_with_key(TEST_ISS, &verifying_key));

        let result = verify_service_jwt(&token, TEST_AUD, &mock).await;

        assert!(result.is_err(), "alg:HS256 must be rejected");
        assert_eq!(
            mock.resolve_did_calls(),
            0,
            "alg-rejection path called resolve_did — algorithm boundary leaked"
        );
        assert_eq!(
            mock.get_signing_key_calls(),
            0,
            "alg-rejection path called get_signing_key — algorithm boundary leaked"
        );
    }

    #[tokio::test]
    async fn rejects_missing_alg_field_without_reaching_resolver() {
        let header = r#"{"typ":"JWT"}"#;
        let claims = well_formed_claims_json(TEST_ISS, TEST_AUD, future_exp());
        let token = manual_jwt(header, &claims, b"junk-signature-bytes");

        let mock = MockIdentityResolver::new();
        let result = verify_service_jwt(&token, TEST_AUD, &mock).await;

        assert!(result.is_err(), "missing alg must be rejected");
        assert_eq!(
            mock.resolve_did_calls(),
            0,
            "alg-rejection path called resolve_did — algorithm boundary leaked"
        );
        assert_eq!(
            mock.get_signing_key_calls(),
            0,
            "alg-rejection path called get_signing_key — algorithm boundary leaked"
        );
    }

    #[tokio::test]
    async fn rejects_non_string_alg_without_reaching_resolver() {
        let header = r#"{"alg":123,"typ":"JWT"}"#;
        let claims = well_formed_claims_json(TEST_ISS, TEST_AUD, future_exp());
        let token = manual_jwt(header, &claims, b"junk-signature-bytes");

        let mock = MockIdentityResolver::new();
        let result = verify_service_jwt(&token, TEST_AUD, &mock).await;

        assert!(result.is_err(), "non-string alg must be rejected");
        assert_eq!(
            mock.resolve_did_calls(),
            0,
            "alg-rejection path called resolve_did — algorithm boundary leaked"
        );
        assert_eq!(
            mock.get_signing_key_calls(),
            0,
            "alg-rejection path called get_signing_key — algorithm boundary leaked"
        );
    }

    // ---------- Resolver-error-path tests ----------

    #[tokio::test]
    async fn propagates_resolver_not_found_error() {
        let signing_key = SigningKey::random(&mut rand::thread_rng());
        let token = create_service_jwt(TEST_ISS, TEST_AUD, Some(60), None, &signing_key.to_bytes())
            .unwrap();

        let mock = MockIdentityResolver::new();
        mock.script_did_error(TEST_ISS, "DID not found");

        let result = verify_service_jwt(&token, TEST_AUD, &mock).await;

        assert!(result.is_err(), "NotFound must propagate as Err");
        assert!(
            mock.resolve_did_calls() >= 1,
            "NotFound path must reach resolver before failing"
        );
    }

    #[tokio::test]
    async fn propagates_resolver_network_error() {
        let signing_key = SigningKey::random(&mut rand::thread_rng());
        let token = create_service_jwt(TEST_ISS, TEST_AUD, Some(60), None, &signing_key.to_bytes())
            .unwrap();

        let mock = MockIdentityResolver::new();
        mock.script_did_error(TEST_ISS, "connection refused");

        let result = verify_service_jwt(&token, TEST_AUD, &mock).await;

        assert!(result.is_err(), "network error must propagate as Err");
        assert!(
            mock.resolve_did_calls() >= 1,
            "network-error path must reach resolver before failing"
        );
    }

    // ---------- Standard negative tests ----------

    #[tokio::test]
    async fn rejects_audience_mismatch_before_reaching_resolver() {
        let signing_key = SigningKey::random(&mut rand::thread_rng());
        let key_bytes = signing_key.to_bytes();
        // Token's `aud` is set to TEST_AUD via create_service_jwt.
        let token = create_service_jwt(TEST_ISS, TEST_AUD, Some(60), None, &key_bytes).unwrap();

        let mock = MockIdentityResolver::new();
        // Don't script anything — if the resolver were called, this
        // test would fail to verify even on the audience-mismatch
        // path. The counter assertion is the load-bearing check.

        let result = verify_service_jwt(&token, "did:plc:wrongAudience", &mock).await;

        assert!(result.is_err(), "audience mismatch must be rejected");
        // Per Q11 / source-ordering: audience check (line 300-306)
        // happens BEFORE resolver invocation (line 308-312).
        assert_eq!(
            mock.resolve_did_calls(),
            0,
            "audience check must reject before resolver invocation"
        );
    }

    #[tokio::test]
    async fn rejects_expired_token_after_reaching_resolver() {
        // The expiry check is at the END of verify_service_jwt
        // (after signature verification). Building a token with a
        // past `exp` directly through create_service_jwt is blocked
        // by claims.validate(); construct the token manually with
        // the real ES256K signature so the path threads through
        // resolver + signature verify and only fails at the final
        // expiry check.
        use k256::ecdsa::{signature::Signer, Signature};

        let signing_key = SigningKey::random(&mut rand::thread_rng());
        let verifying_key = *signing_key.verifying_key();

        let header = r#"{"alg":"ES256K","typ":"JWT"}"#;
        let claims = well_formed_claims_json(TEST_ISS, TEST_AUD, past_exp());
        let header_b64 = URL_SAFE_NO_PAD.encode(header.as_bytes());
        let claims_b64 = URL_SAFE_NO_PAD.encode(claims.as_bytes());
        let signing_input = format!("{}.{}", header_b64, claims_b64);
        let sig: Signature = signing_key.sign(signing_input.as_bytes());
        let sig_b64 = URL_SAFE_NO_PAD.encode(sig.to_der().as_bytes());
        let token = format!("{}.{}.{}", header_b64, claims_b64, sig_b64);

        let mock = MockIdentityResolver::new();
        mock.script_did(TEST_ISS, did_doc_with_key(TEST_ISS, &verifying_key));

        let result = verify_service_jwt(&token, TEST_AUD, &mock).await;

        assert!(result.is_err(), "expired token must be rejected");
        // Source-ordering contract: expiry is checked AFTER resolver
        // and signature verify, so the resolver must have been
        // invoked.
        assert!(
            mock.resolve_did_calls() >= 1,
            "expired path is reached only after resolver invocation"
        );
    }

    #[tokio::test]
    async fn rejects_corrupted_signature_after_reaching_resolver() {
        let signing_key = SigningKey::random(&mut rand::thread_rng());
        let verifying_key = *signing_key.verifying_key();
        let token = create_service_jwt(TEST_ISS, TEST_AUD, Some(60), None, &signing_key.to_bytes())
            .unwrap();

        // Flip the last byte of the base64-encoded signature segment
        // to corrupt the signature without breaking JWT structure.
        let mut parts: Vec<&str> = token.split('.').collect();
        let mut sig_bytes = URL_SAFE_NO_PAD.decode(parts[2]).unwrap();
        let last_idx = sig_bytes.len() - 1;
        sig_bytes[last_idx] ^= 0xff;
        let corrupted_sig_b64 = URL_SAFE_NO_PAD.encode(&sig_bytes);
        parts[2] = &corrupted_sig_b64;
        let corrupted_token = parts.join(".");

        let mock = MockIdentityResolver::new();
        mock.script_did(TEST_ISS, did_doc_with_key(TEST_ISS, &verifying_key));

        let result = verify_service_jwt(&corrupted_token, TEST_AUD, &mock).await;

        assert!(result.is_err(), "corrupted signature must be rejected");
        // Source-ordering contract: signature verify happens AFTER
        // resolver invocation.
        assert!(
            mock.resolve_did_calls() >= 1,
            "signature-corrupted path is reached only after resolver invocation"
        );
        // verify_service_jwt does not call resolver.get_signing_key();
        // it reads the key off the DidDocument directly.
        assert_eq!(mock.get_signing_key_calls(), 0);
    }

    fn future_exp() -> i64 {
        Utc::now().timestamp() + 60
    }

    fn past_exp() -> i64 {
        Utc::now().timestamp() - 60
    }
}
