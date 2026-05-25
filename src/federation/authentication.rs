//! Cross-PDS authentication via DID resolution

// Allow dead_code - public APIs defined for future feature completion
#![allow(dead_code)]
//!
//! Enables users from other PDS instances to authenticate and interact
//! with this PDS by resolving their DIDs and verifying signatures.

use crate::error::{PdsError, PdsResult};
use crate::federation::ServiceAuthenticator;
use crate::identity::did_document::DidDocument;
use crate::identity::IdentityResolverApi;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Federation authenticator for cross-PDS authentication
#[derive(Clone)]
pub struct FederationAuthenticator {
    identity_resolver: Arc<dyn IdentityResolverApi>,
    http_client: Client,
    #[allow(dead_code)] // TODO: Implement token caching
    cache_ttl: u64,
    /// Service authenticator for Phase 4 (DID-based JWT verification)
    pub authenticator: Arc<ServiceAuthenticator>,
}

impl FederationAuthenticator {
    /// Create a new federation authenticator
    pub fn new(identity_resolver: Arc<dyn IdentityResolverApi>) -> Self {
        let authenticator = Arc::new(ServiceAuthenticator::new(Arc::clone(&identity_resolver)));

        Self {
            identity_resolver,
            http_client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap(),
            cache_ttl: 3600, // 1 hour
            authenticator,
        }
    }

    /// Verify a remote user's authentication token
    ///
    /// This resolves the user's DID, fetches their PDS endpoint,
    /// and verifies the token with their home PDS.
    pub async fn verify_remote_token(&self, did: &str, token: &str) -> PdsResult<RemoteUser> {
        debug!("Verifying remote token for DID: {}", did);

        // Resolve DID to get user's home PDS
        let did_doc = self.identity_resolver.resolve_did(did).await.map_err(|e| {
            warn!("Failed to resolve DID {}: {}", did, e);
            PdsError::Authentication(format!("Failed to resolve DID: {}", e))
        })?;

        // Extract PDS endpoint from DID document
        let pds_endpoint = self.extract_pds_endpoint(&did_doc).ok_or_else(|| {
            warn!("No PDS endpoint found in DID document for {}", did);
            PdsError::Authentication("No PDS endpoint in DID document".to_string())
        })?;

        debug!("User's home PDS: {}", pds_endpoint);

        // Verify token with remote PDS
        self.verify_token_with_pds(&pds_endpoint, did, token).await
    }

    /// Extract PDS endpoint from DID document.
    ///
    /// Thin instance-method shim over [`extract_pds_endpoint`] so legacy
    /// `self.extract_pds_endpoint(...)` call sites keep compiling. Arc 16f
    /// Step 2 shares the helper with `src/federation/blob_fetch.rs`.
    fn extract_pds_endpoint(&self, did_doc: &DidDocument) -> Option<String> {
        extract_pds_endpoint(did_doc)
    }

    /// Verify token with remote PDS
    async fn verify_token_with_pds(
        &self,
        pds_endpoint: &str,
        did: &str,
        token: &str,
    ) -> PdsResult<RemoteUser> {
        let url = format!("{}/xrpc/com.atproto.server.getSession", pds_endpoint);

        debug!("Verifying token with remote PDS: {}", url);

        let response = self
            .http_client
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .map_err(|e| {
                warn!("Failed to connect to remote PDS: {}", e);
                PdsError::Authentication(format!("Failed to verify with remote PDS: {}", e))
            })?;

        if !response.status().is_success() {
            warn!(
                "Remote PDS returned non-success status: {}",
                response.status()
            );
            return Err(PdsError::Authentication(
                "Token verification failed".to_string(),
            ));
        }

        let session: SessionResponse = response.json().await.map_err(|e| {
            warn!("Failed to parse session response: {}", e);
            PdsError::Authentication(format!("Invalid session response: {}", e))
        })?;

        // Verify DID matches
        if session.did != did {
            warn!("DID mismatch: expected {}, got {}", did, session.did);
            return Err(PdsError::Authentication("DID mismatch".to_string()));
        }

        info!("Successfully verified remote user: {}", did);

        Ok(RemoteUser {
            did: session.did,
            handle: session.handle,
            pds_endpoint: pds_endpoint.to_string(),
            email: session.email,
        })
    }

    /// Fetch public profile from remote PDS
    pub async fn fetch_remote_profile(&self, did: &str) -> PdsResult<RemoteProfile> {
        debug!("Fetching remote profile for DID: {}", did);

        // Resolve DID to get PDS endpoint
        let did_doc = self.identity_resolver.resolve_did(did).await?;
        let pds_endpoint = self
            .extract_pds_endpoint(&did_doc)
            .ok_or_else(|| PdsError::NotFound("No PDS endpoint found".to_string()))?;

        // Fetch profile via xrpc
        let url = format!(
            "{}/xrpc/app.bsky.actor.getProfile?actor={}",
            pds_endpoint, did
        );

        let response = self.http_client.get(&url).send().await.map_err(|e| {
            warn!("Failed to fetch remote profile: {}", e);
            PdsError::Internal(format!("Failed to fetch profile: {}", e))
        })?;

        if !response.status().is_success() {
            return Err(PdsError::NotFound("Profile not found".to_string()));
        }

        let profile: RemoteProfile = response.json().await.map_err(|e| {
            warn!("Failed to parse profile response: {}", e);
            PdsError::Internal(format!("Invalid profile response: {}", e))
        })?;

        debug!("Fetched profile for {}: @{}", did, profile.handle);

        Ok(profile)
    }

    /// Check if a DID is from a known/trusted PDS
    pub async fn is_trusted_pds(&self, did: &str, trusted_instances: &[String]) -> PdsResult<bool> {
        let did_doc = self.identity_resolver.resolve_did(did).await?;
        let pds_endpoint = self.extract_pds_endpoint(&did_doc);

        match pds_endpoint {
            Some(endpoint) => {
                // Check if endpoint is in trusted list
                Ok(trusted_instances
                    .iter()
                    .any(|trusted| endpoint.contains(trusted)))
            }
            None => Ok(false),
        }
    }
}

/// Remote user information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteUser {
    pub did: String,
    pub handle: String,
    pub pds_endpoint: String,
    pub email: Option<String>,
}

/// Session response from remote PDS
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionResponse {
    pub did: String,
    pub handle: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

/// Remote user profile
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteProfile {
    pub did: String,
    pub handle: String,
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub avatar: Option<String>,
    #[serde(rename = "followersCount")]
    pub followers_count: Option<i64>,
    #[serde(rename = "followsCount")]
    pub follows_count: Option<i64>,
    #[serde(rename = "postsCount")]
    pub posts_count: Option<i64>,
}

/// Pull the ATProto PDS endpoint URL out of a resolved DID document.
///
/// Walks the document's `service` array and returns the first
/// `serviceEndpoint` whose `type` is `AtprotoPersonalDataServer`.
/// Returns `None` if no such service entry exists.
///
/// Extracted from `FederationAuthenticator::extract_pds_endpoint` in Arc 16f
/// Step 2 so the origin-blob-fetch primitive
/// (`crate::federation::blob_fetch`) shares one canonical implementation
/// with the cross-PDS auth verifier rather than duplicating the DID-doc
/// walking logic.
pub(crate) fn extract_pds_endpoint(did_doc: &DidDocument) -> Option<String> {
    // Format: https://atproto.com/specs/did#service-endpoints
    let json = serde_json::to_value(did_doc).ok()?;
    let services = json.get("service")?.as_array()?;

    for service in services {
        let service_type = service.get("type")?.as_str()?;
        if service_type == "AtprotoPersonalDataServer" {
            return service.get("serviceEndpoint")?.as_str().map(String::from);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_remote_user_serialization() {
        let user = RemoteUser {
            did: "did:plc:test123".to_string(),
            handle: "alice.bsky.social".to_string(),
            pds_endpoint: "https://pds.bsky.social".to_string(),
            email: Some("alice@example.com".to_string()),
        };

        let json = serde_json::to_string(&user).unwrap();
        let deserialized: RemoteUser = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.did, "did:plc:test123");
        assert_eq!(deserialized.handle, "alice.bsky.social");
    }

    #[test]
    fn test_extract_pds_endpoint() {
        let did_doc = serde_json::json!({
            "id": "did:plc:test123",
            "service": [
                {
                    "id": "#atproto_pds",
                    "type": "AtprotoPersonalDataServer",
                    "serviceEndpoint": "https://pds.example.com"
                }
            ]
        });

        // We can't easily test without creating a full authenticator,
        // but we can test the JSON structure
        assert!(did_doc.get("service").is_some());
    }

    fn did_doc_with_services(services: serde_json::Value) -> DidDocument {
        let raw = serde_json::json!({
            "id": "did:plc:test123",
            "service": services,
        });
        serde_json::from_value(raw).expect("synthetic DID doc deserialises")
    }

    #[test]
    fn extract_pds_endpoint_returns_atproto_pds_service_endpoint() {
        let doc = did_doc_with_services(serde_json::json!([
            {
                "id": "#atproto_pds",
                "type": "AtprotoPersonalDataServer",
                "serviceEndpoint": "https://pds.example.com"
            }
        ]));

        assert_eq!(
            extract_pds_endpoint(&doc),
            Some("https://pds.example.com".to_string())
        );
    }

    #[test]
    fn extract_pds_endpoint_returns_none_for_missing_service() {
        let doc = did_doc_with_services(serde_json::json!([]));
        assert_eq!(extract_pds_endpoint(&doc), None);
    }

    #[test]
    fn extract_pds_endpoint_skips_non_atproto_service_types() {
        let doc = did_doc_with_services(serde_json::json!([
            { "id": "#x", "type": "OtherService", "serviceEndpoint": "https://other.example.com" }
        ]));
        assert_eq!(extract_pds_endpoint(&doc), None);
    }
}
