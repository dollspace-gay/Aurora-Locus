// Allow dead_code - service auth features for future use
#![allow(dead_code)]

//! Service Authentication for Cross-PDS Requests
//!
//! Implements ATProto service auth specification:
//! - Short-lived JWTs (<60 seconds) signed with user's atproto key
//! - DID-based verification (no callback to origin PDS)
//! - Claims: iss (user DID), aud (service DID), exp, lxm (endpoint), jti (nonce)
//!
//! References:
//! - https://atproto.com/specs/xrpc
//! - https://docs.bsky.app/docs/advanced-guides/service-auth

use crate::error::{PdsError, PdsResult};
use crate::identity::IdentityResolverApi;
use chrono::Utc;
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, warn};

/// Service auth JWT claims (ATProto spec)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceAuthClaims {
    /// Issuer: User's DID
    pub iss: String,

    /// Audience: Target service's DID
    pub aud: String,

    /// Expiration time (Unix timestamp)
    pub exp: i64,

    /// Issued at (Unix timestamp)
    pub iat: i64,

    /// Lexicon method (optional endpoint identifier)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lxm: Option<String>,

    /// JWT ID (unique nonce for replay prevention)
    pub jti: String,
}

/// Service authenticator for creating and verifying cross-PDS JWTs
pub struct ServiceAuthenticator {
    identity_resolver: Arc<dyn IdentityResolverApi>,
}

impl ServiceAuthenticator {
    /// Create a new service authenticator
    pub fn new(identity_resolver: Arc<dyn IdentityResolverApi>) -> Self {
        Self { identity_resolver }
    }

    // Cluster 2 Member 2.3 — deleted `create_service_jwt` method
    // here. It called `identity_resolver.get_signing_key(user_did)`
    // (which returns a PUBLIC key from the issuer's DID document) and
    // tried to use it as a PRIVATE signing key via
    // EncodingKey::from_ec_pem — cannot work; public keys don't sign.
    // Zero callers (grep confirmed; file carries `#![allow(dead_code)]`
    // so the compiler didn't flag it). Deleting removes a foot-gun: the
    // method looked callable on a type wired into AppContext, and a
    // future contributor searching for "how do I mint a service JWT"
    // could have landed here and shipped the public-key-as-private bug
    // to production. The correct minting path is the free function
    // `src/service_auth.rs::create_service_jwt`, which takes the
    // private-key bytes as a parameter — that's the path
    // src/api/server.rs::get_service_auth uses (post-Member 2.1 fix,
    // with the per-account `get_atproto_signing_key_bytes` bytes).
    // Folded into the Member 2.1 chainlink (#143) as hygiene.

    /// Verify a service auth JWT from another PDS
    ///
    /// This performs DID-based cryptographic verification:
    /// 1. Decode JWT to extract issuer DID
    /// 2. Resolve issuer's DID document
    /// 3. Fetch atproto signing key from DID document
    /// 4. Verify JWT signature using that public key
    /// 5. Validate audience, expiration, and nonce
    ///
    /// # Arguments
    /// * `token` - The JWT token to verify
    /// * `expected_audience` - The expected audience (this service's DID)
    ///
    /// # Returns
    /// The verified claims if successful
    pub async fn verify_service_jwt(
        &self,
        token: &str,
        expected_audience: &str,
    ) -> PdsResult<ServiceAuthClaims> {
        debug!("Verifying service JWT for audience={}", expected_audience);

        // Decode JWT without verification first to get issuer DID
        let unverified = decode::<ServiceAuthClaims>(
            token,
            &DecodingKey::from_secret(&[]), // Dummy key for header-only decode
            &Validation::default(),
        )
        .map_err(|e| {
            warn!("Failed to decode JWT: {}", e);
            PdsError::Authentication("Invalid JWT format".to_string())
        })?;

        let issuer_did = &unverified.claims.iss;

        debug!("JWT issuer: {}", issuer_did);

        // Resolve issuer's DID document to get signing key.
        //
        // Cluster 2 Member 2.2 (#144): propagate typed
        // PdsError::DidTombstoned unchanged so IntoResponse maps it to
        // HTTP 400 `{"error": "DidTombstoned", ...}` per
        // src/error.rs:620-624. Pre-#144 the .map_err destroyed the
        // typed variant by stringifying into PdsError::Authentication
        // → HTTP 401 opaque. The PLC-410 → DidTombstoned mapping in
        // src/identity/resolver.rs::fetch_plc_document (#134 / Arc
        // 13 v4.2) emitted the typed variant but never reached the
        // wire because every live `verify_service_jwt` caller
        // swallowed it here. Pattern-match: pass DidTombstoned
        // through; wrap everything else as Authentication (preserving
        // today's wire shape with the source-detail appended for
        // tracing — non-tombstone strings are tracing-only, not
        // grep'd by any test/runbook/metric).
        let signing_key = self
            .identity_resolver
            .get_signing_key(issuer_did)
            .await
            .map_err(|e| match e {
                PdsError::DidTombstoned(_) => e,
                other => {
                    warn!("Failed to resolve signing key for {}: {}", issuer_did, other);
                    PdsError::Authentication(format!(
                        "Could not verify issuer DID: {}: {}",
                        issuer_did, other
                    ))
                }
            })?;

        // Verify JWT signature with issuer's public key
        let decoding_key = DecodingKey::from_ec_pem(&signing_key)
            .map_err(|e| PdsError::Internal(format!("Invalid public key: {}", e)))?;

        let mut validation = Validation::new(Algorithm::ES256);
        validation.set_audience(&[expected_audience]);
        validation.leeway = 0; // Strict expiration (no grace period)
        validation.validate_exp = true;

        let token_data =
            decode::<ServiceAuthClaims>(token, &decoding_key, &validation).map_err(|e| {
                warn!("JWT verification failed: {}", e);
                match e.kind() {
                    jsonwebtoken::errors::ErrorKind::ExpiredSignature => {
                        PdsError::Authentication("JWT expired".to_string())
                    }
                    jsonwebtoken::errors::ErrorKind::InvalidAudience => {
                        PdsError::Authentication("Invalid audience".to_string())
                    }
                    _ => PdsError::Authentication(format!("JWT verification failed: {}", e)),
                }
            })?;

        let claims = token_data.claims;

        // Additional validations
        let now = Utc::now().timestamp();

        // Ensure expiration is < 60 seconds from now
        let time_to_expire = claims.exp - now;
        if time_to_expire > 60 {
            warn!(
                "JWT expiration too far in future: {} seconds",
                time_to_expire
            );
            return Err(PdsError::Authentication(
                "JWT expiration exceeds 60 second limit".to_string(),
            ));
        }

        // Ensure JWT was issued recently (not from past)
        let time_since_issued = now - claims.iat;
        if time_since_issued > 120 {
            // Allow up to 2 minutes of clock skew
            warn!("JWT issued too long ago: {} seconds", time_since_issued);
            return Err(PdsError::Authentication("JWT too old".to_string()));
        }

        debug!("✓ JWT verified: issuer={}, jti={}", claims.iss, claims.jti);

        Ok(claims)
    }

    /// Verify JWT and check nonce (replay prevention)
    ///
    /// This is a convenience method that verifies the JWT and checks if the
    /// nonce has been used before. The caller is responsible for tracking nonces.
    ///
    /// # Arguments
    /// * `token` - The JWT token to verify
    /// * `expected_audience` - The expected audience (this service's DID)
    /// * `nonce_checker` - Async function to check if nonce has been used
    ///
    /// # Returns
    /// The verified claims if successful and nonce is unused
    pub async fn verify_with_nonce_check<F, Fut>(
        &self,
        token: &str,
        expected_audience: &str,
        nonce_checker: F,
    ) -> PdsResult<ServiceAuthClaims>
    where
        F: FnOnce(String) -> Fut,
        Fut: std::future::Future<Output = PdsResult<bool>>,
    {
        // Verify JWT signature and claims
        let claims = self.verify_service_jwt(token, expected_audience).await?;

        // Check if nonce has been used (replay attack prevention)
        let nonce_is_new = nonce_checker(claims.jti.clone()).await?;

        if !nonce_is_new {
            warn!("Replay attack detected: nonce {} already used", claims.jti);
            return Err(PdsError::Authentication(
                "Replay attack detected".to_string(),
            ));
        }

        Ok(claims)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_service_auth_claims_serialization() {
        let claims = ServiceAuthClaims {
            iss: "did:plc:user123".to_string(),
            aud: "did:plc:service456".to_string(),
            exp: 1234567890,
            iat: 1234567830,
            lxm: Some("com.atproto.repo.getRecord".to_string()),
            jti: "550e8400-e29b-41d4-a716-446655440000".to_string(),
        };

        let json = serde_json::to_string(&claims).unwrap();
        let deserialized: ServiceAuthClaims = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.iss, "did:plc:user123");
        assert_eq!(deserialized.aud, "did:plc:service456");
        assert_eq!(deserialized.jti, "550e8400-e29b-41d4-a716-446655440000");
    }

    #[test]
    fn test_service_auth_claims_without_lxm() {
        let claims = ServiceAuthClaims {
            iss: "did:plc:user123".to_string(),
            aud: "did:plc:service456".to_string(),
            exp: 1234567890,
            iat: 1234567830,
            lxm: None,
            jti: "test-jti".to_string(),
        };

        let json = serde_json::to_string(&claims).unwrap();
        // lxm should be omitted from JSON when None
        assert!(!json.contains("lxm"));
    }
}
