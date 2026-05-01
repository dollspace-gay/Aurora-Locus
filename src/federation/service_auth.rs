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
use crate::identity::IdentityResolver;
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, warn};
use uuid::Uuid;

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
    identity_resolver: Arc<IdentityResolver>,
}

impl ServiceAuthenticator {
    /// Create a new service authenticator
    pub fn new(identity_resolver: Arc<IdentityResolver>) -> Self {
        Self { identity_resolver }
    }

    /// Create a service auth JWT for cross-PDS request
    ///
    /// This JWT is signed with the user's atproto signing key from their DID document.
    /// The receiving service will verify by resolving the issuer DID and checking the signature.
    ///
    /// # Arguments
    /// * `user_did` - The user's DID (issuer)
    /// * `target_service_did` - The target service's DID (audience)
    /// * `endpoint` - Optional endpoint identifier (e.g., "com.atproto.repo.getRecord")
    ///
    /// # Returns
    /// A signed JWT token string
    pub async fn create_service_jwt(
        &self,
        user_did: &str,
        target_service_did: &str,
        endpoint: Option<&str>,
    ) -> PdsResult<String> {
        debug!(
            "Creating service JWT: user={}, target={}, endpoint={:?}",
            user_did, target_service_did, endpoint
        );

        // Get user's atproto signing key from DID document
        let signing_key = self
            .identity_resolver
            .get_signing_key(user_did)
            .await
            .map_err(|e| {
                PdsError::Internal(format!("Failed to get signing key for {}: {}", user_did, e))
            })?;

        // Create JWT claims
        let now = Utc::now();
        let exp = now + Duration::seconds(59); // <60 seconds (ATProto requirement)

        let claims = ServiceAuthClaims {
            iss: user_did.to_string(),
            aud: target_service_did.to_string(),
            exp: exp.timestamp(),
            iat: now.timestamp(),
            lxm: endpoint.map(|s| s.to_string()),
            jti: Uuid::new_v4().to_string(), // Unique nonce
        };

        // Create JWT header (ES256 algorithm)
        let mut header = Header::new(Algorithm::ES256);
        header.typ = Some("at+jwt".to_string()); // ATProto JWT type

        // Sign JWT with user's atproto key
        let encoding_key = EncodingKey::from_ec_pem(&signing_key)
            .map_err(|e| PdsError::Internal(format!("Invalid signing key: {}", e)))?;

        let token = encode(&header, &claims, &encoding_key)
            .map_err(|e| PdsError::Internal(format!("Failed to create JWT: {}", e)))?;

        debug!("✓ Created service JWT with jti={}", claims.jti);

        Ok(token)
    }

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

        // Resolve issuer's DID document to get signing key
        let signing_key = self
            .identity_resolver
            .get_signing_key(issuer_did)
            .await
            .map_err(|e| {
                warn!("Failed to resolve signing key for {}: {}", issuer_did, e);
                PdsError::Authentication(format!("Could not verify issuer DID: {}", issuer_did))
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
