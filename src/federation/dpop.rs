//! DPoP (Demonstrating Proof of Possession) Support

// Allow dead_code - public APIs defined for future feature completion
#![allow(dead_code)]
//!
//! Implements ATProto DPoP specification for client-to-PDS authentication:
//! - Binds access tokens to specific client devices via public key cryptography
//! - Prevents token theft and replay attacks
//! - Requires clients to prove possession of private key on each request
//!
//! DPoP Proof JWT Format:
//! - Header: typ="dpop+jwt", alg=ES256, jwk={client's public key}
//! - Claims: jti (nonce), htm (HTTP method), htu (HTTP URI), iat, exp
//!
//! References:
//! - https://datatracker.ietf.org/doc/html/rfc9449
//! - https://atproto.com/specs/xrpc#dpop

use crate::distributed::{DistributedError, DistributedStore, Lease};
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
    /// JWT ID — client-generated, used for replay prevention. RFC
    /// 9449 §11.1: distinct from the server-issued nonce in §8 (which
    /// would carry a separate `nonce` claim and is not yet shipped on
    /// this PDS).
    pub jti: String,

    /// HTTP method (GET, POST, etc.)
    pub htm: String,

    /// HTTP URI (target endpoint)
    pub htu: String,

    /// Issued at (Unix timestamp)
    pub iat: i64,

    /// Expiration (Unix timestamp) - typically <60s
    pub exp: i64,

    /// Access-token hash (RFC 9449 §4.3) — `base64url(SHA-256(access_token))`.
    /// Required on resource-request DPoP proofs to bind the proof to
    /// the specific token presented; absent on token-issuance proofs
    /// (no access token exists yet).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ath: Option<String>,
}

/// JWK (JSON Web Key) representation from DPoP proof header
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Jwk {
    pub kty: String, // Key type (e.g., "EC")
    pub crv: String, // Curve (e.g., "P-256")
    pub x: String,   // X coordinate (base64url)
    pub y: String,   // Y coordinate (base64url)
}

/// DPoP nonce generator and JTI replay tracker.
///
/// The server-issued nonce flow (`generate_nonce` /
/// `check_and_consume_nonce`) stays in-memory regardless of
/// substrate mode — only the `/xrpc/com.atproto.federation.getDpopNonce`
/// federation endpoint issues those, and there's no
/// cross-instance correctness story for them in v0.4
/// (per Step 0 OQ3 / federation-scoped, out-of-arc).
///
/// The client-issued JTI replay flow (`check_and_record_jti`)
/// migrates to the distributed substrate when one is
/// configured (Arc 7 Step 3, V04_DESIGN.md §6.3.4). The
/// `distributed_store` field is populated by
/// `AppContext::new` in `Distributed` mode and left as `None`
/// in `SingleInstanceInmemory`. The `check_and_record_jti`
/// method dispatches accordingly.
pub struct DPopNonceStore {
    /// In-memory server-nonce + JTI-replay state. Always
    /// present; in distributed mode the JTI side is a
    /// vestigial path (`check_and_record_jti` skips it when
    /// `distributed_store` is `Some`), but the server-nonce
    /// half still uses it.
    nonces: Arc<RwLock<HashMap<String, i64>>>,
    /// Substrate handle for cross-instance JTI replay
    /// tracking. `Some` in `Distributed` mode, `None` in
    /// `SingleInstanceInmemory`. Per V04_DESIGN.md §6.3.4
    /// the substrate is the source of truth — NO per-instance
    /// caching of JTI verdicts; the substrate is consulted on
    /// every verification.
    distributed_store: Option<Arc<dyn DistributedStore>>,
}

impl Default for DPopNonceStore {
    fn default() -> Self {
        Self::new()
    }
}

impl DPopNonceStore {
    /// In-memory-only constructor. Used by tests and by
    /// SingleInstanceInmemory deployments.
    pub fn new() -> Self {
        Self {
            nonces: Arc::new(RwLock::new(HashMap::new())),
            distributed_store: None,
        }
    }

    /// Builder: attach a distributed-store handle for
    /// cross-instance JTI replay tracking. Returns a new
    /// `DPopNonceStore` with the store configured.
    /// AppContext::new wires this in `Distributed` mode.
    pub fn with_distributed_store(mut self, store: Arc<dyn DistributedStore>) -> Self {
        self.distributed_store = Some(store);
        self
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

    /// Record a client-issued JTI for replay prevention
    /// (RFC 9449 §11.1).
    ///
    /// In `Distributed` mode (Arc 7 Step 3) this consults the
    /// substrate's `dpop_jti_replay` table — a JTI accepted on
    /// one instance is rejected on every sibling, atomically.
    /// In `SingleInstanceInmemory` mode it falls back to the
    /// in-memory map (the pre-Arc-7 behaviour). Per
    /// V04_DESIGN.md §6.3.4 there is NO per-instance cache
    /// of single-use verdicts in `Distributed` mode — the
    /// substrate is consulted on every verification.
    ///
    /// Returns `Ok(true)` on first sighting (recorded),
    /// `Ok(false)` if the JTI was already recorded (replay)
    /// or has already expired by the wall clock.
    ///
    /// The `jkt` parameter (JWK thumbprint of the key that
    /// signed the proof) is recorded alongside the JTI for
    /// observability; not consulted on the hot path. Callers
    /// pass the thumbprint they computed during proof
    /// verification.
    pub async fn check_and_record_jti(
        &self,
        jti: &str,
        jkt: &str,
        exp: i64,
    ) -> PdsResult<bool> {
        let now = Utc::now().timestamp();
        if exp <= now {
            return Ok(false);
        }

        if let Some(store) = self.distributed_store.as_ref() {
            // Distributed mode: substrate's atomic INSERT IS the
            // single-use guarantee. KeyExists → already-seen.
            let value = serde_json::to_vec(&serde_json::json!({ "jkt": jkt }))
                .map_err(|e| {
                    PdsError::Internal(format!("DPoP JTI value encode failed: {}", e))
                })?;
            // `exp` is seconds; the substrate's Lease takes
            // milliseconds. Saturating multiply guards the
            // theoretical i64::MAX/1000 overflow boundary.
            let lease = Lease::until(exp.saturating_mul(1000));
            match store
                .insert("dpop_jti_replay", jti, &value, Some(lease))
                .await
            {
                Ok(()) => Ok(true),
                Err(DistributedError::KeyExists { .. }) => Ok(false),
                Err(e) => Err(PdsError::Internal(format!(
                    "DPoP JTI replay tracking failed: {}",
                    e
                ))),
            }
        } else {
            // SingleInstanceInmemory mode: pre-Arc-7 path. No
            // cross-instance siblings to worry about, so the
            // local map suffices.
            let _ = jkt; // observability column is distributed-mode only
            let mut nonces = self.nonces.write().await;
            if nonces.contains_key(jti) {
                return Ok(false);
            }
            nonces.insert(jti.to_string(), exp);
            Ok(true)
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

    /// Get the number of active nonces
    pub async fn count(&self) -> usize {
        self.nonces.read().await.len()
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
    /// 3. Validate claims (htm, htu, jti replay, expiration, optionally ath)
    /// 4. Extract and return the public key thumbprint (for token binding)
    ///
    /// # Arguments
    /// * `dpop_proof` - The DPoP proof JWT
    /// * `http_method` - Expected HTTP method (e.g., "GET")
    /// * `http_uri` - Expected HTTP URI (e.g., "https://pds.example.com/xrpc/com.atproto.repo.getRecord")
    /// * `expected_ath` - When `Some`, the proof MUST carry an `ath`
    ///   claim equal to this value. Pass `Some(ath_for(access_token))`
    ///   on resource requests (RFC 9449 §4.3); pass `None` on
    ///   token-issuance proofs where no access token exists yet.
    ///
    /// # Returns
    /// The JWK thumbprint (SHA-256 hash) if verification succeeds
    pub async fn verify_dpop_proof(
        &self,
        dpop_proof: &str,
        http_method: &str,
        http_uri: &str,
        expected_ath: Option<&str>,
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
        let jwk_json = serde_json::to_value(&jwk)
            .map_err(|e| PdsError::Internal(format!("Failed to serialize JWK: {}", e)))?;

        let decoding_key = jwk_to_decoding_key(&jwk_json)?;

        // Verify JWT signature
        let mut validation = Validation::new(Algorithm::ES256);
        validation.validate_exp = true;
        validation.leeway = 0; // Strict expiration

        let token_data =
            decode::<DPopClaims>(dpop_proof, &decoding_key, &validation).map_err(|e| {
                warn!("DPoP proof verification failed: {}", e);
                match e.kind() {
                    jsonwebtoken::errors::ErrorKind::ExpiredSignature => {
                        PdsError::Authentication("DPoP proof expired".to_string())
                    }
                    _ => PdsError::Authentication(format!("DPoP proof invalid: {}", e)),
                }
            })?;

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

        // Compute JWK thumbprint EARLY — it's needed both for
        // the JTI replay record's `jkt` column (Arc 7 Step 3
        // distributed-store path) and for the return value
        // (token binding). Cheap to compute (SHA-256 over a
        // small canonical JSON string); doing it once here
        // covers both uses without re-deriving downstream.
        let thumbprint = compute_jwk_thumbprint(&jwk_json)?;

        // Replay tracking on the client-issued JTI (RFC 9449 §11.1).
        // In Distributed mode this consults the substrate's
        // dpop_jti_replay table atomically; in
        // SingleInstanceInmemory mode it uses the in-memory
        // map. See `DPopNonceStore::check_and_record_jti`.
        if !self
            .nonce_store
            .check_and_record_jti(&claims.jti, &thumbprint, claims.exp)
            .await?
        {
            warn!("DPoP proof jti replay or expired: {}", claims.jti);
            return Err(PdsError::Authentication(
                "DPoP proof jti replay or expired".to_string(),
            ));
        }

        // Validate access-token binding if the caller asked for it.
        // RFC 9449 §4.3: resource-request proofs MUST carry `ath` =
        // base64url(SHA-256(access_token)) so the server can confirm
        // the proof was generated for *this* token, not some other
        // token issued to the same client.
        if let Some(expected) = expected_ath {
            match claims.ath.as_deref() {
                Some(actual) if actual == expected => {}
                Some(actual) => {
                    warn!(
                        "DPoP proof ath mismatch: expected {}, got {}",
                        expected, actual
                    );
                    return Err(PdsError::Authentication(
                        "DPoP proof access-token hash mismatch".to_string(),
                    ));
                }
                None => {
                    warn!("DPoP proof missing ath claim on resource request");
                    return Err(PdsError::Authentication(
                        "DPoP proof missing required ath claim".to_string(),
                    ));
                }
            }
        }

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
    let kty = jwk["kty"]
        .as_str()
        .ok_or_else(|| PdsError::Authentication("JWK missing 'kty' field".to_string()))?;

    let crv = jwk["crv"]
        .as_str()
        .ok_or_else(|| PdsError::Authentication("JWK missing 'crv' field".to_string()))?;

    let x = jwk["x"]
        .as_str()
        .ok_or_else(|| PdsError::Authentication("JWK missing 'x' field".to_string()))?;

    let y = jwk["y"]
        .as_str()
        .ok_or_else(|| PdsError::Authentication("JWK missing 'y' field".to_string()))?;

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

    let x_bytes = URL_SAFE_NO_PAD
        .decode(x)
        .map_err(|e| PdsError::Authentication(format!("Invalid JWK x coordinate: {}", e)))?;

    let y_bytes = URL_SAFE_NO_PAD
        .decode(y)
        .map_err(|e| PdsError::Authentication(format!("Invalid JWK y coordinate: {}", e)))?;

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

    let encoded_point = EncodedPoint::from_bytes(&public_key_bytes)
        .map_err(|e| PdsError::Authentication(format!("Invalid EC point encoding: {}", e)))?;

    let verifying_key = VerifyingKey::from_encoded_point(&encoded_point)
        .map_err(|e| PdsError::Authentication(format!("Invalid P-256 public key: {}", e)))?;

    // Convert to SPKI (SubjectPublicKeyInfo) DER format
    // This is the standard format for public keys in X.509/PKCS#8
    use p256::pkcs8::EncodePublicKey;

    let public_key_der = verifying_key
        .to_public_key_der()
        .map_err(|e| PdsError::Internal(format!("Failed to encode public key to DER: {}", e)))?;

    // Convert DER to PEM format for jsonwebtoken
    let public_key_pem = format!(
        "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----",
        base64::engine::general_purpose::STANDARD.encode(public_key_der.as_bytes())
    );

    // Create DecodingKey from EC PEM
    DecodingKey::from_ec_pem(public_key_pem.as_bytes())
        .map_err(|e| PdsError::Internal(format!("Failed to create DecodingKey from PEM: {}", e)))
}

/// Compute the `ath` claim value for a given access token, per RFC
/// 9449 §4.3. Callers wiring resource-request DPoP validation pass
/// the result as `expected_ath` to `verify_dpop_proof`.
pub fn compute_ath(access_token: &str) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(access_token.as_bytes());
    URL_SAFE_NO_PAD.encode(hasher.finalize())
}

/// Compute JWK thumbprint (SHA-256 hash)
///
/// The thumbprint is used for token binding - it uniquely identifies the client's key.
/// Reference: RFC 7638
fn compute_jwk_thumbprint(jwk: &Value) -> PdsResult<String> {
    // Extract required fields in canonical order: kty, crv, x, y (for EC keys)
    let kty = jwk["kty"]
        .as_str()
        .ok_or_else(|| PdsError::Internal("JWK missing kty field".to_string()))?;

    let crv = jwk["crv"]
        .as_str()
        .ok_or_else(|| PdsError::Internal("JWK missing crv field".to_string()))?;

    let x = jwk["x"]
        .as_str()
        .ok_or_else(|| PdsError::Internal("JWK missing x field".to_string()))?;

    let y = jwk["y"]
        .as_str()
        .ok_or_else(|| PdsError::Internal("JWK missing y field".to_string()))?;

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
            ath: None,
        };

        let json = serde_json::to_string(&claims).unwrap();
        let deserialized: DPopClaims = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.jti, "test-nonce-123");
        assert_eq!(deserialized.htm, "POST");
        assert!(deserialized.ath.is_none());
    }

    #[test]
    fn test_dpop_claims_round_trip_with_ath() {
        let claims = DPopClaims {
            jti: "j".to_string(),
            htm: "POST".to_string(),
            htu: "https://pds/x".to_string(),
            iat: 0,
            exp: 60,
            ath: Some("EXPECTED_ATH_VALUE".to_string()),
        };
        let json = serde_json::to_string(&claims).unwrap();
        assert!(json.contains("\"ath\":\"EXPECTED_ATH_VALUE\""));
        let parsed: DPopClaims = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.ath.as_deref(), Some("EXPECTED_ATH_VALUE"));
    }

    #[test]
    fn test_dpop_claims_omits_ath_when_none() {
        // Issuance-time proofs send no ath; the JSON must omit the
        // field entirely rather than serialize it as null, so existing
        // strict client validators don't choke.
        let claims = DPopClaims {
            jti: "j".to_string(),
            htm: "POST".to_string(),
            htu: "https://pds/oauth/token".to_string(),
            iat: 0,
            exp: 60,
            ath: None,
        };
        let json = serde_json::to_string(&claims).unwrap();
        assert!(!json.contains("ath"), "serialized: {}", json);
    }

    #[test]
    fn test_compute_ath_matches_rfc_9449_recipe() {
        // base64url(SHA-256("test-token")) without padding.
        // Computed independently with `printf 'test-token' | sha256sum`
        // and base64url-encoded.
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use base64::Engine;
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(b"test-token");
        let expected = URL_SAFE_NO_PAD.encode(h.finalize());
        assert_eq!(compute_ath("test-token"), expected);
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
        let valid = store
            .check_and_consume_nonce("invalid-nonce")
            .await
            .unwrap();
        assert!(!valid);
    }

    #[tokio::test]
    async fn test_check_and_record_jti_first_sighting_recorded() {
        let store = DPopNonceStore::new();
        let exp = Utc::now().timestamp() + 60;
        let recorded = store
            .check_and_record_jti("client-jti-1", "thumb-1", exp)
            .await
            .unwrap();
        assert!(recorded, "first sighting should be recorded");
    }

    #[tokio::test]
    async fn test_check_and_record_jti_replay_rejected() {
        let store = DPopNonceStore::new();
        let exp = Utc::now().timestamp() + 60;
        store
            .check_and_record_jti("dupe", "thumb", exp)
            .await
            .unwrap();
        let replayed = store
            .check_and_record_jti("dupe", "thumb", exp)
            .await
            .unwrap();
        assert!(!replayed, "replay must be rejected");
    }

    #[tokio::test]
    async fn test_check_and_record_jti_already_expired_rejected() {
        let store = DPopNonceStore::new();
        let already_expired = Utc::now().timestamp() - 1;
        let recorded = store
            .check_and_record_jti("late", "thumb", already_expired)
            .await
            .unwrap();
        assert!(!recorded, "already-expired proof must be rejected");
    }

    // Distributed-mode JTI replay path. Uses a `PostgresCasStore`
    // against in-memory SQLite — same pattern as the OAuth
    // adapter tests. Cross-instance correctness against real
    // Postgres is exercised in `tests/distributed_substrate_test.rs`.
    #[tokio::test]
    async fn test_check_and_record_jti_distributed_mode_first_sighting_accepted() {
        use crate::distributed::PostgresCasStore;
        use sqlx::any::AnyPoolOptions;
        use std::sync::Once;

        static INSTALL: Once = Once::new();
        INSTALL.call_once(sqlx::any::install_default_drivers);
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE dpop_jti_replay (
                jti TEXT PRIMARY KEY,
                jkt TEXT NOT NULL,
                exp_at_epoch_ms BIGINT NOT NULL,
                created_at_epoch_ms BIGINT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        let substrate: Arc<dyn DistributedStore> =
            Arc::new(PostgresCasStore::new(Arc::new(pool)));
        let store = DPopNonceStore::new().with_distributed_store(substrate);

        let exp = Utc::now().timestamp() + 60;
        let recorded = store
            .check_and_record_jti("dist-jti-1", "thumb-dist", exp)
            .await
            .unwrap();
        assert!(recorded, "first sighting accepted in distributed mode");

        let replay = store
            .check_and_record_jti("dist-jti-1", "thumb-dist", exp)
            .await
            .unwrap();
        assert!(!replay, "replay rejected in distributed mode");
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
        if let Err(e) = result {
            assert!(
                e.to_string().contains("P-384"),
                "Error should mention P-384"
            );
        }
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
        if let Err(e) = result {
            assert!(e.to_string().contains("RSA"), "Error should mention RSA");
        }
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

    // ---- End-to-end DPoP proof verification with a real EC keypair ----
    //
    // Generates a fresh P-256 keypair, signs a real DPoP proof with
    // it, and exercises `verify_dpop_proof` along the resource-flow
    // path that requires `ath`. Covers the four states the disposition
    // calls out:
    //   - Issuance flow (`expected_ath = None`): ath claim ignored.
    //   - Resource flow with matching ath: proof accepted.
    //   - Resource flow with mismatched ath: rejected.
    //   - Resource flow with missing ath claim: rejected.

    fn make_signed_dpop_proof(
        signing_key: &p256::ecdsa::SigningKey,
        jwk: &Jwk,
        claims: &DPopClaims,
    ) -> String {
        use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
        use p256::pkcs8::EncodePrivateKey;
        let pem = signing_key
            .to_pkcs8_pem(Default::default())
            .expect("PKCS#8 PEM encode")
            .to_string();
        let encoding_key =
            EncodingKey::from_ec_pem(pem.as_bytes()).expect("EC PEM decoding");
        let mut header = Header::new(Algorithm::ES256);
        header.typ = Some("dpop+jwt".to_string());
        // The jsonwebtoken crate's Header.jwk accepts its own Jwk type.
        // Round-trip via serde_json so we don't depend on that crate's
        // internals here.
        let jwk_value = serde_json::to_value(jwk).expect("jwk to value");
        header.jwk = Some(serde_json::from_value(jwk_value).expect("jwk parse"));
        encode(&header, claims, &encoding_key).expect("sign DPoP proof")
    }

    fn fresh_keypair_jwk() -> (p256::ecdsa::SigningKey, Jwk) {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use base64::Engine;
        use p256::ecdsa::SigningKey;
        use p256::EncodedPoint;
        let signing_key = SigningKey::random(&mut rand::rngs::OsRng);
        let verifying_key = signing_key.verifying_key();
        let encoded: EncodedPoint = verifying_key.to_encoded_point(false);
        let x = encoded.x().expect("x coord");
        let y = encoded.y().expect("y coord");
        let jwk = Jwk {
            kty: "EC".to_string(),
            crv: "P-256".to_string(),
            x: URL_SAFE_NO_PAD.encode(x),
            y: URL_SAFE_NO_PAD.encode(y),
        };
        (signing_key, jwk)
    }

    fn fresh_claims(ath: Option<String>) -> DPopClaims {
        DPopClaims {
            jti: uuid::Uuid::new_v4().to_string(),
            htm: "POST".to_string(),
            htu: "https://pds.example.com/xrpc/com.atproto.repo.createRecord".to_string(),
            iat: Utc::now().timestamp(),
            exp: Utc::now().timestamp() + 60,
            ath,
        }
    }

    #[tokio::test]
    async fn verify_dpop_proof_accepts_issuance_proof_with_no_ath() {
        let store = Arc::new(DPopNonceStore::new());
        let verifier = DPopVerifier::new(Arc::clone(&store));
        let (signing_key, jwk) = fresh_keypair_jwk();
        let mut claims = fresh_claims(None);
        claims.htu = "https://pds.example.com/oauth/token".to_string();
        let proof = make_signed_dpop_proof(&signing_key, &jwk, &claims);
        let thumbprint = verifier
            .verify_dpop_proof(
                &proof,
                "POST",
                "https://pds.example.com/oauth/token",
                None,
            )
            .await
            .expect("issuance proof with no ath should verify");
        assert!(!thumbprint.is_empty());
    }

    #[tokio::test]
    async fn verify_dpop_proof_resource_flow_accepts_matching_ath() {
        let store = Arc::new(DPopNonceStore::new());
        let verifier = DPopVerifier::new(Arc::clone(&store));
        let (signing_key, jwk) = fresh_keypair_jwk();
        let access_token = "at_test_token_value";
        let ath = compute_ath(access_token);
        let claims = fresh_claims(Some(ath.clone()));
        let proof = make_signed_dpop_proof(&signing_key, &jwk, &claims);
        verifier
            .verify_dpop_proof(
                &proof,
                "POST",
                "https://pds.example.com/xrpc/com.atproto.repo.createRecord",
                Some(&ath),
            )
            .await
            .expect("matching ath should verify");
    }

    #[tokio::test]
    async fn verify_dpop_proof_resource_flow_rejects_mismatched_ath() {
        let store = Arc::new(DPopNonceStore::new());
        let verifier = DPopVerifier::new(Arc::clone(&store));
        let (signing_key, jwk) = fresh_keypair_jwk();
        // Proof carries one ath value, server expects another.
        let claims = fresh_claims(Some(compute_ath("token_in_proof")));
        let proof = make_signed_dpop_proof(&signing_key, &jwk, &claims);
        let server_ath = compute_ath("token_actually_presented");
        let err = verifier
            .verify_dpop_proof(
                &proof,
                "POST",
                "https://pds.example.com/xrpc/com.atproto.repo.createRecord",
                Some(&server_ath),
            )
            .await
            .expect_err("mismatched ath must be rejected");
        assert!(
            format!("{}", err).contains("access-token hash mismatch"),
            "expected ath-mismatch error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn verify_dpop_proof_resource_flow_rejects_missing_ath() {
        let store = Arc::new(DPopNonceStore::new());
        let verifier = DPopVerifier::new(Arc::clone(&store));
        let (signing_key, jwk) = fresh_keypair_jwk();
        let claims = fresh_claims(None); // No ath in proof
        let proof = make_signed_dpop_proof(&signing_key, &jwk, &claims);
        let server_ath = compute_ath("any-token");
        let err = verifier
            .verify_dpop_proof(
                &proof,
                "POST",
                "https://pds.example.com/xrpc/com.atproto.repo.createRecord",
                Some(&server_ath),
            )
            .await
            .expect_err("resource-flow proof without ath must be rejected");
        assert!(
            format!("{}", err).contains("missing required ath claim"),
            "expected missing-ath error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn verify_dpop_proof_jti_replay_rejected() {
        // Second presentation of the same proof bytes is a replay
        // even when the signature and claims are otherwise valid.
        let store = Arc::new(DPopNonceStore::new());
        let verifier = DPopVerifier::new(Arc::clone(&store));
        let (signing_key, jwk) = fresh_keypair_jwk();
        let claims = fresh_claims(None);
        let proof = make_signed_dpop_proof(&signing_key, &jwk, &claims);
        let url = "https://pds.example.com/xrpc/com.atproto.repo.createRecord";
        verifier
            .verify_dpop_proof(&proof, "POST", url, None)
            .await
            .expect("first presentation accepted");
        let err = verifier
            .verify_dpop_proof(&proof, "POST", url, None)
            .await
            .expect_err("replay must be rejected");
        assert!(format!("{}", err).contains("replay"), "got: {}", err);
    }
}
