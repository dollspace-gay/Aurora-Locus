//! `com.atproto.identity.*` (handle resolution and changes) and the
//! documents locus serves: `/.well-known/atproto-did` and
//! `/.well-known/did.json`, plus the identity repair job.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use locus_identity::did_doc::{DidDocument, ServiceEntry, VerificationMethod};
use locus_identity::handle::{self, HandleError, LabelLimits};
use locus_identity::plc::{self, Operation};
use locus_identity::plc_client::PlcClientError;
use locus_repo::event::identity_body;
use locus_store::accounts::Status;
use locus_store::identity::RepairKind;
use proto_blue_crypto::Keypair;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth;
use crate::error::{XrpcError, XrpcResult};
use crate::state::AppState;

type St = State<Arc<AppState>>;

/// The request's host, lowercased, without a port.
pub fn request_host(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::HOST)?.to_str().ok()?;
    let host = if raw.starts_with('[') {
        raw.split_once(']')
            .map_or(raw, |(h, _)| h)
            .trim_start_matches('[')
    } else {
        raw.split(':').next().unwrap_or(raw)
    };
    Some(host.trim_end_matches('.').to_ascii_lowercase())
}

fn handle_error(e: HandleError) -> XrpcError {
    match e {
        HandleError::Invalid(m) => XrpcError::named("InvalidHandle", m),
        HandleError::NotAvailable(m) => XrpcError::named("HandleNotAvailable", m),
    }
}

fn taken() -> XrpcError {
    XrpcError::named("HandleNotAvailable", "Handle already taken")
}

/// Validates a handle for an account, as the reference's
/// `normalizeAndValidateHandle`: `did` is None for a new account, which must
/// use one of this PDS's handle domains; an existing account may also take
/// an external handle that resolves to its DID. On top of the reference,
/// PDS hostnames and hosted did:web hosts of other DIDs are reserved forever.
pub async fn validate_handle(s: &AppState, raw: &str, did: Option<&str>) -> XrpcResult<String> {
    let h = handle::normalize_and_validate(raw).map_err(handle_error)?;
    if handle::has_explicit_slur(&h) {
        return Err(XrpcError::named(
            "InvalidHandle",
            "Inappropriate language in handle",
        ));
    }
    if h == s.hostname {
        return Err(taken());
    }
    if let Some(web) = s.store.hosted_did_web_by_host(&h).await?
        && Some(web.did.as_str()) != did
    {
        return Err(taken());
    }
    let domains = s.handle_domains();
    if handle::service_domain(&h, &domains).is_some() {
        handle::ensure_service_constraints(&h, &domains, LabelLimits::default(), &[], false)
            .map_err(handle_error)?;
        return Ok(h);
    }
    let Some(did) = did else {
        return Err(XrpcError::named(
            "UnsupportedDomain",
            "Not a supported handle domain",
        ));
    };
    // An external handle: it must resolve to this DID now, not from a cache.
    let resolved = s.resolvers()?.resolve_handle_uncached(&h).await;
    if resolved.as_deref() != Some(did) {
        return Err(XrpcError::invalid_request(
            "External handle did not resolve to DID",
        ));
    }
    Ok(h)
}

#[derive(Deserialize)]
pub struct ResolveHandleParams {
    handle: String,
}

pub async fn resolve_handle(
    State(s): St,
    Query(p): Query<ResolveHandleParams>,
) -> XrpcResult<Json<Value>> {
    let h = handle::normalize_and_validate(&p.handle)
        .map_err(|e| XrpcError::named("InvalidHandle", e.to_string()))?;
    if let Some(account) = s.store.account_by_handle(&h).await?
        && account.status == Status::Active
    {
        return Ok(Json(json!({ "did": account.did })));
    }
    // A name on our own handle domains with no local account doesn't exist;
    // anything else is looked up on the network (cache, then DNS and HTTPS).
    if handle::service_domain(&h, &s.handle_domains()).is_none()
        && let Some(did) = s.resolvers()?.resolve_handle(&h).await
    {
        return Ok(Json(json!({ "did": did })));
    }
    Err(XrpcError::named(
        "HandleNotFound",
        "Unable to resolve handle",
    ))
}

#[derive(Deserialize)]
pub struct UpdateHandleInput {
    handle: String,
}

pub async fn update_handle(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<UpdateHandleInput>,
) -> XrpcResult<()> {
    let authed = auth::access(&s, &headers).await?;
    let account = auth::account_checked(&s, &authed.did, false).await?;
    let did = account.did.as_str();
    let handle = validate_handle(&s, &input.handle, Some(did)).await?;
    if let Some(other) = s.store.account_by_handle(&handle).await?
        && other.did != did
    {
        return Err(XrpcError::named(
            "HandleNotAvailable",
            format!("Handle already taken: {handle}"),
        ));
    }
    if did.starts_with("did:plc:") {
        // PLC first, then the database. The repair entry makes the change
        // converge if anything after the PLC step fails.
        s.store
            .queue_identity_repair(did, RepairKind::Handle, &handle)
            .await?;
        match plc_set_handle(&s, did, &handle).await {
            Ok(()) => {}
            Err(PlcStep::Refused(e)) => {
                s.store
                    .finish_identity_repair(did, RepairKind::Handle, &handle)
                    .await?;
                return Err(e);
            }
            Err(PlcStep::Ambiguous(e)) => return Err(e),
        }
        apply_handle(&s, did, &handle, None).await?;
        s.store
            .finish_identity_repair(did, RepairKind::Handle, &handle)
            .await?;
    } else if let Some(hosted) = s.store.hosted_did_web(did).await?
        && let Some(doc) = hosted.doc
    {
        // A document locus serves: edit it in the same transaction.
        let mut doc: Value =
            serde_json::from_str(&doc).map_err(|e| XrpcError::internal(e.to_string()))?;
        set_doc_handle(&mut doc, &handle);
        apply_handle(&s, did, &handle, Some(&doc.to_string())).await?;
    } else {
        // Someone else serves the document: it must already name the handle.
        let doc = s
            .resolvers()?
            .resolve_did(did, true)
            .await
            .map_err(|e| XrpcError::invalid_request(e.to_string()))?;
        let named = doc.as_ref().and_then(|d| proto_blue_common::get_handle(d));
        if named != Some(handle.as_str()) {
            return Err(XrpcError::invalid_request(
                "DID is not properly configured for handle",
            ));
        }
        apply_handle(&s, did, &handle, None).await?;
    }
    for h in [&account.handle, &handle] {
        let _ = s.store.clear_handle_cache(h).await;
    }
    let _ = s.store.clear_did_doc(did).await;
    Ok(())
}

/// Puts `handle` first among a document's `at://` names.
fn set_doc_handle(doc: &mut Value, handle: &str) {
    let uri = Value::String(format!("at://{handle}"));
    let aka = doc
        .as_object_mut()
        .map(|m| m.entry("alsoKnownAs").or_insert_with(|| json!([])));
    if let Some(Value::Array(list)) = aka {
        match list
            .iter()
            .position(|v| v.as_str().is_some_and(|a| a.starts_with("at://")))
        {
            Some(i) => list[i] = uri,
            None => list.insert(0, uri),
        }
    }
}

/// The database half of a handle change: the account row, a hosted did:web
/// document if any, and `#identity`, under the DID's repo lock.
async fn apply_handle(
    s: &AppState,
    did: &str,
    handle: &str,
    web_doc: Option<&str>,
) -> XrpcResult<()> {
    let (_lock, mut tx) = s
        .repo
        .locked_tx(did)
        .await
        .map_err(|e| XrpcError::internal(e.to_string()))?;
    tx.set_account_handle(did, handle)
        .await
        .map_err(|e| match e {
            locus_core::CoreError::Conflict(m) => XrpcError::named("HandleNotAvailable", m),
            other => other.into(),
        })?;
    if let Some(doc) = web_doc {
        tx.set_hosted_did_web_doc(did, Some(doc)).await?;
    }
    let body = identity_body(did, Some(handle)).map_err(|e| XrpcError::internal(e.to_string()))?;
    tx.insert_outbox(did, "identity", &body).await?;
    tx.commit().await?;
    Ok(())
}

/// A PLC step that failed: refused (nothing changed) or ambiguous (it may
/// have applied; the repair job finds out).
pub(crate) enum PlcStep {
    Refused(XrpcError),
    Ambiguous(XrpcError),
}

fn upstream(m: impl Into<String>) -> XrpcError {
    XrpcError::new(StatusCode::BAD_GATEWAY, "UpstreamFailure", m)
}

/// The DID's current PLC document and the CID its next op must name.
async fn plc_state(s: &AppState, did: &str) -> Result<(Option<Operation>, String), PlcStep> {
    let plc = s.plc().map_err(PlcStep::Ambiguous)?;
    let raw = match plc.last_op_json(did).await {
        Ok(Some(v)) => v,
        Ok(None) => {
            return Err(PlcStep::Refused(XrpcError::invalid_request(
                "DID not found in the PLC directory",
            )));
        }
        Err(e) => return Err(PlcStep::Ambiguous(upstream(e.to_string()))),
    };
    plc::last_op_state(&raw).map_err(|e| PlcStep::Ambiguous(upstream(e.to_string())))
}

fn no_held_key() -> PlcStep {
    PlcStep::Refused(XrpcError::invalid_request(
        "This PDS holds none of the rotation keys the DID document lists",
    ))
}

/// Submits an op naming `handle`, signed with the held key the DID's
/// document lists first. Already naming it counts as done.
async fn plc_set_handle(s: &AppState, did: &str, handle: &str) -> Result<(), PlcStep> {
    let (state, prev) = plc_state(s, did).await?;
    let current =
        state.ok_or_else(|| PlcStep::Refused(XrpcError::invalid_request("DID is tombstoned")))?;
    if current.handle() == Some(handle) {
        let _ = s
            .store
            .set_plc_key_listing(did, &s.secrets.plc_keys.listed(&current.rotation_keys))
            .await;
        return Ok(());
    }
    let mut next = current.with_handle(handle);
    next.prev = Some(prev);
    next.validate()
        .map_err(|e| PlcStep::Refused(XrpcError::invalid_request(e.to_string())))?;
    let signer = s
        .secrets
        .plc_keys
        .signer_for(&current.rotation_keys)
        .ok_or_else(no_held_key)?;
    let signed = next
        .sign(signer)
        .map_err(|e| PlcStep::Refused(XrpcError::internal(e.to_string())))?;
    let plc = s.plc().map_err(PlcStep::Ambiguous)?;
    match plc.submit(did, &signed).await {
        Ok(()) => {}
        Err(PlcClientError::Refused(m)) => {
            return Err(PlcStep::Refused(XrpcError::invalid_request(format!(
                "The PLC directory refused the update: {m}"
            ))));
        }
        Err(e) => {
            // Maybe it applied: look before reporting a failure.
            match plc_state(s, did).await {
                Ok((Some(op), _)) if op.handle() == Some(handle) => {}
                _ => return Err(PlcStep::Ambiguous(upstream(e.to_string()))),
            }
        }
    }
    let _ = s
        .store
        .set_plc_key_listing(did, &s.secrets.plc_keys.listed(&next.rotation_keys))
        .await;
    Ok(())
}

/// Tombstones a DID locus created for an account that was never created.
pub(crate) async fn plc_tombstone(s: &AppState, did: &str) -> Result<(), PlcStep> {
    let (state, prev) = plc_state(s, did).await?;
    let Some(current) = state else {
        return Ok(());
    };
    let signer = s
        .secrets
        .plc_keys
        .signer_for(&current.rotation_keys)
        .ok_or_else(no_held_key)?;
    let t = plc::sign_tombstone(&prev, signer)
        .map_err(|e| PlcStep::Refused(XrpcError::internal(e.to_string())))?;
    let plc = s.plc().map_err(PlcStep::Ambiguous)?;
    match plc.submit(did, &t).await {
        Ok(()) => Ok(()),
        Err(PlcClientError::Refused(m)) => Err(PlcStep::Refused(XrpcError::invalid_request(m))),
        Err(e) => Err(PlcStep::Ambiguous(upstream(e.to_string()))),
    }
}

/// One pass of the identity repair job over every due entry. Returns how
/// many entries finished.
pub async fn run_identity_repairs(s: &AppState) -> locus_core::CoreResult<usize> {
    let mut done = 0;
    for r in s.store.due_identity_repairs(100).await? {
        let outcome = match r.kind {
            RepairKind::Handle => repair_handle(s, &r.did, &r.value).await,
            RepairKind::Tombstone => {
                // Never tombstone a DID that has an account after all.
                if s.store.account_by_did(&r.did).await?.is_some() {
                    Ok(())
                } else {
                    plc_tombstone(s, &r.did).await
                }
            }
        };
        match outcome {
            Ok(()) => {
                s.store
                    .finish_identity_repair(&r.did, r.kind, &r.value)
                    .await?;
                done += 1;
            }
            Err(PlcStep::Refused(e)) => {
                tracing::warn!(
                    did = r.did,
                    kind = r.kind.as_str(),
                    "identity repair dropped: {}",
                    e.message
                );
                s.store
                    .finish_identity_repair(&r.did, r.kind, &r.value)
                    .await?;
            }
            Err(PlcStep::Ambiguous(e)) => {
                let backoff = (30_000_i64 << r.attempts.clamp(0, 7)).min(3_600_000);
                s.store
                    .retry_identity_repair(&r.did, r.kind, &e.message, backoff)
                    .await?;
            }
        }
    }
    Ok(done)
}

async fn repair_handle(s: &AppState, did: &str, handle: &str) -> Result<(), PlcStep> {
    let account = match s.store.account_by_did(did).await {
        Ok(Some(a)) => a,
        Ok(None) => return Ok(()),
        Err(e) => return Err(PlcStep::Ambiguous(e.into())),
    };
    // The PLC step again (a no-op if it applied), then the database.
    plc_set_handle(s, did, handle).await?;
    if account.handle != handle {
        apply_handle(s, did, handle, None).await.map_err(|e| {
            if e.error == "HandleNotAvailable" {
                PlcStep::Refused(e)
            } else {
                PlcStep::Ambiguous(e)
            }
        })?;
    }
    Ok(())
}

/// The document this PDS would have an account's PLC document say, as the
/// reference: the operator recovery key (if set) and the current PDS
/// rotation key, the account's signing key, handle and this PDS.
pub async fn get_recommended_did_credentials(
    State(s): St,
    headers: HeaderMap,
) -> XrpcResult<Json<Value>> {
    let authed = auth::access(&s, &headers).await?;
    let signing = s.signing_key(&authed.did).await?;
    let mut rotation_keys = Vec::new();
    let recovery = s.snapshot().text("identity.recovery_did_key").to_owned();
    if !recovery.is_empty() {
        rotation_keys.push(recovery);
    }
    rotation_keys.push(s.secrets.plc_keys.current().did());
    let mut out = json!({
        "rotationKeys": rotation_keys,
        "verificationMethods": { "atproto": signing.did() },
        "services": { "atproto_pds": { "type": "AtprotoPersonalDataServer", "endpoint": s.public_url } },
    });
    if let Some(a) = s.store.account_by_did(&authed.did).await? {
        out["alsoKnownAs"] = json!([format!("at://{}", a.handle)]);
    }
    Ok(Json(out))
}

/// Emails a code that `signPlcOperation` needs.
pub async fn request_plc_operation_signature(State(s): St, headers: HeaderMap) -> XrpcResult<()> {
    let authed = auth::access_with(
        &s,
        &headers,
        auth::FULL,
        &[locus_auth::session::SCOPE_TAKENDOWN],
    )
    .await?;
    let account = s
        .store
        .account_by_did(&authed.did)
        .await?
        .ok_or_else(|| XrpcError::invalid_request("account not found"))?;
    let email = account
        .email
        .ok_or_else(|| XrpcError::invalid_request("account does not have an email address"))?;
    crate::email::require_email(&s).await?;
    let token = crate::email::issue(
        &s,
        &account.did,
        locus_store::email::TokenPurpose::PlcOperation,
    )
    .await?;
    crate::email::send(
        &s,
        locus_mail::Template::PlcOperation,
        &email,
        &token,
        &account.handle,
    )
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignPlcOperationInput {
    token: Option<String>,
    rotation_keys: Option<Vec<String>>,
    also_known_as: Option<Vec<String>>,
    verification_methods: Option<serde_json::Map<String, Value>>,
    services: Option<serde_json::Map<String, Value>>,
}

/// Signs (doesn't submit) an update of the account's PLC document with the
/// fields given, for migrating away. **[deviation]** The fields are
/// validated, which the reference doesn't do.
pub async fn sign_plc_operation(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<SignPlcOperationInput>,
) -> XrpcResult<Json<Value>> {
    let authed = auth::access_with(
        &s,
        &headers,
        auth::FULL,
        &[locus_auth::session::SCOPE_TAKENDOWN],
    )
    .await?;
    let did = authed.did.as_str();
    let token = input
        .token
        .as_deref()
        .filter(|t| !t.is_empty())
        .ok_or_else(|| {
            XrpcError::invalid_request("email confirmation token required to sign PLC operations")
        })?;
    let purpose = locus_store::email::TokenPurpose::PlcOperation;
    crate::email::check(&s, did, purpose, token).await?;
    let mut tx = s.store.begin().await?;
    tx.delete_email_token(did, purpose).await?;
    tx.commit().await?;

    let (state, prev) = plc_state(&s, did).await.map_err(PlcStep::into_error)?;
    let current = state.ok_or_else(|| XrpcError::invalid_request("Did is tombstoned"))?;
    let strings = |m: serde_json::Map<String, Value>,
                   what: &str|
     -> XrpcResult<std::collections::BTreeMap<String, String>> {
        m.into_iter()
            .map(|(k, v)| match v {
                Value::String(s) => Ok((k, s)),
                _ => Err(XrpcError::invalid_request(format!(
                    "{what} {k:?} must be a did:key"
                ))),
            })
            .collect()
    };
    let services = match input.services {
        Some(m) => m
            .into_iter()
            .map(|(k, v)| {
                let field = |f: &str| v.get(f).and_then(Value::as_str).map(str::to_owned);
                match (field("type"), field("endpoint")) {
                    (Some(kind), Some(endpoint)) => Ok((k, plc::Service { kind, endpoint })),
                    _ => Err(XrpcError::invalid_request(format!(
                        "service {k:?} needs a type and an endpoint"
                    ))),
                }
            })
            .collect::<XrpcResult<_>>()?,
        None => current.services.clone(),
    };
    let next = Operation {
        rotation_keys: input
            .rotation_keys
            .unwrap_or_else(|| current.rotation_keys.clone()),
        verification_methods: match input.verification_methods {
            Some(m) => strings(m, "verification method")?,
            None => current.verification_methods.clone(),
        },
        also_known_as: input
            .also_known_as
            .unwrap_or_else(|| current.also_known_as.clone()),
        services,
        prev: Some(prev),
    };
    next.validate()
        .map_err(|e| XrpcError::invalid_request(e.to_string()))?;
    let signer = s
        .secrets
        .plc_keys
        .signer_for(&current.rotation_keys)
        .ok_or_else(|| no_held_key().into_error())?;
    let signed = next
        .sign(signer)
        .map_err(|e| XrpcError::internal(e.to_string()))?;
    Ok(Json(json!({ "operation": signed.to_json() })))
}

#[derive(Deserialize)]
pub struct SubmitPlcOperationInput {
    operation: Value,
}

/// Submits a signed op for the account's DID, after checking that it keeps
/// the account here: it lists the current PDS rotation key, this PDS as
/// `atproto_pds`, the account's signing key and handle.
pub async fn submit_plc_operation(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<SubmitPlcOperationInput>,
) -> XrpcResult<()> {
    let authed = auth::access(&s, &headers).await?;
    let did = authed.did.as_str();
    let invalid = || XrpcError::invalid_request("Invalid operation");
    let op = plc::SignedOp::from_json(&input.operation).map_err(|_| invalid())?;
    let plc::SignedOp::Operation { op: fields, .. } = &op else {
        return Err(invalid());
    };
    if !fields
        .rotation_keys
        .contains(&s.secrets.plc_keys.current().did())
    {
        return Err(XrpcError::invalid_request(
            "Rotation keys do not include server's rotation key",
        ));
    }
    let pds = fields.services.get("atproto_pds");
    if pds.map(|p| p.kind.as_str()) != Some("AtprotoPersonalDataServer") {
        return Err(XrpcError::invalid_request(
            "Incorrect type on atproto_pds service",
        ));
    }
    if pds.map(|p| p.endpoint.as_str()) != Some(s.public_url.as_str()) {
        return Err(XrpcError::invalid_request(
            "Incorrect endpoint on atproto_pds service",
        ));
    }
    if fields.verification_methods.get("atproto") != Some(&s.signing_key(did).await?.did()) {
        return Err(XrpcError::invalid_request("Incorrect signing key"));
    }
    if let Some(a) = s.store.account_by_did(did).await?
        && fields.also_known_as.first() != Some(&format!("at://{}", a.handle))
    {
        return Err(XrpcError::invalid_request(
            "Incorrect handle in alsoKnownAs",
        ));
    }
    match s.plc()?.submit(did, &op).await {
        Ok(()) => {}
        Err(PlcClientError::Refused(m)) => return Err(XrpcError::invalid_request(m)),
        Err(e) => return Err(upstream(e.to_string())),
    }
    let _ = s
        .store
        .set_plc_key_listing(did, &s.secrets.plc_keys.listed(&fields.rotation_keys))
        .await;
    let (_lock, mut tx) = s
        .repo
        .locked_tx(did)
        .await
        .map_err(|e| XrpcError::internal(e.to_string()))?;
    let body = identity_body(did, None).map_err(|e| XrpcError::internal(e.to_string()))?;
    tx.insert_outbox(did, "identity", &body).await?;
    tx.commit().await?;
    let _ = s.store.clear_did_doc(did).await;
    Ok(())
}

impl PlcStep {
    fn into_error(self) -> XrpcError {
        match self {
            Self::Refused(e) | Self::Ambiguous(e) => e,
        }
    }
}

/// `/.well-known/atproto-did`: the DID of the active account whose handle is
/// the request's host, on one of this PDS's handle domains.
pub async fn well_known_atproto_did(State(s): St, headers: HeaderMap) -> Response {
    let not_found = || (StatusCode::NOT_FOUND, "User not found").into_response();
    let Some(host) = request_host(&headers) else {
        return not_found();
    };
    if handle::service_domain(&host, &s.handle_domains()).is_none() {
        return not_found();
    }
    match s.store.account_by_handle(&host).await {
        Ok(Some(a)) if a.status == Status::Active => {
            ([(header::CONTENT_TYPE, "text/plain")], a.did).into_response()
        }
        Ok(_) => not_found(),
        Err(e) => XrpcError::from(e).into_response(),
    }
}

/// `/.well-known/did.json`: the service DID document on the PDS hostname,
/// or a hosted did:web account's document on its host.
pub async fn well_known_did_json(State(s): St, headers: HeaderMap) -> Response {
    let not_found = || (StatusCode::NOT_FOUND, "Not found").into_response();
    let Some(host) = request_host(&headers) else {
        return not_found();
    };
    if host == s.hostname {
        return Json(service_document(&s).to_json()).into_response();
    }
    match s.store.hosted_did_web_by_host(&host).await {
        Ok(Some(web)) => match web.doc.and_then(|d| serde_json::from_str::<Value>(&d).ok()) {
            Some(doc) => Json(doc).into_response(),
            None => not_found(),
        },
        Ok(None) => not_found(),
        Err(e) => XrpcError::from(e).into_response(),
    }
}

fn service_document(s: &AppState) -> DidDocument {
    DidDocument {
        id: s.service_did.clone(),
        also_known_as: Vec::new(),
        verification_methods: vec![VerificationMethod {
            fragment: "atproto".into(),
            did_key: s.service_did_key(),
        }],
        services: vec![ServiceEntry {
            fragment: "atproto_pds".into(),
            kind: "AtprotoPersonalDataServer".into(),
            endpoint: s.public_url.clone(),
        }],
    }
}

pub async fn health() -> Json<Value> {
    Json(json!({ "version": env!("CARGO_PKG_VERSION") }))
}

#[cfg(test)]
mod tests {
    use super::set_doc_handle;
    use serde_json::json;

    #[test]
    fn hosted_documents_get_the_new_handle_in_place() {
        let mut doc = json!({"id": "did:web:a.test", "alsoKnownAs": ["https://x", "at://a.test"]});
        set_doc_handle(&mut doc, "b.test");
        assert_eq!(doc["alsoKnownAs"], json!(["https://x", "at://b.test"]));
        let mut doc = json!({"id": "did:web:a.test"});
        set_doc_handle(&mut doc, "b.test");
        assert_eq!(doc["alsoKnownAs"], json!(["at://b.test"]));
    }
}
