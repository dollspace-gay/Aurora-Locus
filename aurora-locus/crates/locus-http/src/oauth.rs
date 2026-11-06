//! OAuth endpoints: the well-known documents and `/oauth/jwks` (more as
//! the flows land).

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use locus_auth::oauth::OAuthError;
use serde_json::{Value, json};

use crate::state::AppState;

type St = State<Arc<AppState>>;

/// The issuer: this PDS's public origin.
#[must_use]
/// The OAuth issuer: this PDS, or in the shard role its entryway (the
/// authorization server its protected-resource metadata names).
pub fn issuer(s: &AppState) -> String {
    match &s.entryway {
        Some(ew) => ew.url.clone(),
        None => s.public_url.trim_end_matches('/').to_owned(),
    }
}

/// An OAuth error response, with its challenge and a fresh DPoP nonce.
#[must_use]
pub fn error_response(s: &AppState, e: &OAuthError) -> Response {
    let mut res = (
        StatusCode::from_u16(e.status).unwrap_or(StatusCode::BAD_REQUEST),
        Json(e.body()),
    )
        .into_response();
    let h = res.headers_mut();
    if let Some(w) = &e.www_authenticate
        && let Ok(v) = HeaderValue::from_str(w)
    {
        h.insert(header::WWW_AUTHENTICATE, v);
    }
    add_nonce(s, h);
    res
}

/// Adds `DPoP-Nonce` and exposes it to browsers.
pub fn add_nonce(s: &AppState, h: &mut HeaderMap) {
    let nonce = s.secrets.dpop_nonces.next(locus_core::time::now_ms());
    if let Ok(v) = HeaderValue::from_str(&nonce) {
        h.insert("dpop-nonce", v);
        h.append(
            header::ACCESS_CONTROL_EXPOSE_HEADERS,
            HeaderValue::from_static("DPoP-Nonce, WWW-Authenticate"),
        );
    }
}

fn public_json(v: Value) -> Response {
    let mut res = Json(v).into_response();
    let h = res.headers_mut();
    h.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    h.insert("access-control-allow-method", HeaderValue::from_static("*"));
    h.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("*"),
    );
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("max-age=300"),
    );
    res
}

pub async fn authorization_server_metadata(State(s): St) -> Response {
    let iss = issuer(&s);
    let mut resources = vec![iss.clone()];
    resources.extend(s.hosts().old_full.iter().map(|h| format!("https://{h}")));
    public_json(locus_auth::oauth::metadata::authorization_server(
        &iss, &resources,
    ))
}

/// The origin a request came to, when it's one this PDS serves in full;
/// otherwise the issuer.
pub(crate) fn served_origin(s: &AppState, headers: &HeaderMap) -> String {
    match crate::identity::request_host(headers) {
        Some(h) if h != s.hostname && s.hosts().serves_in_full(&h) => format!("https://{h}"),
        _ => issuer(s),
    }
}

pub async fn protected_resource_metadata(State(s): St, headers: HeaderMap) -> Response {
    let iss = issuer(&s);
    let resource = served_origin(&s, &headers);
    public_json(locus_auth::oauth::metadata::protected_resource(
        &resource, &iss,
    ))
}

pub async fn jwks(State(s): St) -> Response {
    public_json(json!({ "keys": [s.secrets.oauth_key.jwk()] }))
}

/// Checks a request's DPoP proof against the URL the client used (this
/// PDS's public origin + the path) and records its `jti` so it can't be
/// replayed. `access_token` is set on resource requests.
pub async fn dpop_proof(
    s: &AppState,
    method: &axum::http::Method,
    uri: &axum::http::Uri,
    headers: &HeaderMap,
    access_token: Option<&str>,
) -> Result<Option<locus_auth::oauth::dpop::DpopProof>, OAuthError> {
    let values: Vec<&str> = headers
        .get_all("dpop")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    let path = uri.path_and_query().map_or(uri.path(), |p| p.as_str());
    let url = url::Url::parse(&format!("{}{path}", served_origin(s, headers)))
        .map_err(|_| OAuthError::invalid_request("invalid request URL"))?;
    let proof = locus_auth::oauth::dpop::check_proof(
        &values,
        method.as_str(),
        &url,
        access_token,
        &s.secrets.dpop_nonces,
        locus_core::time::now_ms(),
    )?;
    if let Some(p) = &proof {
        let unique = s
            .store
            .oauth_unique("DPoP", &p.jti, locus_auth::oauth::dpop::JTI_WINDOW_MS)
            .await
            .map_err(|e| OAuthError::server_error(e.to_string()))?;
        if !unique {
            return Err(OAuthError::invalid_dpop_proof("DPoP proof replayed"));
        }
    }
    Ok(proof)
}

// ---------------------------------------------------------------- clients

use locus_auth::oauth::client::{self as oclient, Client, ClientMetadata};

/// How long fetched client metadata and keys are kept (as the reference).
const CLIENT_CACHE_MS: i64 = 10 * 60_000;

/// Fetched client metadata (and `jwks_uri` keys), per node.
#[derive(Default)]
pub struct ClientCache {
    map: std::sync::Mutex<std::collections::HashMap<String, (i64, Value)>>,
}

impl ClientCache {
    fn get(&self, key: &str, now: i64) -> Option<Value> {
        let map = self
            .map
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        map.get(key)
            .filter(|(exp, _)| *exp > now)
            .map(|(_, v)| v.clone())
    }

    fn put(&self, key: &str, v: Value, now: i64) {
        let mut map = self
            .map
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if map.len() > 10_000 {
            map.retain(|_, (exp, _)| *exp > now);
        }
        map.insert(key.to_owned(), (now + CLIENT_CACHE_MS, v));
    }

    /// Stands in for a fetched document (tests).
    pub fn seed(&self, url: &str, v: Value) {
        self.put(url, v, locus_core::time::now_ms());
    }
}

/// Fetches a JSON document a client points at, with the user-influenced
/// client under the stricter rule (no IP-literal hosts).
async fn fetch_json(s: &AppState, url: &str) -> Result<Value, String> {
    use proto_blue_common::fetch::{FetchHandler, HttpRequest};
    let now = locus_core::time::now_ms();
    if let Some(v) = s.oauth_clients.get(url, now) {
        return Ok(v);
    }
    let fetcher = locus_identity::fetch::SafeFetcher::new(locus_identity::fetch::FetchPolicy {
        ssrf_protection: s.ssrf_protection,
        strict: true,
    })
    .map_err(|e| e.to_string())?;
    let mut req = HttpRequest::get(url);
    req.headers
        .insert("accept".into(), "application/json".into());
    let res = fetcher.fetch(req).await.map_err(|e| e.to_string())?;
    if res.status != 200 {
        return Err(format!("Unexpected status code {}", res.status));
    }
    let v: Value = serde_json::from_slice(&res.body).map_err(|_| "Invalid JSON".to_owned())?;
    s.oauth_clients.put(url, v.clone(), now);
    Ok(v)
}

/// A client by id, as the reference's `ClientManager.getClient`.
pub async fn get_client(s: &AppState, client_id: &str) -> Result<Client, OAuthError> {
    let unable = |e: &str| {
        OAuthError::invalid_client_metadata(format!(
            "Unable to obtain client metadata for \"{client_id}\": {e}"
        ))
    };
    let raw = if oclient::is_loopback_client_id(client_id) {
        oclient::loopback_metadata(client_id)?
    } else if oclient::is_discoverable_client_id(client_id) {
        let url = oclient::metadata_url(client_id)?;
        fetch_json(s, url.as_str()).await.map_err(|e| unable(&e))?
    } else {
        return Err(OAuthError::invalid_client_metadata(format!(
            "Invalid client ID \"{client_id}\""
        )));
    };
    let metadata = ClientMetadata::parse(&raw).map_err(|e| unable(&e.description))?;
    oclient::validate_metadata(client_id, &metadata)?;
    let jwks = match (&metadata.jwks, &metadata.jwks_uri) {
        (Some(j), _) => Some(j.clone()),
        (None, Some(uri)) => {
            let raw = fetch_json(s, uri).await.map_err(|e| {
                OAuthError::invalid_client_metadata(format!(
                    "Unable to obtain jwks from \"{uri}\" for \"{client_id}\": {e}"
                ))
            })?;
            Some(oclient::public_jwks(&raw)?)
        }
        (None, None) => None,
    };
    let is_trusted = s
        .snapshot()
        .list("oauth.trusted_clients")
        .contains(&client_id);
    Ok(Client {
        id: client_id.to_owned(),
        metadata,
        jwks,
        is_trusted,
        is_first_party: false,
    })
}

// ---------------------------------------------------------------- flows

use locus_auth::oauth::client::ClientAuth;
use locus_auth::oauth::dpop::DpopProof;
use locus_auth::oauth::request::{self as oreq, Parameters};
use locus_store::oauth::{RequestRow, TokenRow};
use serde_json::Map;

/// A body as JSON or form fields.
fn parse_body(headers: &HeaderMap, body: &[u8]) -> Result<Map<String, Value>, OAuthError> {
    let ct = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ct.starts_with("application/json") {
        match serde_json::from_slice(body) {
            Ok(Value::Object(m)) => Ok(m),
            _ => Err(OAuthError::invalid_request("Invalid JSON body")),
        }
    } else if ct.starts_with("application/x-www-form-urlencoded") {
        Ok(url::form_urlencoded::parse(body)
            .map(|(k, v)| (k.into_owned(), Value::String(v.into_owned())))
            .collect())
    } else {
        Err(OAuthError::invalid_request("Unsupported content-type"))
    }
}

fn field<'a>(m: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    m.get(k).and_then(Value::as_str)
}

fn store_err(e: locus_core::CoreError) -> OAuthError {
    tracing::error!("oauth store: {e}");
    OAuthError::server_error("Internal Server Error")
}

/// As the reference's `authenticateClient`.
async fn authenticate_client(
    s: &AppState,
    m: &Map<String, Value>,
    dpop: Option<&DpopProof>,
    allow_missing_dpop: bool,
    missing_credentials: fn(String) -> OAuthError,
) -> Result<(Client, ClientAuth), OAuthError> {
    let client_id = field(m, "client_id")
        .ok_or_else(|| missing_credentials("Client credentials missing".into()))?;
    let client = get_client(s, client_id).await?;
    let dpop_bound = client.metadata.dpop_bound_access_tokens == Some(true);
    if dpop_bound && dpop.is_none() && !allow_missing_dpop {
        return Err(OAuthError::invalid_dpop_proof("DPoP proof required"));
    }
    if dpop.is_some() && !dpop_bound {
        return Err(OAuthError::invalid_dpop_proof(
            "DPoP proof not allowed for this client",
        ));
    }
    let auth = client.authenticate(
        field(m, "client_assertion_type"),
        field(m, "client_assertion"),
        &issuer(s),
        locus_core::time::now_ms() / 1000,
    )?;
    if let ClientAuth::PrivateKeyJwt { jkt, jti, exp, .. } = &auth {
        if dpop.is_some_and(|d| &d.jkt == jkt) {
            return Err(OAuthError::invalid_request(
                "The DPoP proof must be signed with a different key than the client assertion",
            ));
        }
        let ttl = exp.map_or(66_000, |e| (e * 1000 - locus_core::time::now_ms()).max(1));
        let unique = s
            .store
            .oauth_unique(&format!("Auth@{}", client.id), jti, ttl)
            .await
            .map_err(store_err)?;
        if !unique {
            return Err(OAuthError::invalid_grant("private_key_jwt jti reused"));
        }
    }
    Ok((client, auth))
}

/// Validates a request and checks its permission sets resolve.
async fn validated(s: &AppState, client: &Client, p: Parameters) -> Result<Parameters, OAuthError> {
    let p = oreq::validate(client, p)?;
    crate::lexicon::permission_sets(s, p.scope.as_deref()).await?;
    Ok(p)
}

/// Request parameters, from a request object (JAR) or the fields.
async fn request_parameters(
    s: &AppState,
    client: &Client,
    m: &Map<String, Value>,
) -> Result<Parameters, OAuthError> {
    let fields = if let Some(jar) = field(m, "request") {
        let payload =
            client.decode_request_object(jar, &issuer(s), locus_core::time::now_ms() / 1000)?;
        let jti = payload.get("jti").and_then(Value::as_str).ok_or_else(|| {
            OAuthError::invalid_request("Request object payload must contain a \"jti\" claim")
        })?;
        if !s
            .store
            .oauth_unique(&format!("JAR@{}", client.id), jti, 59_000 * 11 / 10)
            .await
            .map_err(store_err)?
        {
            return Err(OAuthError::invalid_request("Request object was replayed"));
        }
        payload
    } else {
        m.clone()
    };
    Parameters::parse(&fields)
        .map_err(|e| OAuthError::invalid_request(format!("Invalid authorization request: {e}")))
}

/// Errors PAR reports as they are; any other authorization error becomes
/// `invalid_request`, as the reference.
fn par_error(e: OAuthError) -> OAuthError {
    const AS_IS: &[&str] = &[
        "invalid_request",
        "invalid_client",
        "invalid_client_metadata",
        "invalid_client_id",
        "invalid_redirect_uri",
        "invalid_scope",
        "invalid_grant",
        "invalid_dpop_proof",
        "invalid_token",
        "use_dpop_nonce",
        "server_error",
        "invalid_authorization_details",
    ];
    if AS_IS.contains(&e.error.as_str()) {
        e
    } else {
        OAuthError {
            error: "invalid_request".into(),
            ..e
        }
    }
}

fn no_store(mut res: Response, s: &AppState) -> Response {
    let h = res.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    h.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    add_nonce(s, h);
    res
}

fn oauth_result(s: &AppState, r: Result<(StatusCode, Value), OAuthError>) -> Response {
    match r {
        Ok((status, v)) => no_store((status, Json(v)).into_response(), s),
        Err(e) => no_store(error_response(s, &e), s),
    }
}

/// `POST /oauth/par`.
pub async fn par(
    State(s): St,
    method: axum::http::Method,
    uri: axum::http::Uri,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let r = async {
        let m = parse_body(&headers, &body)?;
        let dpop = dpop_proof(&s, &method, &uri, &headers, None).await?;
        let (client, auth) =
            authenticate_client(&s, &m, dpop.as_ref(), true, OAuthError::invalid_client).await?;
        let mut p = request_parameters(&s, &client, &m).await?;
        let dpop_bound = client.metadata.dpop_bound_access_tokens == Some(true);
        match &p.dpop_jkt {
            None => {
                if dpop_bound && let Some(d) = &dpop {
                    p.dpop_jkt = Some(d.jkt.clone());
                }
            }
            Some(jkt) => {
                if !dpop_bound {
                    return Err(OAuthError::invalid_request(
                        "DPoP bound access tokens are not enabled for this client",
                    ));
                }
                if dpop.as_ref().is_some_and(|d| &d.jkt != jkt) {
                    return Err(OAuthError::invalid_dpop_key_binding());
                }
            }
        }
        let p = validated(&s, &client, p).await?;
        let id = oreq::new_request_id();
        let expires = locus_core::time::now_ms() + oreq::PAR_EXPIRES_MS;
        s.store
            .oauth_create_request(&RequestRow {
                id: id.clone(),
                did: None,
                device_id: None,
                client_id: client.id.clone(),
                client_auth: Some(serde_json::to_string(&auth).unwrap_or_default()),
                parameters: serde_json::to_string(&p).unwrap_or_default(),
                expires_at: expires,
                code: None,
            })
            .await
            .map_err(store_err)?;
        Ok((
            StatusCode::CREATED,
            json!({
                "request_uri": format!("{}{id}", oreq::REQUEST_URI_PREFIX),
                "expires_in": oreq::PAR_EXPIRES_MS / 1000,
            }),
        ))
    }
    .await
    .map_err(par_error);
    oauth_result(&s, r)
}

/// An authorization request opened on a device (`RequestManager.get`):
/// it must be unexpired, unauthorized, for this client and (once bound)
/// this device; it then stays open for the inactivity timeout. Any
/// refusal deletes it.
pub async fn open_request(
    s: &AppState,
    request_id: &str,
    device_id: &str,
    client_id: &str,
) -> Result<(RequestRow, Parameters), OAuthError> {
    let mut row = s
        .store
        .oauth_read_request(request_id)
        .await
        .map_err(store_err)?
        .ok_or_else(|| OAuthError::invalid_request("Unknown request_uri"))?;
    let now = locus_core::time::now_ms();
    let refusal = if row.did.is_some() || row.code.is_some() {
        Some("This request was already authorized")
    } else if row.expires_at < now {
        Some("This request has expired")
    } else if row.client_id != client_id {
        Some("This request was initiated for another client")
    } else if row.device_id.as_deref().is_some_and(|d| d != device_id) {
        Some("This request was initiated from another device")
    } else {
        None
    };
    if let Some(r) = refusal {
        let _ = s.store.oauth_delete_request(request_id).await;
        return Err(OAuthError::access_denied(r));
    }
    row.device_id = Some(device_id.to_owned());
    row.expires_at = now + oreq::AUTHORIZATION_INACTIVITY_MS;
    s.store
        .oauth_update_request(&row)
        .await
        .map_err(store_err)?;
    let p: Parameters = serde_json::from_str(&row.parameters)
        .map_err(|e| OAuthError::server_error(e.to_string()))?;
    Ok((row, p))
}

/// Creates a request from the authorize endpoint's own parameters (no
/// PAR, as the reference allows), bound to the device.
pub async fn create_direct_request(
    s: &AppState,
    client: &Client,
    fields: &Map<String, Value>,
    device_id: &str,
) -> Result<(RequestRow, Parameters), OAuthError> {
    let p = request_parameters(s, client, fields).await?;
    let p = validated(s, client, p).await?;
    let row = RequestRow {
        id: oreq::new_request_id(),
        did: None,
        device_id: Some(device_id.to_owned()),
        client_id: client.id.clone(),
        client_auth: None,
        parameters: serde_json::to_string(&p).unwrap_or_default(),
        expires_at: locus_core::time::now_ms() + oreq::PAR_EXPIRES_MS,
        code: None,
    };
    s.store
        .oauth_create_request(&row)
        .await
        .map_err(store_err)?;
    Ok((row, p))
}

/// The account approves a request (`RequestManager.setAuthorized`): the
/// scopes are narrowed to those approved, the grant recorded, and a code
/// issued. Returns the code and the final parameters.
pub async fn authorize(
    s: &AppState,
    request_id: &str,
    device_id: &str,
    did: &str,
    approved_scope: Option<&str>,
) -> Result<(String, Parameters), OAuthError> {
    let mut row = s
        .store
        .oauth_read_request(request_id)
        .await
        .map_err(store_err)?
        .ok_or_else(|| OAuthError::invalid_request("Unknown request_uri"))?;
    let result = async {
        let p: Parameters = serde_json::from_str(&row.parameters)
            .map_err(|e| OAuthError::server_error(e.to_string()))?;
        let account = s.store.account_by_did(did).await.map_err(store_err)?;
        if account
            .as_ref()
            .is_none_or(|a| a.status != locus_store::accounts::Status::Active)
        {
            return Err(OAuthError::access_denied(
                "This account has been deactivated",
            ));
        }
        if row.expires_at < locus_core::time::now_ms() {
            return Err(OAuthError::access_denied("This request has expired"));
        }
        match row.device_id.as_deref() {
            None => return Err(OAuthError::access_denied("This request was not initiated")),
            Some(d) if d != device_id => {
                return Err(OAuthError::access_denied(
                    "This request was initiated from another device",
                ));
            }
            Some(_) => {}
        }
        if row.did.is_some() || row.code.is_some() {
            return Err(OAuthError::access_denied(
                "This request was already authorized",
            ));
        }
        let p = oreq::narrow(p, approved_scope)?;
        let code = oreq::new_code();
        row.did = Some(did.to_owned());
        row.code = Some(code.clone());
        row.expires_at = locus_core::time::now_ms() + oreq::AUTHORIZATION_INACTIVITY_MS;
        row.parameters = serde_json::to_string(&p).unwrap_or_default();
        s.store
            .oauth_update_request(&row)
            .await
            .map_err(store_err)?;
        // Consent records the scopes the user approved.
        let mut approved: Vec<String> = s
            .store
            .oauth_authorized_client(did, &row.client_id)
            .await
            .map_err(store_err)?
            .and_then(|d| serde_json::from_str::<Value>(&d).ok())
            .and_then(|d| d["authorizedScopes"].as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect();
        for sc in p.scopes() {
            if !approved.iter().any(|a| a == sc) {
                approved.push(sc.to_owned());
            }
        }
        s.store
            .oauth_set_authorized_client(
                did,
                &row.client_id,
                &json!({"authorizedScopes": approved}).to_string(),
            )
            .await
            .map_err(store_err)?;
        Ok((code, p))
    }
    .await;
    if result.is_err() {
        let _ = s.store.oauth_delete_request(request_id).await;
    }
    result
}

/// Whether the account must approve the request again (as
/// `checkConsentRequired`).
pub async fn consent_required(s: &AppState, did: &str, p: &Parameters) -> Result<bool, OAuthError> {
    let Some(data) = s
        .store
        .oauth_authorized_client(did, &p.client_id)
        .await
        .map_err(store_err)?
    else {
        return Ok(true);
    };
    if p.prompt.as_deref() == Some("consent") {
        return Ok(true);
    }
    let approved: Vec<String> = serde_json::from_str::<Value>(&data)
        .ok()
        .and_then(|d| d["authorizedScopes"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect();
    Ok(!p.scopes().iter().all(|sc| approved.iter().any(|a| a == sc)))
}

fn access_token(
    s: &AppState,
    token_id: &str,
    did: &str,
    client_id: &str,
    jkt: Option<&str>,
    iat_ms: i64,
    exp_ms: i64,
) -> String {
    let mut claims = json!({
        "iss": issuer(s),
        "jti": token_id,
        "sub": did,
        "aud": s.service_did,
        "iat": iat_ms / 1000,
        "exp": exp_ms / 1000,
        "client_id": client_id,
    });
    if let Some(j) = jkt {
        claims["cnf"] = json!({ "jkt": j });
    }
    let mut header = Map::new();
    header.insert("typ".into(), json!("at+jwt"));
    s.secrets.oauth_key.sign(header, &claims)
}

fn token_expiry(client: &Client, now: i64, created: i64) -> Result<i64, OAuthError> {
    let lifetime = oreq::TOKEN_MAX_AGE_MS.min(created + client.session_lifetime_ms() - now);
    if lifetime <= 250 {
        return Err(OAuthError::invalid_grant("Session expired"));
    }
    Ok(now + lifetime)
}

fn token_response(
    access: &str,
    refresh: Option<&str>,
    scope: &str,
    expires_ms: i64,
    did: &str,
    dpop: bool,
) -> Value {
    let mut v = json!({
        "access_token": access,
        "token_type": if dpop { "DPoP" } else { "Bearer" },
        "scope": scope,
        "expires_in": (expires_ms - locus_core::time::now_ms()) / 1000,
        "sub": did,
    });
    if let Some(r) = refresh {
        v["refresh_token"] = json!(r);
    }
    v
}

/// As the reference's `compareClientAuth`.
fn compare_client_auth(
    client: &Client,
    auth: &ClientAuth,
    dpop: Option<&DpopProof>,
    initial_client: &str,
    initial_params: &Parameters,
    initial_auth: Option<&str>,
) -> Result<(), OAuthError> {
    let method = match auth {
        ClientAuth::None => "none",
        ClientAuth::PrivateKeyJwt { .. } => "private_key_jwt",
    };
    if method != client.metadata.token_endpoint_auth_method {
        return Err(OAuthError::invalid_grant(format!(
            "Client authentication method mismatch (expected {}, got {method})",
            client.metadata.token_endpoint_auth_method
        )));
    }
    if initial_client != client.id {
        return Err(OAuthError::invalid_grant(
            "Token was not issued to this client",
        ));
    }
    if let Some(jkt) = &initial_params.dpop_jkt {
        match dpop {
            None => {
                return Err(OAuthError::invalid_grant(
                    "DPoP proof is required for this request",
                ));
            }
            Some(d) if &d.jkt != jkt => {
                return Err(OAuthError::invalid_grant(
                    "DPoP proof does not match the expected JKT",
                ));
            }
            Some(_) => {}
        }
    }
    let Some(initial) = initial_auth.and_then(|a| serde_json::from_str::<Value>(a).ok()) else {
        return Ok(());
    };
    match initial["method"].as_str() {
        Some("private_key_jwt" | locus_auth::oauth::client::CLIENT_ASSERTION_TYPE_JWT_BEARER) => {
            let ClientAuth::PrivateKeyJwt { kid, alg, jkt, .. } = auth else {
                return Err(OAuthError::invalid_grant(format!(
                    "Client authentication method mismatch (expected {})",
                    initial["method"].as_str().unwrap_or("")
                )));
            };
            if initial["kid"].as_str() != Some(kid.as_str())
                || initial["alg"].as_str() != Some(alg.as_str())
                || initial["jkt"].as_str() != Some(jkt.as_str())
            {
                return Err(OAuthError::invalid_grant(
                    "The session was initiated with a different key than the client assertion currently used",
                ));
            }
            Ok(())
        }
        Some("none") => Ok(()),
        other => Err(OAuthError::invalid_grant(format!(
            "Invalid method \"{}\"",
            other.unwrap_or("")
        ))),
    }
}

async fn code_grant(
    s: &AppState,
    client: &Client,
    auth: &ClientAuth,
    m: &Map<String, Value>,
    dpop: Option<&DpopProof>,
) -> Result<Value, OAuthError> {
    let code = field(m, "code")
        .filter(|c| !c.is_empty())
        .ok_or_else(|| OAuthError::invalid_grant("Invalid request payload"))?;
    let redirect_uri = field(m, "redirect_uri")
        .ok_or_else(|| OAuthError::invalid_grant("Invalid request payload"))?;
    let verifier = field(m, "code_verifier");
    if verifier.is_some_and(|v| !oreq::is_code_verifier(v)) {
        return Err(OAuthError::invalid_grant("Invalid request payload"));
    }
    if !code.starts_with("cod-") {
        return Err(OAuthError::invalid_grant("Invalid code"));
    }
    let consumed = s.store.oauth_consume_code(code).await.map_err(store_err)?;
    let Some(row) = consumed.filter(|r| r.did.is_some() && r.code.as_deref() == Some(code)) else {
        // A replayed code revokes the token it was exchanged for.
        if let Some(t) = s.store.oauth_token_by_code(code).await.map_err(store_err)? {
            s.store.oauth_delete_token(t.id).await.map_err(store_err)?;
        }
        return Err(OAuthError::invalid_grant("Invalid code"));
    };
    if row.expires_at < locus_core::time::now_ms() {
        return Err(OAuthError::invalid_grant("This code has expired"));
    }
    let mut p: Parameters = serde_json::from_str(&row.parameters)
        .map_err(|e| OAuthError::server_error(e.to_string()))?;
    compare_client_auth(
        client,
        auth,
        dpop,
        &row.client_id,
        &p,
        row.client_auth.as_deref(),
    )?;
    if let Some(d) = dpop
        && client.metadata.dpop_bound_access_tokens == Some(true)
        && p.dpop_jkt.is_none()
    {
        p.dpop_jkt = Some(d.jkt.clone());
    }
    oreq::check_code_grant(&p, redirect_uri, verifier)?;
    if let Some(challenge) = &p.code_challenge
        && !s
            .store
            .oauth_unique("CodeChallenge", challenge, oreq::CODE_CHALLENGE_REPLAY_MS)
            .await
            .map_err(store_err)?
    {
        return Err(OAuthError::invalid_grant("Code challenge already used"));
    }
    let did = row.did.clone().unwrap_or_default();
    if s.store
        .account_by_did(&did)
        .await
        .map_err(store_err)?
        .is_none()
    {
        return Err(OAuthError::invalid_grant("Account no longer exists"));
    }
    if client.metadata.dpop_bound_access_tokens == Some(true) && p.dpop_jkt.is_none() {
        return Err(OAuthError::invalid_grant(
            "DPoP JKT is required for DPoP bound access tokens",
        ));
    }
    let token_id = oreq::new_token_id();
    let refresh = client
        .metadata
        .grant_types
        .iter()
        .any(|g| g == "refresh_token")
        .then(oreq::new_refresh_token);
    let now = locus_core::time::now_ms();
    let expires = token_expiry(client, now, now)?;
    let scope = crate::lexicon::token_scope(s, p.scope.as_deref().unwrap_or("atproto"))
        .await
        .map_err(|e| OAuthError::invalid_request(e.description))?;
    let at = access_token(
        s,
        &token_id,
        &did,
        &client.id,
        p.dpop_jkt.as_deref(),
        now,
        expires,
    );
    s.store
        .oauth_create_token(&TokenRow {
            id: 0,
            did: did.clone(),
            token_id,
            created_at: now,
            updated_at: now,
            expires_at: expires,
            client_id: client.id.clone(),
            client_auth: serde_json::to_string(auth).unwrap_or_default(),
            device_id: row.device_id.clone(),
            parameters: serde_json::to_string(&p).unwrap_or_default(),
            details: None,
            code: Some(code.to_owned()),
            current_refresh_token: refresh.clone(),
            scope: Some(scope.clone()),
        })
        .await
        .map_err(store_err)?;
    Ok(token_response(
        &at,
        refresh.as_deref(),
        &scope,
        expires,
        &did,
        p.dpop_jkt.is_some(),
    ))
}

async fn refresh_grant(
    s: &AppState,
    client: &Client,
    auth: &ClientAuth,
    m: &Map<String, Value>,
    dpop: Option<&DpopProof>,
) -> Result<Value, OAuthError> {
    let refresh = field(m, "refresh_token")
        .filter(|r| r.starts_with("ref-"))
        .ok_or_else(|| OAuthError::invalid_grant("Invalid refresh token"))?;
    let row = s
        .store
        .oauth_token_by_refresh(refresh)
        .await
        .map_err(store_err)?
        .ok_or_else(|| OAuthError::invalid_grant("Invalid refresh token"))?;
    if row.current_refresh_token.as_deref() != Some(refresh) {
        // A replayed refresh token ends the session.
        s.store
            .oauth_delete_token(row.id)
            .await
            .map_err(store_err)?;
        return Err(OAuthError::invalid_grant("Refresh token replayed"));
    }
    let result = async {
        let p: Parameters = serde_json::from_str(&row.parameters)
            .map_err(|e| OAuthError::server_error(e.to_string()))?;
        compare_client_auth(
            client,
            auth,
            dpop,
            &row.client_id,
            &p,
            Some(&row.client_auth),
        )?;
        if !client
            .metadata
            .grant_types
            .iter()
            .any(|g| g == "refresh_token")
        {
            return Err(OAuthError::invalid_grant("Refresh token grant not allowed"));
        }
        let now = locus_core::time::now_ms();
        if now - row.created_at > client.session_lifetime_ms() {
            return Err(OAuthError::invalid_grant("Session expired"));
        }
        if now - row.updated_at > client.refresh_lifetime_ms() {
            return Err(OAuthError::invalid_grant("Refresh token expired"));
        }
        if client.metadata.dpop_bound_access_tokens == Some(true) && p.dpop_jkt.is_none() {
            return Err(OAuthError::invalid_grant(
                "DPoP JKT is required for DPoP bound access tokens",
            ));
        }
        let token_id = oreq::new_token_id();
        let next = oreq::new_refresh_token();
        let expires = token_expiry(client, now, row.created_at)?;
        let scope = crate::lexicon::token_scope(s, p.scope.as_deref().unwrap_or("atproto")).await?;
        s.store
            .oauth_rotate_token(
                row.id,
                &token_id,
                &next,
                now,
                expires,
                &serde_json::to_string(auth).unwrap_or_default(),
                Some(&scope),
            )
            .await
            .map_err(store_err)?;
        let at = access_token(
            s,
            &token_id,
            &row.did,
            &client.id,
            p.dpop_jkt.as_deref(),
            now,
            expires,
        );
        Ok(token_response(
            &at,
            Some(&next),
            &scope,
            expires,
            &row.did,
            p.dpop_jkt.is_some(),
        ))
    }
    .await;
    if result.is_err() {
        let _ = s.store.oauth_delete_token(row.id).await;
    }
    result
}

/// `POST /oauth/token`.
pub async fn token(
    State(s): St,
    method: axum::http::Method,
    uri: axum::http::Uri,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let r = async {
        let m = parse_body(&headers, &body)?;
        let dpop = dpop_proof(&s, &method, &uri, &headers, None).await?;
        let (client, auth) =
            authenticate_client(&s, &m, dpop.as_ref(), false, OAuthError::invalid_grant).await?;
        let grant = field(&m, "grant_type").unwrap_or("");
        if ![
            "authorization_code",
            "refresh_token",
            "password",
            "client_credentials",
        ]
        .contains(&grant)
        {
            return Err(OAuthError::invalid_grant("Invalid request payload"));
        }
        if !["authorization_code", "refresh_token"].contains(&grant) {
            return Err(OAuthError::invalid_grant(format!(
                "Grant type \"{grant}\" is not supported by the server"
            )));
        }
        if !client.metadata.grant_types.iter().any(|g| g == grant) {
            return Err(OAuthError::invalid_grant(format!(
                "\"{grant}\" grant type is not allowed for this client"
            )));
        }
        let v = if grant == "authorization_code" {
            code_grant(&s, &client, &auth, &m, dpop.as_ref()).await?
        } else {
            refresh_grant(&s, &client, &auth, &m, dpop.as_ref()).await?
        };
        Ok((StatusCode::OK, v))
    }
    .await;
    oauth_result(&s, r)
}

/// `POST /oauth/revoke`: answers `{}` whatever happened, as the reference.
pub async fn revoke(
    State(s): St,
    method: axum::http::Method,
    uri: axum::http::Uri,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let r = async {
        let m = parse_body(&headers, &body)?;
        let token = field(&m, "token")
            .filter(|t| !t.is_empty())
            .ok_or_else(|| OAuthError::invalid_request("Invalid request payload"))?
            .to_owned();
        let dpop = dpop_proof(&s, &method, &uri, &headers, None).await?;
        let found = async {
            let (client, auth) =
                authenticate_client(&s, &m, dpop.as_ref(), false, OAuthError::invalid_request)
                    .await?;
            let row = if token.starts_with("tok-") {
                s.store.oauth_token(&token).await.map_err(store_err)?
            } else if token.starts_with("cod-") {
                s.store
                    .oauth_token_by_code(&token)
                    .await
                    .map_err(store_err)?
            } else if token.starts_with("ref-") {
                s.store
                    .oauth_token_by_refresh(&token)
                    .await
                    .map_err(store_err)?
            } else {
                let jws = locus_auth::oauth::jose::Jws::decode(&token)
                    .map_err(|_| OAuthError::invalid_request("Invalid token"))?;
                s.secrets
                    .oauth_key
                    .verify(&jws)
                    .map_err(|_| OAuthError::invalid_request("Invalid token"))?;
                let jti = jws.payload.get("jti").and_then(Value::as_str).unwrap_or("");
                s.store.oauth_token(jti).await.map_err(store_err)?
            };
            if let Some(row) = row {
                let p: Parameters = serde_json::from_str(&row.parameters).unwrap_or_default();
                compare_client_auth(
                    &client,
                    &auth,
                    dpop.as_ref(),
                    &row.client_id,
                    &p,
                    Some(&row.client_auth),
                )?;
                s.store
                    .oauth_delete_token(row.id)
                    .await
                    .map_err(store_err)?;
            }
            Ok::<_, OAuthError>(())
        }
        .await;
        if let Err(e) = found {
            tracing::info!("revoke: {e}");
        }
        Ok((StatusCode::OK, json!({})))
    }
    .await;
    oauth_result(&s, r)
}

/// Preflight for the OAuth endpoints.
pub async fn preflight() -> Response {
    let mut res = StatusCode::OK.into_response();
    let h = res.headers_mut();
    h.insert("access-control-max-age", HeaderValue::from_static("86400"));
    h.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    h.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("*"),
    );
    h.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("Content-Type,DPoP"),
    );
    res
}
