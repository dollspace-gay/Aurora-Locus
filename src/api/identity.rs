/// Identity API endpoints
/// Implements com.atproto.identity.* endpoints for handle and DID resolution
use crate::{
    auth::AuthContext,
    crypto::plc::{PlcOperationBuilder, PlcSigner},
    error::{PdsError, PdsResult},
    AppContext,
};
use axum::{
    extract::{Query, State},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

/// com.atproto.identity.resolveHandle
///
/// Resolve a handle to a DID
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveHandleParams {
    /// Handle to resolve (e.g., "alice.bsky.social")
    pub handle: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveHandleResponse {
    pub did: String,
}

pub async fn resolve_handle(
    State(ctx): State<AppContext>,
    Query(params): Query<ResolveHandleParams>,
) -> PdsResult<Json<ResolveHandleResponse>> {
    // Validate handle format
    if params.handle.is_empty() {
        return Err(PdsError::Validation("Handle cannot be empty".to_string()));
    }

    // Resolve via identity resolver (with caching)
    let did = ctx.identity_resolver.resolve_handle(&params.handle).await?;

    Ok(Json(ResolveHandleResponse { did }))
}

/// com.atproto.identity.updateHandle
///
/// Update the handle for the authenticated user's DID
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateHandleRequest {
    /// New handle for the user
    pub handle: String,
}

pub async fn update_handle(
    State(ctx): State<AppContext>,
    auth: AuthContext,
    Json(req): Json<UpdateHandleRequest>,
) -> PdsResult<Json<()>> {
    let did = auth.did;

    // Validate handle format
    if req.handle.is_empty() {
        return Err(PdsError::Validation("Handle cannot be empty".to_string()));
    }

    // Basic handle validation (lowercase, alphanumeric + dots/hyphens)
    if !req.handle.chars().all(|c| c.is_alphanumeric() || c == '.' || c == '-') {
        return Err(PdsError::Validation(
            "Handle contains invalid characters".to_string(),
        ));
    }

    // Check handle length (max 253 chars for DNS compatibility)
    if req.handle.len() > 253 {
        return Err(PdsError::Validation("Handle too long (max 253 characters)".to_string()));
    }

    // Normalize handle to lowercase
    let new_handle = req.handle.to_lowercase();

    // Update handle via identity resolver
    // This will verify the handle resolves to this DID
    ctx.identity_resolver
        .update_handle(&did, &new_handle)
        .await?;

    // Update account table with new handle
    let old_handle = ctx.account_manager
        .update_handle(&did, &new_handle)
        .await?;

    // Invalidate old handle in cache (force re-resolution)
    ctx.identity_resolver
        .invalidate_handle(&old_handle)
        .await?;

    // Emit identity event to sequencer for firehose consumers
    use crate::sequencer::events::IdentityEvent;
    let identity_event = IdentityEvent::new(did.clone(), Some(new_handle.clone()));
    ctx.sequencer
        .sequence_identity(identity_event)
        .await?;

    Ok(Json(()))
}

/// com.atproto.identity.getRecommendedDidCredentials
///
/// Get recommended DID credentials (for migration/key rotation)
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecommendedDidCredentialsResponse {
    /// Rotation keys that can be used
    pub rotation_keys: Vec<String>,
    /// Also known as (alternate identifiers)
    pub also_known_as: Vec<String>,
    /// Verification methods
    pub verification_methods: Vec<VerificationMethod>,
    /// Services
    pub services: Vec<Service>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationMethod {
    pub id: String,
    #[serde(rename = "type")]
    pub method_type: String,
    pub controller: String,
    pub public_key_multibase: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Service {
    pub id: String,
    #[serde(rename = "type")]
    pub service_type: String,
    pub service_endpoint: String,
}

pub async fn get_recommended_did_credentials(
    State(ctx): State<AppContext>,
    auth: AuthContext,
) -> PdsResult<Json<RecommendedDidCredentialsResponse>> {
    let did = auth.did;

    // Fetch current DID document
    let doc = ctx.identity_resolver.resolve_did(&did).await?;

    // Extract rotation keys
    let rotation_keys: Vec<String> = doc
        .verification_method
        .iter()
        .filter_map(|vm| {
            // Only include keys that can be used for rotation
            if vm.id.contains("#atproto") {
                vm.public_key_multibase.clone()
            } else {
                None
            }
        })
        .collect();

    // Map verification methods
    let verification_methods: Vec<VerificationMethod> = doc
        .verification_method
        .iter()
        .map(|vm| VerificationMethod {
            id: vm.id.clone(),
            method_type: vm.key_type.clone(),
            controller: vm.controller.clone(),
            public_key_multibase: vm.public_key_multibase.clone().unwrap_or_default(),
        })
        .collect();

    // Map services
    let services: Vec<Service> = doc
        .service
        .iter()
        .map(|s| Service {
            id: s.id.clone(),
            service_type: s.service_type.clone(),
            service_endpoint: s.service_endpoint.clone(),
        })
        .collect();

    Ok(Json(RecommendedDidCredentialsResponse {
        rotation_keys,
        also_known_as: doc.also_known_as,
        verification_methods,
        services,
    }))
}

/// com.atproto.identity.signPlcOperation
///
/// Sign a PLC operation for DID:PLC update
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignPlcOperationRequest {
    /// Token from PLC directory
    pub token: Option<String>,
    /// Rotation keys
    pub rotation_keys: Option<Vec<String>>,
    /// Also known as
    pub also_known_as: Option<Vec<String>>,
    /// Verification methods
    pub verification_methods: Option<Vec<String>>,
    /// Services
    pub services: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignPlcOperationResponse {
    /// Signed operation
    pub operation: serde_json::Value,
}

pub async fn sign_plc_operation(
    State(ctx): State<AppContext>,
    auth: AuthContext,
    Json(req): Json<SignPlcOperationRequest>,
) -> PdsResult<Json<SignPlcOperationResponse>> {
    let did = auth.did;

    // Ensure this is a did:plc
    if !did.starts_with("did:plc:") {
        return Err(PdsError::Validation(
            "Only did:plc identifiers support PLC operations".to_string(),
        ));
    }

    // Load the PLC rotation key from config
    let plc_key_hex = &ctx.config.authentication.plc_rotation_key;
    let signer = PlcSigner::from_hex(plc_key_hex)?;

    // Get current DID document to extract previous operation CID
    let _did_doc = ctx.identity_resolver.resolve_did(&did).await?;
    // In production, extract prev CID from DID doc metadata

    // Build PLC operation
    let mut builder = PlcOperationBuilder::new()
        .did(did.clone());

    // Add previous operation CID if available
    if let Some(token) = req.token {
        // Token from PLC directory contains previous CID
        builder = builder.prev(token);
    }

    // Add rotation keys
    if let Some(rotation_keys) = req.rotation_keys {
        builder = builder.rotation_keys(rotation_keys);
    }

    // Add also known as (handles)
    if let Some(also_known_as) = req.also_known_as {
        builder = builder.also_known_as(also_known_as);
    }

    // Add verification methods
    if let Some(verification_methods) = req.verification_methods {
        let vm_json = serde_json::to_value(verification_methods).map_err(|e| {
            PdsError::Validation(format!("Invalid verification methods: {}", e))
        })?;
        builder = builder.verification_methods(vm_json);
    }

    // Add services
    if let Some(services) = req.services {
        builder = builder.services(services);
    }

    // Build unsigned operation
    let operation = builder.build()?;

    // Sign the operation
    let signed_operation = signer.sign_operation(operation)?;

    // Convert to JSON value
    let operation_json = serde_json::to_value(&signed_operation).map_err(|e| {
        PdsError::Internal(format!("Failed to serialize operation: {}", e))
    })?;

    Ok(Json(SignPlcOperationResponse {
        operation: operation_json,
    }))
}

/// com.atproto.identity.submitPlcOperation
///
/// Submit a signed PLC operation to update DID:PLC
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitPlcOperationRequest {
    /// Signed operation
    pub operation: serde_json::Value,
}

pub async fn submit_plc_operation(
    State(ctx): State<AppContext>,
    auth: AuthContext,
    Json(req): Json<SubmitPlcOperationRequest>,
) -> PdsResult<Json<()>> {
    let did = auth.did;

    // Ensure this is a did:plc
    if !did.starts_with("did:plc:") {
        return Err(PdsError::Validation(
            "Only did:plc identifiers support PLC operations".to_string(),
        ));
    }

    // Validate operation format - must have required fields
    let operation = &req.operation;

    // Check that operation contains required fields
    if !operation.is_object() {
        return Err(PdsError::Validation("Operation must be a JSON object".to_string()));
    }

    let obj = operation.as_object().unwrap();

    // Validate required fields
    if !obj.contains_key("type") || obj.get("type").and_then(|v| v.as_str()) != Some("plc_operation") {
        return Err(PdsError::Validation(
            "Operation must have type=plc_operation".to_string(),
        ));
    }

    if !obj.contains_key("did") {
        return Err(PdsError::Validation("Operation must contain 'did'".to_string()));
    }

    if !obj.contains_key("sig") {
        return Err(PdsError::Validation("Operation must be signed".to_string()));
    }

    // Verify the DID in the operation matches the authenticated user
    if let Some(op_did) = obj.get("did").and_then(|v| v.as_str()) {
        if op_did != did {
            return Err(PdsError::Authorization(
                "Operation DID does not match authenticated user".to_string(),
            ));
        }
    }

    // Submit to PLC directory
    let plc_url = &ctx.config.identity.did_plc_url;
    let submit_endpoint = format!("{}/{}", plc_url, did);

    // Create HTTP client
    let http_client = reqwest::Client::new();

    // Submit the operation
    let response = http_client
        .post(&submit_endpoint)
        .json(&req.operation)
        .send()
        .await
        .map_err(|e| PdsError::Internal(format!("Failed to submit to PLC directory: {}", e)))?;

    // Check response status
    if !response.status().is_success() {
        let status = response.status();
        let error_body = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
        return Err(PdsError::Internal(format!(
            "PLC directory returned error {}: {}",
            status, error_body
        )));
    }

    // Invalidate cached DID document so it will be refreshed on next resolution
    ctx.identity_resolver.invalidate_did(&did).await?;

    Ok(Json(()))
}

/// com.atproto.identity.requestPlcOperationSignature
///
/// Request a signature token from PLC directory for updating DID
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestPlcOperationSignatureResponse {
    /// Token for signing operation
    pub token: String,
}

pub async fn request_plc_operation_signature(
    State(ctx): State<AppContext>,
    auth: AuthContext,
) -> PdsResult<Json<RequestPlcOperationSignatureResponse>> {
    let did = auth.did;

    // Ensure this is a did:plc
    if !did.starts_with("did:plc:") {
        return Err(PdsError::Validation(
            "Only did:plc identifiers support PLC operations".to_string(),
        ));
    }

    // Make request to PLC directory for signature token
    let plc_url = &ctx.config.identity.did_plc_url;
    let token_endpoint = format!("{}/{}/log/audit", plc_url, did);

    // Create HTTP client for this request
    let http_client = reqwest::Client::new();

    // Request signature token from PLC directory
    let response = http_client
        .get(&token_endpoint)
        .send()
        .await
        .map_err(|e| PdsError::Internal(format!("Failed to contact PLC directory: {}", e)))?;

    if !response.status().is_success() {
        return Err(PdsError::Internal(format!(
            "PLC directory returned error: {}",
            response.status()
        )));
    }

    // Parse response to extract token (last operation CID)
    let audit_log: serde_json::Value = response.json().await.map_err(|e| {
        PdsError::Internal(format!("Failed to parse PLC response: {}", e))
    })?;

    // Extract the last operation CID as token
    let token = if let Some(operations) = audit_log.as_array() {
        if let Some(last_op) = operations.last() {
            if let Some(cid) = last_op.get("cid").and_then(|v| v.as_str()) {
                cid.to_string()
            } else {
                return Err(PdsError::Internal(
                    "PLC audit log missing CID".to_string(),
                ));
            }
        } else {
            // No previous operations, return empty token (genesis operation)
            String::new()
        }
    } else {
        return Err(PdsError::Internal(
            "Invalid PLC audit log format".to_string(),
        ));
    };

    Ok(Json(RequestPlcOperationSignatureResponse { token }))
}

/// Build identity API routes
pub fn routes() -> Router<AppContext> {
    Router::new()
        // Public endpoints (no auth required)
        .route(
            "/xrpc/com.atproto.identity.resolveHandle",
            get(resolve_handle),
        )
        // Authenticated endpoints
        .route(
            "/xrpc/com.atproto.identity.updateHandle",
            post(update_handle),
        )
        .route(
            "/xrpc/com.atproto.identity.getRecommendedDidCredentials",
            get(get_recommended_did_credentials),
        )
        .route(
            "/xrpc/com.atproto.identity.requestPlcOperationSignature",
            post(request_plc_operation_signature),
        )
        .route(
            "/xrpc/com.atproto.identity.signPlcOperation",
            post(sign_plc_operation),
        )
        .route(
            "/xrpc/com.atproto.identity.submitPlcOperation",
            post(submit_plc_operation),
        )
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_handle_validation() {
        // Valid handles
        assert!("alice.bsky.social".chars().all(|c| c.is_alphanumeric() || c == '.' || c == '-'));
        assert!("bob-test.com".chars().all(|c| c.is_alphanumeric() || c == '.' || c == '-'));

        // Invalid handles
        assert!(!"alice@test.com".chars().all(|c| c.is_alphanumeric() || c == '.' || c == '-'));
        assert!(!"alice test".chars().all(|c| c.is_alphanumeric() || c == '.' || c == '-'));
    }

    #[test]
    fn test_handle_length() {
        let short_handle = "a.com";
        assert!(short_handle.len() <= 253);

        let long_handle = "a".repeat(254);
        assert!(long_handle.len() > 253);
    }
}
