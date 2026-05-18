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
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateHandleRequest {
    /// New handle for the user
    pub handle: String,
}

pub async fn update_handle(
    State(ctx): State<AppContext>,
    auth: crate::auth::AuthContextForwarded,
    Json(req): Json<UpdateHandleRequest>,
) -> PdsResult<Json<()>> {
    let did = auth.did;

    // Arc 12 §5.3.8 mint-pattern forward.
    if ctx.entryway_client.is_some() {
        let headers = ctx
            .entryway_auth_headers(&did, "com.atproto.identity.updateHandle")
            .await?;
        // updateHandle's upstream response is empty (`{}`); decode as
        // serde_json::Value and discard.
        let _: serde_json::Value = ctx
            .entryway_client
            .as_ref()
            .expect("checked above")
            .xrpc_post_json("com.atproto.identity.updateHandle", headers, &req)
            .await?;
        return Ok(Json(()));
    }

    // Standalone path (unchanged).
    // Validate handle format
    if req.handle.is_empty() {
        return Err(PdsError::Validation("Handle cannot be empty".to_string()));
    }

    // Basic handle validation (lowercase, alphanumeric + dots/hyphens)
    if !req
        .handle
        .chars()
        .all(|c| c.is_alphanumeric() || c == '.' || c == '-')
    {
        return Err(PdsError::Validation(
            "Handle contains invalid characters".to_string(),
        ));
    }

    // Check handle length (max 253 chars for DNS compatibility)
    if req.handle.len() > 253 {
        return Err(PdsError::Validation(
            "Handle too long (max 253 characters)".to_string(),
        ));
    }

    // Normalize handle to lowercase
    let new_handle = req.handle.to_lowercase();

    // For did:plc, submit handle update to PLC directory.
    //
    // Arc 13 §6.3.6 / Step 1.2 snapshot-mutator pattern: fetch
    // the last accepted op via `PlcClient::get_last_op` (§6.3.4),
    // inherit ALL its fields (rotation_keys, verification_methods,
    // services), mutate ONLY `also_known_as` for the new handle,
    // set `prev` to the prior op's CID, sign with the PDS-wide
    // rotation key (§6.3.2), submit. Diff-build is gone.
    if did.starts_with("did:plc:") {
        use crate::crypto::plc::{register_plc_did, PlcOperationBuilder, PlcSigner};
        use crate::crypto::plc_client::{PlcClient, PlcClientConfig};

        let plc_client = PlcClient::new(PlcClientConfig {
            plc_url: ctx.config.identity.did_plc_url.clone(),
            ..Default::default()
        })?;

        // §6.3.4: full-fetch the last accepted op. Tombstoned →
        // PdsError::DidTombstoned → HTTP 400 via IntoResponse.
        let (last_op, last_cid) = plc_client.get_last_op(&did).await?;

        // §6.3.6 mutator: inherit every field from last_op,
        // override `also_known_as` with `[at://{new_handle}]`,
        // set prev to last_cid, sign.
        let unsigned = PlcOperationBuilder::new()
            .rotation_keys(last_op.rotation_keys.clone())
            .verification_methods(last_op.verification_methods.clone())
            .services(last_op.services.clone())
            .also_known_as(vec![format!("at://{}", new_handle)])
            .prev(last_cid)
            .build()?;

        // §6.3.2: PDS-wide rotation key from config signs every
        // update op (its did:key is in `rotation_keys` inherited
        // from the genesis op, satisfying chainlink #61 §1.4.5
        // signer-in-rotation-keys invariant).
        let signer = PlcSigner::from_hex(&ctx.config.authentication.plc_rotation_key)?;
        let signed_operation = signer.sign_operation(unsigned)?;

        // Submit via the spec-correct register_plc_did helper
        // (POSTs the signed op JSON to `{plc_url}/{did}`).
        register_plc_did(
            &ctx.config.identity.did_plc_url,
            &did,
            signed_operation,
        )
        .await?;

        tracing::info!(
            "Successfully submitted PLC handle update for {}: {}",
            did,
            new_handle
        );
    }

    // Update handle via identity resolver
    // This will verify the handle resolves to this DID
    ctx.identity_resolver
        .update_handle(&did, &new_handle)
        .await?;

    // Update account table with new handle
    let old_handle = ctx.account_manager.update_handle(&did, &new_handle).await?;

    // Invalidate old handle in cache (force re-resolution)
    ctx.identity_resolver.invalidate_handle(&old_handle).await?;

    // Emit identity event to sequencer for firehose consumers
    use crate::sequencer::events::IdentityEvent;
    let identity_event = IdentityEvent::new(did.clone(), Some(new_handle.clone()));
    ctx.sequencer.sequence_identity(identity_event).await?;

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
#[derive(Debug, Serialize, Deserialize)]
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

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignPlcOperationResponse {
    /// Signed operation
    pub operation: serde_json::Value,
}

pub async fn sign_plc_operation(
    State(ctx): State<AppContext>,
    auth: crate::auth::AuthContextForwarded,
    Json(req): Json<SignPlcOperationRequest>,
) -> PdsResult<Json<SignPlcOperationResponse>> {
    let did = auth.did;

    // Arc 12 §5.3.8 mint-pattern forward. When entryway mode is
    // configured, mint a service-auth JWT scoped to this NSID via
    // `entryway_auth_headers` and proxy the request body upstream;
    // the entryway is the canonical signer in that deployment shape.
    if ctx.entryway_client.is_some() {
        let headers = ctx
            .entryway_auth_headers(&did, "com.atproto.identity.signPlcOperation")
            .await?;
        let resp: SignPlcOperationResponse = ctx
            .entryway_client
            .as_ref()
            .expect("checked above")
            .xrpc_post_json("com.atproto.identity.signPlcOperation", headers, &req)
            .await?;
        return Ok(Json(resp));
    }

    // Standalone path (unchanged).
    // Ensure this is a did:plc
    if !did.starts_with("did:plc:") {
        return Err(PdsError::Validation(
            "Only did:plc identifiers support PLC operations".to_string(),
        ));
    }

    // Arc 13 Step 1.3: snapshot-mutator skeleton lands here.
    // Step 3.4 layers email-token two-phase (validate-only first,
    // consume-after-sign) on top of this; until Step 3.4 the
    // standalone-mode handler still returns 501 because the
    // SignPlcOperationRequest input shape (`Vec<String>` for
    // verification_methods, `serde_json::Value` for services)
    // can't safely express the new BTreeMap-keyed wire shape per
    // §6.3.1 — Step 3.4 also updates the input shape.
    //
    // The snapshot-mutator scaffolding below (full-fetch via
    // get_last_op, inherit-all-fields-from-last_op) is what
    // Step 3.4's email-token-enabled rewrite will build on. We
    // exercise it here (read-only) so the code path is verified
    // against the directory shape before Step 3.4 adds the
    // token + override logic.
    //
    // Entryway-mode forwarding (the branch above this point) still
    // works — Arc 12's §5.3.8 handler-forward path is unchanged
    // and is the production code path when entryway is configured.
    use crate::crypto::plc_client::{PlcClient, PlcClientConfig};
    let plc_client = PlcClient::new(PlcClientConfig {
        plc_url: ctx.config.identity.did_plc_url.clone(),
        ..Default::default()
    })?;
    let (_last_op, _last_cid) = plc_client.get_last_op(&did).await?;

    let _ = req;
    Err(PdsError::Internal(
        "sign_plc_operation standalone-mode path pending Arc 13 Step 3.4 rewrite \
         (snapshot-mutator scaffolding is in place; email-token two-phase flow + \
         caller-override-shape redesign land at Step 3.4 per V05_DESIGN.md §6.3.6)"
            .to_string(),
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

    // Validate operation format - must have required fields
    let operation = &req.operation;

    // Check that operation contains required fields
    if !operation.is_object() {
        return Err(PdsError::Validation(
            "Operation must be a JSON object".to_string(),
        ));
    }

    let obj = operation.as_object().unwrap();

    // Validate required fields
    if !obj.contains_key("type")
        || obj.get("type").and_then(|v| v.as_str()) != Some("plc_operation")
    {
        return Err(PdsError::Validation(
            "Operation must have type=plc_operation".to_string(),
        ));
    }

    if !obj.contains_key("did") {
        return Err(PdsError::Validation(
            "Operation must contain 'did'".to_string(),
        ));
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
        let error_body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
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
    let audit_log: serde_json::Value = response
        .json()
        .await
        .map_err(|e| PdsError::Internal(format!("Failed to parse PLC response: {}", e)))?;

    // Extract the last operation CID as token
    let token = if let Some(operations) = audit_log.as_array() {
        if let Some(last_op) = operations.last() {
            if let Some(cid) = last_op.get("cid").and_then(|v| v.as_str()) {
                cid.to_string()
            } else {
                return Err(PdsError::Internal("PLC audit log missing CID".to_string()));
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
        assert!("alice.bsky.social"
            .chars()
            .all(|c| c.is_alphanumeric() || c == '.' || c == '-'));
        assert!("bob-test.com"
            .chars()
            .all(|c| c.is_alphanumeric() || c == '.' || c == '-'));

        // Invalid handles
        assert!(!"alice@test.com"
            .chars()
            .all(|c| c.is_alphanumeric() || c == '.' || c == '-'));
        assert!(!"alice test"
            .chars()
            .all(|c| c.is_alphanumeric() || c == '.' || c == '-'));
    }

    #[test]
    fn test_handle_length() {
        let short_handle = "a.com";
        assert!(short_handle.len() <= 253);

        let long_handle = "a".repeat(254);
        assert!(long_handle.len() > 253);
    }
}
