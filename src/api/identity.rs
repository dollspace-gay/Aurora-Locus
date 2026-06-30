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
    if let Some(entryway) = ctx.entryway_client.as_ref() {
        let headers = ctx
            .entryway_auth_headers(&did, "com.atproto.identity.updateHandle")
            .await?;
        // updateHandle's upstream response is empty (`{}`); decode as
        // serde_json::Value and discard.
        let _: serde_json::Value = entryway
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
    // v0.10 Arc 1 §6 served-identity-input audit (AD-2 β): only did:plc accounts
    // republish a handle change to the PLC directory. A did:web handle change has
    // no PLC doc to republish — the local `actor.handle` UPDATE below runs for
    // both methods, and a did:web account's served `alsoKnownAs` recomposes from
    // `actor.handle` at the per-account serve route (Phase D). No method guard
    // here: handle mutation is allowed for both, only the PLC submission is gated.
    if crate::identity::did_method::is_plc(&did) {
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
    // Arc 13 §6.3.6 Step 3.6 rewrite — return *server-recommended*
    // credentials (what an account migrating TO this PDS should
    // adopt), not the credentials currently in the resolved DID
    // document.
    //
    // Per §6.3.2 + §6.3.3 the recommended set is:
    //   - rotation_keys = §6.3.3 priority-ordered list
    //     [account_recovery? (not knowable server-side),
    //      config.identity.recovery_did_key?,
    //      config.authentication.plc_rotation_key.did_key()]
    //   - verification_methods = {atproto: <per-actor signing key
    //     did:key from plc_keys.atproto_signing_key>}
    //   - services = {atproto_pds: {type, endpoint}}
    //   - also_known_as = the account's handle
    //
    // The per-account recovery_key isn't included here — it's
    // known only to the account holder; the server's recommendation
    // is "your existing recovery_key + the server's PDS recovery +
    // rotation keys."
    use crate::crypto::plc::PlcSigner;

    let did = auth.did;

    // PDS-wide rotation key did:key (always present).
    let pds_rotation_signer =
        PlcSigner::from_hex(&ctx.config.authentication.plc_rotation_key)?;
    let pds_rotation_did_key = pds_rotation_signer.public_key_did_key();

    // Per-actor atproto signing key from plc_keys (Arc 12 Step 1.5
    // column).
    let plc_row: Option<(String,)> =
        sqlx::query_as("SELECT atproto_signing_key FROM plc_keys WHERE did = $1")
            .bind(&did)
            .fetch_optional(&ctx.account_db)
            .await
            .map_err(PdsError::Database)?;
    let atproto_signing_key_hex = plc_row
        .map(|(k,)| k)
        .filter(|k| !k.is_empty())
        .ok_or_else(|| {
            PdsError::NotFound(format!(
                "No plc_keys.atproto_signing_key for {} \
                 (account either pre-Arc-12-Step-1.5 vintage or absent)",
                did
            ))
        })?;
    let atproto_signer = PlcSigner::from_hex(&atproto_signing_key_hex)?;
    let _atproto_did_key = atproto_signer.public_key_did_key();

    // §6.3.3 priority order. Per-account recovery_key isn't
    // surfaced here (server-side doesn't know it).
    let mut rotation_keys = Vec::with_capacity(2);
    if let Some(pds_recovery) = &ctx.config.identity.recovery_did_key {
        if !pds_recovery.is_empty() {
            rotation_keys.push(pds_recovery.clone());
        }
    }
    rotation_keys.push(pds_rotation_did_key);

    // verification_methods + services match what generate_plc_did
    // wires for new accounts.
    let verification_methods = vec![VerificationMethod {
        id: format!("{}#atproto", did),
        method_type: "Multikey".to_string(),
        controller: did.clone(),
        public_key_multibase: atproto_signer.public_key_multibase(),
    }];
    let services = vec![Service {
        id: "#atproto_pds".to_string(),
        service_type: "AtprotoPersonalDataServer".to_string(),
        service_endpoint: ctx.service_url(),
    }];

    // also_known_as from the account's handle.
    let account = ctx.account_manager.get_account(&did).await?;
    let also_known_as = account
        .handle
        .map(|h| vec![format!("at://{}", h)])
        .unwrap_or_default();

    // Forget pds_rotation_did_key after use (its private key is
    // never returned; only the did:key URI).
    let _ = pds_rotation_signer;

    Ok(Json(RecommendedDidCredentialsResponse {
        rotation_keys,
        also_known_as,
        verification_methods,
        services,
    }))
}

/// com.atproto.identity.signPlcOperation request shape per
/// Arc 13 §6.3.6 Step 3.4.
///
/// `token` is REQUIRED — the email-token-confirmation flow gives
/// the caller a 30-minute single-use token via
/// `requestPlcOperationSignature`.
///
/// `verification_methods` / `services` are JSON map shapes
/// matching the on-wire PLC spec (compatible with bsky-PDS's
/// signPlcOperation lexicon — both crates pass serde_json::Value
/// here and parse at handler time):
///   - verification_methods: `{"<name>": "<did:key URI>", …}`
///   - services: `{"<name>": {"type": "<type>", "endpoint": "<url>"}, …}`
///
/// Any field absent from input means "inherit from prior op"
/// (snapshot mutator pattern per §6.3.6); any field present
/// overrides the inherited value.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignPlcOperationRequest {
    /// Email confirmation token (Arc 13 §6.3.6 — required).
    pub token: String,
    /// Override `rotation_keys`. Absent → inherit from prior op.
    pub rotation_keys: Option<Vec<String>>,
    /// Override `also_known_as`. Absent → inherit.
    pub also_known_as: Option<Vec<String>>,
    /// Override `verification_methods`. JSON `{name: did:key, …}`.
    /// Absent → inherit.
    pub verification_methods: Option<serde_json::Value>,
    /// Override `services`. JSON
    /// `{name: {type, endpoint}, …}`. Absent → inherit.
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
    if let Some(entryway) = ctx.entryway_client.as_ref() {
        let headers = ctx
            .entryway_auth_headers(&did, "com.atproto.identity.signPlcOperation")
            .await?;
        let resp: SignPlcOperationResponse = entryway
            .xrpc_post_json("com.atproto.identity.signPlcOperation", headers, &req)
            .await?;
        return Ok(Json(resp));
    }

    // Standalone path (unchanged).
    // Ensure this is a did:plc
    if !crate::identity::did_method::is_plc(&did) {
        return Err(PdsError::Validation(
            "Only did:plc identifiers support PLC operations".to_string(),
        ));
    }

    // Arc 13 §6.3.6 / Step 3.4 — two-phase email-token flow:
    //
    // 1. Validate request (above): did is did:plc.
    // 2. Validate the email token (validate-only, NO consume yet).
    //    On invalid → PdsError::Authentication → HTTP 401 in
    //    IntoResponse — close enough to the spec's HTTP 400
    //    InvalidToken; for the Arc 13 sweep we accept this as a
    //    Path B documented deviation (HTTP 401 vs 400) since
    //    the existing IntoResponse for Authentication is HTTP 401
    //    and adding a dedicated InvalidToken variant is Step 7
    //    audit cleanup. The wire message string contains
    //    "InvalidToken" so clients can string-match.
    // 3. Fetch full last op (get_last_op) — tombstoned → 400.
    // 4. Build new op via mutator: inherit all fields from
    //    last_op, override any field provided in input.
    // 5. Set prev to last_op's CID.
    // 6. Sign with PDS-wide rotation key.
    // 7. Consume token via CAS. On race-lose → HTTP 409
    //    TokenAlreadyConsumed (Path B per round-4 F4 closure —
    //    proto-blue's lexicon doesn't declare this error, so it's
    //    emitted as a non-declared error with warning logging).
    // 8. Return signed op.
    use crate::crypto::plc::{PlcOperationBuilder, PlcSigner, ServiceEntry};
    use crate::crypto::plc_client::{PlcClient, PlcClientConfig};
    use crate::account::ConsumeResult;
    use std::collections::BTreeMap;

    // #71 / Phase B finding 3 — wrap every `?` in this handler
    // with a tracing::error so the actual error class surfaces
    // (the handler previously swallowed every failure into a
    // generic HTTP 500 with no observable cause). Each
    // `at_step_*` label identifies the failure point uniquely
    // in stderr so operator-side diagnosis is one grep away.

    // Step 2 — validate-only.
    if let Err(e) = ctx
        .account_manager
        .validate_plc_operation_token(&did, &req.token)
        .await
    {
        tracing::error!(
            did = %did,
            token_prefix = %req.token.chars().take(8).collect::<String>(),
            at_step = "step-2-validate-token",
            error = %e,
            error_kind = ?std::mem::discriminant(&e),
            "sign_plc_operation failed at validate_plc_operation_token"
        );
        return Err(e);
    }

    // Step 3a — caller-input validation (§71 finding-3
    // hardening). Per the §6.3.6 spec, caller overrides for
    // rotation_keys / verification_methods values are did:key
    // URI strings; if any is malformed we reject as HTTP 400
    // InvalidRequest BEFORE the get_last_op + mutator + sign
    // pipeline runs. Pre-fix, a placeholder like
    // `did:key:zNewRotation` survived all the way through to
    // submit-time (or, in the standalone path, was just baked
    // into the returned op for the caller to later submit) —
    // either way, no early-fail signal.
    if let Some(keys) = req.rotation_keys.as_ref() {
        for (i, k) in keys.iter().enumerate() {
            if let Err(reason) = validate_did_key_shape(k) {
                tracing::error!(
                    did = %did,
                    at_step = "step-3a-validate-rotation-key",
                    index = i,
                    value = %k,
                    reason = %reason,
                    "sign_plc_operation: caller-supplied rotationKeys[{}] is not a parseable did:key URI",
                    i
                );
                return Err(PdsError::Validation(format!(
                    "InvalidRequest: rotationKeys[{}] is not a parseable did:key URI ({}): {}",
                    i, reason, k
                )));
            }
        }
    }
    if let Some(vm_val) = req.verification_methods.as_ref() {
        if let Some(obj) = vm_val.as_object() {
            for (name, v) in obj {
                if let Some(s) = v.as_str() {
                    if let Err(reason) = validate_did_key_shape(s) {
                        tracing::error!(
                            did = %did,
                            at_step = "step-3a-validate-verification-method",
                            name = %name,
                            value = %s,
                            reason = %reason,
                            "sign_plc_operation: caller-supplied verificationMethods[{:?}] is not a parseable did:key URI",
                            name
                        );
                        return Err(PdsError::Validation(format!(
                            "InvalidRequest: verificationMethods[{:?}] is not a parseable did:key URI ({}): {}",
                            name, reason, s
                        )));
                    }
                }
            }
        }
    }

    // Step 3b — full-fetch last op from the PLC directory.
    let plc_client = match PlcClient::new(PlcClientConfig {
        plc_url: ctx.config.identity.did_plc_url.clone(),
        ..Default::default()
    }) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(
                at_step = "step-3b-plc-client-new",
                error = %e,
                "sign_plc_operation: PlcClient::new failed"
            );
            return Err(e);
        }
    };
    let (last_op, last_cid) = match plc_client.get_last_op(&did).await {
        Ok(pair) => pair,
        Err(e) => {
            tracing::error!(
                did = %did,
                at_step = "step-3b-get-last-op",
                plc_url = %ctx.config.identity.did_plc_url,
                error = %e,
                error_kind = ?std::mem::discriminant(&e),
                "sign_plc_operation: PlcClient::get_last_op failed — check mock-PLC reachability + audit-log shape"
            );
            return Err(e);
        }
    };

    // Step 4 — build mutator. Start from inherited values, apply
    // overrides. Parse JSON map shapes per §6.3.6 Step 3.4
    // contract.
    let rotation_keys = req
        .rotation_keys
        .clone()
        .unwrap_or_else(|| last_op.rotation_keys.clone());
    let also_known_as = req
        .also_known_as
        .clone()
        .unwrap_or_else(|| last_op.also_known_as.clone());

    let verification_methods: BTreeMap<String, String> = match req.verification_methods.as_ref() {
        Some(v) => match parse_verification_methods(v) {
            Ok(parsed) => parsed,
            Err(e) => {
                tracing::error!(
                    did = %did,
                    at_step = "step-4-parse-verification-methods",
                    error = %e,
                    "sign_plc_operation: parse_verification_methods rejected caller input"
                );
                return Err(e);
            }
        },
        None => last_op.verification_methods.clone(),
    };
    let services: BTreeMap<String, ServiceEntry> = match req.services.as_ref() {
        Some(v) => match parse_services(v) {
            Ok(parsed) => parsed,
            Err(e) => {
                tracing::error!(
                    did = %did,
                    at_step = "step-4-parse-services",
                    error = %e,
                    "sign_plc_operation: parse_services rejected caller input"
                );
                return Err(e);
            }
        },
        None => last_op.services.clone(),
    };

    let unsigned = match PlcOperationBuilder::new()
        .rotation_keys(rotation_keys)
        .verification_methods(verification_methods)
        .also_known_as(also_known_as)
        .services(services)
        .prev(last_cid)
        .build()
    {
        Ok(op) => op,
        Err(e) => {
            tracing::error!(
                did = %did,
                at_step = "step-4-builder-build",
                error = %e,
                "sign_plc_operation: PlcOperationBuilder::build failed (unexpected — currently infallible)"
            );
            return Err(e);
        }
    };

    // Step 5/6 — sign with PDS-wide rotation key.
    let signer = match PlcSigner::from_hex(&ctx.config.authentication.plc_rotation_key) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(
                at_step = "step-6-signer-from-hex",
                error = %e,
                "sign_plc_operation: PlcSigner::from_hex on PDS rotation key failed — \
                 check PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX env value"
            );
            return Err(e);
        }
    };
    let signed = match signer.sign_operation(unsigned) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(
                did = %did,
                at_step = "step-6-sign-operation",
                error = %e,
                "sign_plc_operation: signer.sign_operation failed (canonical DAG-CBOR encode + ECDSA sign)"
            );
            return Err(e);
        }
    };

    // Step 7 — CAS-style consume. Path B for TokenAlreadyConsumed:
    // proto-blue's signPlcOperation lexicon doesn't declare this
    // error variant; we emit it as a non-declared XRPC error with
    // warning log per §6.3.6 round-4 F4 closure.
    match ctx
        .account_manager
        .consume_plc_operation_token(&did, &req.token)
        .await
    {
        ConsumeResult::Consumed => {
            tracing::info!(
                did = %did,
                token_prefix = %req.token.chars().take(8).collect::<String>(),
                at_step = "step-7-consume-token",
                "sign_plc_operation: token consumed cleanly"
            );
        }
        ConsumeResult::AlreadyConsumed => {
            tracing::warn!(
                did = %did,
                token_prefix = %req.token.chars().take(8).collect::<String>(),
                "TokenAlreadyConsumed (race lost between validate and consume; \
                 Path B non-declared XRPC error per §6.3.6 round-4 F4)"
            );
            return Err(PdsError::Conflict(
                "TokenAlreadyConsumed: plc_operation token was consumed by a concurrent call"
                    .to_string(),
            ));
        }
        ConsumeResult::NotFound => {
            // Effectively impossible if validate succeeded; log
            // at warn if observed.
            tracing::warn!(
                did = %did,
                "consume_plc_operation_token returned NotFound after validate succeeded \
                 (contract violation — investigate)"
            );
            return Err(PdsError::Conflict(
                "TokenAlreadyConsumed".to_string(),
            ));
        }
        ConsumeResult::Error(e) => {
            tracing::error!(
                did = %did,
                at_step = "step-7-consume-token-db-error",
                error = %e,
                error_kind = ?std::mem::discriminant(&e),
                "sign_plc_operation: consume_plc_operation_token returned Error variant"
            );
            return Err(e);
        }
    }

    // Step 8 — return signed op JSON.
    let operation_json = serde_json::to_value(&signed).map_err(|e| {
        tracing::error!(
            did = %did,
            at_step = "step-8-serialize-signed",
            error = %e,
            "sign_plc_operation: serde_json::to_value(&signed) failed (unexpected — PlcOperation derives Serialize)"
        );
        PdsError::Internal(format!("Failed to serialize signed op: {}", e))
    })?;
    Ok(Json(SignPlcOperationResponse {
        operation: operation_json,
    }))
}

/// #71 finding-3 hardening — shape-check a `did:key:zXXX` URI
/// without doing crypto. Returns `Err(reason)` on any of:
/// missing prefix, missing `z` multibase prefix, payload not
/// base58btc-decodable, multicodec not secp256k1-pub
/// (`0xe7 0x01`), pubkey not 33 bytes.
///
/// Catches placeholder values like `did:key:zNewRotation` BEFORE
/// they propagate through the mutator → sign → consume pipeline
/// and surface as a generic HTTP 500 with no observable cause.
fn validate_did_key_shape(s: &str) -> Result<(), &'static str> {
    if !s.starts_with("did:key:z") {
        return Err("must start with `did:key:z`");
    }
    let payload = &s["did:key:z".len()..];
    let bytes = bs58::decode(payload)
        .into_vec()
        .map_err(|_| "payload is not base58btc-decodable")?;
    if bytes.len() != 35 {
        return Err("decoded payload length mismatch (expected 35 bytes: 2 multicodec + 33 secp256k1 compressed pubkey)");
    }
    if bytes[0] != 0xE7 || bytes[1] != 0x01 {
        return Err("multicodec is not secp256k1-pub (expect 0xe7 0x01)");
    }
    Ok(())
}

/// §6.3.6 Step 3.4 helper — convert JSON
/// `{"<name>": "<did:key URI>", …}` (bsky-PDS wire shape) into
/// the canonical `BTreeMap<String, String>` consumed by
/// `PlcOperationBuilder::verification_methods`.
fn parse_verification_methods(
    v: &serde_json::Value,
) -> PdsResult<std::collections::BTreeMap<String, String>> {
    let obj = v.as_object().ok_or_else(|| {
        PdsError::Validation(
            "verification_methods must be a JSON object {name: did:key}".to_string(),
        )
    })?;
    let mut out = std::collections::BTreeMap::new();
    for (k, val) in obj {
        let s = val.as_str().ok_or_else(|| {
            PdsError::Validation(format!(
                "verification_methods[{:?}] must be a string did:key URI",
                k
            ))
        })?;
        out.insert(k.clone(), s.to_string());
    }
    Ok(out)
}

/// §6.3.6 Step 3.4 helper — convert JSON
/// `{"<name>": {"type": "<type>", "endpoint": "<url>"}, …}` into
/// `BTreeMap<String, ServiceEntry>`.
fn parse_services(
    v: &serde_json::Value,
) -> PdsResult<std::collections::BTreeMap<String, crate::crypto::plc::ServiceEntry>> {
    let obj = v.as_object().ok_or_else(|| {
        PdsError::Validation(
            "services must be a JSON object {name: {type, endpoint}}".to_string(),
        )
    })?;
    let mut out = std::collections::BTreeMap::new();
    for (k, val) in obj {
        let inner = val.as_object().ok_or_else(|| {
            PdsError::Validation(format!(
                "services[{:?}] must be an object with `type` and `endpoint`",
                k
            ))
        })?;
        let type_ = inner
            .get("type")
            .and_then(|t| t.as_str())
            .ok_or_else(|| {
                PdsError::Validation(format!("services[{:?}] missing string `type`", k))
            })?
            .to_string();
        let endpoint = inner
            .get("endpoint")
            .and_then(|e| e.as_str())
            .ok_or_else(|| {
                PdsError::Validation(format!("services[{:?}] missing string `endpoint`", k))
            })?
            .to_string();
        out.insert(
            k.clone(),
            crate::crypto::plc::ServiceEntry { type_, endpoint },
        );
    }
    Ok(out)
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
    if !crate::identity::did_method::is_plc(&did) {
        return Err(PdsError::Validation(
            "Only did:plc identifiers support PLC operations".to_string(),
        ));
    }

    // Arc 13 §6.3.6 Step 3.5 — submit_plc_operation rewrite.
    //
    // Sum-type dispatch: parse input as either PlcOperation
    // (regular update) or PlcTombstone (terminal retire). Reject
    // malformed via PdsError::Validation → HTTP 400.
    //
    // For PlcOperation, validate service-endpoint matches
    // `ctx.service_url()` (signers can't redirect their account
    // to a third-party PDS via submit). Accept ops that remove
    // the server's rotation key from `op.rotation_keys`
    // (migration-away scenario per §6.7.1).
    //
    // For PlcTombstone, skip the service-endpoint check (tombstones
    // carry no services).
    //
    // The pre-Arc-13 `op.did` check is gone — the new wire shape
    // has no `did` field; the DID is the URL path component the
    // caller specifies via their authenticated session.
    use crate::crypto::plc::{PlcOperation, ServiceEntry};

    // Sum-type dispatch by `type` field.
    let op_type = req
        .operation
        .get("type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            PdsError::Validation("Operation must have string `type` field".to_string())
        })?;

    match op_type {
        "plc_operation" => {
            let op: PlcOperation = serde_json::from_value(req.operation.clone())
                .map_err(|e| PdsError::Validation(format!("Malformed plc_operation: {}", e)))?;
            // §6.3.6 step 3: services["atproto_pds"] must match
            // `ctx.service_url()`. Reject if absent or mismatched.
            let pds_svc: &ServiceEntry =
                op.services.get("atproto_pds").ok_or_else(|| {
                    PdsError::Validation(
                        "InvalidServiceEndpoint: op.services must include \
                         `atproto_pds` entry"
                            .to_string(),
                    )
                })?;
            if pds_svc.type_ != "AtprotoPersonalDataServer" {
                return Err(PdsError::Validation(format!(
                    "InvalidServiceEndpoint: services.atproto_pds.type must be \
                     `AtprotoPersonalDataServer`, got {:?}",
                    pds_svc.type_
                )));
            }
            if pds_svc.endpoint != ctx.service_url() {
                return Err(PdsError::Validation(format!(
                    "InvalidServiceEndpoint: services.atproto_pds.endpoint must \
                     match this PDS's service URL ({}); got {}",
                    ctx.service_url(),
                    pds_svc.endpoint
                )));
            }
            // §6.3.6 step 5: sig present and base64url-decodable.
            crate::crypto::plc::validate_plc_operation(&op)?;
            // Step 4 in §6.3.6 — accept ops that remove server's
            // rotation key. No additional check here; the caller's
            // rotation_keys decision flows through.
        }
        "plc_tombstone" => {
            // Tombstones carry only `type`, `prev`, `sig`. Parse
            // shape-check; service-endpoint validation skipped.
            let prev = req
                .operation
                .get("prev")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    PdsError::Validation(
                        "Malformed plc_tombstone: missing string `prev` (CID of last op)"
                            .to_string(),
                    )
                })?;
            let sig = req
                .operation
                .get("sig")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    PdsError::Validation(
                        "Malformed plc_tombstone: missing string `sig` (base64url)"
                            .to_string(),
                    )
                })?;
            use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
            URL_SAFE_NO_PAD.decode(sig).map_err(|e| {
                PdsError::Validation(format!(
                    "Malformed plc_tombstone: sig is not valid base64url: {}",
                    e
                ))
            })?;
            let _ = prev; // No further validation beyond presence.
        }
        other => {
            return Err(PdsError::Validation(format!(
                "Operation type must be `plc_operation` or `plc_tombstone`; got {:?}",
                other
            )));
        }
    }

    // §6.3.6 step 7: submit to PLC directory.
    let plc_url = &ctx.config.identity.did_plc_url;
    let submit_endpoint = format!("{}/{}", plc_url, did);

    let http_client = reqwest::Client::new();
    let response = http_client
        .post(&submit_endpoint)
        .json(&req.operation)
        .send()
        .await
        .map_err(|e| PdsError::Internal(format!("Failed to submit to PLC directory: {}", e)))?;

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

    // Invalidate cached DID document so it'll be refreshed.
    ctx.identity_resolver.invalidate_did(&did).await?;

    // Arc 15 §8.3.7: emit #identity event for the DID with handle
    // ABSENT on the wire (signals "DID document changed but handle
    // unchanged"). Per Sub-step 0.4 recon, all three §8.3.7
    // conditions hold here, so append-after-success is acceptable
    // under Arc 13's lock. The omit-if-none discipline is owned by
    // the firehose encoder's `identity_body_to_lex_value` builder
    // (Case M2 per Sub-step 0.3(d)).
    ctx.sequencer
        .sequence_identity(crate::sequencer::events::IdentityEvent {
            did: did.clone(),
            handle: None,
        })
        .await?;

    Ok(Json(()))
}

/// com.atproto.identity.requestPlcOperationSignature
///
/// Arc 13 §6.3.6 / Step 3.3 — generates a single-use email
/// confirmation token, sends it to the account holder's email, and
/// returns 200 with an empty body. Pre-Arc-13 this returned the
/// PLC-directory prev-CID synchronously; that was the wrong
/// endpoint purpose entirely (chainlink #61 §6.1).
///
/// The returned token is consumed by `sign_plc_operation`. TTL: 30
/// minutes. Single-use via CAS at consume time.
pub async fn request_plc_operation_signature(
    State(ctx): State<AppContext>,
    auth: AuthContext,
) -> PdsResult<Json<serde_json::Value>> {
    let did = auth.did;

    // Ensure this is a did:plc — only did:plc supports PLC ops.
    if !crate::identity::did_method::is_plc(&did) {
        return Err(PdsError::Validation(
            "Only did:plc identifiers support PLC operations".to_string(),
        ));
    }

    // Fetch the account so we have an email + handle to send to.
    let account = ctx.account_manager.get_account(&did).await?;
    let email = account.email.clone().ok_or_else(|| {
        PdsError::Validation(
            "Account does not have an email address; cannot send PLC operation token"
                .to_string(),
        )
    })?;

    // §6.3.6 step 2: generate the token row. Persisted; consumed
    // later by sign_plc_operation's CAS UPDATE.
    let token = ctx
        .account_manager
        .generate_plc_operation_token(&did)
        .await?;

    // §6.3.6 step 3: send via mailer. Mailer no-ops + warns when
    // SMTP isn't configured (dev paths) — the token still exists
    // in the DB and can be retrieved by Phase B operators via
    // MailHog (or, in dev with no MailHog, via direct DB query;
    // see Step 3.7 for the operator-side procedure).
    if ctx.mailer.is_configured() {
        ctx.mailer
            .send_plc_operation_email(
                &email,
                account.handle.as_deref().unwrap_or("unknown"),
                &token,
            )
            .await?;
    } else {
        tracing::warn!(
            did = %did,
            "PLC operation token generated but mailer not configured; \
             retrieve via Phase B operator path"
        );
    }

    // §6.3.6 step 4: 200 empty.
    Ok(Json(serde_json::json!({})))
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

    // ============================================================
    // #71 finding-3 hardening — validate_did_key_shape tests.
    // ============================================================

    #[test]
    fn validate_did_key_shape_accepts_real_secp256k1_did_key() {
        // Real secp256k1 did:key derived from a known compressed
        // pubkey. We mint a fresh signer and derive its did:key the
        // same way Aurora-Locus does, then validate that string.
        use crate::crypto::plc::PlcSigner;
        let signer = PlcSigner::new(&[42u8; 32]).expect("signer");
        let did_key = signer.public_key_did_key();
        assert!(did_key.starts_with("did:key:z"));
        assert!(
            super::validate_did_key_shape(&did_key).is_ok(),
            "real secp256k1 did:key must validate: {}",
            did_key
        );
    }

    #[test]
    fn validate_did_key_shape_rejects_placeholder_did_key_z_new_rotation() {
        // The exact placeholder skydeval used in Phase B Scenario 5
        // that surfaced as a generic HTTP 500. Now caught as 400
        // InvalidRequest at handler entry.
        let result = super::validate_did_key_shape("did:key:zNewRotation");
        assert!(
            result.is_err(),
            "placeholder did:key:zNewRotation must be rejected (caught the HTTP 500 → 400 InvalidRequest fix in #71)"
        );
    }

    #[test]
    fn validate_did_key_shape_rejects_missing_prefix() {
        assert!(super::validate_did_key_shape("z1234").is_err());
        assert!(super::validate_did_key_shape("did:plc:abc").is_err());
        assert!(super::validate_did_key_shape("did:key:").is_err());
    }

    #[test]
    fn validate_did_key_shape_rejects_non_base58btc_chars() {
        // '0' is NOT in the base58btc alphabet (deliberately
        // excluded to avoid 0/O confusion).
        let bad = "did:key:z000notbase58";
        assert!(super::validate_did_key_shape(bad).is_err());
    }

    #[test]
    fn validate_did_key_shape_rejects_wrong_multicodec() {
        // Construct a did:key with the ed25519 multicodec
        // (0xed 0x01) instead of secp256k1-pub (0xe7 0x01).
        // 35-byte payload: 0xed 0x01 + 33 zero bytes.
        let mut payload = vec![0xED_u8, 0x01];
        payload.extend(vec![0u8; 33]);
        let did_key = format!("did:key:z{}", bs58::encode(&payload).into_string());
        let result = super::validate_did_key_shape(&did_key);
        assert!(result.is_err(), "ed25519 multicodec must be rejected");
        let msg = result.unwrap_err();
        assert!(
            msg.contains("multicodec"),
            "error message should mention multicodec: {}",
            msg
        );
    }

    #[test]
    fn validate_did_key_shape_rejects_wrong_length() {
        // Correct multicodec + 32-byte (not 33) pubkey.
        let mut payload = vec![0xE7_u8, 0x01];
        payload.extend(vec![0u8; 32]);
        let did_key = format!("did:key:z{}", bs58::encode(&payload).into_string());
        let result = super::validate_did_key_shape(&did_key);
        assert!(result.is_err(), "wrong-length payload must be rejected");
    }
}
