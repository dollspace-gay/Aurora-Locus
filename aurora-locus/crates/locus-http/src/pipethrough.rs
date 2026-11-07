//! Pipethrough: XRPC methods locus doesn't serve itself go to another
//! service, as the reference. `atproto-proxy: <did>#<service>` picks the
//! target; without it, the reference's moderation methods go to the
//! moderation service, `createReport` to the report service, and everything
//! else to the AppView. The request is forwarded with a service token from
//! the account (bare-DID audience) and the response streamed back.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use futures::{Stream, StreamExt};
use locus_auth::session::SCOPE_TAKENDOWN;
use serde_json::{Value, json};

use crate::auth;
use crate::error::{XrpcError, XrpcResult};
use crate::state::AppState;

type St = State<std::sync::Arc<AppState>>;

/// A response or an error response (with any headers it carries).
pub type ProxyResult<T> = Result<T, Response>;

fn err(e: XrpcError) -> Response {
    e.into_response()
}

// ---------------------------------------------------------------- methods

/// Account-management methods used directly, never proxied or reached by
/// service auth (the reference's list, matched case-insensitively).
pub const PROTECTED_METHODS: &[&str] = &[
    "com.atproto.admin.sendEmail",
    "com.atproto.identity.requestPlcOperationSignature",
    "com.atproto.identity.signPlcOperation",
    "com.atproto.identity.updateHandle",
    "com.atproto.server.activateAccount",
    "com.atproto.server.confirmEmail",
    "com.atproto.server.createAppPassword",
    "com.atproto.server.deactivateAccount",
    "com.atproto.server.getAccountInviteCodes",
    "com.atproto.server.getSession",
    "com.atproto.server.listAppPasswords",
    "com.atproto.server.requestAccountDelete",
    "com.atproto.server.requestEmailConfirmation",
    "com.atproto.server.requestEmailUpdate",
    "com.atproto.server.revokeAppPassword",
    "com.atproto.server.updateEmail",
];

#[must_use]
pub fn is_protected_method(lxm: &str) -> bool {
    PROTECTED_METHODS
        .iter()
        .any(|m| m.eq_ignore_ascii_case(lxm))
}

/// Methods that need full access or a privileged app password.
#[must_use]
pub fn is_privileged_method(lxm: &str) -> bool {
    let l = lxm.to_ascii_lowercase();
    l.starts_with("chat.bsky.") || l == "com.atproto.server.createaccount"
}

/// The reference's moderation methods that go to the moderation service.
const MODERATION_METHODS: &[&str] = &[
    "tools.ozone.inbox.appealActionedSubject",
    "tools.ozone.communication.createTemplate",
    "tools.ozone.communication.deleteTemplate",
    "tools.ozone.communication.listTemplates",
    "tools.ozone.communication.updateTemplate",
    "tools.ozone.moderation.cancelScheduledActions",
    "tools.ozone.moderation.emitEvent",
    "tools.ozone.moderation.getAccountTimeline",
    "tools.ozone.moderation.getEvent",
    "tools.ozone.moderation.getRecord",
    "tools.ozone.moderation.getRepo",
    "tools.ozone.moderation.listScheduledActions",
    "tools.ozone.moderation.queryEvents",
    "tools.ozone.moderation.queryStatuses",
    "tools.ozone.moderation.scheduleAction",
    "tools.ozone.moderation.searchRepos",
    "tools.ozone.safelink.addRule",
    "tools.ozone.safelink.queryEvents",
    "tools.ozone.safelink.queryRules",
    "tools.ozone.safelink.removeRule",
    "tools.ozone.safelink.syncSource",
    "tools.ozone.safelink.updateRule",
    "tools.ozone.team.addMember",
    "tools.ozone.team.deleteMember",
    "tools.ozone.team.listMembers",
    "tools.ozone.team.updateMember",
    "tools.ozone.verification.grantVerifications",
    "tools.ozone.verification.listVerifications",
    "tools.ozone.verification.revokeVerifications",
];

// ---------------------------------------------------------------- targets

/// Where a request goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub url: String,
    pub did: String,
    pub service_id: String,
    /// Resolved from an `atproto-proxy` DID (the SSRF-safe client), rather
    /// than configured by the operator.
    pub user_influenced: bool,
}

/// A configured service: (url, did), if both are set.
pub(crate) fn configured(s: &AppState, url_key: &str, did_key: &str) -> Option<(String, String)> {
    let snap = s.snapshot();
    let (url, did) = (snap.text(url_key), snap.text(did_key));
    (!url.is_empty() && !did.is_empty()).then(|| (url.to_owned(), did.to_owned()))
}

/// The configured AppView (url, did): public reads are proxied here.
#[must_use]
pub fn appview(s: &AppState) -> Option<(String, String)> {
    configured(s, "appview.url", "appview.did")
}

/// The private AppView (prism; url, did): the private tier's authority.
/// It has its own entry, even when it's the same service as the AppView;
/// with none, the private tier is off.
#[must_use]
pub fn private_appview(s: &AppState) -> Option<(String, String)> {
    configured(s, "appview.private_url", "appview.private_did")
}

/// The reference's default service for a method.
fn default_service(s: &AppState, lxm: &str) -> (&'static str, Option<(String, String)>) {
    if MODERATION_METHODS.contains(&lxm) {
        (
            "atproto_labeler",
            configured(s, "moderation.service_url", "moderation.service_did"),
        )
    } else if lxm == "com.atproto.moderation.createReport" {
        (
            "atproto_labeler",
            configured(
                s,
                "moderation.report_service_url",
                "moderation.report_service_did",
            )
            .or_else(|| configured(s, "moderation.service_url", "moderation.service_did")),
        )
    } else {
        ("bsky_appview", appview(s))
    }
}

/// The target of a request for `lxm`: the `atproto-proxy` header's service,
/// or the method's default.
pub async fn target(s: &AppState, headers: &HeaderMap, lxm: &str) -> XrpcResult<Target> {
    if let Some(v) = headers.get("atproto-proxy") {
        let v = v
            .to_str()
            .map_err(|_| XrpcError::invalid_request("invalid proxy header format"))?;
        return parse_proxy_header(s, v).await;
    }
    let (service_id, info) = default_service(s, lxm);
    let (url, did) =
        info.ok_or_else(|| XrpcError::invalid_request(format!("No service configured for {lxm}")))?;
    Ok(Target {
        url,
        did,
        service_id: service_id.to_owned(),
        user_influenced: false,
    })
}

/// Parses `<did>#<service>` and resolves the service endpoint, as the
/// reference (the configured AppView's `#bsky_appview` without resolving).
pub async fn parse_proxy_header(s: &AppState, value: &str) -> XrpcResult<Target> {
    let bad = XrpcError::invalid_request;
    let hash = value.find('#');
    match hash {
        Some(0) => return Err(bad("no did specified in proxy header")),
        None => return Err(bad("no service id specified in proxy header")),
        Some(i) if i == value.len() - 1 => {
            return Err(bad("no service id specified in proxy header"));
        }
        Some(i) if value[i + 1..].contains('#') => return Err(bad("invalid proxy header format")),
        _ => {}
    }
    if value.contains(' ') {
        return Err(bad("proxy header cannot contain spaces"));
    }
    let (did, service_id) = value.split_once('#').expect("checked above");
    if let Some((url, av_did)) = appview(s)
        && value == format!("{av_did}#bsky_appview")
    {
        return Ok(Target {
            url,
            did: did.to_owned(),
            service_id: service_id.to_owned(),
            user_influenced: false,
        });
    }
    let doc = s
        .resolve_did_doc(did, false)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| bad("could not resolve proxy did"))?;
    let url =
        proto_blue_common::did_doc::get_service_endpoint(&doc, &format!("#{service_id}"), None)
            .ok_or_else(|| bad("could not resolve proxy did service url"))?;
    Ok(Target {
        url,
        did: did.to_owned(),
        service_id: service_id.to_owned(),
        user_influenced: true,
    })
}

// ---------------------------------------------------------------- sending

/// Clients by (user-influenced, SSRF protection, HTTP/2).
type Clients = Mutex<HashMap<(bool, bool, bool), reqwest::Client>>;

/// One client per (user-influenced, HTTP/2) pair, built on first use.
pub(crate) fn client(s: &AppState, user_influenced: bool) -> XrpcResult<reqwest::Client> {
    static CLIENTS: OnceLock<Clients> = OnceLock::new();
    let http2 = s.snapshot().bool("proxy.allow_http2");
    // Operator-configured services may be private addresses (§ outbound
    // clients); only user-influenced targets get the unicast resolver.
    let ssrf = user_influenced && s.ssrf_protection;
    let key = (user_influenced, ssrf, http2);
    let mut map = CLIENTS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(c) = map.get(&key) {
        return Ok(c.clone());
    }
    let c = locus_identity::fetch::streaming_client(ssrf, http2)
        .map_err(|e| XrpcError::internal(e.to_string()))?;
    map.insert(key, c.clone());
    Ok(c)
}

/// What to forward.
pub struct Outgoing<'a> {
    pub method: Method,
    pub lxm: &'a str,
    /// Path and query, as received.
    pub path_and_query: &'a str,
    pub headers: &'a HeaderMap,
    pub body: Option<Body>,
    /// Sign a service token as this account.
    pub iss: Option<&'a str>,
    /// The token's audience, if not the target's DID (`getFeed`'s feed
    /// generator).
    pub aud: Option<&'a str>,
    pub target: &'a Target,
    /// `accept-encoding` to send (the client's, or `identity`).
    pub accept_encoding: Option<String>,
    /// The stricter URL rule (no IP-literal hosts): `createReport` and push
    /// services resolved from a DID.
    pub strict: bool,
}

/// Request headers the reference forwards.
fn forwarded_headers(from: &HeaderMap, out: &mut reqwest::header::HeaderMap, with_body: bool) {
    let mut copy = |name: &str| {
        if let Some(v) = from.get(name)
            && let (Ok(n), Ok(v)) = (
                reqwest::header::HeaderName::from_bytes(name.as_bytes()),
                reqwest::header::HeaderValue::from_bytes(v.as_bytes()),
            )
        {
            out.insert(n, v);
        }
    };
    copy("accept-language");
    copy("atproto-accept-labelers");
    copy("x-bsky-topics");
    if with_body {
        copy("content-type");
        copy("content-encoding");
        copy("content-length");
    }
    for (name, _) in from {
        if name.as_str().starts_with("x-atproto-") {
            copy(name.as_str());
        }
    }
}

/// Sends a request to its target. Upstream errors (status ≥ 400) come back
/// as the error response the client gets: the upstream's `{error, message}`
/// with its status (500 → 502 `UpstreamFailure`) and forwarded headers.
pub async fn send(s: &AppState, o: Outgoing<'_>) -> ProxyResult<reqwest::Response> {
    let upstream = send_inner(s, o).await?;
    if upstream.status().as_u16() >= 400 {
        return Err(UpstreamError::read(s, upstream).await.into_response());
    }
    Ok(upstream)
}

async fn send_inner(s: &AppState, o: Outgoing<'_>) -> ProxyResult<reqwest::Response> {
    let snap = s.snapshot();
    let base = o.target.url.trim_end_matches('/');
    let url = reqwest::Url::parse(&format!("{base}{}", o.path_and_query))
        .map_err(|_| err(XrpcError::invalid_request("invalid service url")))?;
    if o.target.user_influenced {
        let policy = locus_identity::fetch::FetchPolicy {
            ssrf_protection: s.ssrf_protection,
            strict: o.strict,
        };
        locus_identity::fetch::check_url(&url, policy).map_err(|e| {
            tracing::info!(url = %url, "proxy target refused: {e}");
            err(unreachable())
        })?;
    }
    let mut headers = reqwest::header::HeaderMap::new();
    forwarded_headers(o.headers, &mut headers, o.body.is_some());
    headers.insert(
        reqwest::header::ACCEPT_ENCODING,
        reqwest::header::HeaderValue::from_str(o.accept_encoding.as_deref().unwrap_or("identity"))
            .unwrap_or(reqwest::header::HeaderValue::from_static("identity")),
    );
    if let Some(iss) = o.iss {
        let token = service_token(s, iss, o.aud.unwrap_or(&o.target.did), o.lxm)
            .await
            .map_err(err)?;
        if let Ok(v) = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}")) {
            headers.insert(reqwest::header::AUTHORIZATION, v);
        }
    }
    let client = client(s, o.target.user_influenced).map_err(err)?;
    let method =
        reqwest::Method::from_bytes(o.method.as_str().as_bytes()).unwrap_or(reqwest::Method::GET);
    let headers_timeout = Duration::from_millis(snap.int("proxy.headers_timeout_ms").max(1) as u64);
    let retries = if o.body.is_none() && matches!(o.method, Method::GET | Method::HEAD) {
        snap.int("proxy.max_retries").max(0)
    } else {
        0
    };
    let mut body = o.body;
    let mut attempt = 0;
    let upstream = loop {
        let mut req = client
            .request(method.clone(), url.clone())
            .headers(headers.clone());
        if let Some(b) = body.take() {
            req = req.body(reqwest::Body::wrap_stream(b.into_data_stream()));
        }
        match tokio::time::timeout(headers_timeout, req.send()).await {
            Ok(Ok(r)) => break r,
            Ok(Err(e)) if attempt < retries && e.is_connect() => {
                attempt += 1;
            }
            Ok(Err(e)) => {
                tracing::warn!(url = %url, "upstream request failed: {e}");
                return Err(err(unreachable()));
            }
            Err(_) => {
                tracing::warn!(url = %url, "upstream headers timed out");
                return Err(err(unreachable()));
            }
        }
    };
    Ok(upstream)
}

fn unreachable() -> XrpcError {
    XrpcError::new(
        StatusCode::BAD_GATEWAY,
        "UpstreamFailure",
        "Upstream service unreachable",
    )
}

pub(crate) async fn service_token(
    s: &AppState,
    iss: &str,
    aud: &str,
    lxm: &str,
) -> XrpcResult<String> {
    let key = s.signing_key(iss).await?;
    let now = locus_core::time::now_ms() / 1000;
    locus_auth::service_jwt::create(&key, iss, aud, None, Some(lxm), now)
        .map_err(|e| XrpcError::internal(e.to_string()))
}

/// Response headers the reference forwards (content headers first).
fn response_headers(
    from: &reqwest::header::HeaderMap,
    content: bool,
) -> Vec<(HeaderName, HeaderValue)> {
    let mut names: Vec<&str> = Vec::new();
    if content {
        names.extend([
            "content-length",
            "content-encoding",
            "content-type",
            "content-language",
        ]);
    }
    names.extend([
        "atproto-repo-rev",
        "atproto-content-labelers",
        "retry-after",
    ]);
    names
        .into_iter()
        .filter_map(|n| {
            let vals: Vec<&str> = from
                .get_all(n)
                .iter()
                .filter_map(|v| v.to_str().ok())
                .collect();
            if vals.is_empty() {
                return None;
            }
            Some((
                HeaderName::from_static(n),
                HeaderValue::from_str(&vals.join(",")).ok()?,
            ))
        })
        .collect()
}

/// The XRPC name of an HTTP status, as the reference's `ResponseType`.
fn type_name(status: u16) -> &'static str {
    match status {
        400 => "InvalidRequest",
        401 => "AuthenticationRequired",
        403 => "Forbidden",
        404 => "XRPCNotSupported",
        406 => "NotAcceptable",
        413 => "PayloadTooLarge",
        415 => "UnsupportedMediaType",
        429 => "RateLimitExceeded",
        500 => "InternalServerError",
        501 => "MethodNotImplemented",
        502 => "UpstreamFailure",
        503 => "NotEnoughResources",
        504 => "UpstreamTimeout",
        _ => "Unknown",
    }
}

/// An upstream error, as the client gets it: the upstream's status (500 →
/// 502), its `error` name (or the status's XRPC name) and `message`, and
/// the forwarded headers.
#[derive(Debug, Clone)]
pub struct UpstreamError {
    pub status: StatusCode,
    pub error: String,
    pub message: String,
    pub headers: Vec<(HeaderName, HeaderValue)>,
}

impl UpstreamError {
    pub async fn read(s: &AppState, upstream: reqwest::Response) -> Self {
        let code = upstream.status().as_u16();
        let status = if code == 500 {
            StatusCode::BAD_GATEWAY
        } else {
            StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_GATEWAY)
        };
        let headers = response_headers(upstream.headers(), false);
        let json_type = upstream
            .headers()
            .get(header::CONTENT_TYPE.as_str())
            .and_then(|v| v.to_str().ok())
            .is_some_and(is_json_type);
        let identity = upstream.headers().get("content-encoding").is_none();
        let max =
            usize::try_from(s.snapshot().int("proxy.max_response_size")).unwrap_or(usize::MAX);
        let parsed: Value = if json_type && identity {
            read_capped(upstream.bytes_stream(), max)
                .await
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or(Value::Null)
        } else {
            Value::Null
        };
        Self {
            error: parsed["error"]
                .as_str()
                .map_or_else(|| type_name(status.as_u16()).to_owned(), str::to_owned),
            message: parsed["message"].as_str().map_or_else(
                || status.canonical_reason().unwrap_or("Unknown").to_owned(),
                str::to_owned,
            ),
            status,
            headers,
        }
    }

    /// A forwarded header's value.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.as_str() == name)
            .and_then(|(_, v)| v.to_str().ok())
    }
}

impl IntoResponse for UpstreamError {
    fn into_response(self) -> Response {
        let mut res = (
            self.status,
            axum::Json(json!({"error": self.error, "message": self.message})),
        )
            .into_response();
        for (n, v) in self.headers {
            res.headers_mut().insert(n, v);
        }
        res
    }
}

/// Like [`send`], but an upstream error comes back as the response itself
/// (for handlers that look at it, such as `getPostThread`'s `NotFound`).
pub async fn send_any(s: &AppState, o: Outgoing<'_>) -> ProxyResult<reqwest::Response> {
    send_inner(s, o).await
}

fn is_json_type(ct: &str) -> bool {
    let ct = ct.to_ascii_lowercase();
    let mime = ct.split(';').next().unwrap_or("").trim();
    mime.strip_prefix("application/")
        .is_some_and(|sub| sub == "json" || sub.ends_with("+json"))
}

/// Reads a body up to `max` bytes.
pub async fn read_capped(
    mut stream: impl Stream<Item = reqwest::Result<axum::body::Bytes>> + Unpin,
    max: usize,
) -> Result<Vec<u8>, XrpcError> {
    let mut out = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| unreachable())?;
        if out.len() + chunk.len() > max {
            return Err(XrpcError::new(
                StatusCode::BAD_GATEWAY,
                "UpstreamFailure",
                "upstream response too large",
            ));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// Streams an upstream response back: its status, the forwarded headers,
/// and the body, cut off past `proxy.max_response_size` or after
/// `proxy.body_timeout_ms` without a chunk.
pub fn stream_back(s: &AppState, upstream: reqwest::Response) -> Response {
    let snap = s.snapshot();
    let max = u64::try_from(snap.int("proxy.max_response_size")).unwrap_or(u64::MAX);
    let idle = Duration::from_millis(
        u64::try_from(snap.int("proxy.body_timeout_ms"))
            .unwrap_or(1)
            .max(1),
    );
    let status = StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::OK);
    let headers = response_headers(upstream.headers(), true);
    let io = |m: String| std::io::Error::other(m);
    let state = (Box::pin(upstream.bytes_stream()), 0u64, false);
    let stream = futures::stream::unfold(state, move |(mut inner, seen, done)| async move {
        if done {
            return None;
        }
        match tokio::time::timeout(idle, inner.next()).await {
            Err(_) => Some((
                Err(io("upstream body timed out".into())),
                (inner, seen, true),
            )),
            Ok(None) => None,
            Ok(Some(Err(e))) => Some((Err(io(e.to_string())), (inner, seen, true))),
            Ok(Some(Ok(chunk))) => {
                let seen = seen + chunk.len() as u64;
                if seen > max {
                    Some((
                        Err(io("upstream response too large".into())),
                        (inner, seen, true),
                    ))
                } else {
                    Some((Ok(chunk), (inner, seen, false)))
                }
            }
        }
    });
    let mut res = Response::new(Body::from_stream(stream));
    *res.status_mut() = status;
    for (n, v) in headers {
        res.headers_mut().insert(n, v);
    }
    res
}

// ---------------------------------------------------------------- handler

/// The catch-all for `/xrpc/<nsid>` methods with no local handler.
pub async fn proxy(
    State(s): St,
    Path(lxm): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Body,
) -> Response {
    if crate::audience::is_prism_audience_call(&s, &headers, &lxm) {
        return crate::audience::proxied(&s, &lxm, method, &uri, &headers, body).await;
    }
    match proxy_inner(&s, &lxm, method, &uri, &headers, body).await {
        Ok(r) | Err(r) => r,
    }
}

pub(crate) async fn proxy_inner(
    s: &AppState,
    lxm: &str,
    method: Method,
    uri: &Uri,
    headers: &HeaderMap,
    body: Body,
) -> ProxyResult<Response> {
    if !matches!(method, Method::GET | Method::HEAD | Method::POST) {
        return Err(err(XrpcError::invalid_request(
            "XRPC requests only supports GET and POST",
        )));
    }
    if is_protected_method(lxm) {
        return Err(err(XrpcError::named("InvalidToken", "Bad token method")));
    }
    let target = target(s, headers, lxm).await.map_err(err)?;
    let extra: &[&str] = if lxm.starts_with("tools.ozone.inbox.") {
        &[SCOPE_TAKENDOWN]
    } else {
        &[]
    };
    let aud = format!("{}#{}", target.did, target.service_id);
    let authed = auth::authorization(s, headers, auth::STANDARD, extra, |p| {
        Ok(p.assert_rpc(lxm, &aud)?)
    })
    .await
    .map_err(err)?;
    if authed.oauth.is_none()
        && is_privileged_method(lxm)
        && !auth::PRIVILEGED.contains(&authed.scope.as_str())
    {
        return Err(err(XrpcError::named("InvalidToken", "Bad token method")));
    }
    let accept = headers
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let path_and_query = uri.path_and_query().map_or(uri.path(), |p| p.as_str());
    let upstream = send(
        s,
        Outgoing {
            body: (method == Method::POST).then_some(body),
            method,
            lxm,
            path_and_query,
            headers,
            iss: Some(&authed.did),
            aud: None,
            target: &target,
            accept_encoding: accept,
            strict: target.user_influenced && lxm == "com.atproto.moderation.createReport",
        },
    )
    .await?;
    Ok(stream_back(s, upstream))
}

/// The `did#service` a request for `lxm` goes to (the reference's
/// `computeProxyTo`): the `atproto-proxy` header, or the default service.
/// None when nothing is configured.
#[must_use]
pub fn proxy_to(s: &AppState, headers: &HeaderMap, lxm: &str) -> Option<String> {
    if let Some(v) = headers.get("atproto-proxy").and_then(|v| v.to_str().ok()) {
        return Some(v.to_owned());
    }
    let (service_id, info) = default_service(s, lxm);
    info.map(|(_, did)| format!("{did}#{service_id}"))
}

/// The OAuth check for a method answered by (or for) another service:
/// `rpc:<lxm>` for its `did#service` (skipped when there's no service).
pub fn rpc_check(
    s: &AppState,
    headers: &HeaderMap,
    lxm: &str,
) -> impl Fn(&locus_auth::scopes::Permissions) -> XrpcResult<()> {
    let aud = proxy_to(s, headers, lxm);
    let lxm = lxm.to_owned();
    move |p| match &aud {
        Some(a) => Ok(p.assert_rpc(&lxm, a)?),
        None => Ok(()),
    }
}
