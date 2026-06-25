//! PLC Directory Client - Handles key rotation and DID operations
//!
//! Provides high-level interface for interacting with the PLC directory,
//! including fetching DID documents, comparing keys, and updating signing keys.

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::identity::did_document::DidDocument;
use crate::{
    crypto::plc::{register_plc_did, PlcOperation, PlcOperationBuilder, PlcSigner},
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

/// One accepted PLC operation in a DID's signing-key history (key-rotation arc
/// #366 Phase A1). The full ordered list (from [`PlcClient::get_op_history`]) is
/// the canonical key history the history-aware verifier resolves each commit's
/// signing key against.
///
/// `op_cid` is kept as a `String` to match [`PlcClient::get_last_op`]'s CID
/// convention (the design's `Cid` placeholder is not load-bearing — the windowing
/// in Phase A2 keys off `accepted_at` + `signing_did_key`). `accepted_at` is the
/// operation's `createdAt`, which is the validity-window boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlcOpHistoryEntry {
    /// CID of the PLC operation.
    pub op_cid: String,
    /// The operation's accepted-at timestamp (`createdAt`) — the validity-window
    /// boundary for the key this operation publishes.
    pub accepted_at: DateTime<Utc>,
    /// The `atproto` signing key this operation publishes, in did:key form.
    pub signing_did_key: String,
}

/// Parse a PLC `/log/audit` response array into the ordered signing-key history.
///
/// Pure (no I/O) so the parse contract is unit-testable without an HTTP layer.
/// The audit-log shape (per `@did-plc/lib`, documented at `get_last_op`) is an
/// oldest-first array of `{cid, did, operation, nullified, createdAt}`; this
/// function is the multi-entry generalization of `get_last_op`'s per-entry
/// extraction (which already iterates the full array reading `nullified`, so the
/// homogeneous-entry shape is established by the existing parser, not assumed).
///
/// Skips nullified entries and tombstone operations (no `atproto` key). Fails
/// closed (`PdsError::IdentityResolution`) on a malformed entry rather than
/// silently dropping it — an incomplete history would resolve commits against the
/// wrong key in Phase A2.
fn parse_op_history(did: &str, entries: &[serde_json::Value]) -> PdsResult<Vec<PlcOpHistoryEntry>> {
    let mut history = Vec::new();
    for (idx, entry) in entries.iter().enumerate() {
        // Skip nullified (rejected) ops — accepted history only.
        if entry.get("nullified").and_then(|n| n.as_bool()).unwrap_or(false) {
            continue;
        }

        let created_at_str = entry.get("createdAt").and_then(|v| v.as_str()).ok_or_else(|| {
            PdsError::IdentityResolution(format!(
                "PLC audit-log entry {} for {} missing createdAt",
                idx, did
            ))
        })?;
        let accepted_at = DateTime::parse_from_rfc3339(created_at_str)
            .map_err(|e| {
                PdsError::IdentityResolution(format!(
                    "PLC audit-log entry {} for {} has unparseable createdAt '{}': {}",
                    idx, did, created_at_str, e
                ))
            })?
            .with_timezone(&Utc);

        let op_cid = entry
            .get("cid")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                PdsError::IdentityResolution(format!(
                    "PLC audit-log entry {} for {} missing cid",
                    idx, did
                ))
            })?
            .to_string();

        let op_value = entry.get("operation").ok_or_else(|| {
            PdsError::IdentityResolution(format!(
                "PLC audit-log entry {} for {} missing operation",
                idx, did
            ))
        })?;

        // A tombstone op publishes no signing key — it terminates the DID; skip
        // it (the windows before it stand). Detect by type before the full parse
        // so a tombstone's shape never trips the parse-fail-closed path.
        if op_value.get("type").and_then(|v| v.as_str()) == Some("plc_tombstone") {
            continue;
        }

        let op: PlcOperation = serde_json::from_value(op_value.clone()).map_err(|e| {
            PdsError::IdentityResolution(format!(
                "PLC audit-log entry {} for {}: unparseable operation: {}",
                idx, did, e
            ))
        })?;

        let signing_did_key = op.verification_methods.get("atproto").ok_or_else(|| {
            PdsError::IdentityResolution(format!(
                "PLC audit-log entry {} for {}: operation has no atproto verification method",
                idx, did
            ))
        })?;

        history.push(PlcOpHistoryEntry {
            op_cid,
            accepted_at,
            signing_did_key: signing_did_key.clone(),
        });
    }
    Ok(history)
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
            .iter().rfind(|e| !e.get("nullified").and_then(|n| n.as_bool()).unwrap_or(false))
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

    /// Fetch the full ordered history of accepted PLC operations for `did` from
    /// the directory's audit log (Arc 13 / key-rotation arc #366 Phase A1).
    ///
    /// Where [`Self::get_last_op`] returns only the last accepted entry, this
    /// returns every accepted (non-nullified) operation that publishes an
    /// `atproto` signing key, oldest-first — the key history a commit chain that
    /// spans rotations must be verified against (consumed by Phase A2's
    /// history-aware verifier).
    ///
    /// Each entry carries the operation CID, its `createdAt` timestamp (the
    /// validity-window boundary), and the `atproto` signing key did:key the
    /// operation publishes. Tombstone operations (no `atproto` key) and
    /// nullified entries are skipped.
    ///
    /// Errors (`PdsError::IdentityResolution`): network failure, non-2xx from the
    /// directory, malformed audit-log JSON.
    pub async fn get_op_history(&self, did: &str) -> PdsResult<Vec<PlcOpHistoryEntry>> {
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

        let entries: Vec<serde_json::Value> = response
            .json()
            .await
            .map_err(|e| PdsError::IdentityResolution(format!("Invalid PLC audit log JSON: {}", e)))?;

        parse_op_history(did, &entries)
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

/// The PLC-directory operations that rotation + rebuild flows reach through
/// `AppContext` (key-rotation arc #372 / B1). Behind a trait so unit tests can
/// substitute [`MockPlcClient`]. `PlcClient` is the production impl; `AppContext`
/// holds `Arc<dyn PlcClientApi>`.
///
/// Only the ctx-reached methods are here. `PlcClient`'s other inherent methods
/// (`get_last_op`, `needs_rotation`, `rotate_key_if_needed`) stay inherent for
/// the ad-hoc callers (identity / dev_routes) that hold a concrete `PlcClient`.
#[async_trait]
pub trait PlcClientApi: Send + Sync {
    async fn get_op_history(&self, did: &str) -> PdsResult<Vec<PlcOpHistoryEntry>>;
    async fn get_document(&self, did: &str) -> PdsResult<DidDocument>;
    fn get_signing_key(&self, doc: &DidDocument) -> PdsResult<String>;
    fn keys_match(&self, key1: &str, key2: &str) -> bool;
    async fn update_signing_key(
        &self,
        did: &str,
        new_signing_key: &str,
        rotation_key_signer: &PlcSigner,
    ) -> PdsResult<()>;
}

#[async_trait]
impl PlcClientApi for PlcClient {
    async fn get_op_history(&self, did: &str) -> PdsResult<Vec<PlcOpHistoryEntry>> {
        // Fully-qualified to call the inherent method (not recurse into the trait).
        PlcClient::get_op_history(self, did).await
    }
    async fn get_document(&self, did: &str) -> PdsResult<DidDocument> {
        PlcClient::get_document(self, did).await
    }
    fn get_signing_key(&self, doc: &DidDocument) -> PdsResult<String> {
        PlcClient::get_signing_key(self, doc)
    }
    fn keys_match(&self, key1: &str, key2: &str) -> bool {
        PlcClient::keys_match(self, key1, key2)
    }
    async fn update_signing_key(
        &self,
        did: &str,
        new_signing_key: &str,
        rotation_key_signer: &PlcSigner,
    ) -> PdsResult<()> {
        PlcClient::update_signing_key(self, did, new_signing_key, rotation_key_signer).await
    }
}

/// Test-only mock of [`PlcClientApi`] (key-rotation arc #372 / B1). Returns
/// pre-configured op-histories; the other methods are minimal stubs B3 extends
/// when the rotation write-path needs `update_signing_key` responses mocked.
/// Tests inject it by reassigning `AppContext::plc_client` (a `pub` field).
#[cfg(test)]
pub(crate) struct MockPlcClient {
    op_histories: std::collections::HashMap<String, Vec<PlcOpHistoryEntry>>,
}

#[cfg(test)]
impl MockPlcClient {
    pub(crate) fn new() -> Self {
        Self { op_histories: std::collections::HashMap::new() }
    }
    pub(crate) fn with_op_history(mut self, did: &str, history: Vec<PlcOpHistoryEntry>) -> Self {
        self.op_histories.insert(did.to_string(), history);
        self
    }
}

#[cfg(test)]
#[async_trait]
impl PlcClientApi for MockPlcClient {
    async fn get_op_history(&self, did: &str) -> PdsResult<Vec<PlcOpHistoryEntry>> {
        self.op_histories
            .get(did)
            .cloned()
            .ok_or_else(|| PdsError::NotFound(format!("mock: no op history configured for {did}")))
    }
    async fn get_document(&self, _did: &str) -> PdsResult<DidDocument> {
        Err(PdsError::Internal("mock: get_document not configured".into()))
    }
    fn get_signing_key(&self, _doc: &DidDocument) -> PdsResult<String> {
        Err(PdsError::Internal("mock: get_signing_key not configured".into()))
    }
    fn keys_match(&self, key1: &str, key2: &str) -> bool {
        key1 == key2
    }
    async fn update_signing_key(
        &self,
        _did: &str,
        _new_signing_key: &str,
        _rotation_key_signer: &PlcSigner,
    ) -> PdsResult<()> {
        Ok(())
    }
}

/// Build a mock PLC op-history from `(signing_did_key, accepted_at_rfc3339)`
/// pairs, ascending. CIDs are test-fake (derived from the key string).
#[cfg(test)]
pub(crate) fn mock_op_history(entries: &[(&str, &str)]) -> Vec<PlcOpHistoryEntry> {
    entries
        .iter()
        .map(|(key, at)| PlcOpHistoryEntry {
            op_cid: format!("bafymock-{key}"),
            accepted_at: DateTime::parse_from_rfc3339(at)
                .expect("test op-history timestamp is valid RFC3339")
                .with_timezone(&Utc),
            signing_did_key: (*key).to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_plc_client_returns_configured_op_history() {
        let mock = MockPlcClient::new().with_op_history(
            "did:plc:a",
            mock_op_history(&[
                ("did:key:zK1", "2026-01-01T00:00:00Z"),
                ("did:key:zK2", "2026-02-01T00:00:00Z"),
            ]),
        );
        // Exercise through the trait object (the shape AppContext uses).
        let api: &dyn PlcClientApi = &mock;
        let h = api.get_op_history("did:plc:a").await.unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].signing_did_key, "did:key:zK1");
        assert!(h[0].accepted_at < h[1].accepted_at);
        assert!(
            api.get_op_history("did:plc:unconfigured").await.is_err(),
            "unconfigured DID errors (no silent empty history)"
        );
    }

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

    // ---- parse_op_history (Phase A1 / #367) ----

    /// Build one audit-log entry. A `None` atproto key produces a tombstone op.
    fn audit_entry(cid: &str, created_at: &str, atproto: Option<&str>, nullified: bool) -> serde_json::Value {
        let operation = match atproto {
            Some(key) => serde_json::json!({
                "type": "plc_operation",
                "rotationKeys": ["did:key:zRotation"],
                "verificationMethods": { "atproto": key },
                "alsoKnownAs": ["at://alice.example"],
                "services": {},
            }),
            None => serde_json::json!({
                "type": "plc_tombstone",
                "rotationKeys": [],
                "verificationMethods": {},
                "alsoKnownAs": [],
                "services": {},
            }),
        };
        serde_json::json!({
            "cid": cid,
            "did": "did:plc:alice",
            "operation": operation,
            "nullified": nullified,
            "createdAt": created_at,
        })
    }

    #[test]
    fn parse_op_history_single_entry() {
        let entries = vec![audit_entry("cidA", "2026-01-01T00:00:00Z", Some("did:key:zK1"), false)];
        let h = parse_op_history("did:plc:alice", &entries).unwrap();
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].op_cid, "cidA");
        assert_eq!(h[0].signing_did_key, "did:key:zK1");
        assert_eq!(h[0].accepted_at.to_rfc3339(), "2026-01-01T00:00:00+00:00");
    }

    #[test]
    fn parse_op_history_three_entries_ascending() {
        // PLC returns oldest-first; we preserve that order.
        let entries = vec![
            audit_entry("cid1", "2026-01-01T00:00:00Z", Some("did:key:zK1"), false),
            audit_entry("cid2", "2026-02-01T00:00:00Z", Some("did:key:zK2"), false),
            audit_entry("cid3", "2026-03-01T00:00:00Z", Some("did:key:zK3"), false),
        ];
        let h = parse_op_history("did:plc:alice", &entries).unwrap();
        assert_eq!(h.len(), 3);
        assert_eq!(
            h.iter().map(|e| e.signing_did_key.as_str()).collect::<Vec<_>>(),
            vec!["did:key:zK1", "did:key:zK2", "did:key:zK3"]
        );
        assert!(h[0].accepted_at < h[1].accepted_at && h[1].accepted_at < h[2].accepted_at);
    }

    #[test]
    fn parse_op_history_skips_nullified() {
        let entries = vec![
            audit_entry("cid1", "2026-01-01T00:00:00Z", Some("did:key:zK1"), false),
            audit_entry("cidX", "2026-01-15T00:00:00Z", Some("did:key:zBad"), true), // rejected
            audit_entry("cid2", "2026-02-01T00:00:00Z", Some("did:key:zK2"), false),
        ];
        let h = parse_op_history("did:plc:alice", &entries).unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].signing_did_key, "did:key:zK1");
        assert_eq!(h[1].signing_did_key, "did:key:zK2");
    }

    #[test]
    fn parse_op_history_skips_tombstone() {
        let entries = vec![
            audit_entry("cid1", "2026-01-01T00:00:00Z", Some("did:key:zK1"), false),
            audit_entry("cidT", "2026-02-01T00:00:00Z", None, false), // tombstone
        ];
        let h = parse_op_history("did:plc:alice", &entries).unwrap();
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].signing_did_key, "did:key:zK1");
    }

    #[test]
    fn parse_op_history_fails_closed_on_bad_timestamp() {
        let entries = vec![audit_entry("cidA", "not-a-timestamp", Some("did:key:zK1"), false)];
        let err = parse_op_history("did:plc:alice", &entries).unwrap_err();
        assert!(err.to_string().contains("createdAt"));
    }

    #[test]
    fn parse_op_history_fails_closed_on_missing_cid() {
        let mut e = audit_entry("cidA", "2026-01-01T00:00:00Z", Some("did:key:zK1"), false);
        e.as_object_mut().unwrap().remove("cid");
        let err = parse_op_history("did:plc:alice", &[e]).unwrap_err();
        assert!(err.to_string().contains("cid"));
    }

    #[test]
    fn parse_op_history_empty_array_is_empty_history() {
        let h = parse_op_history("did:plc:alice", &[]).unwrap();
        assert!(h.is_empty());
    }
}
