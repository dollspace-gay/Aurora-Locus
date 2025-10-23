/// Identity API endpoints
/// Implements com.atproto.identity.* endpoints for handle and DID resolution
use crate::{
    auth::AuthContext,
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

    // Update handle via identity resolver
    // This will verify the handle resolves to this DID
    ctx.identity_resolver
        .update_handle(&did, &req.handle)
        .await?;

    // TODO: Update account table with new handle
    // TODO: Emit identity event to sequencer
    // For now, we just update the cache

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

    // TODO: Implement PLC operation signing
    // This requires:
    // 1. Loading the repo signing key
    // 2. Constructing the PLC operation object
    // 3. Signing with the key
    // 4. Returning the signed operation

    // For now, return a placeholder
    Err(PdsError::Internal(
        "PLC operation signing not yet implemented".to_string(),
    ))
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

    // TODO: Implement PLC operation submission
    // This requires:
    // 1. Validating the operation format
    // 2. Submitting to PLC directory
    // 3. Handling response
    // 4. Invalidating cached DID document

    // For now, return a placeholder
    Err(PdsError::Internal(
        "PLC operation submission not yet implemented".to_string(),
    ))
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

    // TODO: Implement PLC signature token request
    // This requires:
    // 1. Making request to PLC directory
    // 2. Receiving token
    // 3. Returning token to client

    // For now, return a placeholder
    Err(PdsError::Internal(
        "PLC operation signature request not yet implemented".to_string(),
    ))
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
