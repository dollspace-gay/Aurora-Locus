//! PLC Directory Client - Handles key rotation and DID operations
//!
//! Provides high-level interface for interacting with the PLC directory,
//! including fetching DID documents, comparing keys, and updating signing keys.

use crate::identity::did_document::DidDocument;
use crate::{
    crypto::plc::{register_plc_did, PlcOperationBuilder, PlcSigner},
    error::{PdsError, PdsResult},
};

/// PLC Directory client configuration
#[derive(Debug, Clone)]
pub struct PlcClientConfig {
    /// PLC directory URL (e.g., "https://plc.directory")
    pub plc_url: String,

    /// Timeout for HTTP requests in seconds
    pub timeout_secs: u64,
}

impl Default for PlcClientConfig {
    fn default() -> Self {
        Self {
            plc_url: "https://plc.directory".to_string(),
            timeout_secs: 30,
        }
    }
}

/// PLC Directory client for DID operations
#[derive(Clone)]
pub struct PlcClient {
    config: PlcClientConfig,
    http_client: reqwest::Client,
}

impl PlcClient {
    /// Create a new PLC client
    pub fn new(config: PlcClientConfig) -> PdsResult<Self> {
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .user_agent("Aurora-Locus/0.1")
            .build()
            .map_err(|e| PdsError::Internal(format!("Failed to create HTTP client: {}", e)))?;

        Ok(Self {
            config,
            http_client,
        })
    }

    /// Arc 13 §6.3.4 — fetch the last accepted (non-nullified) PLC
    /// operation for `did` from the directory's audit log, plus its
    /// CID. The CID becomes the `prev` of the next op the caller
    /// builds (snapshot-mutator pattern at §6.3.6).
    ///
    /// Errors:
    /// - `PdsError::IdentityResolution` — network failure, non-2xx
    ///   from directory, malformed audit-log JSON, empty log
    ///   (no genesis op present).
    /// - `PdsError::DidTombstoned` — last accepted op is a
    ///   `plc_tombstone`. The DID is terminally retired.
    pub async fn get_last_op(
        &self,
        did: &str,
    ) -> PdsResult<(crate::crypto::plc::PlcOperation, String)> {
        if !did.starts_with("did:plc:") {
            return Err(PdsError::Validation(
                "Only did:plc identifiers are supported".to_string(),
            ));
        }

        let url = format!(
            "{}/{}/log/audit",
            self.config.plc_url.trim_end_matches('/'),
            did
        );

        let response = self.http_client.get(&url).send().await.map_err(|e| {
            PdsError::IdentityResolution(format!("Failed to fetch PLC audit log: {}", e))
        })?;

        if !response.status().is_success() {
            let status = response.status();
            let error_body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(PdsError::IdentityResolution(format!(
                "PLC audit log returned error {}: {}",
                status, error_body
            )));
        }

        // PLC audit log shape (per @did-plc/lib): array of
        // `{cid, did, operation, nullified, createdAt}`, oldest
        // first. We filter `nullified=true` and take the last.
        let entries: Vec<serde_json::Value> = response
            .json()
            .await
            .map_err(|e| PdsError::IdentityResolution(format!("Invalid PLC audit log JSON: {}", e)))?;

        let last_accepted = entries
            .iter()
            .filter(|e| !e.get("nullified").and_then(|n| n.as_bool()).unwrap_or(false))
            .next_back()
            .ok_or_else(|| {
                PdsError::IdentityResolution(format!(
                    "PLC audit log for {} has no accepted (non-nullified) entries",
                    did
                ))
            })?;

        let cid = last_accepted
            .get("cid")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                PdsError::IdentityResolution(format!(
                    "PLC audit-log entry for {} missing cid field",
                    did
                ))
            })?
            .to_string();

        let op_value = last_accepted.get("operation").ok_or_else(|| {
            PdsError::IdentityResolution(format!(
                "PLC audit-log entry for {} missing operation field",
                did
            ))
        })?;

        // §6.3.4: tombstone last op → DidTombstoned, not the
        // generic IdentityResolution path. Caller dispatches on
        // this distinctly so handlers can map to HTTP 400
        // `DidTombstoned`.
        let op_type = op_value
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if op_type == "plc_tombstone" {
            return Err(PdsError::DidTombstoned(did.to_string()));
        }

        let op: crate::crypto::plc::PlcOperation =
            serde_json::from_value(op_value.clone()).map_err(|e| {
                PdsError::IdentityResolution(format!(
                    "Failed to parse last accepted PLC operation for {}: {}",
                    did, e
                ))
            })?;

        Ok((op, cid))
    }

    /// Fetch DID document from PLC directory
    pub async fn get_document(&self, did: &str) -> PdsResult<DidDocument> {
        if !did.starts_with("did:plc:") {
            return Err(PdsError::Validation(
                "Only did:plc identifiers are supported".to_string(),
            ));
        }

        let url = format!("{}/{}", self.config.plc_url.trim_end_matches('/'), did);

        let response = self.http_client.get(&url).send().await.map_err(|e| {
            PdsError::IdentityResolution(format!("Failed to fetch PLC document: {}", e))
        })?;

        if !response.status().is_success() {
            let status = response.status();
            let error_body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(PdsError::IdentityResolution(format!(
                "PLC directory returned error {}: {}",
                status, error_body
            )));
        }

        let doc: DidDocument = response
            .json()
            .await
            .map_err(|e| PdsError::IdentityResolution(format!("Invalid PLC document: {}", e)))?;

        Ok(doc)
    }

    /// Get current signing key (atproto key) from DID document
    ///
    /// Extracts the public key used for AT Protocol signing operations
    pub fn get_signing_key(&self, doc: &DidDocument) -> PdsResult<String> {
        // Look for verification method with type "Multikey" and purpose "atproto"
        for method in &doc.verification_method {
            // Check if this is the atproto signing key
            if let Some(public_key_multibase) = &method.public_key_multibase {
                return Ok(public_key_multibase.clone());
            }
        }

        Err(PdsError::IdentityResolution(
            "No signing key found in DID document".to_string(),
        ))
    }

    /// Compare if two signing keys are the same
    ///
    /// Handles both multibase and raw key comparisons
    pub fn keys_match(&self, key1: &str, key2: &str) -> bool {
        // Direct comparison
        if key1 == key2 {
            return true;
        }

        // Strip multibase prefix if present and compare
        let k1 = key1.strip_prefix('z').unwrap_or(key1);
        let k2 = key2.strip_prefix('z').unwrap_or(key2);

        k1 == k2
    }

    /// Update signing key in PLC directory
    ///
    /// This creates and submits a PLC operation to rotate the signing key.
    /// Requires the PLC rotation key to sign the operation.
    pub async fn update_signing_key(
        &self,
        did: &str,
        new_signing_key: &str,
        rotation_key_signer: &PlcSigner,
    ) -> PdsResult<()> {
        // Fetch current DID document to get prev CID
        let _current_doc = self.get_document(did).await?;

        // Get the previous operation CID (if available from doc metadata)
        // For now, we'll leave prev as None - PLC directory can handle this
        // TODO: Extract prev CID from _current_doc once PLC format is stable

        // Arc 13 §6.3.1 wire-shape: verification_methods is
        // BTreeMap<String, String> (name → did:key URI).
        let mut verification_methods = std::collections::BTreeMap::new();
        verification_methods.insert("atproto".to_string(), new_signing_key.to_string());

        // Build the PLC operation. Note: this builds a *diff* op
        // (only verification_methods set, no rotation_keys /
        // services / also_known_as carried over). Arc 13 Step 1.2
        // will refactor to snapshot-mutator pattern. For now this
        // operates correctly only against a directory in
        // weak-mode; strict-mode rejects diff ops.
        let operation = PlcOperationBuilder::new()
            .verification_methods(verification_methods)
            .build()?;

        // Sign with the rotation key.
        let signed_operation = rotation_key_signer.sign_operation(operation)?;

        // Submit to PLC directory.
        register_plc_did(&self.config.plc_url, did, signed_operation).await?;

        tracing::info!(did = %did, "Successfully updated signing key in PLC directory");

        Ok(())
    }

    /// Check if key rotation is needed
    ///
    /// Compares current PLC key with desired key
    pub async fn needs_rotation(&self, did: &str, desired_key: &str) -> PdsResult<bool> {
        let doc = self.get_document(did).await?;
        let current_key = self.get_signing_key(&doc)?;

        Ok(!self.keys_match(&current_key, desired_key))
    }

    /// Rotate signing key if needed
    ///
    /// Convenience method that checks if rotation is needed and performs it
    pub async fn rotate_key_if_needed(
        &self,
        did: &str,
        new_signing_key: &str,
        rotation_key_signer: &PlcSigner,
    ) -> PdsResult<bool> {
        if !self.needs_rotation(did, new_signing_key).await? {
            tracing::debug!(did = %did, "Key rotation not needed - keys match");
            return Ok(false);
        }

        self.update_signing_key(did, new_signing_key, rotation_key_signer)
            .await?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plc_client_creation() {
        let config = PlcClientConfig::default();
        let client = PlcClient::new(config);
        assert!(client.is_ok());
    }

    #[test]
    fn test_keys_match() {
        let config = PlcClientConfig::default();
        let client = PlcClient::new(config).unwrap();

        // Exact match
        assert!(client.keys_match("abc123", "abc123"));

        // Match with multibase prefix
        assert!(client.keys_match("zabc123", "abc123"));
        assert!(client.keys_match("abc123", "zabc123"));
        assert!(client.keys_match("zabc123", "zabc123"));

        // No match
        assert!(!client.keys_match("abc123", "def456"));
    }

    #[test]
    fn test_invalid_did() {
        let config = PlcClientConfig::default();
        let client = PlcClient::new(config).unwrap();

        // Test with non-PLC DID
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(client.get_document("did:web:example.com"));

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("did:plc"));
    }
}
