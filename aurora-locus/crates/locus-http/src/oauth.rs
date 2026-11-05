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
pub fn issuer(s: &AppState) -> String {
    s.public_url.trim_end_matches('/').to_owned()
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
    public_json(locus_auth::oauth::metadata::authorization_server(
        &iss,
        &[iss.clone()],
    ))
}

pub async fn protected_resource_metadata(State(s): St) -> Response {
    let iss = issuer(&s);
    public_json(locus_auth::oauth::metadata::protected_resource(&iss, &iss))
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
    let url = url::Url::parse(&format!("{}{path}", issuer(s)))
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
